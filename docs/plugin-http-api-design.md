# Plugin HTTP & Book-File API — Design

Status: **implemented** (owner decision, 2026-08-25:
"wasm 插件的请求不用检查了，提供联网能力就行" — the host provides
plain network access with timeout/size caps only). The rest of this
 document (book-file acquisition, config-driven base URLs) stands.
Extends the v2 plugin interface (`crates/mandara-plugin/wit/mandara.wit`,
`mandara:plugin@0.2.0`). Does **not** replace the
capabilities model, or the lazy catalog policy of `docs/plugin-v2-design.md`.

## 1. Motivation

Before HTTP support, plugins could only log and pull configuration, with no
network or filesystem access. That kept sandboxing simple, but forced demo
plugins to hardcode their sources. This design adds the minimal API surface
for **fetching metadata and book content from an external source**, while
keeping every request under host control. In the current WIT, configuration
travels as explicit business export parameters rather than a host import
(see `docs/plugins.md`).

Two acquisition patterns are supported:

- **Chapter mode** — the source exposes per-chapter text (typical for
  reading sites): `search-books` / `get-book` return metadata,
  `get-content` declares chapters and loading; `get-chapter` provides lazy bodies. This already exists;
  what was missing is the network access behind it.
- **Book-file mode** — the source offers whole files (epub/txt downloads,
  typical for archive sites): a new `get-book-file` export returns the
  bytes, and the **host parses them with the existing upload pipeline**
  (epub/txt → chapters + hierarchical TOC + sanitized HTML), so file-mode
  books get everything uploaded books get, for free.

## 2. Goals / non-goals

Goals:

- One primitive (`fetch`) covers all acquisition needs; plugins bring
  their own parsing (JSON/HTML is guest-side code compiled into the wasm —
  a guest can carry `serde_json` etc.; `bytes` responses keep this open).
- Configuration stays the single input channel: URL templates, API keys,
  page sizes are per-instance `config-schema` values, so one wasm file
  serves many sites.
- Every request is subject to host policy: timeouts, size caps,
  redirect limits, secret-free logging.
- Book files flow through the existing parser instead of a second,
  plugin-specific storage path.

Non-goals (dev phase):

- No WASI: plugins do not get `wasi:http` or the filesystem. The host
  stays the only way to touch the outside world.
- No automatic retries / caching / rate limiting in phase 1 (the central
  DB already caches materialized chapters; retries are the plugin's job —
  `fetch-error` is typed so it can decide).
- No streaming responses (a full response body is returned at once; sizes
  are capped).

## 3. WIT additions (`mandara:plugin@0.3.0`)

A new imported interface `http`, plus a `book-file` type and one optional
(declared-via-capabilities) export.

### 3.1 Import: `http.fetch`

```wit
interface http {
    record header {
        name: string,
        value: string,
    }

    record request {
        method: string,           // "GET", "POST", ...
        url: string,              // absolute http(s) URL
        headers: list<header>,    // plugin-controlled; host adds nothing
        body: option<list<u8>>,
        timeout-ms: option<u64>,  // plugin's desired timeout (host caps it)
    }

    record response {
        status: u16,              // pluggable semantics: plugin decides
        headers: list<header>,    // what an error status means
        body: list<u8>,
        final-url: string,        // after redirects (host-validated)
    }

    /// Transport-level outcome. Application-level statuses (404, ...)
    /// are *not* errors — the plugin inspects `response.status`.
    variant fetch-error {
        invalid-url(string),      // permanent: URL parse/protocol problem
        denied(string),           // permanent: host policy rejected the URL
        redirect-limit(u32),      // permanent: too many redirects
        timeout(u64),             // transient: took longer than the cap
        size-limit(u64),          // permanent: response exceeded the cap
        transport(string),        // transient-if-connect: DNS/TLS/connection
    }

    /// Perform one HTTP request. Only `url`, `method` and `body` when
    /// present are sent; the host never adds headers, cookies or auth.
    fetch: func(request: request) -> result<response, fetch-error>;
}
```

Notes:

