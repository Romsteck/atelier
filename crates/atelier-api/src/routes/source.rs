//! Source routes — exploration du **working tree** d'une app (`…/{slug}/src`),
//! façon explorateur de fichiers + panneau git (UI Studio). Distinct de
//! [`crate::routes::git`] qui sert les **bare repos** (`{slug}.git`) : ici on lit
//! l'arbre de travail réel que l'agent (Bypass) modifie,
//! pour pouvoir relire en direct ce qui a changé.
//!
//! Majoritairement en lecture ; deux mutations restreintes au working tree :
//! `git commit` (stage-all + commit) et `git push` (vers l'upstream). Tout `git`
//! tourne avec `-c safe.directory=*`
//! car le process Atelier est root alors que `src/` appartient à `romain:hr-studio`
//! (sinon « detected dubious ownership »). Les chemins fournis par le client sont
//! sanitisés (pas de `..`, pas d'absolu, pas de symlink hors-src) et passés à git
//! après `--` pour écarter toute injection d'option.
use std::path::{Component, Path as FsPath, PathBuf};
use std::process::Stdio;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use tracing::{info, instrument, warn};

use crate::state::ApiState;

const MAX_FILE_BYTES: u64 = 256 * 1024; // viewer read-only — au-delà on tronque
const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024; // borne le patch renvoyé au front
const BINARY_SNIFF: usize = 8000; // octets inspectés pour détecter un binaire (NUL)

/// Monté sous `/api/apps` (comme la surveillance et l'agent) :
///   GET /api/apps/{slug}/source/tree?path=
///   GET /api/apps/{slug}/source/file?path=
///   GET /api/apps/{slug}/source/git/status
///   GET /api/apps/{slug}/source/git/diff?path=
///   GET /api/apps/{slug}/source/git/log?limit=
///   GET /api/apps/{slug}/source/git/show?sha=
///   POST /api/apps/{slug}/source/git/commit  {message}
///   POST /api/apps/{slug}/source/git/push
pub fn app_router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}/source/tree", get(tree))
        .route("/{slug}/source/file", get(file))
        .route("/{slug}/source/git/status", get(git_status))
        .route("/{slug}/source/git/diff", get(git_diff))
        .route("/{slug}/source/git/log", get(git_log))
        .route("/{slug}/source/git/show", get(git_show))
        .route("/{slug}/source/git/commit", post(git_commit))
        .route("/{slug}/source/git/push", post(git_push))
}

fn err(status: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({"error": msg.into()}))).into_response()
}

/// Résout + valide le dossier source d'une app. 400 si slug invalide, 404 si absent.
fn resolve_src(state: &ApiState, slug: &str) -> Result<PathBuf, axum::response::Response> {
    if !atelier_apps::valid_slug(slug) {
        return Err(err(StatusCode::BAD_REQUEST, "slug invalide"));
    }
    let src = state.apps_src_root.join(slug).join("src");
    if !src.is_dir() {
        return Err(err(StatusCode::NOT_FOUND, "source d'app introuvable"));
    }
    Ok(src)
}

/// Joint un chemin relatif fourni par le client SOUS `src`, en rejetant toute
/// composante d'évasion (`..`, racine, préfixe Windows). Retourne le PathBuf
/// absolu et le chemin relatif normalisé (slash-séparé) — `None` si invalide.
fn safe_join(src: &FsPath, rel: &str) -> Option<(PathBuf, String)> {
    let rel = rel.trim().trim_start_matches('/');
    let mut abs = src.to_path_buf();
    let mut parts: Vec<String> = Vec::new();
    for comp in FsPath::new(rel).components() {
        match comp {
            Component::Normal(c) => {
                let s = c.to_str()?;
                abs.push(s);
                parts.push(s.to_string());
            }
            Component::CurDir => {}
            // ParentDir / RootDir / Prefix → tentative d'évasion : rejet.
            _ => return None,
        }
    }
    Some((abs, parts.join("/")))
}

