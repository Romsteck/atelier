//! `hr-dv-codegen` — generate a typed Rust client crate (`dv-{slug}`)
//! from an app's dataverse `$schema`.
//!
//! Usage:
//! ```sh
//! hr-dv-codegen --slug wallet \
//!     --base-url http://127.0.0.1:4000/api/dv/wallet \
//!     --token "$HR_DV_TOKEN" \
//!     --output /opt/homeroute/apps/wallet/src/server/dv-client
//! ```
//!
//! Output:
//! - `Cargo.toml`           (committed, stable)
//! - `.gitignore`           (committed, ignores generated `src/`)
//! - `schema.lock`          (committed: schema_version + sha256)
//! - `src/lib.rs`           (generated, gitignored)

mod generator;

use anyhow::{Context, Result};
use clap::Parser;
use sha2::Digest;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version, about = "Generate a typed dv-{slug} crate from a dataverse $schema")]
struct Args {
    /// App slug (e.g. wallet, trader). Used for naming and the `Cargo.toml`.
    #[arg(long)]
    slug: String,

    /// Gateway base URL (`https://dv.mynetwk.biz/{slug}` or
    /// `http://127.0.0.1:4000/api/dv/{slug}`). Mutually exclusive with
    /// `--schema-file`.
    #[arg(long)]
    base_url: Option<String>,

    /// Bearer token for fetching the schema. Required when using
    /// `--base-url` (the gateway always requires auth).
    #[arg(long, env = "HR_DV_TOKEN")]
    token: Option<String>,

    /// Read the schema from a local JSON file instead of HTTPS.
    /// Mutually exclusive with `--base-url`.
    #[arg(long)]
    schema_file: Option<PathBuf>,

    /// Output directory for the generated crate.
    #[arg(long)]
    output: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let schema_json = if let Some(f) = &args.schema_file {
        std::fs::read_to_string(f).with_context(|| format!("read {}", f.display()))?
    } else {
        let url = args
            .base_url
            .as_deref()
            .context("--base-url or --schema-file required")?;
        let token = args
            .token
            .as_deref()
            .context("--token required when fetching from gateway")?;
        let endpoint = format!("{}/$schema", url.trim_end_matches('/'));
        let resp = reqwest::Client::builder()
            .danger_accept_invalid_certs(false)
            .build()?
            .get(&endpoint)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .with_context(|| format!("GET {}", endpoint))?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("gateway returned {}: {}", status, body);
        }
        body
    };

    let schema: hr_dataverse::DatabaseSchema = serde_json::from_str(&schema_json)
        .with_context(|| "deserialise schema JSON into DatabaseSchema")?;
    let hash = sha256_hex(&schema_json);

    let lib_rs = generator::generate_lib(&args.slug, &schema)?;
    let cargo_toml = generator::generate_cargo_toml(&args.slug);
    let gitignore = "src/\n";
    let lock = format!(
        "schema_version={}\nschema_sha256={}\nslug={}\n",
        schema.version, hash, args.slug
    );

    std::fs::create_dir_all(&args.output)
        .with_context(|| format!("mkdir {}", args.output.display()))?;
    std::fs::create_dir_all(args.output.join("src"))?;

    write_if_different(&args.output.join("Cargo.toml"), &cargo_toml)?;
    write_if_different(&args.output.join(".gitignore"), gitignore)?;
    write_if_different(&args.output.join("schema.lock"), &lock)?;
    write_if_different(&args.output.join("src/lib.rs"), &lib_rs)?;

    println!(
        "✓ dv-{} regenerated (schema_version={}, sha256={}, tables={})",
        args.slug,
        schema.version,
        &hash[..16],
        schema.tables.len()
    );
    Ok(())
}

fn sha256_hex(s: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// Atomic-ish file write: only touch the file if its contents change.
/// Avoids spurious `mtime` bumps that would re-trigger downstream
/// `cargo build` invocations.
fn write_if_different(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(());
        }
    }
    std::fs::write(path, content)
}
