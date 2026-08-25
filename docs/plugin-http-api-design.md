# Plugin HTTP & Book-File API — Design

Status: **implemented, with the fetch policy removed** (owner decision,
2026-08-25: "wasm 插件的请求不用检查了，提供联网能力就行" — the allow
list, scheme gate and SSRF resolver below are **gone**; the host
provides plain network access with timeout/size caps only). The rest of
this document (book-file acquisition, config-driven base URLs) stands.
Extends the v2 plugin interface (`crates/bookshelf-plugin/wit/bookshelf.wit`,
`bookshelf:plugin@0.2.0`). Does **not** replace the config channel, the
capabilities model, or the lazy catalog policy of `docs/plugin-v2-design.md`.

## 1. Motivation

Today a plugin is a pure computation: its only host imports are
`store::log` and `config::configure`, and it has no access to the network
or the filesystem. That is great for sandboxing, but it means the demo
plugins must hardcode their "sources" — a real wiki/reading-site plugin
cannot exist. This design adds the minimal API surface that lets a plugin
**fetch metadata and book content from an external source** conveniently,
while keeping every request under host control.

Two acquisition patterns are supported:

- **Chapter mode** — the source exposes per-chapter text (typical for
  reading sites): `search-books` / `get-book` return metadata,
  `chapter-titles` / `get-chapter` provide chapters. This already exists;
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
- Every request is subject to host policy: allow-listed hosts, SSRF
  protection, timeouts, size caps, redirect limits, secret-free logging.
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

## 3. WIT additions (`bookshelf:plugin@0.3.0`)

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
  `min(requested, BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS)`.
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