/// Garde anti-symlink : le chemin canonique doit rester sous `src` canonique.
fn within_src(src: &FsPath, abs: &FsPath) -> bool {
    match (src.canonicalize(), abs.canonicalize()) {
        (Ok(s), Ok(a)) => a.starts_with(s),
        _ => false,
    }
}

#[derive(Debug, Deserialize)]
struct PathQuery {
    #[serde(default)]
    path: String,
}

// ── Explorateur de fichiers ────────────────────────────────────────────────

#[instrument(skip(state), fields(slug = %slug))]
async fn tree(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<PathQuery>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (dir, rel) = match safe_join(&src, &q.path) {
        Some(v) => v,
        None => return err(StatusCode::BAD_REQUEST, "chemin invalide"),
    };
    if !within_src(&src, &dir) || !dir.is_dir() {
        return err(StatusCode::NOT_FOUND, "dossier introuvable");
    }

    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("read_dir: {e}")),
    };
    let mut entries: Vec<Value> = Vec::new();
    while let Ok(Some(ent)) = rd.next_entry().await {
        let name = ent.file_name().to_string_lossy().into_owned();
        let ft = ent.file_type().await.ok();
        let is_dir = ft.map(|t| t.is_dir()).unwrap_or(false);
        let size = if is_dir {
            None
        } else {
            ent.metadata().await.ok().map(|m| m.len())
        };
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        entries.push(json!({
            "name": name,
            "path": child_rel,
            "is_dir": is_dir,
            "size": size,
        }));
    }
    // Dossiers d'abord, puis tri alpha insensible à la casse (façon explorateur).
    entries.sort_by(|a, b| {
        let ad = a["is_dir"].as_bool().unwrap_or(false);
        let bd = b["is_dir"].as_bool().unwrap_or(false);
        bd.cmp(&ad).then_with(|| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    Json(json!({ "path": rel, "entries": entries })).into_response()
}

#[instrument(skip(state), fields(slug = %slug))]
async fn file(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<PathQuery>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if q.path.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "param 'path' requis");
    }
    let (abs, rel) = match safe_join(&src, &q.path) {
        Some(v) => v,
        None => return err(StatusCode::BAD_REQUEST, "chemin invalide"),
    };
    if !within_src(&src, &abs) || !abs.is_file() {
        return err(StatusCode::NOT_FOUND, "fichier introuvable");
    }
    let size = abs.metadata().map(|m| m.len()).unwrap_or(0);
    let bytes = match tokio::fs::read(&abs).await {
        Ok(b) => b,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, format!("read: {e}")),
    };
    // Binaire si un NUL apparaît dans les premiers octets.
    let binary = bytes.iter().take(BINARY_SNIFF).any(|&b| b == 0);
    if binary {
        return Json(json!({
            "path": rel, "size": size, "binary": true, "truncated": false, "content": "",
        }))
        .into_response();
    }
    let truncated = size > MAX_FILE_BYTES;
    let slice = if truncated {
        &bytes[..MAX_FILE_BYTES as usize]
    } else {
        &bytes[..]
    };
    let content = String::from_utf8_lossy(slice).into_owned();
    Json(json!({
        "path": rel, "size": size, "binary": false, "truncated": truncated, "content": content,
    }))
    .into_response()
}

// ── Git working tree ───────────────────────────────────────────────────────

