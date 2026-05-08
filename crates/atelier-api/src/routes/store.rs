//! Read-only REST routes for the app store (mirrored from homeroute hr-api).
//!
//! Atelier (Phase 3) ne porte PAS les mutations (publish_release / delete_app) —
//! les uploads APK continuent d'aller sur homeroute (Medion) qui reste
//! la source de vérité pour le catalog. Le rsync /var/lib/atelier/store/ tourne
//! toutes les 5 min via atelier-sync-store.timer.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::error;

use crate::state::ApiState;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreCatalog {
    pub apps: Vec<StoreApp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreApp {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub android_package: Option<String>,
    pub publisher_app_id: String,
    pub releases: Vec<StoreRelease>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreRelease {
    pub version: String,
    pub changelog: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
}

fn catalog_path(store_dir: &std::path::Path) -> std::path::PathBuf {
    store_dir.join("catalog.json")
}

fn load_catalog(store_dir: &std::path::Path) -> StoreCatalog {
    match std::fs::read(catalog_path(store_dir)) {
        Ok(data) => serde_json::from_slice(&data).unwrap_or_default(),
        Err(_) => StoreCatalog::default(),
    }
}

/// Compare two dotted version strings segment-by-segment as u64.
fn version_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .filter_map(|seg| seg.parse::<u64>().ok())
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    let max_len = va.len().max(vb.len());
    for i in 0..max_len {
        let sa = va.get(i).copied().unwrap_or(0);
        let sb = vb.get(i).copied().unwrap_or(0);
        if sa > sb {
            return true;
        }
        if sa < sb {
            return false;
        }
    }
    false
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/apps", get(list_apps))
        .route("/apps/{slug}", get(get_app))
        .route("/apps/{slug}/icon", get(get_app_icon))
        .route("/releases/{slug}/{version}/download", get(download_release))
        .route("/updates", get(check_updates))
        .route("/client/apk", get(download_client_apk))
        .route("/client/version", get(client_version))
}

async fn list_apps(State(state): State<ApiState>) -> impl IntoResponse {
    let catalog = load_catalog(&state.store_dir);
    let summary: Vec<serde_json::Value> = catalog
        .apps
        .iter()
        .map(|app| {
            let latest = app.releases.last();
            json!({
                "slug": app.slug,
                "name": app.name,
                "description": app.description,
                "category": app.category,
                "icon": app.icon,
                "android_package": app.android_package,
                "publisher_app_id": app.publisher_app_id,
                "latest_version": latest.map(|r| r.version.as_str()),
                "latest_size_bytes": latest.map(|r| r.size_bytes),
                "release_count": app.releases.len(),
                "created_at": app.created_at,
                "updated_at": app.updated_at,
            })
        })
        .collect();

    Json(json!({"success": true, "apps": summary})).into_response()
}

async fn get_app(State(state): State<ApiState>, Path(slug): Path<String>) -> impl IntoResponse {
    let catalog = load_catalog(&state.store_dir);
    match catalog.apps.iter().find(|a| a.slug == slug) {
        Some(app) => Json(json!({"success": true, "app": app})).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": "App not found"})),
        )
            .into_response(),
    }
}

