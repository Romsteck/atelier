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
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
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
        // Worktrees par conversation (isolation Phase 1 « branch-per-conversation »).
        // Chaque conversation édite dans `…/{slug}/wt/{conv_id}` (branche `conv/{id}`),
        // jamais directement dans `src/` (= runtime de l'app, qui reste sur `main`).
        .route(
            "/{slug}/source/worktrees",
            get(worktree_list).post(worktree_create),
        )
        .route("/{slug}/source/worktrees/{conv_id}", delete(worktree_remove))
        // « Merge & deploy » (Phase 1) : merge `conv/<id>` → main, rebuild, restart, cleanup.
        .route(
            "/{slug}/source/worktrees/{conv_id}/merge",
            post(worktree_merge),
        )
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

/// Résout le **working dir** d'une op source : le worktree `…/{slug}/wt/{conv_id}`
/// si `conv_id` est fourni (et valide + existant), sinon `…/{slug}/src`. Permet au
/// panneau git / explorateur de suivre la conversation active (son worktree).
fn resolve_workdir(
    state: &ApiState,
    slug: &str,
    conv_id: Option<&str>,
) -> Result<PathBuf, axum::response::Response> {
    if !atelier_apps::valid_slug(slug) {
        return Err(err(StatusCode::BAD_REQUEST, "slug invalide"));
    }
    match conv_id.map(str::trim).filter(|c| !c.is_empty()) {
        Some(cid) => {
            if !valid_conv_id(cid) {
                return Err(err(StatusCode::BAD_REQUEST, "conv_id invalide"));
            }
            let wt = worktrees_base(state, slug).join(cid);
            if !wt.is_dir() {
                return Err(err(StatusCode::NOT_FOUND, "worktree introuvable"));
            }
            Ok(wt)
        }
        None => {
            let src = state.apps_src_root.join(slug).join("src");
            if !src.is_dir() {
                return Err(err(StatusCode::NOT_FOUND, "source d'app introuvable"));
            }
            Ok(src)
        }
    }
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
    // Scope worktree (conversation active) ; absent → src/.
    #[serde(default)]
    conv_id: Option<String>,
}

/// Scope-only (status / commit / push) : worktree de la conversation, sinon src/.
#[derive(Debug, Deserialize, Default)]
struct WtScope {
    #[serde(default)]
    conv_id: Option<String>,
}

// ── Explorateur de fichiers ────────────────────────────────────────────────

