//! Source routes — exploration du **working tree** d'une app (`…/{slug}/src`),
//! façon explorateur de fichiers + panneau git de code-server. Distinct de
//! [`crate::routes::git`] qui sert les **bare repos** (`{slug}.git`) : ici on lit
//! l'arbre de travail réel que code-server édite et que l'agent (Bypass) modifie,
//! pour pouvoir relire en direct ce qui a changé.
//!
//! Lecture seule (aucune mutation). Tout `git` tourne avec `-c safe.directory=*`
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
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use tracing::{instrument, warn};

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
pub fn app_router() -> Router<ApiState> {
    Router::new()
        .route("/{slug}/source/tree", get(tree))
        .route("/{slug}/source/file", get(file))
        .route("/{slug}/source/git/status", get(git_status))
        .route("/{slug}/source/git/diff", get(git_diff))
        .route("/{slug}/source/git/log", get(git_log))
        .route("/{slug}/source/git/show", get(git_show))
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
    let out = cmd.output().await.map_err(|e| format!("git spawn: {e}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
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
    let (ok, stdout, stderr) =
        match git_capture(&src, &["show", "--no-color", "--format=fuller", &q.sha]).await {
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
    let (patch, truncated) = cap_diff(stdout);
    Json(json!({ "sha": q.sha, "patch": patch, "truncated": truncated })).into_response()
}
