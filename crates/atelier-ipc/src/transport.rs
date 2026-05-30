/// Generic JSON-line transport over Unix socket.
/// Pattern: one connection per request (UI-frequency calls, no pool needed).
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::warn;

/// Send a request and receive a response via Unix socket (JSON-line protocol).
/// Wraps the actual I/O in a timeout.
pub async fn request<Req, Resp>(socket_path: &Path, req: &Req, timeout: Duration) -> Result<Resp>
where
    Req: Serialize,
    Resp: DeserializeOwned,
{
    let result = tokio::time::timeout(timeout, do_request(socket_path, req)).await;
    match result {
        Ok(inner) => inner,
        Err(_) => {
            warn!(socket = %socket_path.display(), "IPC request timed out");
            anyhow::bail!("IPC request timed out (socket: {})", socket_path.display());
        }
    }
}

async fn do_request<Req, Resp>(socket_path: &Path, req: &Req) -> Result<Resp>
where
    Req: Serialize,
    Resp: DeserializeOwned,
{
    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("IPC unavailable (socket: {})", socket_path.display()))?;

    let (reader, mut writer) = stream.into_split();

    // Write request as JSON line
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.shutdown().await?;

    // Read response
    let mut buf_reader = BufReader::new(reader);
    let mut response_line = String::new();
    buf_reader.read_line(&mut response_line).await?;

    if response_line.is_empty() {
        anyhow::bail!(
            "IPC returned empty response (socket: {})",
            socket_path.display()
        );
    }

    let resp: Resp = serde_json::from_str(response_line.trim())?;
    Ok(resp)
}
