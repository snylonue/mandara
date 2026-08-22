# Plugin authoring guide (v3)

Bookshelf's wasm plugin system lets users provide custom book and metadata
sources. Plugins run as **WebAssembly components** inside the host sandbox
(wasmtime): the only way they touch the outside world is the host's
`http.fetch` import — under host policy (allow list, SSRF checks, caps).
No WASI, no filesystem, no ambient network.

See `docs/plugin-v2-design.md` (design, implemented) and
`docs/plugin-http-api-design.md` (v3 HTTP acquisition, implemented) for
the full design. This guide is for plugin authors.

## Concepts

- **Wasm file** (`data/plugins/*.wasm`) — the compiled plugin code. The
  server compiles every file at startup and exposes it for registration.
- **Instance** — one registered configuration of a wasm file, with its own
  id (the source id in `book_files.source`), validated config, and enable
  flag. One wasm file can back several instances (e.g. one "web source"
  plugin pointed at two sites). Instances are registered via the admin UI
  or `POST /api/plugins/instances`; instance ids default to a uuid, or an
  admin-chosen stable id (needed when metadata plugins point at content
  plugins).
- **Capabilities** — `capabilities()` declares which catalog patterns the
  plugin implements (see below). The host keys all policy off this list.
- **Statelessness** — a fresh store per call; config is injected at the
  start of every call via `config::configure()`. Never keep state in the
  guest (no caching either — the central DB is the cache).

## Lifecycle

- Startup: wasm files compiled; instances loaded from `plugin_instances`.
- `declare`-capable instances are **fully synced** at startup and on
  `POST /api/plugins/sync` (metadata + title rows; eager bodies for
  non-empty `content`).
- `search`/`lookup`-capable instances are **lazy**: nothing is
  materialized until a user picks a book in the source browser
  (`POST /api/plugins/{id}/books`).
- Acquisition modes on materialize (v3): **file mode first** when the
  instance declares `book-file` — the host calls `get-book-file` and runs
  the bytes through the normal upload parser (chapters, hierarchical TOC,
  sanitized HTML; stored like a local upload, format `epub`/`txt`). If
  `get-book-file` returns `none` or errors, the host falls back to
  **chapter mode** (`get-book` + title placeholders; bodies lazy until
  first read). Virtual rows (e.g. from `declare` sync) are materialized
  in file mode on **first access** too.
- Chapter bodies materialize into the central DB on first read; afterwards
  the book behaves like any uploaded file (readable even if the plugin
  file is removed).
- `identify`-capable instances are asked to recognize uploaded files
  (`identify-upload`, first match wins, in instance creation order).
- `POST /api/books/{id}/refresh` re-pulls **metadata only** via
  `get-book` (never a catalog scan); file-mode rows are never re-parsed;
  materialized chapter bodies are never overwritten.

## Interface (WIT)

Interface file: `crates/bookshelf-plugin/wit/bookshelf.wit`, world
`bookshelf:plugin/bookshelf-plugin@0.3.0`. Every export is **required**
(the WIT parser has no optional exports) — unsupported features return
`none`/empty and are declared via `capabilities()`:

```wit
interface types {
    record book-entry {
        id: string,
        title: string,
        authors: list<string>,
        description: option<string>,
        cover-url: option<string>,
        // metadata/content separation (R5):
        content-source: option<string>,  // instance id providing the chapters
        content-id: option<string>,      // book id inside that instance
    }
    record chapter { title: string, content: string }  // empty content = lazy body
    record declared-book { book: book-entry, chapters: list<chapter> }
    record search-result { total: u64, items: list<book-entry> }
    record book-file { filename: string, mime: string, bytes: list<u8> }
}

interface store { log: func(message: string); }

interface config {
    record config-field { key, label, kind, default, required, hint }
    variant config-kind { text, number, boolean, enum-options(list<string>), list-of-string }
    variant config-value { text(string), number(f64), boolean(bool), enum-index(u32), string-list(list<string>) }
    configure: func() -> list<config-value>;   // host-provided
}

interface http {
    record header { name: string, value: string }
    record request {
        method: string, url: string, headers: list<header>,
        body: option<list<u8>>, timeout-ms: option<u64>,
    }
    record response { status: u16, headers: list<header>, body: list<u8>, final-url: string }
    variant fetch-error {
        invalid-url(string), denied(string), redirect-limit(u32),
        timeout(u64), size-limit(u64), transport(string),
    }
    fetch: func(request: request) -> result<response, fetch-error>;
}
```