/// Exécute `git -C <src> -c safe.directory=* <args>` et capture stdout/stderr +
/// le code de sortie (certaines commandes — `diff --no-index` — sortent en 1 tout
/// en produisant un patch valide ; les callers décident quoi en faire).
async fn git_capture(src: &FsPath, args: &[&str]) -> Result<(bool, String, String), String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(src).arg("-c").arg("safe.directory=*");
    for a in args {
        cmd.arg(a);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Jamais d'invite interactive (credentials / host key) : un `git push` qui en
    // attendrait une bloquerait le handler indéfiniment. Échec rapide à la place.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    let out = cmd.output().await.map_err(|e| format!("git spawn: {e}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Somme des lignes ajoutées/supprimées + nb de fichiers depuis la sortie
/// `git ... --numstat` ("add\tdel\tpath" par ligne ; "-" pour les binaires).
fn parse_numstat(out: &str) -> (u64, u64, u64) {
    let mut add = 0u64;
    let mut del = 0u64;
    let mut files = 0u64;
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split('\t');
        let a = it.next().unwrap_or("");
        let d = it.next().unwrap_or("");
        if it.next().is_none() {
            continue; // pas un enregistrement numstat valide
        }
        files += 1;
        add += a.parse::<u64>().unwrap_or(0); // "-" (binaire) → 0
        del += d.parse::<u64>().unwrap_or(0);
    }
    (add, del, files)
}

fn cap_diff(mut patch: String) -> (String, bool) {
    if patch.len() > MAX_DIFF_BYTES {
        patch.truncate(MAX_DIFF_BYTES);
        // Coupe à la dernière frontière de ligne pour ne pas laisser une demi-ligne.
        if let Some(nl) = patch.rfind('\n') {
            patch.truncate(nl);
        }
        (patch, true)
    } else {
        (patch, false)
    }
}

#[instrument(skip(state), fields(slug = %slug))]
async fn git_status(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // core.quotePath=false → chemins non-ascii lisibles (pas d'octal escaping).
    let (ok, stdout, stderr) = match git_capture(
        &src,
        &["-c", "core.quotePath=false", "status", "--porcelain=v1", "--branch"],
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if !ok {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("git status: {stderr}"));
    }

    let mut branch = String::new();
    let mut ahead = 0i64;
    let mut behind = 0i64;
    let mut files: Vec<Value> = Vec::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            // ## main...origin/main [ahead 1, behind 2]
            let head = rest.split([' ', '.']).next().unwrap_or("");
            branch = head.to_string();
            if let Some(b) = rest.find("[ahead ") {
                ahead = rest[b + 7..].split([',', ']']).next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            }
            if let Some(b) = rest.find("behind ") {
                behind = rest[b + 7..].split([',', ']']).next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            }
            continue;
        }
        if line.len() < 3 {
            continue;
        }
        let x = line.as_bytes()[0] as char; // index
        let y = line.as_bytes()[1] as char; // worktree
        let rest = &line[3..];
        // Renommage : "old -> new".
        let (path, old_path) = if let Some(idx) = rest.find(" -> ") {
            (rest[idx + 4..].to_string(), Some(rest[..idx].to_string()))
        } else {
            (rest.to_string(), None)
        };
        // Lettre pour le badge : '?' (untracked) → A ; sinon worktree non-vide, sinon index.
        let code = if x == '?' {
            'A'
        } else if y != ' ' {
            y
        } else {
            x
        };
        let staged = x != ' ' && x != '?';
        files.push(json!({
            "path": path,
            "old_path": old_path,
            "status": code.to_string(),
            "index": x.to_string(),
            "worktree": y.to_string(),
            "staged": staged,
            "untracked": x == '?',
        }));
    }
    Json(json!({
        "branch": branch, "ahead": ahead, "behind": behind,
        "files": files, "clean": files.is_empty(),
    }))
    .into_response()
}

