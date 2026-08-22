//! Host-controlled outbound HTTP for plugins (design doc
//! `docs/plugin-http-api-design.md` §3).
//!
//! One import (`http.fetch`) backed by a synchronous ureq client. All
//! policy lives here:
//!
//! - **Allow list**: `BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS`; empty = every
//!   `fetch` is denied. Entries are `host` (any port) or `host:port`.
//! - **Scheme gate**: `BOOKSHELF_PLUGIN_FETCH_HTTP=false` turns plain
//!   http off (self-hosted sources often use http, so it defaults on).
//! - **SSRF**: enforced *at resolve time* — the same addresses ureq then
//!   connects to (no DNS-rebinding TOCTOU) — and re-checked (scheme +
//!   allow-list + addresses) before every redirect hop. Private,
//!   loopback, link-local and ULA addresses are refused unless the host
//!   itself is allow-listed.
//! - **Redirects**: at most 5 hops; a hop leaving the original host
//!   loses credential-ish headers (`authorization`, `cookie`,
//!   `proxy-authorization`, `x-api-key`).
//! - **Timeout**: overall deadline = `min(requested, policy cap)`.
//! - **Size**: response bodies are capped (`BOOKSHELF_PLUGIN_FETCH_MAX_BYTES`,
//!   default 64 MiB); oversized responses abort with `size-limit` and no
//!   partial data.
//! - **Logging**: URLs are logged `scheme://host/path` only (query and
//!   fragment stripped; credentials in URLs are rejected outright).
//!
//! The guest never gets ambient credentials: only the headers it sent are
//! forwarded, and headers ureq manages itself (`host`, `content-length`,
//! `transfer-encoding`, `connection`) are stripped.

use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ipnet::IpNet;
use tracing::{info, warn};
use url::Url;

/// Maximum number of redirect hops per `fetch` call.
pub const MAX_REDIRECTS: u32 = 5;

/// How far a plugin instance's `fetch` may reach (host policy).
#[derive(Debug, Clone)]
pub struct FetchPolicy {
    /// `host` (any port) or `host:port` entries. Empty = every `fetch`
    /// call is denied. An allow-listed host may resolve to private
    /// addresses (e.g. `localhost:8081` for a local test source).
    pub allowed_hosts: Vec<String>,
    /// Allow `http://` URLs (self-hosted sources are often plain http).
    pub http_allowed: bool,
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
            allowed_hosts: Vec::new(),
            http_allowed: true,
            timeout_ms: 30_000,
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

impl FetchPolicy {
    /// Read the policy from the standard environment variables:
    ///
    /// - `BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS` — comma-separated hosts
    ///   (optionally `host:port`); empty = all `fetch` calls denied.
    /// - `BOOKSHELF_PLUGIN_FETCH_HTTP` — `false`/`0` turns http off
    ///   (default: on, the allow list is the real gate).
    /// - `BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS` — hard timeout cap
    ///   (default 30 000).
    /// - `BOOKSHELF_PLUGIN_FETCH_MAX_BYTES` — response size cap
    ///   (default 64 MiB, aligned with `BOOKSHELF_MAX_UPLOAD_MB`).
    pub fn from_env() -> Self {
        use std::env;
        let parse = |name: &str, default: u64| -> u64 {
            env::var(name)
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(default)
        };
        FetchPolicy {
            allowed_hosts: env::var("BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS")
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            http_allowed: env::var("BOOKSHELF_PLUGIN_FETCH_HTTP")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
            timeout_ms: parse("BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS", 30_000),
            max_bytes: parse("BOOKSHELF_PLUGIN_FETCH_MAX_BYTES", 64 * 1024 * 1024),
        }
    }