World exports:

| Export | Required by | Purpose |
|---|---|---|
| `name()` | always | Human-readable name |
| `config-schema()` | always | Config fields the host renders as an admin form |
| `capabilities()` | always | `["declare","search","lookup","identify","content","book-file"]` subset |
| `declare()` | always | Small static catalog (`None` for large sources) |
| `search-books(query, offset, limit)` | `search` | Paginated catalog browsing |
| `get-book(book-id)` | `lookup` | Single book (materialize on demand, refresh) |
| `chapter-titles(book-id)` | `content` | Titles in reading order |
| `get-chapter(book-id, index)` | `content` | One chapter body |
| `identify-upload(filename, file-hash)` | `identify` | Upload recognition |
| `get-book-file(book-id)` | `book-file` | Whole book file (epub/txt download) |

Host caps: `search-books` limit ≤ 50 / offset ≤ 10 000 (400 beyond);
`declare` ≤ 10 000 books / 100 000 chapters (error beyond, warn past 90%);
chapter content ≤ 2 MiB per call; book files ≤ 256 MiB (the http size cap
usually applies first). Runaway plugins are trapped by the epoch deadline:
per call, a plugin may run for at most the fetch-timeout budget
(`BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS`, default 30 s — network waits
count, they are bounded by that same cap) plus a few 200 ms pump ticks;
compute-only calls (e.g. `declare` sync) keep that minimum. A call that
exceeds the budget is interrupted at the next wasm backedge — keep
catalog/chapter generation fast, or split it across calls. Plugin
calls run on blocking threads, so a slow source never stalls a worker.

## HTTP acquisition (`http.fetch`)

`http.fetch` is the one way to reach an external source. The host enforces
everything; the plugin decides what a status means (4xx/5xx are plain
`response`s, not errors):

- **Allow list**: `BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS` — comma-separated
  `host` or `host:port` entries; **empty = every fetch is denied** (the
  demo default until you configure it). Allow-listed hosts may resolve to
  private addresses (e.g. `localhost:8081` for a local source).
- **Scheme**: http and https; `BOOKSHELF_PLUGIN_FETCH_HTTP=false` turns
  http off.
- **SSRF**: at resolve time — the exact addresses the connection uses —
  private/loopback/link-local/ULA addresses are refused unless the host
  is allow-listed; redirect hops (max 5) are re-validated; credential-ish
  headers (`authorization`, `cookie`, `proxy-authorization`, `x-api-key`)
  are dropped when a hop leaves the original host. URLs with embedded
  credentials are rejected — put keys in config, not URLs.
- **Caps**: overall timeout = `min(requested, BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS)`
  (default 30 s); response bodies ≤ `BOOKSHELF_PLUGIN_FETCH_MAX_BYTES`
  (default 64 MiB, no partial data).
- **No ambient credentials**: only the headers you set are sent (plus the
  ones the host manages: host/content-length/transfer-encoding/connection;
  yours are stripped). Logs show `scheme://host/path` only (query and
  fragment stripped).