- `timeout-ms`: the plugin asks for a timeout, the host enforces
  `min(requested, MANDARA_PLUGIN_FETCH_TIMEOUT_MS)`.
- `fetch-error` distinguishes **permanent** (do not retry: bad URL,
  denied, redirect loop, oversized) from **transient** (retry with
  backoff: timeout, connect failures) errors.
- Bodies are `list<u8>`; the plugin cheaply re-borrows them as
  `&[u8]` for `serde_json::from_slice` / `rowan`-style parsing, or
  returns them verbatim from `get-book-file`.

### 3.2 `book-file` + `get-book-file` export

```wit
interface types {
    // added:
    /// A whole book as the source stores it (epub/txt download).
    record book-file {
        filename: string,         // e.g. "mystery-ep1.epub"
        mime: string,             // "application/epub+zip" | "text/plain" | ...
        bytes: list<u8>,
    }
}

world mandara-plugin {
    // added export (declared via the new `book-file` capability):
    /// Fetch a whole book file. `none` = not available as a file
    /// (fall back to chapter mode). Errors are `fetch`-style results;
    /// the host treats an `err` as "chapter mode" too (logged).
    export get-book-file: func(book-id: string) -> option<book-file>;
}
```

Capabilities gain one entry: `"book-file"`.

### 3.3 Host policy (decode of the fetch routing)

| Concern | Policy |
|---|---|
| Scheme | http + https (self-hosted sources are often plain http). |
| Redirects | Followed up to 5 hops; exceeding the cap is a `redirect-limit` error (not the last response). |
| Timeout | Hard cap `MANDARA_PLUGIN_FETCH_TIMEOUT_MS` (default 30 000). |
| Size | `MANDARA_PLUGIN_FETCH_MAX_BYTES` (default 64 MiB, aligned with `max_upload_mb`). Oversized responses abort with `size-limit` — no partial data. |
| Secrets | Host logs never include full URLs (only `scheme://host/path`, query/fragment stripped). Plugins should put credentials in config, not URLs. |
| State | `fetch` is a synchronous import; the host runs it on a blocking thread (`tokio::task::spawn_blocking`), so a slow source never stalls a worker. |

## 4. Convenience: how the pieces fit

### 4.1 Typical metadata-only plugin (wiki, chapter mode)

```
config-schema:  base-url (string, required), api-key (string), page-size (number)
capabilities:   ["search", "lookup"]            (+ "book-file" when offering files)

search-books(q, offset, limit):
    fetch GET {base-url}/api/search?q=..&offset=..&limit=..   (timeout-ms: 5_000)
    parse JSON -> book-entries        // content-source/content-id point at the
                                      // reader instance, or none for self-content
get-book(id):
    fetch GET {base-url}/api/books/{id} -> book-entry (none on 404)
```

### 4.2 Content plugin (reading site)

```
capabilities:   ["search", "lookup", "content"]   // chapter mode
                ["search", "lookup", "book-file"] // file mode (mutually
                                                  // exclusive with content)

chapter mode:
    get-content(id): fetch content identity, mode and chapter declarations
    get-chapter(id, i): fetch one chapter page -> extract text
                        (or return the raw HTML and let the host sanitize —
                         see open question Q4)

file mode:
    get-content(id): declare file loading, with an empty chapter list
    get-book-file(id): fetch {base-url}/files/{id}/download
                       -> book-file { filename, mime, bytes }
```

Materialization rules (host side, extending §6 of the v2 design):

- **Chapter mode**: unchanged — titles placeholders, bodies lazy on first
  read, refresh never touches materialized bodies.
- **File mode**: content declaring `file` through `get-content` is materialized by fetching the
  file *once* (on first access / on the unified acquisition endpoint
  `POST /api/books` with `plugin_source` + `plugin_book_id`) and
  running the **existing upload parser** (`mandara-formats::parse`):
  chapters, `chapters.format`, hierarchical `toc`, sanitized HTML all come
  for free; the file row is stored like a local upload
  (`format` = `epub`|`txt` from the mime). The DB remains the single
  source of truth — afterwards the plugin is not consulted for content.