#[instrument(skip(state), fields(slug = %slug))]
async fn git_diff(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<PathQuery>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if q.path.trim().is_empty() {
        return err(StatusCode::BAD_REQUEST, "param 'path' requis");
    }
    let (_abs, rel) = match safe_join(&src, &q.path) {
        Some(v) => v,
        None => return err(StatusCode::BAD_REQUEST, "chemin invalide"),
    };
    // Working tree (staged + unstaged) vs HEAD pour le fichier. `--` isole le
    // chemin de toute option. Si vide → fichier non suivi : on synthétise un diff
    // « tout ajouté » via --no-index contre /dev/null.
    let (ok, mut stdout, stderr) =
        match git_capture(&src, &["diff", "--no-color", "HEAD", "--", &rel]).await {
            Ok(v) => v,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
    if !ok && !stderr.is_empty() && stdout.is_empty() {
        // HEAD peut ne pas exister (repo sans commit) → fallback --no-index.
        warn!(slug = %slug, "git diff HEAD failed, fallback --no-index");
    }
    if stdout.trim().is_empty() {
        // Untracked / pas de HEAD : diff contre /dev/null (exit 1 attendu).
        if let Ok((_ok2, out2, _err2)) =
            git_capture(&src, &["diff", "--no-color", "--no-index", "--", "/dev/null", &rel]).await
        {
            stdout = out2;
        }
    }
    let (patch, truncated) = cap_diff(stdout);
    Json(json!({ "path": rel, "patch": patch, "truncated": truncated })).into_response()
}

#[derive(Debug, Deserialize)]
struct LogQuery {
    #[serde(default = "default_log_limit")]
    limit: usize,
}
fn default_log_limit() -> usize {
    50
}

#[instrument(skip(state), fields(slug = %slug))]
async fn git_log(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<LogQuery>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let limit = q.limit.clamp(1, 500);
    let max = format!("--max-count={limit}");
    // Champs séparés par 0x1f (unit sep), commits par 0x1e (record sep) — robuste
    // face aux retours-ligne dans les sujets.
    let fmt = "--pretty=format:%H%x1f%h%x1f%an%x1f%aI%x1f%s%x1e";
    let (ok, stdout, stderr) = match git_capture(&src, &["log", &max, fmt]).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if !ok {
        // Repo sans commit → liste vide plutôt que 500.
        if stderr.contains("does not have any commits") || stderr.contains("bad default revision") {
            return Json(json!({ "commits": [] })).into_response();
        }
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("git log: {stderr}"));
    }
    let mut commits: Vec<Value> = Vec::new();
    for rec in stdout.split('\u{1e}') {
        let rec = rec.trim_matches(['\n', '\r']);
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\u{1f}').collect();
        if f.len() < 5 {
            continue;
        }
        commits.push(json!({
            "sha": f[0], "short": f[1], "author": f[2], "date": f[3], "subject": f[4],
        }));
    }
    Json(json!({ "commits": commits })).into_response()
}

#[derive(Debug, Deserialize)]
struct ShaQuery {
    sha: String,
}

#[instrument(skip(state), fields(slug = %slug))]
async fn git_show(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<ShaQuery>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // Hex-only → écarte toute injection d'argument git.
    let valid = (4..=64).contains(&q.sha.len()) && q.sha.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid {
        return err(StatusCode::BAD_REQUEST, "sha invalide");
    }
    // Métadonnées + patch en un appel : champs séparés par 0x1f, un 0x1e sépare
    // l'entête du diff (robuste face aux retours-ligne dans le sujet/corps).
    let fmt = "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%aI%x1f%cI%x1f%s%x1f%b%x1e";
    let (ok, stdout, stderr) =
        match git_capture(&src, &["show", "--no-color", "--patch", fmt, &q.sha]).await {
            Ok(v) => v,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
    if !ok {
        let st = if stderr.contains("unknown revision") || stderr.contains("bad object") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        return err(st, format!("git show: {stderr}"));
    }
    let (meta, diff_raw) = stdout.split_once('\u{1e}').unwrap_or((stdout.as_str(), ""));
    let f: Vec<&str> = meta.split('\u{1f}').collect();
    let g = |i: usize| f.get(i).map(|s| s.trim()).unwrap_or("");
    let (patch, truncated) = cap_diff(diff_raw.trim_start_matches('\n').to_string());

    // Totaux exacts via --numstat (indépendant de la troncature du patch).
    let (additions, deletions, files_changed) =
        match git_capture(&src, &["show", "--numstat", "--format=", &q.sha]).await {
            Ok((true, out, _)) => parse_numstat(&out),
            _ => (0u64, 0u64, 0u64),
        };

    Json(json!({
        "sha": g(0),
        "short": g(1),
        "author": g(2),
        "email": g(3),
        "author_date": g(4),
        "commit_date": g(5),
        "subject": g(6),
        "body": g(7),
        "additions": additions,
        "deletions": deletions,
        "files_changed": files_changed,
        "patch": patch,
        "truncated": truncated,
    }))
    .into_response()
}

// ── Mutations git (working tree) ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CommitBody {
    message: String,
}