world bookshelf-plugin {
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
| Allow list | `BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS` — comma-separated hosts (optionally `host:port`). Empty = **all `fetch` calls denied**. E.g. `zh.wikipedia.org`, `localhost:8081` for a local test source. |
| Scheme | http + https (self-hosted sources are often plain http); `BOOKSHELF_PLUGIN_FETCH_HTTP=false` turns http off. |
| SSRF | Before every request (and after each redirect hop): the hostname is resolved and the final IP must **not** be loopback/private/link-local/ULA (unless the resolved host itself is allow-listed), guarding DNS-rebinding and localhost probing. |
| Redirects | Followed up to 5 hops; each hop re-validated against allow list + IP policy; `Authorization`/`Cookie`-like headers are dropped when a hop leaves the original host. |
| Timeout | Hard cap `BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS` (default 30 000). |
| Size | `BOOKSHELF_PLUGIN_FETCH_MAX_BYTES` (default 64 MiB, aligned with `max_upload_mb`). Oversized responses abort with `size-limit` — no partial data. |
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
    chapter-titles(id): fetch the chapter list -> titles
    get-chapter(id, i): fetch one chapter page -> extract text
                        (or return the raw HTML and let the host sanitize —
                         see open question Q4)

file mode:
    get-book-file(id): fetch {base-url}/files/{id}/download
                       -> book-file { filename, mime, bytes }
```

Materialization rules (host side, extending §6 of the v2 design):

- **Chapter mode**: unchanged — titles placeholders, bodies lazy on first
  read, refresh never touches materialized bodies.
- **File mode**: declarers of `book-file` are materialized by fetching the
  file *once* (on first access / on the unified acquisition endpoint
  `POST /api/books` with `plugin_source` + `plugin_book_id`) and
  running the **existing upload parser** (`bookshelf-formats::parse`):
  chapters, `chapters.format`, hierarchical `toc`, sanitized HTML all come
  for free; the file row is stored like a local upload
  (`format` = `epub`|`txt` from the mime). The DB remains the single
  source of truth — afterwards the plugin is not consulted for content.
- Priority when both `content` and `book-file` are declared: **file mode
  first**, chapter mode as fallback when `get-book-file` returns `none` /
  errors.
- `identify-upload` is unchanged; an instance with `book-file` may
  recognize uploads and supply `book-entry.content-source` as usual.

### 4.3 Retry guidance for plugin authors

Transient `fetch-error`s (timeout, transport) should be retried with small
backoff inside the export (the epoch deadline allows a few hundred ms of
work per call — do a bounded retry, or return `none`/empty and let the
user retrigger via refresh/materialize). Permanent errors should surface
as `none`/empty results (the host logs the underlying error).

## 5. Security review (summary)

1. The plugin can only reach allow-listed hosts — a compromise of one
   plugin does not turn into general outbound access.
2. SSRF is blocked at resolve time, including after redirects, so
   `http://127.0.0.1:8080/admin` or rebinding tricks fail.
3. No ambient credentials: the host never forwards host cookies/proxies
   to plugin requests (explicit `BOOKSHELF_HTTPS_PROXY` support is
   deferred — see open questions).
4. Response size caps keep one plugin from exhausting memory; the epoch
   deadline keeps one call from dominating a worker; `spawn_blocking`
   keeps the async runtime healthy.
5. Logs are query-stripped; API keys live in plugin config (already
   admin-only in `GET /api/plugins/{id}/config-schema`).

## 6. Implementation outline

| Phase | Scope | Acceptance |
|---|---|---|
| P1 | WIT 0.3.0 (`http` interface + `book-file`), host `fetch` import (`ureq` or `reqwest::blocking` inside `spawn_blocking`; allow-list, SSRF check, timeout/size caps, redirect policy, stripped logging), config knobs; a tiny local test source (e.g. `python3 -m http.server`-style fixture or a mock endpoint in `bookshelf-server`) | unit tests: allow-list denial, SSRF block (localhost/loopback), timeout, size-limit, redirect-limit, header-strip-on-cross-host; chapter-mode e2e against the mock source |
| P2 | Rewrite `wiki-plugin` against a real public API (e.g. Chinese Wikipedia REST/Action API) and `reader-plugin` chapter-mode against it (or a documented mock); config-driven base-url | e2e: search → materialize → lazy chapters all served from the network source; refresh pulls metadata only |
| P3 | `get-book-file` export + host file-mode materialization through `bookshelf-formats::parse` + capability plumbing; `reader-plugin` gains file mode returning a generated epub | e2e: file-mode book materializes with real TOC + sanitized chapters like an upload |
| P4 | Guard hardening (per-instance QPS token bucket if needed), `docs/plugins.md` rewrite, AGENTS.md progress log | `just` checks green; both patterns documented |

The `fetch` import keeps the world's imports free of WASI, so the build
pipeline (`wasm-tools component new`, no adapter) stays untouched.

## 7. Open questions (decide during P1)

- Q1 **Rate limiting**: per-instance QPS token bucket? (Self-hosted single
  user: probably unnecessary; a global
  `BOOKSHELF_PLUGIN_FETCH_CONCURRENCY` semaphore is a cheap middle ground.)
- Q2 **HTTPS proxy**: forward plugin requests through the server's
  configured proxy (`BOOKSHELF_HTTPS_PROXY`)? Needed for some networks;
  conflicts with "no ambient credentials" — decide whether proxy use
  requires an explicit flag.
- Q3 **HTTP scheme default**: allow http by default (self-hosted sources)
  or require `BOOKSHELF_PLUGIN_FETCH_HTTP=true`? Leaning: default **on**
  with a warning, since the allow-list is the real gate.
- Q4 **Chapter HTML**: should `chapter` gain a `format` field so plugins
  can return HTML and the host sanitizes it through the existing EPUB
  pipeline? (Nice for page-scraping sources; text extraction stays the
  default.) Leaning: yes in P2, default `text`.
- Q5 **Caching of remote metadata** in the plugin (e.g. search results
  already seen)? Statelessness forbids guest-side state; the host DB is
  the cache. No action, but worth stating explicitly in `docs/plugins.md`.