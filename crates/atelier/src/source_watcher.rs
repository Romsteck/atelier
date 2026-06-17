//! Watcher inotify des sources d'apps → event WS `source:changed`.
//!
//! WHY : l'explorateur de fichiers du Studio doit se rafraîchir tout seul quand
//! une source change sur le disque (agent Claude, code-server, git, …) au lieu
//! d'un bouton refresh manuel. On respecte la convention « live = WebSocket » :
//! on émet un `SourceChangedEvent { slug }` grossier (le front relit l'arbre),
//! debouncé pour coalescer les rafales (save-all, `git checkout`, rename-storm
//! d'éditeur → un seul event ~500 ms après la dernière écriture).
//!
//! Design :
//!   - **Une seule watch récursive** sur la racine des apps (`apps_src_root`),
//!     pas une par app : couvre automatiquement les apps créées après le boot
//!     (essentiel, on a retiré le bouton fallback) et la limite inotify
//!     (~1M watches) rend le coût négligeable.
//!   - **Filtrage sur le thread de callback notify**, AVANT le canal, pour que
//!     les rafales de build (`target/`, `node_modules/`) n'atteignent jamais le
//!     debounce. Seuls les chemins `{slug}/src/…` hors dossiers exclus passent.
//!   - **Boucle de debounce en thread std** (pas tokio) : `broadcast::send` est
//!     synchrone et n'a pas besoin du runtime, donc un `std::thread` dédié évite
//!     de monopoliser un slot du pool blocking tokio à vie.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use atelier_common::events::{EventBus, SourceChangedEvent};
use notify::{EventKind, RecursiveMode, Watcher};
use tracing::{debug, info, warn};

/// Dossiers dont les écritures ne doivent JAMAIS déclencher un refresh (artefacts
/// de build + métadonnées git) — sinon un `cargo build` / `npm ci` spammerait.
const EXCLUDED_DIRS: &[&str] = &["node_modules", "target", ".git", "dist", ".next", "build"];

/// Fenêtre de silence avant d'émettre (trailing-edge debounce, par slug).
const DEBOUNCE: Duration = Duration::from_millis(500);
/// Granularité de réveil de la boucle de flush.
const TICK: Duration = Duration::from_millis(100);

/// Mappe un chemin absolu modifié → slug d'app, ou `None` si hors périmètre.
/// N'accepte que `{apps_src_root}/{slug}/src/…` (slug valide, 2e composant `src`)
/// et rejette tout chemin contenant un segment exclu.
fn slug_for_path(apps_src_root: &Path, p: &Path) -> Option<String> {
    let rel = p.strip_prefix(apps_src_root).ok()?;
    let mut comps = rel.components();

    let slug = match comps.next()? {
        Component::Normal(s) => s.to_str()?.to_string(),
        _ => return None,
    };
    if !atelier_apps::valid_slug(&slug) {
        return None;
    }
    // Le 2e composant DOIT être `src` (écarte les dossiers frères bin/runs/.env
    // et les fichiers top-level de l'app).
    match comps.next() {
        Some(Component::Normal(s)) if s.to_str() == Some("src") => {}
        _ => return None,
    }
    // Aucun segment exclu dans le reste du chemin.
    for c in comps {
        if let Component::Normal(s) = c {
            if let Some(name) = s.to_str() {
                if EXCLUDED_DIRS.contains(&name) {
                    return None;
                }
            }
        }
    }
    Some(slug)
}

/// Démarre le watcher (no-op loggué si la racine n'existe pas ou si la watch
/// échoue : l'auto-refresh est non-critique, le reste du process doit servir).
pub fn spawn_source_watcher(events: Arc<EventBus>, apps_src_root: PathBuf) {
    if !apps_src_root.is_dir() {
        warn!(root = %apps_src_root.display(), "source watcher: racine absente — auto-refresh désactivé");
        return;
    }

    // Canal thread-notify → thread-debounce. Le callback notify mappe+filtre et
    // n'envoie que des slugs (pas des chemins) : le thread de debounce reste trivial.
    let (tx, rx) = channel::<String>();
    let root_cb = apps_src_root.clone();

    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let event = match res {
            Ok(e) => e,
            Err(_) => return,
        };
        // Seules les vraies mutations comptent — on ignore les accès/lectures.
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return;
        }
        for p in &event.paths {
            if let Some(slug) = slug_for_path(&root_cb, p) {
                let _ = tx.send(slug);
            }
        }
    }) {
        Ok(w) => w,
        Err(err) => {
            warn!(?err, "source watcher: échec création — auto-refresh désactivé");
            return;
        }
    };

    if let Err(err) = watcher.watch(&apps_src_root, RecursiveMode::Recursive) {
        warn!(?err, root = %apps_src_root.display(), "source watcher: échec watch — auto-refresh désactivé");
        return;
    }

    // Thread dédié : possède le watcher (le drop arrêterait les watches) + la
    // boucle de debounce. `broadcast::send` est sync → pas besoin du runtime tokio.
    std::thread::Builder::new()
        .name("source-watcher".into())
        .spawn(move || {
            let _watcher = watcher; // garder vivant pour toute la durée du thread
            let mut pending: HashMap<String, Instant> = HashMap::new();
            loop {
                match rx.recv_timeout(TICK) {
                    Ok(slug) => {
                        pending.insert(slug, Instant::now());
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                // Flush des slugs silencieux depuis >= DEBOUNCE (trailing-edge).
                let now = Instant::now();
                let ready: Vec<String> = pending
                    .iter()
                    .filter(|(_, t)| now.duration_since(**t) >= DEBOUNCE)
                    .map(|(s, _)| s.clone())
                    .collect();
                for slug in ready {
                    pending.remove(&slug);
                    let _ = events.source_changed.send(SourceChangedEvent { slug: slug.clone() });
                    debug!(slug = %slug, "source change broadcast");
                }
            }
            warn!("source watcher: canal fermé — auto-refresh arrêté");
        })
        .expect("spawn source-watcher thread");

    info!(root = %apps_src_root.display(), "source watcher started");
}
