//! Construction de l'environnement restic+rclone pour atteindre le partage SMB.
//!
//! restic parle au partage via le backend `rclone` (repo `rclone:<remote>:<path>`).
//! Le remote SMB est défini **par variables d'environnement** (pas de fichier
//! de conf) passées au process enfant restic — qui les relaie à rclone. Aucun
//! secret n'apparaît en argv ni dans les logs : le mot de passe SMB est
//! **obscurci** par `rclone obscure`, et `RESTIC_PASSWORD` voyage par l'env.

use std::process::Stdio;

use tokio::process::Command;

use crate::target::FullTarget;

/// Nom du remote rclone (in fine, rclone met le nom en MAJUSCULES pour résoudre
/// les `RCLONE_CONFIG_<NAME>_*`).
pub const REMOTE: &str = "atelierbackup";

/// Env + URL de dépôt prêts à être injectés sur un process `restic`.
pub struct ResticEnv {
    pub vars: Vec<(String, String)>,
    pub repository: String,
}

/// Concatène `share` + `repo_subpath` en un chemin propre.
fn repo_path(t: &FullTarget) -> String {
    let mut parts: Vec<&str> = vec![t.share.trim_matches('/')];
    let repo = t.repo_subpath.trim_matches('/');
    if !repo.is_empty() {
        parts.push(repo);
    }
    parts.join("/")
}

/// URL de dépôt restic (`rclone:<remote>:<share>/<sub>/<repo>`).
pub fn repository_url(t: &FullTarget) -> String {
    format!("rclone:{}:{}", REMOTE, repo_path(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> FullTarget {
        FullTarget {
            kind: "smb".into(),
            host: "nas".into(),
            share: "/backups/".into(),
            username: "u".into(),
            domain: String::new(),
            password: Some("p".into()),
            restic_password: Some("rp".into()),
            repo_subpath: "atelier-backup".into(),
            schedule_enabled: false,
            schedule_cadence: "daily".into(),
            schedule_hour: 3,
            retention_keep: 7,
        }
    }

    #[test]
    fn repo_path_joins_and_trims_slashes() {
        assert_eq!(repo_path(&t()), "backups/atelier-backup");
    }

    #[test]
    fn repository_url_uses_remote() {
        assert_eq!(repository_url(&t()), "rclone:atelierbackup:backups/atelier-backup");
    }

    #[test]
    fn repo_path_custom_subpath() {
        let mut x = t();
        x.repo_subpath = "snaps".into();
        assert_eq!(repo_path(&x), "backups/snaps");
    }
}

/// Variables d'env du remote SMB rclone (sans RESTIC_*). Le mot de passe est
/// obscurci. Réutilisé par `build_env` (backup) et la découverte de partages.
pub async fn smb_vars(
    rclone_bin: &str,
    host: &str,
    username: &str,
    password: &str,
    domain: &str,
) -> Result<Vec<(String, String)>, String> {
    let obscured = obscure(rclone_bin, password).await?;
    let up = REMOTE.to_uppercase();
    let mut vars = vec![
        (format!("RCLONE_CONFIG_{up}_TYPE"), "smb".to_string()),
        (format!("RCLONE_CONFIG_{up}_HOST"), host.trim().to_string()),
        (format!("RCLONE_CONFIG_{up}_USER"), username.trim().to_string()),
        (format!("RCLONE_CONFIG_{up}_PASS"), obscured),
    ];
    if !domain.trim().is_empty() {
        vars.push((format!("RCLONE_CONFIG_{up}_DOMAIN"), domain.trim().to_string()));
    }
    Ok(vars)
}

/// Construit l'env restic+rclone. Échoue si le mot de passe du dépôt n'est pas
/// encore généré (l'appelant doit l'avoir provisionné avant le 1ᵉʳ backup).
pub async fn build_env(rclone_bin: &str, t: &FullTarget) -> Result<ResticEnv, String> {
    let restic_pw = t
        .restic_password
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "mot de passe du dépôt restic non initialisé".to_string())?;
    let mut vars = smb_vars(
        rclone_bin,
        &t.host,
        &t.username,
        t.password.as_deref().unwrap_or(""),
        &t.domain,
    )
    .await?;
    vars.push(("RESTIC_REPOSITORY".to_string(), repository_url(t)));
    vars.push(("RESTIC_PASSWORD".to_string(), restic_pw));
    Ok(ResticEnv {
        vars,
        repository: repository_url(t),
    })
}

/// Liste les partages exposés par le serveur SMB (`rclone lsjson <remote>:` à la
/// racine = les partages). Découverte sans rien persister ni écrire.
pub async fn list_shares(
    rclone_bin: &str,
    vars: &[(String, String)],
) -> Result<Vec<String>, String> {
    let target = format!("{}:", REMOTE);
    let mut cmd = Command::new(rclone_bin);
    // Découverte interactive → échec rapide si l'hôte est injoignable (sinon
    // rclone bloque ~60 s sur le contimeout par défaut).
    cmd.args([
        "lsjson",
        &target,
        "--contimeout=8s",
        "--timeout=20s",
        "--retries=1",
        "--low-level-retries=2",
    ]);
    for (k, v) in vars {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn rclone lsjson failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "connexion SMB échouée : {}",
            err.trim().chars().take(400).collect::<String>()
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("parse lsjson: {e}"))?;
    let shares = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e.get("IsDir").and_then(|b| b.as_bool()).unwrap_or(false))
                .filter_map(|e| e.get("Name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok(shares)
}

/// `rclone obscure <password>` — chiffre réversiblement le mot de passe pour la
/// conf rclone (exigé par le backend). Le mot de passe transite par stdin pour
/// ne pas apparaître dans la table des process.
pub async fn obscure(rclone_bin: &str, password: &str) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    // `rclone obscure -` lit le secret sur stdin (pas d'exposition en argv).
    let mut child = Command::new(rclone_bin)
        .args(["obscure", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn `{rclone_bin} obscure` failed: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(password.as_bytes()).await;
        let _ = stdin.shutdown().await;
        drop(stdin);
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| format!("rclone obscure wait failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("rclone obscure failed: {}", err.trim()));
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        return Err("rclone obscure produced empty output".into());
    }
    Ok(s)
}

/// Liste les répertoires du partage (`rclone lsd <REMOTE>:<share>`) — smoke test
/// de connectivité SMB. Renvoie Ok(()) si la commande réussit.
pub async fn test_share(rclone_bin: &str, env: &ResticEnv, t: &FullTarget) -> Result<(), String> {
    let target = format!("{}:{}", REMOTE, t.share.trim_matches('/'));
    let mut cmd = Command::new(rclone_bin);
    cmd.args(["lsd", &target, "--contimeout=8s", "--timeout=20s", "--retries=1"]);
    for (k, v) in &env.vars {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn rclone lsd failed: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("rclone lsd échec: {}", err.trim().chars().take(400).collect::<String>()))
    }
}