/// Identité git du commit. On NE surcharge JAMAIS une identité déjà configurée
/// (repo/local) ; on n'en injecte une neutre que si elle manque — le process
/// Atelier tourne en root sans `user.name`/`user.email` global, et `git commit`
/// échouerait alors avec « empty ident name ».
async fn commit_identity(src: &FsPath) -> Vec<String> {
    let configured = |r: Result<(bool, String, String), String>| {
        matches!(r, Ok((ok, v, _)) if ok && !v.trim().is_empty())
    };
    let has_name = configured(git_capture(src, &["config", "user.name"]).await);
    let has_email = configured(git_capture(src, &["config", "user.email"]).await);
    if has_name && has_email {
        Vec::new()
    } else {
        vec![
            "-c".into(),
            "user.name=Atelier Studio".into(),
            "-c".into(),
            "user.email=studio@atelier.local".into(),
        ]
    }
}

/// Stage tout le working tree puis commit. Renvoie le sha créé.
#[instrument(skip(state, body), fields(slug = %slug))]
async fn git_commit(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Json(body): Json<CommitBody>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let msg = body.message.trim().to_string();
    if msg.is_empty() {
        return err(StatusCode::BAD_REQUEST, "message de commit requis");
    }
    // `git add -A` : suivis + non-suivis + suppressions → index complet.
    match git_capture(&src, &["add", "-A"]).await {
        Ok((true, _, _)) => {}
        Ok((false, _, e)) => {
            return err(StatusCode::INTERNAL_SERVER_ERROR, format!("git add: {}", e.trim()));
        }
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
    // `-m` prend le message en un seul argv (pas de shell) → aucune injection.
    let mut args = commit_identity(&src).await;
    args.extend(["commit".into(), "-m".into(), msg]);
    let argref: Vec<&str> = args.iter().map(String::as_str).collect();
    let (ok, out, e) = match git_capture(&src, &argref).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if !ok {
        if out.contains("nothing to commit") || e.contains("nothing to commit") {
            return err(StatusCode::BAD_REQUEST, "rien à committer");
        }
        let detail = if e.trim().is_empty() { out.trim() } else { e.trim() };
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("git commit: {detail}"));
    }
    let sha = match git_capture(&src, &["rev-parse", "HEAD"]).await {
        Ok((true, s, _)) => s.trim().to_string(),
        _ => String::new(),
    };
    let short: String = sha.chars().take(7).collect();
    info!(slug = %slug, sha = %sha, "git commit (working tree)");
    Json(json!({ "ok": true, "sha": sha, "short": short })).into_response()
}

/// Pousse la branche courante vers son upstream. L'origin du working tree est le
/// bare repo local (`/var/lib/atelier/git/{slug}.git`) → pas de credentials.
#[instrument(skip(state), fields(slug = %slug))]
async fn git_push(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (ok, out, e) = match git_capture(&src, &["push"]).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if !ok {
        // Échec « attendu » (pas d'upstream, non-fast-forward, creds) : 409 + message git.
        warn!(slug = %slug, "git push failed");
        return err(StatusCode::CONFLICT, format!("git push: {}", e.trim()));
    }
    info!(slug = %slug, "git push (working tree)");
    // git push écrit son compte-rendu sur stderr → on le renvoie tel quel.
    let output = format!("{} {}", e.trim(), out.trim());
    Json(json!({ "ok": true, "output": output.trim() })).into_response()
}