    /// Does an allow-list entry let this host:port through? An entry
    /// without a port matches any port of the host, `host:port` restricts
    /// to that exact endpoint.
    pub fn allows_host(&self, host: &str, port: u16) -> bool {
        self.allowed_hosts
            .iter()
            .any(|entry| match entry.rsplit_once(':') {
                Some((h, p)) => h == host && p.parse::<u16>().ok() == Some(port),
                None => entry == host,
            })
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
    /// URL after all redirect hops (host-validated).
    pub final_url: String,
}

/// Transport-level outcome (the WIT `fetch-error` variant).
#[derive(Debug, Clone)]
pub enum FetchError {
    /// Permanent: URL parse/protocol problem.
    InvalidUrl(String),
    /// Permanent: host policy rejected the URL.
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

/// Address families a plugin fetch may never reach (unless the host is
/// allow-listed): RFC 1918 + loopback + link-local + CGNAT + ULA +
/// multicast/reserved/unspecified.
const BLOCKED_NETS: &[&str] = &[
    "0.0.0.0/8",      // "this network"
    "10.0.0.0/8",     // RFC 1918
    "100.64.0.0/10",  // CGNAT
    "127.0.0.0/8",    // loopback
    "169.254.0.0/16", // link-local
    "172.16.0.0/12",  // RFC 1918
    "192.0.0.0/24",   // IETF protocol assignments
    "192.168.0.0/16", // RFC 1918
    "198.18.0.0/15",  // benchmarking
    "224.0.0.0/4",    // multicast
    "240.0.0.0/4",    // reserved
    "::/128",         // unspecified
    "::1/128",        // loopback
    "64:ff9b::/96",   // NAT64 (could reach the blocked v4 space)
    "fc00::/7",       // ULA
    "fe80::/10",      // link-local
];

/// Parsed [`BLOCKED_NETS`], built once.
fn blocked_nets() -> &'static [IpNet] {
    use std::sync::OnceLock;
    static NETS: OnceLock<Vec<IpNet>> = OnceLock::new();
    NETS.get_or_init(|| {
        BLOCKED_NETS
            .iter()
            .map(|net| net.parse::<IpNet>().expect("hardcoded CIDR is valid"))
            .collect()
    })
}

fn is_public_addr(ip: IpAddr) -> bool {
    // v4-mapped v6 addresses carry an IPv4 payload; check that one
    // (::ffff:127.0.0.1 is a loopback in disguise).
    match ip {
        IpAddr::V4(v4) => !blocked_nets()
            .iter()
            .any(|net| net.contains(&IpAddr::V4(v4))),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => !blocked_nets()
                .iter()
                .any(|net| net.contains(&IpAddr::V4(v4))),
            None => !blocked_nets()
                .iter()
                .any(|net| net.contains(&IpAddr::V6(v6))),
        },
    }
}

/// Split ureq's `netloc` (`host:port`, possibly `[v6]:port`) — never
/// fails, so a weird netloc can't bypass the policy.
fn split_netloc(netloc: &str) -> (String, u16) {
    if let Some(rest) = netloc.strip_prefix('[') {
        if let Some((host, port)) = rest.split_once("]:") {
            return (host.to_string(), port.parse().unwrap_or(80));
        }
        return (rest.trim_end_matches(']').to_string(), 80);
    }
    match netloc.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(80)),
        None => (netloc.to_string(), 80),
    }
}

/// Scheme + allow-list + resolve-time SSRF check of one URL, with a
/// *nice* error for the common cases. The enforcement point (no TOCTOU)
/// is the resolver passed to ureq, which only ever hands over validated
/// addresses; this check additionally rejects before the connection.
fn check_url(policy: &FetchPolicy, url: &Url) -> Result<(), FetchError> {
    let host = url
        .host_str()
        .ok_or_else(|| FetchError::InvalidUrl("URL has no host".into()))?;
    match url.scheme() {
        "https" => {}
        "http" if policy.http_allowed => {}
        "http" => {
            return Err(FetchError::Denied(
                "http is disabled by host policy (BOOKSHELF_PLUGIN_FETCH_HTTP)".into(),
            ));
        }
        scheme => {
            return Err(FetchError::InvalidUrl(format!(
                "unsupported URL scheme `{scheme}`"
            )));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FetchError::InvalidUrl(
            "URL must not contain credentials; put them in plugin config".into(),
        ));
    }
    let port = url.port_or_known_default().unwrap_or(80);
    if policy.allows_host(host, port) {
        return Ok(()); // allow-listed host may resolve anywhere
    }
    let netloc = format!("{host}:{port}");
    let addrs: Vec<SocketAddr> = ToSocketAddrs::to_socket_addrs(&netloc)
        .map_err(|e| FetchError::Transport(format!("resolving `{netloc}`: {e}")))?
        .collect();
    if addrs.iter().all(|a| !is_public_addr(a.ip())) {
        return Err(FetchError::Denied(format!(
            "host `{host}` is not allow-listed and resolves only to private/loopback addresses"
        )));
    }
    Ok(())
}

/// ureq resolver: the security enforcement point. Called right before
/// each connection attempt, it returns exactly the addresses ureq will
/// connect to — validating here closes the DNS-rebinding window.
fn ssrf_resolver(
    policy: Arc<FetchPolicy>,
) -> impl Fn(&str) -> std::io::Result<Vec<SocketAddr>> + Send + Sync {
    move |netloc: &str| {
        let (host, port) = split_netloc(netloc);
        if policy.allows_host(&host, port) {
            return netloc.to_socket_addrs().map(|i| i.collect());
        }
        let all = netloc.to_socket_addrs()?;
        let public: Vec<SocketAddr> = all.filter(|a| is_public_addr(a.ip())).collect();
        if public.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "bookshelf: host `{host}` is not allow-listed and resolves only to private/loopback addresses"
                ),
            ));
        }
        Ok(public)
    }
}

/// Headers the host manages itself, or that could smuggle a different
/// destination / body than the one the URL spells out.
fn is_host_managed_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host" | "content-length" | "transfer-encoding" | "connection"
    )
}

/// Credential-ish headers dropped when a redirect leaves the original
/// host.
fn is_credential_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "proxy-authorization" | "x-api-key"
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
        ureq::ErrorKind::Dns if msg.contains("bookshelf:") => FetchError::Denied(
            msg.split_once("bookshelf:")
                .map(|(_, r)| r.trim().to_string())
                .unwrap_or(msg),
        ),
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

