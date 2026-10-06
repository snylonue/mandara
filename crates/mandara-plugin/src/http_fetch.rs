//! Outbound HTTP for plugins.
//!
//! One import (`http.fetch`) backed by a synchronous ureq 3 client — the
//! host provides plain network access (every host is reachable, no
//! destination policy; outbound requests honor the standard proxy
//! environment variables):
//!
//! - **Timeout**: overall deadline = `min(requested,
//!   MANDARA_PLUGIN_FETCH_TIMEOUT_MS)` (default 30 s).
//! - **Size**: response bodies are capped
//!   (`MANDARA_PLUGIN_FETCH_MAX_BYTES`, default 64 MiB); oversized
//!   responses abort with `size-limit` and no partial data.
//! - **Redirects**: at most 5 hops, with curl-style method downgrade on
//!   301–303. Explicit Cookie/authorization headers are retained only for
//!   same-origin redirects.
//! - **Proxy**: outbound requests honor the standard environment
//!   variables (`ALL_PROXY` / `HTTPS_PROXY` / `HTTP_PROXY`; `NO_PROXY`
//!   exempts hosts) — ureq 3's default `proxy-from-env` behavior. Useful
//!   where the server has no direct internet route.
//! - **Logging**: URLs are logged `scheme://host/path` only — query and
//!   fragment may carry secrets and must not reach the logs.
//!
//! 4xx/5xx are responses, not errors (the plugin decides what they
//! mean; status-as-error is disabled on the agent).

use std::time::Duration;

use tracing::{info, warn};
use ureq::http::{HeaderName, HeaderValue};
use url::Url;

/// Maximum number of redirect hops per `fetch` call.
pub const MAX_REDIRECTS: u32 = 5;

/// Operational caps for plugin `fetch` calls (timeout/size only).
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
    /// - `MANDARA_PLUGIN_FETCH_TIMEOUT_MS` — hard timeout cap
    ///   (default 30 000).
    /// - `MANDARA_PLUGIN_FETCH_MAX_BYTES` — response size cap
    ///   (default 64 MiB, aligned with `MANDARA_MAX_UPLOAD_MB`).
    pub fn from_env() -> Self {
        let parse = |name: &str, default: u64| -> u64 {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(default)
        };
        FetchPolicy {
            timeout_ms: parse("MANDARA_PLUGIN_FETCH_TIMEOUT_MS", 30_000),
            max_bytes: parse("MANDARA_PLUGIN_FETCH_MAX_BYTES", 64 * 1024 * 1024),
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
    /// kept for WIT compat.
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
/// would be ignored at best and conflict at worst.
fn is_host_managed_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host" | "content-length" | "transfer-encoding" | "connection"
    )
}

fn map_error(e: ureq::Error, policy_timeout_ms: u64) -> FetchError {
    match e {
        ureq::Error::Timeout(_) => FetchError::Timeout(policy_timeout_ms),
        ureq::Error::BadUri(msg) => FetchError::InvalidUrl(msg),
        ureq::Error::TooManyRedirects => FetchError::RedirectLimit(MAX_REDIRECTS),
        ureq::Error::RedirectFailed => FetchError::RedirectLimit(MAX_REDIRECTS),
        ureq::Error::BodyExceedsLimit(limit) => FetchError::SizeLimit(limit),
        ureq::Error::InvalidProxyUrl => {
            FetchError::Denied("invalid proxy URL in environment".into())
        }
        ureq::Error::HostNotFound => FetchError::Transport("host not found".into()),
        ureq::Error::Http(e) => FetchError::Transport(format!("http: {e}")),
        ureq::Error::Protocol(e) => FetchError::Transport(format!("protocol: {e}")),
        ureq::Error::Io(e) => FetchError::Transport(format!("io: {e}")),
        other => FetchError::Transport(format!("{other}")),
    }
}

fn log_url(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).to_string()
}