Error handling (docs §4.3): `fetch-error` distinguishes **permanent**
(`invalid-url`, `denied`, `redirect-limit`, `size-limit` — don't retry)
from **transient** (`timeout`, `transport` — retry once or twice, or
return `none`/empty and let the user retrigger via refresh /
materialize). A typical guest request:

```rust
let resp = bookshelf::plugin::http::fetch(&bookshelf::plugin::http::Request {
    method: "GET".into(),
    url: url.into(),
    headers: vec![],                       // or an api-key header from config
    body: None,
    timeout_ms: Some(5_000),
});
match resp {
    Ok(r) if r.status == 200 => /* parse r.body (serde_json etc. — the
                                   guest can carry any std-compatible
                                   parser) */,
    Ok(r) => /* application status: log + none/empty */,
    Err(e) => /* transport-level: log + none/empty (docs §4.3) */,
}
```

## Configuration

`config-schema()` returns the fields; values live in JSON keyed by field
key. The host validates (unknown keys rejected, `required` enforced,
enum indices range-checked, optional fields without a default stay
absent), stores the validated JSON, and **injects the values at the start
of every call** — call `config::configure()` first in your exports and map
the returned list back by schema order:

```rust
const FIELDS: [&str; 2] = ["site-name", "page-size"];

fn site_name() -> String {
    let idx = FIELDS.iter().position(|k| *k == "site-name").unwrap();
    match bookshelf::plugin::config::configure().get(idx) {
        Some(ConfigValue::Text(s)) if !s.is_empty() => s.clone(),
        _ => "默认站点".into(),
    }
}
```

Defaults are serialized JSON (`"8"`, `"false"`, `"\"x\""`); a text field
whose default is not valid JSON is treated as the literal string.
URL templates, api keys and page sizes are all config values, so one wasm
file serves many sites (per-instance config).

## Metadata / content separation (R5)

A metadata-only plugin (e.g. a wiki) returns `book-entry`s with
`content-source`/`content-id` pointing at a content plugin's **instance
id** — register the content plugin with a stable id for this to be
predictable. Materializing such a book stores its metadata from the wiki
and pulls titles/bodies from the reader instance on demand. Reader-side,
users can also rebind a file's content source manually
(`POST /api/files/{id}/content-source`, owner/admin; validates the target
and, when it has `lookup`, the book id).

## Example plugins

- `plugins/hello-plugin/` — small `declare` + `identify` + `content`
  plugin; config-driven (site name, content variant, verbose logging, a
  deliberate infinite-loop switch for testing the epoch timeout).
- `plugins/wiki-plugin/` — metadata-only over HTTP: `search` + `lookup`,
  config `base-url`/`api-key`/`site-name`/`page-size`; every entry points
  `content-source: "reader"`. Try it with `scripts/mock-source.py`.
- `plugins/reader-plugin/` — reading-site source over HTTP: `search` +
  `lookup` + `content` + `book-file`; chapter mode for `r-N` books, and
  `get-book-file` returning the epub the source serves (file mode wins,
  chapter mode is the fallback).
- `plugins/wenku8-plugin/` — a **real-world** source: 轻小说文库
  (`https://www.wenku8.net/`, GBK-encoded pages). `lookup` + `content`
  only: wenku8's search and listings are login-walled, so books are
  materialized by their numeric id (the number in the book URL, e.g.
  `3617`) — the library source browser's manual-id entry, and the
  rebind dialog, support lookup-only sources. Config: `base-url`
  (mirror-switchable), optional `referer` override, and
  `illustration-placement` — wenku8 appends an `插图` (color plates)
  chapter at the *end of every volume*; the plugin keeps it **in that
  original position** by default (`end`), or moves it to the volume
  front like the physical book / linovelib2epub (`front`), or drops it
  (`skip`). Plates are extracted
  as numbered image URLs (plugin chapters are text; the host has no
  per-chapter HTML format yet). Needs the host allow list:
  `BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS=www.wenku8.net`.
  Build: `./scripts/build-plugins.sh wenku8`.

## Building & running the demos

```sh
./scripts/build-plugins.sh hello wiki reader   # -> plugins-built/*.wasm
cp plugins-built/*.wasm data/plugins/
```

The guest crates use `wit-bindgen = "0.60"` with
`wit_bindgen::generate!({ world: "bookshelf-plugin", path: "../../crates/bookshelf-plugin/wit" })`
and `export!(PluginName)`. The generated core module embeds a
`component-type` section, so `wasm-tools component new` lifts it into a
component without adapters (the world imports no wasi interfaces).

End-to-end demo of the HTTP acquisition layer (chapter mode + file mode +
policy denial):

```sh
python3 scripts/mock-source.py 8765 &           # the remote source
BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS=127.0.0.1:8765 \
    cargo run -p bookshelf-server                # server with the allow list
./scripts/e2e-http-plugins.sh                    # full automated check
```

The host unit tests embed `plugins-built/hello.wasm` as a fixture — after
changing the WIT or the hello guest, rebuild with
`./scripts/build-plugins.sh hello` to refresh it.