#[instrument(skip(state), fields(slug = %slug))]
async fn tree(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<PathQuery>,
) -> impl IntoResponse {
    let src = match resolve_workdir(&state, &slug, q.conv_id.as_deref()) {
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
    let src = match resolve_workdir(&state, &slug, q.conv_id.as_deref()) {
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
    Query(scope): Query<WtScope>,
) -> impl IntoResponse {
    let src = match resolve_workdir(&state, &slug, scope.conv_id.as_deref()) {
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
    let src = match resolve_workdir(&state, &slug, q.conv_id.as_deref()) {
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
    #[serde(default)]
    conv_id: Option<String>,
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
    let src = match resolve_workdir(&state, &slug, q.conv_id.as_deref()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let limit = q.limit.clamp(1, 500);
    let max = format!("--max-count={limit}");
    // `--branches --topo-order` : graphe multi-branches (main + toutes les `conv/*`),
    // ordonné pour un rendu de lanes propre (façon VSCode). Champs séparés par 0x1f,
    // commits par 0x1e. On ajoute `%P` (parents → topologie pour le graphe) et `%D`
    // (décorations de refs → puces de branche). `--decorate-refs=refs/heads/` limite
    // les décorations aux branches locales (pas les remotes).
    let fmt = "--pretty=format:%H%x1f%h%x1f%an%x1f%aI%x1f%s%x1f%P%x1f%D%x1e";
    let (ok, stdout, stderr) = match git_capture(
        &src,
        &[
            "log",
            "--branches",
            "--topo-order",
            "--decorate=short",
            "--decorate-refs=refs/heads/",
            &max,
            fmt,
        ],
    )
    .await
    {
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
        // parents : SHAs séparés par espace ("" si commit racine).
        let parents: Vec<&str> = f
            .get(5)
            .map(|p| p.split_whitespace().collect())
            .unwrap_or_default();
        // refs : décorations `%D` ("HEAD -> main, conv/x") → noms de branches nettoyés.
        let refs: Vec<String> = f
            .get(6)
            .map(|d| {
                d.split(',')
                    .map(|s| s.trim().trim_start_matches("HEAD -> ").to_string())
                    .filter(|s| !s.is_empty() && s != "HEAD")
                    .collect()
            })
            .unwrap_or_default();
        commits.push(json!({
            "sha": f[0], "short": f[1], "author": f[2], "date": f[3], "subject": f[4],
            "parents": parents, "refs": refs,
        }));
    }
    Json(json!({ "commits": commits })).into_response()
}

#[derive(Debug, Deserialize)]
struct ShaQuery {
    sha: String,
    #[serde(default)]
    conv_id: Option<String>,
}

#[instrument(skip(state), fields(slug = %slug))]
async fn git_show(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(q): Query<ShaQuery>,
) -> impl IntoResponse {
    let src = match resolve_workdir(&state, &slug, q.conv_id.as_deref()) {
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
    Query(scope): Query<WtScope>,
    Json(body): Json<CommitBody>,
) -> impl IntoResponse {
    let src = match resolve_workdir(&state, &slug, scope.conv_id.as_deref()) {
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
async fn git_push(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Query(scope): Query<WtScope>,
) -> impl IntoResponse {
    let src = match resolve_workdir(&state, &slug, scope.conv_id.as_deref()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // `-u origin HEAD` : pousse la branche courante (main OU `conv/<id>`) en posant
    // l'upstream → fonctionne pour un worktree dont la branche n'a pas encore d'upstream.
    let (ok, out, e) = match git_capture(&src, &["push", "-u", "origin", "HEAD"]).await {
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

// ── Worktrees par conversation ───────────────────────────────────────────────
//
// Isolation Phase 1 : chaque conversation de l'agent travaille dans un worktree
// git dédié (`…/{slug}/wt/{conv_id}`, branche `conv/{conv_id}`) au lieu d'éditer
// `src/` partagé. `src/` reste le **runtime** servi par le superviseur (toujours
// sur `main`) ; le worktree partage l'object store de `src/` (merge local au
// moment du « Merge & deploy », pas de push intermédiaire).
//
// Emplacement `apps/{slug}/wt/` choisi car (1) hérite du setgid `hr-studio` du
// parent → éditable par l'agent, (2) hors de portée de `cleanup_legacy_parent_context`
// (chirurgical : ne touche que CLAUDE.md/.mcp.json/.claude au niveau `{slug}/`),
// (3) dans `ReadWritePaths` du service. Les fichiers de contexte agent (gitignorés)
// sont régénérés dans le worktree par le provisioning (lot B), pas ici.

/// Valide un identifiant de conversation pour usage comme **nom de branche**
/// (`conv/<id>`) ET **segment de chemin** (`wt/<id>`). Alphabet sûr ; bloque
/// `..`, le préfixe `-` (injection d'option git) et `.` (fichier caché).
pub(crate) fn valid_conv_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('-')
        && !s.starts_with('.')
        && !s.contains("..")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Base des worktrees d'une app : `…/{slug}/wt/`.
fn worktrees_base(state: &ApiState, slug: &str) -> PathBuf {
    state.apps_src_root.join(slug).join("wt")
}

/// Le worktree est créé par le process Atelier (root) ; l'agent tourne en
/// `hr-studio` et doit pouvoir éditer les fichiers checkout. On aligne donc le
/// worktree sur ce qui rend `src/` éditable par l'agent : groupe agent,
/// group-writable, setgid sur les répertoires (pour que les fichiers créés
/// ensuite — build, edits — héritent du groupe). Best-effort : un échec est
/// loggé sans bloquer (le worktree reste créé, juste root-only).
async fn fixup_worktree_perms(wt: &FsPath) {
    let group = std::env::var("ATELIER_RULES_GROUP").unwrap_or_else(|_| "hr-studio".into());
    let warn_fail = |tool: &str, r: Result<std::process::Output, std::io::Error>| match r {
        Ok(o) if !o.status.success() => warn!(
            tool, wt = %wt.display(), stderr = %String::from_utf8_lossy(&o.stderr).trim(),
            "worktree perms fixup failed (non-fatal)"
        ),
        Err(e) => warn!(tool, wt = %wt.display(), error = %e, "worktree perms fixup spawn failed"),
        _ => {}
    };
    warn_fail("chgrp", Command::new("chgrp").args(["-R", &group]).arg(wt).output().await);
    warn_fail("chmod", Command::new("chmod").args(["-R", "g+rwX"]).arg(wt).output().await);
    // setgid sur les seuls répertoires (g+s sur un fichier = bit setgid sans
    // effet voire refusé ; `X` ne le pose pas).
    warn_fail(
        "find-setgid",
        Command::new("find")
            .arg(wt)
            .args(["-type", "d", "-exec", "chmod", "g+s", "{}", "+"])
            .output()
            .await,
    );
}

#[derive(Debug, Deserialize)]
struct CreateWorktreeBody {
    conv_id: String,
}

/// Provisionne (idempotent) le worktree d'une conversation et retourne
/// `(chemin, créé)`. Crée le worktree `conv/<conv_id>` issu de `HEAD`
/// (= `main` de `src/`) s'il manque, aligne les perms pour l'agent, et régénère
/// le contexte agent (`.claude/`, `.mcp.json`, `CLAUDE.md` — gitignorés, donc
/// HORS du diff de merge) dans le worktree. No-op (créé=false) si déjà présent.
/// Appelé par la route `POST …/worktrees` ET par `/agent/query` (auto-provision).
pub(crate) async fn provision_worktree(
    state: &ApiState,
    slug: &str,
    conv_id: &str,
) -> Result<(PathBuf, bool), (StatusCode, String)> {
    if !atelier_apps::valid_slug(slug) {
        return Err((StatusCode::BAD_REQUEST, "slug invalide".into()));
    }
    if !valid_conv_id(conv_id) {
        return Err((StatusCode::BAD_REQUEST, "conv_id invalide".into()));
    }
    let src = state.apps_src_root.join(slug).join("src");
    if !src.is_dir() {
        return Err((StatusCode::NOT_FOUND, "source d'app introuvable".into()));
    }
    let base = worktrees_base(state, slug);
    let wt = base.join(conv_id);
    if wt.exists() {
        return Ok((wt, false)); // déjà provisionné (chemin de reprise)
    }
    if let Err(e) = tokio::fs::create_dir_all(&base).await {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("mkdir wt/: {e}")));
    }
    let branch = format!("conv/{conv_id}");
    let wt_str = wt.to_string_lossy().into_owned();
    // `git worktree add` lancé sous **umask 002** : les dossiers que git crée dans
    // `.git` (`worktrees/{id}/`, `refs/heads/conv/`) héritent du group-write ;
    // combiné au setgid `hr-studio` déjà présent sur `.git`, le gitdir + les refs
    // deviennent **writables par l'agent (hr-studio)** → il peut committer dans son
    // worktree (sans ça : « cannot lock ref / index.lock: Permission denied »).
    // `safe.directory=*` car le process est root et `src/` appartient à romain.
    // conv_id validé (alphanum + _-.) + paths fixes → interpolation shell sûre.
    let add_script = format!(
        "umask 002 && git -C '{src}' -c safe.directory='*' worktree add '{wt}' -b '{branch}' HEAD",
        src = src.to_string_lossy(),
        wt = wt_str,
        branch = branch,
    );
    let add_out = Command::new("bash")
        .arg("-c")
        .arg(&add_script)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("bash spawn: {e}")))?;
    if !add_out.status.success() {
        // Branche déjà existante / HEAD absent (repo vide) → 409 + message git.
        let e = String::from_utf8_lossy(&add_out.stderr);
        let _ = tokio::fs::remove_dir_all(&wt).await;
        return Err((StatusCode::CONFLICT, format!("git worktree add: {}", e.trim())));
    }
    fixup_worktree_perms(&wt).await;
    generate_worktree_context(state, slug, &src, &wt).await;
    info!(slug = %slug, conv_id = %conv_id, branch = %branch, wt = %wt_str, "worktree provisioned");
    Ok((wt, true))
}

/// Régénère le contexte Claude Code dans le worktree (mêmes fichiers que `src/`,
/// tous gitignorés) + reprend le carnet `CLAUDE.md` de `src/` (gitignoré → absent
/// du checkout) AVANT la génération pour que `write_if_missing` le préserve.
/// Best-effort : un échec est loggé sans bloquer le provisioning.
async fn generate_worktree_context(state: &ApiState, slug: &str, src: &FsPath, wt: &FsPath) {
    let from = src.join("CLAUDE.md");
    let to = wt.join("CLAUDE.md");
    if from.is_file() && !to.exists() {
        if let Err(e) = tokio::fs::copy(&from, &to).await {
            warn!(slug = %slug, error = %e, "worktree CLAUDE.md seed failed (non-fatal)");
        }
    }
    let Some(app) = state.app_registry.get(slug).await else {
        warn!(slug = %slug, "worktree context skipped: app introuvable au registre");
        return;
    };
    let all = state.app_registry.list().await;
    // db_tables=None : n'enrichit que la liste de tables d'app-info.md (cosmétique) ;
    // on évite ainsi un aller-retour dataverse sur le chemin de provisioning.
    if let Err(e) = state
        .context_generator
        .generate_for_app_at(&app, wt, &all, None, false)
    {
        warn!(slug = %slug, error = %e, "worktree context generation failed (non-fatal)");
    }
}

/// `POST …/worktrees` — provisionne explicitement le worktree d'une conversation
/// (idempotent : `created:false` si déjà là). Même chemin que l'auto-provision
/// de `/agent/query`.
#[instrument(skip(state, body), fields(slug = %slug))]
async fn worktree_create(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
    Json(body): Json<CreateWorktreeBody>,
) -> impl IntoResponse {
    let conv_id = body.conv_id.trim().to_string();
    match provision_worktree(&state, &slug, &conv_id).await {
        Ok((wt, created)) => Json(json!({
            "ok": true,
            "created": created,
            "conv_id": conv_id,
            "branch": format!("conv/{conv_id}"),
            "path": wt.to_string_lossy(),
        }))
        .into_response(),
        Err((st, msg)) => err(st, msg),
    }
}

/// Liste les worktrees liés à `src/` (la branche `main`/principale incluse,
/// flaggée `is_main: true`). Source : `git worktree list --porcelain`.
#[instrument(skip(state), fields(slug = %slug))]
async fn worktree_list(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (ok, out, e) = match git_capture(&src, &["worktree", "list", "--porcelain"]).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    if !ok {
        return err(StatusCode::INTERNAL_SERVER_ERROR, format!("git worktree list: {}", e.trim()));
    }
    let main_canon = src.canonicalize().ok();
    let worktrees = parse_worktree_porcelain(&out, main_canon.as_deref());
    Json(json!({ "worktrees": worktrees })).into_response()
}

/// Retire le worktree `conv/<conv_id>` et supprime sa branche. `--force` jette
/// d'éventuelles modifs non commitées du worktree (en Phase 1 la résolution
/// passe par le merge ; un retrait = abandon explicite). Idempotent-ish : si le
/// worktree n'est plus reconnu, on `prune` + nettoie le dossier résiduel.
#[instrument(skip(state), fields(slug = %slug))]
async fn worktree_remove(
    State(state): State<ApiState>,
    Path((slug, conv_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !valid_conv_id(&conv_id) {
        return err(StatusCode::BAD_REQUEST, "conv_id invalide");
    }
    let branch = format!("conv/{conv_id}");
    let wt = worktrees_base(&state, &slug).join(&conv_id);
    let wt_str = wt.to_string_lossy().into_owned();

    let (ok, _o, e) =
        match git_capture(&src, &["worktree", "remove", "--force", &wt_str]).await {
            Ok(v) => v,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
    if !ok {
        // Worktree non reconnu (dossier supprimé à la main, métadonnée stale) :
        // prune les liens morts puis retire le dossier résiduel s'il reste.
        warn!(slug = %slug, conv_id = %conv_id, stderr = %e.trim(), "worktree remove failed, pruning");
        let _ = git_capture(&src, &["worktree", "prune"]).await;
        if wt.exists() {
            let _ = tokio::fs::remove_dir_all(&wt).await;
        }
    }
    // Supprime la branche (force : non encore mergée). Ignoré si déjà absente.
    let _ = git_capture(&src, &["branch", "-D", &branch]).await;
    info!(slug = %slug, conv_id = %conv_id, branch = %branch, "worktree removed");
    Json(json!({ "ok": true })).into_response()
}

/// Parse `git worktree list --porcelain` en entrées JSON. Chaque worktree est un
/// bloc de lignes (`worktree <path>`, `HEAD <sha>`, `branch refs/heads/<name>`),
/// les blocs séparés par une ligne vide. `main_canon` = chemin canonique de la
/// worktree principale (`src/`) pour flagger `is_main`.
fn parse_worktree_porcelain(out: &str, main_canon: Option<&FsPath>) -> Vec<Value> {
    let mut entries = Vec::new();
    for block in out.split("\n\n") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        let mut path: Option<String> = None;
        let mut head: Option<String> = None;
        let mut branch: Option<String> = None;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(p.to_string());
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                head = Some(h.to_string());
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = Some(b.trim_start_matches("refs/heads/").to_string());
            }
        }
        let Some(path) = path else { continue };
        let conv_id = branch
            .as_ref()
            .and_then(|b| b.strip_prefix("conv/").map(str::to_string));
        let is_main = match (main_canon, FsPath::new(&path).canonicalize().ok()) {
            (Some(m), Some(p)) => p == m,
            _ => false,
        };
        entries.push(json!({
            "path": path,
            "head": head,
            "branch": branch,
            "conv_id": conv_id,
            "is_main": is_main,
        }));
    }
    entries
}

// ── Merge & deploy (Phase 1) ─────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct MergeBody {
    #[serde(default)]
    timeout_secs: Option<u64>,
}

/// Un `IpcResponse` de build/ship a réussi ? `ship()`/`build()` renvoient `ok`
/// même sur échec de pipeline (le code de sortie est rangé dans `data.exit_code`)
/// → on inspecte les deux, comme `ship_app` (sinon on annoncerait un succès alors
/// que l'app est down).
fn ipc_succeeded(resp: &atelier_ipc::types::IpcResponse) -> bool {
    let exit_code = resp
        .data
        .as_ref()
        .and_then(|d| d.get("exit_code"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    resp.ok && exit_code == 0
}

/// Reset `src/` au sha pré-merge. Suffit quand l'app n'a pas encore été redémarrée
/// (build échoué) : le process tourne toujours sur l'ancien artefact.
async fn rollback_src(src: &FsPath, pre_sha: &str) {
    match git_capture(src, &["reset", "--hard", pre_sha]).await {
        Ok((true, _, _)) => info!(pre_sha = %pre_sha, "merge rollback: src reset"),
        Ok((false, _, e)) => warn!(pre_sha = %pre_sha, stderr = %e.trim(), "merge rollback: git reset failed"),
        Err(e) => warn!(error = %e, "merge rollback: git reset spawn failed"),
    }
}

/// Rollback complet quand l'app a déjà été redémarrée sur le code mergé (ship /
/// healthcheck KO) : reset `src/` + rebuild + ship pour restaurer l'état pré-merge.
/// Best-effort (loggé) — la priorité est de remettre l'app debout.
async fn rollback_deploy(
    ctx: &crate::mcp::apps_ops::AppsContext,
    src: &FsPath,
    pre_sha: &str,
    slug: &str,
    timeout_secs: Option<u64>,
) {
    warn!(slug = %slug, pre_sha = %pre_sha, "merge rollback: redeploying pre-merge src");
    rollback_src(src, pre_sha).await;
    let _ = ctx.build(slug.to_string(), timeout_secs).await;
    let _ = ctx.ship(slug.to_string(), timeout_secs).await;
}

/// Attend que l'app soit `Running` après un (re)déploiement. Gate volontairement
/// basé sur l'état du superviseur (et non un GET HTTP) : certaines apps répondent
/// non-2xx sur leur health_path tout en étant saines (ex. `www` → 404), un check
/// HTTP générique provoquerait des faux négatifs → rollbacks intempestifs.
async fn verify_running(
    supervisor: &atelier_apps::AppSupervisor,
    slug: &str,
) -> Result<(), String> {
    use atelier_apps::AppState;
    for _ in 0..15 {
        match supervisor.status(slug).await {
            Some(s) if matches!(s.state, AppState::Running) => return Ok(()),
            Some(s) if matches!(s.state, AppState::Crashed) => {
                return Err("app crashed après deploy".into());
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    match supervisor.status(slug).await {
        Some(s) if matches!(s.state, AppState::Running) => Ok(()),
        Some(s) => Err(format!("app état={} après deploy (pas Running)", s.state.as_str())),
        None => Err("app sans status après deploy".into()),
    }
}

/// `POST …/worktrees/{conv_id}/merge` — pipeline « Merge & deploy » (Phase 1).
/// merge `conv/<conv_id>` → branche courante de `src/`, rebuild en place, restart,
/// vérifie que l'app est Running, puis retire worktree + branche. Conflit → 409
/// (résolution humaine en Phase 1). Échec build → rollback git (app intacte) ;
/// échec ship/health → rollback complet (reset + rebuild + ship). Synchrone et
/// long (le front passe un timeout large, comme `/ship`).
#[instrument(skip(state, body), fields(slug = %slug, conv_id = %conv_id))]
async fn worktree_merge(
    State(state): State<ApiState>,
    Path((slug, conv_id)): Path<(String, String)>,
    body: Option<Json<MergeBody>>,
) -> impl IntoResponse {
    let src = match resolve_src(&state, &slug) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if !valid_conv_id(&conv_id) {
        return err(StatusCode::BAD_REQUEST, "conv_id invalide");
    }
    let branch = format!("conv/{conv_id}");
    let wt = worktrees_base(&state, &slug).join(&conv_id);
    let wt_str = wt.to_string_lossy().into_owned();
    let timeout_secs = body.and_then(|Json(b)| b.timeout_secs);
    if !wt.exists() {
        return err(StatusCode::NOT_FOUND, "worktree introuvable pour cette conversation");
    }

    // 1. src/ = runtime + cible de merge ; l'agent ne l'édite JAMAIS (il bosse dans son
    //    worktree). Les seules modifs possibles dans src/ = résidus de build (ex.
    //    `tsconfig.tsbuildinfo` tracké, régénéré à chaque build) → on les jette pour
    //    partir d'un working tree propre (sinon `git merge` refuserait). Sans danger :
    //    aucun travail réel ne vit dans src/. Le travail à merger est sur la branche.
    let _ = git_capture(&src, &["checkout", "--", "."]).await;
    let pre_sha = match git_capture(&src, &["rev-parse", "HEAD"]).await {
        Ok((true, s, _)) => s.trim().to_string(),
        _ => return err(StatusCode::INTERNAL_SERVER_ERROR, "rev-parse HEAD échec"),
    };

    // 1b. Garde-fous anti-perte de travail (le cleanup `--force` final supprime le
    // worktree) : (a) la branche doit avoir des commits à merger ; (b) le worktree
    // ne doit pas avoir de travail NON commité. Sinon → 422 « commit d'abord ».
    let ahead = match git_capture(&src, &["rev-list", "--count", &format!("HEAD..{branch}")]).await {
        Ok((true, o, _)) => o.trim().parse::<u32>().unwrap_or(0),
        _ => 0,
    };
    if ahead == 0 {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "rien à merger : la branche n'a aucun commit. La conversation doit committer son travail d'abord.",
        );
    }
    let dirty = match git_capture(&wt, &["status", "--porcelain"]).await {
        Ok((true, o, _)) => !o.trim().is_empty(),
        _ => false,
    };
    if dirty {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "le worktree a des modifications non commitées — commite-les d'abord (sinon elles seraient perdues au cleanup).",
        );
    }

    // 2. Drain (filet serveur ; le front a déjà fermé les conversations).
    let cancelled = crate::routes::agent::cancel_runs_for_slug(&slug);
    if cancelled > 0 {
        info!(slug = %slug, cancelled, "merge: live runs coupés");
    }

    // 3. Merge --no-ff (garde la trace de la branche dans l'historique).
    let msg = format!("merge {branch}");
    let (merge_ok, mout, merr) =
        match git_capture(&src, &["merge", "--no-ff", "-m", &msg, &branch]).await {
            Ok(v) => v,
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
    if !merge_ok {
        let conflicts: Vec<String> = match git_capture(&src, &["diff", "--name-only", "--diff-filter=U"]).await {
            Ok((true, o, _)) => o.lines().map(str::to_string).collect(),
            _ => Vec::new(),
        };
        let _ = git_capture(&src, &["merge", "--abort"]).await;
        warn!(slug = %slug, branch = %branch, ?conflicts, "merge conflit — abort");
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "conflit de merge — résolution manuelle requise",
                "conflicts": conflicts,
                "detail": format!("{} {}", mout.trim(), merr.trim()).trim().to_string(),
            })),
        )
            .into_response();
    }
    info!(slug = %slug, branch = %branch, "merge propre");

    // 4. Rebuild en place puis restart (réutilise build()/ship() — events WS + lock).
    let ctx = crate::mcp::apps_ops::AppsContext::from_api_state(&state);
    let build_resp = ctx.build(slug.clone(), timeout_secs).await;
    if !ipc_succeeded(&build_resp) {
        rollback_src(&src, &pre_sha).await; // app pas encore redémarrée
        let detail = build_resp.error.clone().unwrap_or_else(|| "build failed".into());
        warn!(slug = %slug, %detail, "merge: build KO → src rollback");
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "build échoué après merge — src/ rollback", "detail": detail })),
        )
            .into_response();
    }
    let ship_resp = ctx.ship(slug.clone(), timeout_secs).await;
    if !ipc_succeeded(&ship_resp) {
        rollback_deploy(&ctx, &src, &pre_sha, &slug, timeout_secs).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "ship échoué après merge — rollback", "detail": ship_resp.error.clone().unwrap_or_default() })),
        )
            .into_response();
    }

    // 5. Gate : l'app est-elle Running ? Sinon rollback complet.
    if let Err(reason) = verify_running(state.supervisor.as_ref(), &slug).await {
        rollback_deploy(&ctx, &src, &pre_sha, &slug, timeout_secs).await;
        warn!(slug = %slug, %reason, "merge: app non saine → rollback complet");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "app non saine après deploy — rollback", "detail": reason })),
        )
            .into_response();
    }

    // 6. Succès : push (historique + miroir GitHub via post-receive), puis cleanup.
    let _ = git_capture(&src, &["push", "origin", "HEAD"]).await; // best-effort
    let _ = git_capture(&src, &["worktree", "remove", "--force", &wt_str]).await;
    let _ = git_capture(&src, &["branch", "-D", &branch]).await;
    info!(slug = %slug, branch = %branch, "merge & deploy OK — worktree retiré");

    Json(json!({ "ok": true, "merged": branch, "pre_sha": pre_sha })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conv_id_validation() {
        assert!(valid_conv_id("1718000000-3"));
        assert!(valid_conv_id("feature_x.2"));
        assert!(!valid_conv_id(""));
        assert!(!valid_conv_id("-rf")); // préfixe option
        assert!(!valid_conv_id(".hidden"));
        assert!(!valid_conv_id("a/b")); // slash interdit (un seul niveau)
        assert!(!valid_conv_id("a..b")); // traversal
        assert!(!valid_conv_id("a b")); // espace
    }

    #[test]
    fn porcelain_parse_main_and_conv() {
        let out = "worktree /var/lib/atelier/apps/home/src\n\
                   HEAD cf41f33aaaa\n\
                   branch refs/heads/main\n\
                   \n\
                   worktree /var/lib/atelier/apps/home/wt/c1\n\
                   HEAD abc123def\n\
                   branch refs/heads/conv/c1\n";
        // main_canon=None → is_main=false partout (pas de FS réel ici), mais on
        // valide le parsing des champs + l'extraction du conv_id.
        let entries = parse_worktree_porcelain(out, None);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["branch"], "main");
        assert_eq!(entries[0]["conv_id"], Value::Null);
        assert_eq!(entries[1]["branch"], "conv/c1");
        assert_eq!(entries[1]["conv_id"], "c1");
        assert_eq!(entries[1]["head"], "abc123def");
    }

    #[test]
    fn porcelain_parse_skips_detached_and_blank() {
        let out = "worktree /x/src\nHEAD deadbeef\ndetached\n";
        let entries = parse_worktree_porcelain(out, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["branch"], Value::Null);
        assert_eq!(entries[0]["conv_id"], Value::Null);
    }
}