async fn get_app_icon(
    State(state): State<ApiState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if slug.contains('/') || slug.contains("..") {
        return (StatusCode::BAD_REQUEST, "Invalid slug").into_response();
    }
    let icon_path = state.store_dir.join("icons").join(format!("{}.png", slug));
    match tokio::fs::read(&icon_path).await {
        Ok(data) => {
            let headers = [(header::CONTENT_TYPE, "image/png")];
            (headers, data).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Icon not found").into_response(),
    }
}

async fn download_release(
    State(state): State<ApiState>,
    Path((slug, version)): Path<(String, String)>,
) -> impl IntoResponse {
    if slug.contains('/') || slug.contains("..") || version.contains('/') || version.contains("..")
    {
        return (StatusCode::BAD_REQUEST, "Invalid slug or version").into_response();
    }
    let apk_path = state
        .store_dir
        .join("releases")
        .join(&slug)
        .join(&version)
        .join("app.apk");

    match tokio::fs::read(&apk_path).await {
        Ok(data) => {
            let catalog = load_catalog(&state.store_dir);
            let sha256 = catalog
                .apps
                .iter()
                .find(|a| a.slug == slug)
                .and_then(|a| a.releases.iter().find(|r| r.version == version))
                .map(|r| r.sha256.clone())
                .unwrap_or_default();

            let filename = format!("{}-{}.apk", slug, version);

            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                "application/vnd.android.package-archive".parse().unwrap(),
            );
            headers.insert(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", filename)
                    .parse()
                    .unwrap(),
            );
            headers.insert("X-Sha256", sha256.parse().unwrap());

            (StatusCode::OK, headers, data).into_response()
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({"success": false, "error": "Release not found"})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct UpdateQuery {
    installed: Option<String>,
}

async fn check_updates(
    State(state): State<ApiState>,
    Query(query): Query<UpdateQuery>,
) -> impl IntoResponse {
    let installed_str = query.installed.unwrap_or_default();
    if installed_str.is_empty() {
        return Json(json!({"success": true, "updates": []})).into_response();
    }

    let installed: Vec<(&str, &str)> = installed_str
        .split(',')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, ':');
            let slug = parts.next()?;
            let version = parts.next()?;
            if slug.is_empty() || version.is_empty() {
                None
            } else {
                Some((slug, version))
            }
        })
        .collect();

    let catalog = load_catalog(&state.store_dir);
    let mut updates = Vec::new();

    for (slug, current_version) in &installed {
        let direct_app = catalog.apps.iter().find(|a| a.slug == *slug);
        let android_pkg = direct_app
            .and_then(|a| a.android_package.as_deref())
            .unwrap_or("");

        let best = catalog
            .apps
            .iter()
            .filter(|a| {
                a.slug == *slug
                    || (!android_pkg.is_empty()
                        && a.android_package.as_deref() == Some(android_pkg))
            })
            .filter_map(|a| a.releases.last().map(|r| (a, r)))
            .max_by(|(_, ra), (_, rb)| {
                if version_newer(&ra.version, &rb.version) {
                    std::cmp::Ordering::Greater
                } else if version_newer(&rb.version, &ra.version) {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            });

        if let Some((app, release)) = best {
            if version_newer(&release.version, current_version) {
                let mut entry = json!({
                    "slug": app.slug,
                    "name": app.name,
                    "current_version": current_version,
                    "latest_version": release.version,
                    "latest_changelog": release.changelog,
                    "latest_sha256": release.sha256,
                    "latest_size_bytes": release.size_bytes,
                });
                if app.slug != *slug {
                    entry["installed_slug"] = json!(slug);
                }
                updates.push(entry);
            }
        }
    }

    Json(json!({"success": true, "updates": updates})).into_response()
}

async fn download_client_apk(State(state): State<ApiState>) -> impl IntoResponse {
    let path = state.store_dir.join("client").join("homeroute-store.apk");
    if !path.exists() {
        return (StatusCode::NOT_FOUND, "Client APK not available").into_response();
    }
    match tokio::fs::read(&path).await {
        Ok(data) => {
            let headers = [
                (
                    header::CONTENT_TYPE,
                    "application/vnd.android.package-archive",
                ),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"homeroute-store.apk\"",
                ),
            ];
            (headers, data).into_response()
        }
        Err(e) => {
            error!(?e, "failed to read client APK");
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read APK").into_response()
        }
    }
}

async fn client_version(State(state): State<ApiState>) -> impl IntoResponse {
    let path = state.store_dir.join("client").join("version.json");
    if !path.exists() {
        return (StatusCode::NOT_FOUND, "Version info not available").into_response();
    }
    match tokio::fs::read(&path).await {
        Ok(data) => {
            let headers = [(header::CONTENT_TYPE, "application/json")];
            (headers, data).into_response()
        }
        Err(e) => {
            error!(?e, "failed to read client version.json");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read version info",
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_newer() {
        assert!(version_newer("1.1.0", "1.0.0"));
        assert!(version_newer("2.0.0", "1.9.9"));
        assert!(version_newer("1.0.1", "1.0.0"));
        assert!(!version_newer("1.0.0", "1.0.0"));
        assert!(!version_newer("1.0.0", "1.0.1"));
        assert!(version_newer("1.0.0.1", "1.0.0"));
        assert!(!version_newer("1.0.0", "1.0.0.1"));
    }
}