/// Log URL without query/fragment (secrets must not reach the logs).
fn log_url(url: &Url) -> String {
    let mut out = format!("{}://{}", url.scheme(), url.host_str().unwrap_or("?"));
    if let Some(port) = url.port() {
        out.push_str(&format!(":{port}"));
    }
    out.push_str(url.path());
    out
}

/// Blocking `fetch` with the full host policy applied. Runs inline on the
/// calling thread; the server calls plugins inside `spawn_blocking`, so a
/// slow source never stalls a tokio worker.
pub fn fetch(policy: &FetchPolicy, request: FetchRequest) -> Result<FetchResponse, FetchError> {
    let deadline = Instant::now() + Duration::from_millis(policy.timeout_ms);
    let capped_timeout = request
        .timeout_ms
        .unwrap_or(policy.timeout_ms)
        .min(policy.timeout_ms);

    let original = Url::parse(&request.url).map_err(|e| {
        FetchError::InvalidUrl(format!("`{}`: {e}", log_fragment(&request.url, None)))
    })?;
    check_url(policy, &original)?;
    let original_host = original.host_str().unwrap_or_default().to_string();
    let original_port = original.port();

    let mut method = request.method.trim().to_uppercase();
    if method.is_empty() {
        method = "GET".into();
    }
    let mut url = original;
    // Host-managed headers are never forwarded; everything else the
    // plugin sent goes through (minus credential-ish ones on cross-host
    // hops, see below).
    let mut headers: Vec<(String, String)> = request
        .headers
        .iter()
        .filter(|(n, _)| !is_host_managed_header(n))
        .cloned()
        .collect();
    let mut body = request.body.clone();
    let mut hops: u32 = 0;

    loop {
        // Per-hop: deadline, then scheme/allow-list/SSRF.
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(FetchError::Timeout(policy.timeout_ms));
        }
        check_url(policy, &url)?;

        let hop_timeout = remaining.min(Duration::from_millis(capped_timeout));
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(hop_timeout)
            .resolver(ssrf_resolver(Arc::new(policy.clone())))
            .build();
        let mut req = agent.request(&method, url.as_str());
        for (name, value) in &headers {
            req = req.set(name, value);
        }
        use ureq::OrAnyStatus;
        let result = match &body {
            Some(bytes) if !bytes.is_empty() => req.send_bytes(bytes).or_any_status(),
            _ => req.call().or_any_status(),
        };

        match result {
            Ok(resp) => {
                let status = resp.status();
                let location = if (300..400).contains(&status) {
                    resp.header("location").map(|s| s.to_string())
                } else {
                    None
                };
                if ((301..=303).contains(&status) || (307..=308).contains(&status))
                    && let Some(location) = location
                {
                    if hops >= MAX_REDIRECTS {
                        return Err(FetchError::RedirectLimit(MAX_REDIRECTS));
                    }
                    let next = url.join(&location).map_err(|e| {
                        FetchError::InvalidUrl(format!("bad redirect `{location}`: {e}"))
                    })?;
                    // The hop itself is a new request: check it too.
                    check_url(policy, &next)?;
                    match status {
                        // curl-style: POST/PUT etc. become GET.
                        301..=303 if method != "HEAD" => {
                            method = "GET".into();
                            body = None;
                        }
                        307 | 308 => {} // keep method + body
                        _ => {
                            body = None;
                        }
                    }
                    let same_origin = next.host_str() == Some(original_host.as_str())
                        && next.port() == original_port;
                    if !same_origin {
                        headers.retain(|(n, _)| !is_credential_header(n));
                    }
                    info!(
                        fetch = %log_url(&url),
                        hop = hops + 1,
                        status,
                        location = %log_fragment(&location, Some(&next)),
                        "plugin fetch redirect"
                    );
                    url = next;
                    hops += 1;
                    continue;
                }
                // 3xx without Location (304 etc.): a plain response.
                let final_url = url.as_str().to_string();
                let resp_headers = response_headers(&resp);
                let bytes = read_body(resp, policy.max_bytes)?;
                info!(
                    fetch = %log_url(&url),
                    status,
                    bytes = bytes.len(),
                    "plugin fetch ok"
                );
                return Ok(FetchResponse {
                    status,
                    headers: resp_headers,
                    body: bytes,
                    final_url,
                });
            }
            Err(t) => {
                let fetched = log_url(&url);
                match map_transport(t, policy.timeout_ms) {
                    FetchError::Timeout(_) => {
                        warn!(fetch = %fetched, "plugin fetch timed out");
                        return Err(FetchError::Timeout(policy.timeout_ms));
                    }
                    e @ (FetchError::Denied(_) | FetchError::InvalidUrl(_)) => {
                        warn!(fetch = %fetched, "plugin fetch rejected: {e:?}");
                        return Err(e);
                    }
                    e => {
                        warn!(fetch = %fetched, "plugin fetch failed: {e:?}");
                        return Err(e);
                    }
                }
            }
        }
    }
}

/// `format!`-safe URL fragment for error messages (query stripped).
fn log_fragment(url: &str, parsed: Option<&Url>) -> String {
    match parsed {
        Some(u) => log_url(u),
        None => url.split(['?', '#']).next().unwrap_or(url).to_string(),
    }
}
