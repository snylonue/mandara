//! Outbound HTTP for plugins.
//!
//! One import (`http.fetch`) backed by a synchronous ureq client — the
//! host provides plain network access, no destination policy:
//!
//! - **Timeout**: overall deadline = `min(requested,
//!   BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS)` (default 30 s).
//! - **Size**: response bodies are capped
//!   (`BOOKSHELF_PLUGIN_FETCH_MAX_BYTES`, default 64 MiB); oversized
//!   responses abort with `size-limit` and no partial data.
//! - **Redirects**: at most 5 hops, curl-style method downgrade on
//!   301–303 (handled by ureq).
//! - **Logging**: URLs are logged `scheme://host/path` only — query and
//!   fragment may carry secrets and must not reach the logs.
//!
//! 4xx/5xx are responses, not errors (the plugin decides what they
//! mean).

use std::io::Read;
use std::time::Duration;

use tracing::{info, warn};

/// Maximum number of redirect hops per `fetch` call.
pub const MAX_REDIRECTS: u32 = 5;

/// Operational caps for plugin `fetch` calls (not destination policy —
/// every host is reachable).
#[derive(Debug, Clone)]
pub struct FetchPolicy {
    /// Hard overall timeout per `fetch` call (ms). The plugin's requested
    /// timeout is clamped to this.
    pub timeout_ms: u64,
    /// Response body size cap (bytes). Oversized responses are aborted
    /// with `FetchError::SizeLimit` — no partial data is returned.
    pub max_bytes: u64,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        FetchPolicy {
            timeout_ms: 30_000,
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

impl FetchPolicy {
    /// Read the caps from the standard environment variables:
    ///
    /// - `BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS` — hard timeout cap
    ///   (default 30 000).
    /// - `BOOKSHELF_PLUGIN_FETCH_MAX_BYTES` — response size cap
    ///   (default 64 MiB, aligned with `BOOKSHELF_MAX_UPLOAD_MB`).
    pub fn from_env() -> Self {
        let parse = |name: &str, default: u64| -> u64 {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(default)
        };
        FetchPolicy {
            timeout_ms: parse("BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS", 30_000),
            max_bytes: parse("BOOKSHELF_PLUGIN_FETCH_MAX_BYTES", 64 * 1024 * 1024),
        }
    }
}

/// A request as the plugin described it (decoded from the WIT `request`).
#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub method: String,
    pub url: String,
    /// (name, value) pairs, exactly what the plugin sent.
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// The plugin's desired timeout; clamped to the policy cap.
    pub timeout_ms: Option<u64>,
}

/// The response as received. `status` is the raw HTTP status — 4xx/5xx
/// are responses, not errors (the plugin decides what they mean).
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// URL after all redirect hops.
    pub final_url: String,
}

/// Transport-level outcome (the WIT `fetch-error` variant).
#[derive(Debug, Clone)]
pub enum FetchError {
    /// Permanent: URL parse/protocol problem.
    InvalidUrl(String),
    /// Permanent: host policy rejected the URL. Never produced today —
    /// the host applies no destination policy — kept for WIT compat.
    Denied(String),
    /// Permanent: too many redirect hops.
    RedirectLimit(u32),
    /// Transient: the call exceeded its (capped) timeout.
    Timeout(u64),
    /// Permanent: response exceeded the size cap.
    SizeLimit(u64),
    /// Transient-if-connect: DNS/TLS/connection problem.
    Transport(String),
}

/// Headers the host manages itself; forwarding the plugin's own copies
/// would be ignored at best and confuse ureq at worst.
fn is_host_managed_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host" | "content-length" | "transfer-encoding" | "connection"
    )
}

fn map_transport(t: ureq::Transport, policy_timeout_ms: u64) -> FetchError {
    let msg = format!("{t}");
    let lower = msg.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("too slow") {
        return FetchError::Timeout(policy_timeout_ms);
    }
    match t.kind() {
        ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme => FetchError::InvalidUrl(msg),
        ureq::ErrorKind::TooManyRedirects => FetchError::RedirectLimit(MAX_REDIRECTS),
        _ => FetchError::Transport(msg),
    }
}

fn read_body(resp: ureq::Response, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
    let mut limited = resp.into_reader().take(max_bytes + 1);
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| FetchError::Transport(format!("reading response body: {e}")))?;
    if bytes.len() as u64 > max_bytes {
        return Err(FetchError::SizeLimit(max_bytes));
    }
    Ok(bytes)
}

fn response_headers(resp: &ureq::Response) -> Vec<(String, String)> {
    resp.headers_names()
        .into_iter()
        .map(|name| {
            let value = resp.header(&name).unwrap_or_default().to_string();
            (name, value)
        })
        .collect()
}

/// `format!`-safe URL for logs and errors: query and fragment stripped
/// (they may carry secrets).
fn log_url(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).to_string()
}

/// Blocking `fetch`. Runs inline on the calling thread; the server calls
/// plugins inside `spawn_blocking`, so a slow source never stalls a tokio
/// worker.
pub fn fetch(policy: &FetchPolicy, request: FetchRequest) -> Result<FetchResponse, FetchError> {
    let capped_timeout = request
        .timeout_ms
        .unwrap_or(policy.timeout_ms)
        .min(policy.timeout_ms);

    let mut method = request.method.trim().to_uppercase();
    if method.is_empty() {
        method = "GET".into();
    }
    let agent = ureq::AgentBuilder::new()
        .redirects(MAX_REDIRECTS)
        .timeout(Duration::from_millis(capped_timeout))
        .build();
    let mut req = agent.request(&method, &request.url);
    for (name, value) in &request.headers {
        if !is_host_managed_header(name) {
            req = req.set(name, value);
        }
    }
    use ureq::OrAnyStatus;
    let result = match &request.body {
        Some(bytes) if !bytes.is_empty() => req.send_bytes(bytes).or_any_status(),
        _ => req.call().or_any_status(),
    };

    match result {
        Ok(resp) => {
            let status = resp.status();
            let final_url = resp.get_url().to_string();
            let headers = response_headers(&resp);
            let bytes = read_body(resp, policy.max_bytes)?;
            info!(
                fetch = %log_url(&request.url),
                status,
                bytes = bytes.len(),
                "plugin fetch ok"
            );
            Ok(FetchResponse {
                status,
                headers,
                body: bytes,
                final_url,
            })
        }
        Err(t) => {
            let fetched = log_url(&request.url);
            match map_transport(t, policy.timeout_ms) {
                FetchError::Timeout(_) => {
                    warn!(fetch = %fetched, "plugin fetch timed out");
                    Err(FetchError::Timeout(policy.timeout_ms))
                }
                e => {
                    warn!(fetch = %fetched, "plugin fetch failed: {e:?}");
                    Err(e)
                }
            }
        }
    }
}