- The plugin declares loading separately from book metadata through
  `get-content(content-id)`, alongside chapter declarations; the host stores it at
  acquisition and never infers loading from capabilities. Plugins own any
  upstream fallback. File `none`/failure is an error, not a mode switch.
- `identify-upload` is unchanged; an instance with `book-file` may
  recognize uploads and supply book metadata, never content selection.

### 4.3 Retry guidance for plugin authors

Use bounded retries for transient `fetch-error`s (timeout, transport).
Each HTTP request has its own timeout; exports have no aggregate
wall-clock deadline, and network waits do not consume guest fuel. When
retries are exhausted, return `none`/empty and let the user retry via
refresh/materialize. Permanent errors should surface as `none`/empty
results (the host logs the underlying error).

## 5. Security review (summary)

1. A compromised plugin gets general outbound access — the host's only
   caps are timeout, size and redirect count.
2. No ambient credentials: the host never forwards host cookies to
   plugin requests (only the headers the plugin sets are sent).
3. Response size caps bound fetched payloads; fuel metering bounds guest
   computation without charging network waits. Per-request timeouts bound
   individual HTTP waits, and `spawn_blocking` keeps them off tokio workers.
4. Logs are query-stripped; API keys live in plugin config (already
   admin-only in `GET /api/plugins/{id}/config-schema`).

## 6. Implementation outline

| Phase | Scope | Acceptance |
|---|---|---|
| P1 | WIT 0.3.0 (`http` interface + `book-file`), host `fetch` import (`ureq` or `reqwest::blocking` inside `spawn_blocking`; timeout/size caps, redirect policy, stripped logging), config knobs; a tiny local test source (e.g. `python3 -m http.server`-style fixture or a mock endpoint in `mandara-server`) | unit tests: timeout, size-limit, redirect-limit, header-strip-on-cross-host; chapter-mode e2e against the mock source |
| P2 | Rewrite `wiki-plugin` against a real public API (e.g. Chinese Wikipedia REST/Action API) and `reader-plugin` chapter-mode against it (or a documented mock); config-driven base-url | e2e: search → materialize → lazy chapters all served from the network source; refresh pulls metadata only |
| P3 | `get-book-file` export + host file-mode materialization through `mandara-formats::parse` + capability plumbing; `reader-plugin` gains file mode returning a generated epub | e2e: file-mode book materializes with real TOC + sanitized chapters like an upload |
| P4 | Guard hardening (per-instance QPS token bucket if needed), `docs/plugins.md` rewrite, AGENTS.md progress log | `just` checks green; both patterns documented |

The `fetch` import keeps the world's imports free of WASI, so the build
pipeline (`wasm-tools component new`, no adapter) stays untouched.

## 7. Open questions (decide during P1)

- Q1 **Rate limiting**: per-instance QPS token bucket? (Self-hosted single
  user: probably unnecessary; a global
  `MANDARA_PLUGIN_FETCH_CONCURRENCY` semaphore is a cheap middle ground.)
- Q2 **HTTPS proxy**: RESOLVED (2026-08-26) — the host upgraded to ureq 3,
  whose default agent honors the standard `ALL_PROXY`/`HTTPS_PROXY`/
  `HTTP_PROXY`/`NO_PROXY` environment variables. No mandara-specific
  flag; deployments behind a proxy just export the usual vars.
- Q3 **HTTP scheme default**: allow http by default (self-hosted sources).
- Q4 **Chapter HTML**: **resolved** — `chapter.format` landed (WIT v7):
  plugins return `"text"` (host escapes + paragraph-wraps, the default)
  or `"html"` (the plugin's own final HTML, stored verbatim with image
  references annotated). wenku8 is the first `"html"` producer; its
  illustration conventions are guest-side. A host-side HTML sanitizer
  (the shared epub whitelist) remains a candidate if untrusted plugins
  ever emit `"html"`.
- Q5 **Caching of remote metadata** in the plugin (e.g. search results
  already seen)? Statelessness forbids guest-side state; the host DB is
  the cache. No action, but worth stating explicitly in `docs/plugins.md`.
