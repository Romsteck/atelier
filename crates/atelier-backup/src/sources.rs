use std::path::PathBuf;

/// Chemins hôte capturés par la sauvegarde, résolus une fois dans `main.rs`
/// depuis l'environnement.
#[derive(Debug, Clone)]
pub struct SourcePaths {
    /// Dossier des dépôts git bare (parent de `ATELIER_GIT_REPOS_DIR`).
    pub git_dir: PathBuf,
    pub env_file: PathBuf,
    pub data_dir: PathBuf,
    pub dv_secrets: PathBuf,
    pub apps_runtime_root: PathBuf,
    pub docs_dir: PathBuf,
}

impl SourcePaths {
    /// Liste des chemins « config » EXISTANTS à passer à `restic backup` : le
    /// `.env`, les registres, les secrets dataverse, chaque `<app>/.env`, le
    /// carnet de bord `<app>/src/CLAUDE.md`, et les docs. Les chemins absents
    /// sont silencieusement ignorés.
    pub fn config_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = Vec::new();
        for p in [&self.env_file, &self.data_dir, &self.dv_secrets, &self.docs_dir] {
            if p.exists() {
                paths.push(p.clone());
            }
        }
        // <apps_runtime_root>/<slug>/{.env, src/CLAUDE.md}. CLAUDE.md est le
        // carnet de bord possédé par l'agent ; il est .gitignore (donc hors du
        // repo déployable) → sans cette ligne, la connaissance accumulée par
        // l'agent ne serait ni versionnée ni sauvegardée (disque runtime only).
        if let Ok(entries) = std::fs::read_dir(&self.apps_runtime_root) {
            for entry in entries.flatten() {
                let env = entry.path().join(".env");
                if env.is_file() {
                    paths.push(env);
                }
                let claude_md = entry.path().join("src").join("CLAUDE.md");
                if claude_md.is_file() {
                    paths.push(claude_md);
                }
            }
        }
        paths
    }
}