fn resolve_redirect(from: &str, location: &str) -> Result<(String, bool), FetchError> {
    let base = Url::parse(from).map_err(|e| FetchError::InvalidUrl(e.to_string()))?;
    let next = base
        .join(location)
        .map_err(|e| FetchError::InvalidUrl(e.to_string()))?;
    let same_origin = base.scheme() == next.scheme()
        && base.host_str() == next.host_str()
        && base.port_or_known_default() == next.port_or_known_default();
    Ok((next.into(), same_origin))
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn keep_redirect_header(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "cookie" | "authorization" | "proxy-authorization" | "referer"
    )
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

    // Proxy selection from the environment (ALL_PROXY/HTTPS_PROXY/
    // HTTP_PROXY + NO_PROXY) is ureq 3's default agent behavior.
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            // Redirects are followed below so explicit same-origin Cookie headers
            // survive the hop; ureq intentionally strips them automatically.
            .max_redirects(0)
            .timeout_global(Some(Duration::from_millis(capped_timeout)))
            // 4xx/5xx are plain responses for plugins, never errors.
            .http_status_as_error(false)
            .build(),
    );

    // Plugin-controlled headers, host-managed ones dropped, invalid
    // ones skipped (they would fail the whole request).
    let headers: Vec<(String, String)> = request
        .headers
        .iter()
        .filter(|(name, _)| !is_host_managed_header(name))
        .filter(|(name, value)| {
            name.parse::<HeaderName>().is_ok() && value.parse::<HeaderValue>().is_ok()
        })
        .cloned()
        .collect();
    if headers.len()
        < request.headers.len()
            - request
                .headers
                .iter()
                .filter(|(n, _)| is_host_managed_header(n))
                .count()
    {
        warn!(fetch = %log_url(&request.url), "plugin fetch: dropping invalid header(s)");
    }

    let send = |with_bytes: bool| -> Result<FetchResponse, FetchError> {
        let mut current_url = request.url.clone();
        let mut current_method = method.clone();
        let mut current_body = if with_bytes {
            request.body.clone()
        } else {
            None
        };
        let mut current_headers = headers.clone();

        for hop in 0..=MAX_REDIRECTS {
            let mut builder = ureq::http::Request::builder()
                .method(current_method.as_str())
                .uri(&current_url);
            for (name, value) in &current_headers {
                builder = builder.header(name, value);
            }
            // Each branch keeps its own concrete body type (unit vs byte slice).
            let result = match current_body.as_deref() {
                Some(bytes) if !bytes.is_empty() => {
                    agent.run(builder.body(bytes).map_err(invalid_request)?)
                }
                _ => agent.run(builder.body(()).map_err(invalid_request)?),
            };
            let response = match result {
                Ok(response) => response,
                Err(error) => return finish(Err(error), policy, &current_url),
            };

            let status = response.status().as_u16();
            let location = response
                .headers()
                .get("Location")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if !is_redirect(status) || location.is_none() {
                return finish(Ok(response), policy, &current_url);
            }
            if hop == MAX_REDIRECTS {
                return Err(FetchError::RedirectLimit(MAX_REDIRECTS));
            }

            let (next_url, same_origin) =
                resolve_redirect(&current_url, location.as_deref().unwrap())?;
            // Consume redirect bodies before issuing the next request.
            read_body(response, policy.max_bytes)?;
            if !same_origin {
                current_headers.retain(|(name, _)| keep_redirect_header(name));
            }
            if status == 303
                || ((status == 301 || status == 302)
                    && current_method != "GET"
                    && current_method != "HEAD")
            {
                current_method = "GET".into();
                current_body = None;
            }
            current_url = next_url;
        }

        unreachable!("redirect loop returns or errors within the hop bound")
    };

    match &request.body {
        Some(bytes) if !bytes.is_empty() => send(true),
        _ => send(false),
    }
}

fn invalid_request(e: ureq::http::Error) -> FetchError {
    FetchError::InvalidUrl(format!("invalid request: {e}"))
}

fn finish(
    result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    policy: &FetchPolicy,
    url: &str,
) -> Result<FetchResponse, FetchError> {
    match result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            use ureq::ResponseExt;
            let final_url = resp.get_uri().to_string();
            let response_headers: Vec<(String, String)> = resp
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        v.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            let bytes = read_body(resp, policy.max_bytes)?;
            info!(
                fetch = %log_url(url),
                status,
                bytes = bytes.len(),
                "plugin fetch ok"
            );
            Ok(FetchResponse {
                status,
                headers: response_headers,
                body: bytes,
                final_url,
            })
        }
        Err(e) => {
            let fetched = log_url(url);
            match map_error(e, policy.timeout_ms) {
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

fn read_body(
    resp: ureq::http::Response<ureq::Body>,
    max_bytes: u64,
) -> Result<Vec<u8>, FetchError> {
    let mut body = resp.into_body();
    match body.with_config().limit(max_bytes).read_to_vec() {
        Ok(bytes) => Ok(bytes),
        Err(ureq::Error::BodyExceedsLimit(_)) => Err(FetchError::SizeLimit(max_bytes)),
        Err(e) => Err(map_error(e, u64::MAX)),
    }
}
