//! Construction du pipeline de dump Postgres.
//!
//! `pg_dumpall` doit tourner en **superuser** (`postgres`) : `dataverse_admin`
//! ne peut pas lire `pg_authid` (rôles/mots de passe). Atelier tournant en root,
//! on bascule via `runuser -u postgres` (pas de sudoers requis), en streamant
//! directement le dump dans `restic backup --stdin` — aucun artefact local.

/// Quote single-quote-safe pour insertion dans un script `bash -c`.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Script bash : `pg_dumpall` (en tant que postgres) | `restic backup --stdin`.
/// `pipefail` propage un échec de `pg_dumpall` (sinon restic snapshoterait un
/// dump tronqué). L'env RESTIC_*/RCLONE_* est fourni au process bash parent.
pub fn pipeline_script(pg_dumpall_bin: &str, restic_bin: &str, run_user: &str) -> String {
    format!(
        "set -o pipefail; runuser -u {user} -- {pg} | {restic} backup --stdin --stdin-filename postgres.sql --tag postgres --json",
        user = shq(run_user),
        pg = shq(pg_dumpall_bin),
        restic = shq(restic_bin),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_has_pipefail_and_tag() {
        let s = pipeline_script("pg_dumpall", "restic", "postgres");
        assert!(s.contains("set -o pipefail"));
        assert!(s.contains("--tag postgres"));
        assert!(s.contains("--stdin-filename postgres.sql"));
        assert!(s.contains("runuser -u 'postgres'"));
    }

    #[test]
    fn shq_escapes_quotes() {
        assert_eq!(shq("a'b"), "'a'\\''b'");
    }
}
