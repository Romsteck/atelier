//! `hr-dataverse-migrate` — CLI thin wrapper around
//! [`hr_dataverse_migrate::migrate_with_manager`].
//!
//! In production the migration is normally triggered by an agent via
//! the MCP tool `db.migrate` (which calls the same library function
//! inside `hr-orchestrator`). This binary is the ops escape hatch for
//! one-off / ad-hoc migrations.
//!
//! Usage:
//! ```text
//! hr-dataverse-migrate \
//!   --slug wallet \
//!   --admin-url postgres://dataverse_admin:…@127.0.0.1:5432/postgres \
//!   --apps-root /opt/homeroute/apps \
//!   [--secrets-path /opt/homeroute/data/dataverse-secrets.json] \
//!   [--dry-run]                         # provision + copy then drop
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use tracing::{error, info};

use hr_dataverse::{DataverseManager, ProvisioningConfig};
use hr_dataverse_migrate::{MigrationReport, migrate_with_manager};

#[derive(Debug)]
struct CliOpts {
    slug: String,
    admin_url: String,
    apps_root: PathBuf,
    secrets_path: Option<PathBuf>,
    pg_host: String,
    pg_port: u16,
    dry_run: bool,
}

fn parse_args() -> Result<CliOpts> {
    let mut slug: Option<String> = None;
    let mut admin_url: Option<String> = std::env::var("HR_DATAVERSE_ADMIN_URL").ok();
    let mut apps_root = PathBuf::from("/opt/homeroute/apps");
    let mut secrets_path: Option<PathBuf> = None;
    let mut pg_host = "127.0.0.1".to_string();
    let mut pg_port: u16 = 5432;
    let mut dry_run = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--slug" => slug = args.next(),
            "--admin-url" => admin_url = args.next(),
            "--apps-root" => apps_root = PathBuf::from(args.next().unwrap_or_default()),
            "--secrets-path" => secrets_path = args.next().map(PathBuf::from),
            "--host" => pg_host = args.next().unwrap_or_else(|| "127.0.0.1".into()),
            "--port" => {
                pg_port = args
                    .next()
                    .ok_or_else(|| anyhow!("--port needs a value"))?
                    .parse()
                    .context("--port not an integer")?
            }
            "--dry-run" => dry_run = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {}", other),
        }
    }

    Ok(CliOpts {
        slug: slug.ok_or_else(|| anyhow!("--slug is required"))?,
        admin_url: admin_url
            .ok_or_else(|| anyhow!("--admin-url is required (or set HR_DATAVERSE_ADMIN_URL)"))?,
        apps_root,
        secrets_path,
        pg_host,
        pg_port,
        dry_run,
    })
}

fn print_help() {
    println!(
        "hr-dataverse-migrate — CLI ad-hoc migration tool (preferred path: agent calls db.migrate via MCP)\n\
         \n\
         Required:\n\
           --slug <name>             App slug (db.sqlite must exist under <apps-root>)\n\
           --admin-url <postgres://> Postgres DSN with CREATEDB+CREATEROLE\n\
                                     (or set HR_DATAVERSE_ADMIN_URL)\n\
         \n\
         Optional:\n\
           --apps-root <path>        default /opt/homeroute/apps\n\
           --secrets-path <path>     where to persist the secret JSON\n\
                                     (omit to skip persistence — useful for --dry-run)\n\
           --host <host>             PG host injected into per-app DSN (default 127.0.0.1)\n\
           --port <port>             PG port (default 5432)\n\
           --dry-run                 provision + copy + validate, then drop\n\
           -h, --help                show this help"
    );
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn")),
        )
        .init();

    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {}\nrun with --help for usage", e);
            return ExitCode::from(2);
        }
    };

    match run(opts).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("migration failed: {:#}", e);
            ExitCode::FAILURE
        }
    }
}

async fn run(opts: CliOpts) -> Result<()> {
    let cfg = ProvisioningConfig {
        host: opts.pg_host.clone(),
        port: opts.pg_port,
    };
    let manager = DataverseManager::connect_admin(
        opts.admin_url.clone(),
        cfg,
        opts.secrets_path.clone(),
    )
    .await
    .context("connect admin postgres")?;

    let sqlite_path = opts.apps_root.join(&opts.slug).join("db.sqlite");
    let report: MigrationReport = migrate_with_manager(&manager, &opts.slug, &sqlite_path).await?;

    print_report(&opts, &report);

    if opts.dry_run {
        info!(slug = %opts.slug, "dropping app (dry-run cleanup)");
        manager.drop_app(&opts.slug).await?;
    }
    Ok(())
}

fn print_report(opts: &CliOpts, report: &MigrationReport) {
    println!("\n=== migration report ===");
    println!("slug:           {}", opts.slug);
    println!(
        "destination:    postgres database `{}` (role `{}`)",
        report.secret.db_name, report.secret.role_name
    );
    if report.adopted_existing {
        println!("note:           ADOPTED an existing PG database (role password reset)");
    }
    for (table, count) in &report.copied {
        println!("  • {:30} {} rows", table, count);
    }
    if !report.skipped.is_empty() {
        println!("  skipped tables:");
        for s in &report.skipped {
            println!("    - {}", s);
        }
    }
    if opts.dry_run {
        println!("\nDRY RUN — postgres database has been dropped.");
    } else {
        println!("\nDB password (already persisted to secrets file if --secrets-path was set):");
        println!("  {}", report.secret.password);
        println!("\nDSN:");
        println!("  {}", report.secret.dsn);
        println!("\nNext steps for the app's agent:");
        println!("  1. Set apps[\"{}\"].db_backend = \"data-migrated\" in /opt/homeroute/data/apps.json", opts.slug);
        println!("  2. Restart hr-orchestrator (it will inject DATABASE_URL into the app at next start)");
        println!("  3. Refactor the app's source code to use DATABASE_URL");
        println!("  4. When validated end-to-end, set db_backend = \"postgres-dataverse\"");
    }
}
