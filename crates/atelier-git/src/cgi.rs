use std::path::Path;

use anyhow::{Context, bail};
use tokio::process::Command;
use tracing::error;

use crate::types::CgiResponse;

const GIT_HTTP_BACKEND: &str = "/usr/lib/git-core/git-http-backend";

/// Execute `git http-backend` as a CGI process and parse its response.
pub async fn git_cgi(
    repos_dir: &Path,
    path_info: &str,
    query_string: &str,
    method: &str,
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<CgiResponse> {
    let mut cmd = Command::new(GIT_HTTP_BACKEND);
    cmd.env("GIT_PROJECT_ROOT", repos_dir.as_os_str())
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path_info)
        .env("QUERY_STRING", query_string)
        .env("REQUEST_METHOD", method)
        .env("CONTENT_TYPE", content_type);

    if !body.is_empty() {
        cmd.env("CONTENT_LENGTH", body.len().to_string());
        cmd.stdin(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn git http-backend")?;

    // Write body to stdin if present
    if !body.is_empty() {
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(body)
                .await
                .context("Failed to write body to git http-backend stdin")?;
            drop(stdin);
        }
    }

    let output = child
        .wait_with_output()
        .await
        .context("Failed to wait for git http-backend")?;

    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(stderr = %stderr, "git http-backend failed");
        bail!("git http-backend failed: {stderr}");
    }

    parse_cgi_response(&output.stdout)
}

/// Parse a CGI response (headers + body separated by empty line).
fn parse_cgi_response(raw: &[u8]) -> anyhow::Result<CgiResponse> {
    // Find the header/body boundary: \r\n\r\n or \n\n
    let (header_end, body_start) =
        find_header_boundary(raw).context("Failed to find header/body boundary in CGI response")?;

    let header_bytes = &raw[..header_end];
    let body = raw[body_start..].to_vec();

    let header_str = String::from_utf8_lossy(header_bytes);

    let mut status: u16 = 200;
    let mut content_type = String::new();
    let mut headers = Vec::new();

    for line in header_str.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();

            match key.to_lowercase().as_str() {
                "status" => {
                    // Status line format: "200 OK" or just "200"
                    if let Some(code_str) = value.split_whitespace().next() {
                        status = code_str.parse::<u16>().unwrap_or(200);
                    }
                }
                "content-type" => {
                    content_type = value.to_string();
                }
                _ => {
                    headers.push((key.to_string(), value.to_string()));
                }
            }
        }
    }

    Ok(CgiResponse {
        status,
        content_type,
        headers,
        body,
    })
}

/// Find the boundary between CGI headers and body.
/// Returns (header_end_offset, body_start_offset).
fn find_header_boundary(data: &[u8]) -> Option<(usize, usize)> {
    // Check for \r\n\r\n
    if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some((pos, pos + 4));
    }

    // Check for \n\n
    if let Some(pos) = data.windows(2).position(|w| w == b"\n\n") {
        return Some((pos, pos + 2));
    }

    None
}
