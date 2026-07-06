//! `atelier-dv-codegen` — generate a typed Rust client crate (`dv-{slug}`)
//! from an app's dataverse `$schema`.
//!
//! Usage:
//! ```sh
//! atelier-dv-codegen --slug wallet \
//!     --base-url http://127.0.0.1:4100/api/dv/wallet \
//!     --token "$HR_DV_TOKEN" \
//!     --output /var/lib/atelier/apps/wallet/src/server/dv-client
//! ```
//!
//! In-process, Atelier regenerates the same crate via the `dv_regen_client`
//! MCP tool (no HTTP/token) — both paths share `atelier_dv_codegen::generate_crate`.
//!
//! Output:
//! - `Cargo.toml`           (committed, stable)
//! - `.gitignore`           (committed, ignores generated `src/`)
//! - `schema.lock`          (committed: schema_version + sha256)
//! - `src/lib.rs`           (generated, gitignored)

use anyhow::{Context, Result};
use atelier_dv_codegen::{generate_crate, write_crate};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version, about = "Generate a typed dv-{slug} crate from a dataverse $schema")]
struct Args {
    /// App slug (e.g. wallet, trader). Used for naming and the `Cargo.toml`.
    #[arg(long)]
    slug: String,

    /// Gateway base URL (`https://dv.mynetwk.biz/{slug}` or
    /// `http://127.0.0.1:4100/api/dv/{slug}`). Mutually exclusive with
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

    let schema: atelier_dataverse::DatabaseSchema = serde_json::from_str(&schema_json)
        .with_context(|| "deserialise schema JSON into DatabaseSchema")?;

    let gc = generate_crate(&args.slug, &schema)?;
    let changed = write_crate(&args.output, &gc)
        .with_context(|| format!("write crate into {}", args.output.display()))?;

    println!(
        "✓ dv-{} regenerated (schema_version={}, sha256={}, tables={}, changed={})",
        args.slug,
        gc.schema_version,
        &gc.schema_sha256[..16],
        schema.tables.len(),
        if changed.is_empty() { "none".to_string() } else { changed.join(",") },
    );
    Ok(())
}
