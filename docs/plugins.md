# Plugin authoring guide (v3)

Mandara's wasm plugin system lets users provide custom book and metadata
sources. Plugins run as **WebAssembly components** inside the host sandbox
(wasmtime): the only way they touch the outside world is the host's
`http.fetch` import — plain network access with timeout/size caps.
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
  materialized until a user acquires a book through the unified
  acquisition flow (`POST /api/books` with `plugin_source` +
  `plugin_book_id` — 获取书籍 → 添加元数据).
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

Interface file: `crates/mandara-plugin/wit/mandara.wit`, world
`mandara:plugin/mandara-plugin@0.9.0`. Every export is **required**
(the WIT parser has no optional exports) — unsupported features return
`none`/empty and are declared via `capabilities()`:

```wit
interface types {
    record series-entry {
        id: string, title: string, authors: list<string>,
        description: option<string>, cover-url: option<string>,
        extra: option<string>, // SeriesExt JSON
    }
    record book-entry {
        id: string,
        title: string,
        authors: list<string>,
        description: option<string>,
        cover-url: option<string>,
        // extended volume metadata: JSON object, BookExt shape
        extra: option<string>,
        series: option<series-entry>,
        volume-no: u32,
    }
    record chapter { title: string, content: string, format: string }  // empty content = lazy body
    record declared-book { book: book-entry, chapters: list<chapter> }
    variant catalog-entry { single(book-entry), series(series-entry) }
    record search-result { total: u64, items: list<catalog-entry> }
    record series-result { series: series-entry, books: list<book-entry> }
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
| `get-series(series-id)` | `lookup` | Series metadata plus independently addressable member books |
| `chapter-titles(book-id)` | `content` | Titles in reading order |
| `get-chapter(book-id, index)` | `content` | One chapter body |
| `identify-upload(filename, file-hash)` | `identify` | Upload recognition |
| `get-book-file(book-id)` | `book-file` | Whole book file (epub/txt download) |

**One volume = one book (v0.9.0).** A plugin adapts its source into
explicit series and individual books. `get-series` returns series metadata
and member `BookEntry` values; every member ID must work with `get-book`,
`chapter-titles`, `get-chapter` and/or `get-book-file` as applicable.
Chapter indices always start at zero within that book. `BookEntry.series`
carries the parent's metadata; `volume-no` is display order, never identity.
The host stores one book/file per selection and groups parent series by
(instance, plugin series ID). It never splits a flat chapter list or matches
volumes across sources. Users choose both metadata and content books.

IDs are opaque plugin-defined values. A website's raw ID may identify a
whole series and need not equal any plugin book ID. Host-local database IDs
are separate UUIDs. IDs must not depend on mutable volume-list positions:
Wenku8 uses `wenku8:volume:{website-id}:{minimum-chapter-cid}` and
`wenku8:series:{website-id}`; Bangumi series members use real related
subject IDs (`bgm:volume:{series-subject}:{member-subject}`), not a
synthetic book per reported volume count. Removing an anchor/member makes
that book unavailable; it must not silently resolve to another volume.

Search hits distinguish books from series. The source picker resolves a
series through `GET /api/plugins/{instance}/lookup?q={id}` and lets the
user select one member. Manual website IDs go through the same lookup.

**Upgrade:** rebuild all components after the WIT change. Existing cached
chapters and reading sessions are retained. Legacy flat-split files are
not automatically rebound: their uncached reads/refresh require explicit
volume selection. Content rebinding resets the old offset and TOC without
changing metadata provenance. Legacy numeric-series metadata may need to
be acquired anew to refresh; no positional ID migration is attempted.

**Extended metadata (`book-entry.extra`, v0.7.0).** A source may
attach a JSON object with catalog-style fields to every `book-entry`
(see `docs/metadata-ext-design.md`):

```json
{
  "subtitle": "…", "original_title": "…", "isbn": "9787536692930",
  "publisher": "电击文库", "pub_date": "2012-5",
  "translators": ["…"], "illustrators": ["…"], "pages": 256,
  "price": "¥ 620", "binding": "文库", "language": "ja"
}
```

- Every key is optional; **unknown keys are preserved verbatim**, so new
  server-side fields flow through without WIT/host changes.
- The host validates the object at the storage boundary: ISBNs are
  checksum-verified and canonicalized to hyphenless ISBN-13 (valid
  ISBN-10 inputs are converted); an invalid object fails the plugin call
  — bad data never lands. Dates are stored verbatim; formatting is the
  source's job.
- Series metadata has its own `series-entry.extra` (`SeriesExt`):
  status, total_volumes, and tags. Book metadata remains volume-specific.
- Manual acquire overrides (`POST /api/books` multipart field `ext`)
  merge per-key on top of what the source produced; refresh replaces it
  wholesale.

Host caps: `search-books` limit ≤ 50 / offset ≤ 10 000 (400 beyond);
`declare` ≤ 10 000 books / 100 000 chapters (error beyond, warn past 90%);
chapter content ≤ 2 MiB per call; book files ≤ 256 MiB (the http size cap
usually applies first). Runaway plugins are trapped by the epoch deadline:
per call, a plugin may run for at most the fetch-timeout budget
(`MANDARA_PLUGIN_FETCH_TIMEOUT_MS`, default 30 s — network waits
count, they are bounded by that same cap) plus a few 200 ms pump ticks;
compute-only calls (e.g. `declare` sync) keep that minimum. A call that
exceeds the budget is interrupted at the next wasm backedge — keep
catalog/chapter generation fast, or split it across calls. Plugin
calls run on blocking threads, so a slow source never stalls a worker.

### Chapter content: `text` vs `html`

Plugins declare how their chapter body is interpreted via the WIT
`chapter.format` (default `"text"`):

- **`"text"`** — the body is plain text. The host escapes it, splits on
  blank lines into paragraphs, wraps each in `<p>` and turns single
  newlines into `<br/>` (the same conversion txt uploads use,
  `mandara_formats::htmlize::text_to_html`). This is the default and
  covers sources that return prose.
- **`"html"`** — the body is the plugin's **own final HTML fragment**
  (escaped text and known safe tags). The host runs it through the same
  whitelist sanitizer epub uploads use (`DROP_TAGS`/`ALLOWED_TAGS`/
  `allowed_attrs` in `mandara_formats::htmlize::sanitize_html_fragment`):
  scripts, styles, forms, foreign content and event handlers are stripped,
  unknown tags are unwrapped, and `href`s are kept only for in-page
  anchors / http(s) / mailto targets — a broken or malicious plugin cannot
  inject markup into the reader. After sanitization the host annotates
  image references (`/api/images/{id}` with stored dimensions). The plugin
  is still expected to escape its text and stick to the safe tag set
  (`<p>`, headings, `<figure><img>`, …); the sanitizer is a defense in
  depth, not a substitute for well-formed output.

The wenku8 plugin is the only `"html"` producer today: its chapter
bodies come from a fixed site layout (插图 plate chapters, inline
`（插图NNN）` marks), so it keeps that expansion guest-side instead of
the host knowing one source's conventions. A second page-scraping
source with its own markup either emits `"html"` itself or returns
plain text and lets the host escape it.

An image reference is an `http(s)://` URL or the canonical stored-image
path `/api/images/{id}` (what the `store-image` import returns). When
in doubt, emit a resolved reference and the host resolves/stores it;
HTML-emitting plugins should use the `/api/images/{id}` form when they
have stored the bytes (the host's `store-image` returns it).

**Images.** The server is a content-addressed image store: producers store
bytes and reference them by id. Plugin chapter HTML should contain only
`<img src="/api/images/{id}">` references — the id is the sha256 of the
bytes, so identical images dedup across chapters and books. (The
host-side URL download of earlier versions is gone; a plugin fetches its
images itself over `http.fetch`, then calls the import:

```wit
interface store {
    log: func(message: string);
    /// bytes → sha256 hex id; embed as `/api/images/{id}`
    store-image: func(bytes: list<u8>, mime: string) -> string;
}
```

Caps: 20 MiB per image, 96 MiB per call; exceeding them traps. A failed
download should fall back to the remote URL (degraded rendering, retried
on the next re-materialization) — see the wenku8 plugin.)

The host sniffs the intrinsic dimensions (PNG/GIF/JPEG/WebP header) of
every stored image and emits `width`/`height` attributes on chapter
`<img>` tags, so readers reserve layout space before the bytes load —
without it every image load grows the page and the view keeps
re-anchoring while scrolling. Dimensions are looked up at chapter read
time too, so chapters materialized before this existed get annotated on
the fly.

**Epoch budget note.** Every `fetch` (pages *and* images) spends the
same call budget (`MANDARA_PLUGIN_FETCH_TIMEOUT_MS`, default 30 s).
A plate-heavy chapter fetched over a slow CDN can need more: the wenku8
verification measured ~7 s per plate, ≈2 min for a 16-plate 插图 chapter.
Sources that store images should run with a raised fetch-timeout cap.

## HTTP acquisition (`http.fetch`)

`http.fetch` is the one way to reach an external source. The host
provides plain network access — any host, http or https; the plugin
decides what a status means (4xx/5xx are plain `response`s, not errors):

- **Redirects**: at most 5 hops, curl-style method downgrade on 301–303;
  exceeding the cap is a `redirect-limit` error (not the last response).
- **Timeout**: overall deadline = `min(requested,
  MANDARA_PLUGIN_FETCH_TIMEOUT_MS)` (default 30 s).
- **Proxy**: outbound requests honor the standard environment variables
  (`ALL_PROXY` / `HTTPS_PROXY` / `HTTP_PROXY`, lowercase variants too);
  hosts in `NO_PROXY` connect directly. Useful where the server has no
  direct internet route.
- **Size**: response bodies are capped
  (`MANDARA_PLUGIN_FETCH_MAX_BYTES`, default 64 MiB); oversized
  responses abort with `size-limit` and no partial data.
- **Caps**: overall timeout = `min(requested, MANDARA_PLUGIN_FETCH_TIMEOUT_MS)`
  (default 30 s); response bodies ≤ `MANDARA_PLUGIN_FETCH_MAX_BYTES`
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
let resp = mandara::plugin::http::fetch(&mandara::plugin::http::Request {
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
    match mandara::plugin::config::configure().get(idx) {
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

Plugins describe only their own source. Users independently choose metadata
and content in the acquisition flow; the host records both source ids and
source-local book ids. Plugins never reference another instance. Owners and
admins may rebind an existing file via `POST /api/files/{id}/content-source`;
sync and refresh preserve that user-selected binding.

## Example plugins

- `plugins/hello-plugin/` — small `declare` + `identify` + `content`
  plugin; config-driven (site name, content variant, verbose logging, a
  deliberate infinite-loop switch for testing the epoch timeout).
- `plugins/wiki-plugin/` — metadata-only over HTTP: `search` + `lookup`,
  config `base-url`/`api-key`/`site-name`/`page-size`; users choose
  the reader source separately. Try it with `scripts/mock-source.py`.
- `plugins/bangumi-plugin/` — a **real-world** metadata-only source:
  Bangumi (`https://bgm.tv`) book subjects via the official v0 API.
  `search` + `lookup`, no content — acquired entries are pure metadata;
  attach content by uploading files or rebinding to a content plugin.
  Config: `base-url` and an optional **access token**
  (<https://next.bgm.tv/demo/access-token>) sent as
  `Authorization: Bearer …` — NSFW subjects are hidden from
  unauthenticated requests, so leave it empty for all-ages only. The
  subject infobox is mapped onto authors/translators/illustrators and
  the extended-metadata JSON (publisher/pub_date/isbn/pages/price/
  subtitle/tags; ISBN checksum-verified guest-side, invalid ones skipped),
  unknown keys preserved under `infobox_misc`. Every request carries a
  descriptive User-Agent as required by the API guidelines. Series lookups
  return real book subjects linked as 单行本 via
  `GET /v0/subjects/{id}/subjects` (not adaptations). Relation failures
  never synthesize volumes from a count. Search stays one paginated request.
- `plugins/reader-plugin/` — reading-site source over HTTP: `search` +
  `lookup` + `content` + `book-file`; chapter mode for `r-N` books, and
  `get-book-file` returning the epub the source serves (file mode wins,
  chapter mode is the fallback).
- `plugins/wenku8-plugin/` — a **real-world** source: 轻小说文库
  (`https://www.wenku8.net/`, GBK-encoded pages). `lookup` + `content`
  only: wenku8's search and listings are login-walled, so books are
  browsed by their numeric series id (the number in the book URL, e.g.
  `3617`) and acquired by selecting an independently identified volume — the library source browser's manual-id entry, and the
  rebind dialog, support lookup-only sources. Config: `base-url`
  (mirror-switchable), optional `referer` override, and
  `illustration-placement` — wenku8 appends an `插图` (color plates)
  chapter at the *end of every volume*; the plugin keeps it **in that
  original position** by default (`end`), or moves it to the volume
  front like the physical book / linovelib2epub (`front`), or drops it
  (`skip`). Plates are extracted
  as numbered image URLs, emitted one per line (`N. <url>`); the host
  expands them into `<figure>` elements at the ingest boundary (see the
  text chapter conventions above). Prose chapters also carry `（插图NNN）`
  marks where the print book has a plate; the plugin resolves them to
  the matching plate of that volume's 插图 chapter (001 → first plate,
  `[插图005] https://…` inline, so the image appears where the text
  references it). wenku8 rate-limits this server's IP under bursty
  scraping (HTTP 429): transient 403/429/5xx/transport responses are
  retried up to three times per request; a failed chapter read can be
  retried later and, once materialized, is served from the library DB.
  Build: `./scripts/build-plugins.sh wenku8`.

## Building & running the demos

```sh
./scripts/build-plugins.sh          # every plugins/*-plugin crate
cp plugins-built/*.wasm data/plugins/
```

Under Nix each plugin is built as its own package
(`nix build .#mandara-plugin-<name>`), with `.#mandara-plugins`
collecting them all.

The guest crates use `wit-bindgen = "0.60"` with
`wit_bindgen::generate!({ world: "mandara-plugin", path: "../../crates/mandara-plugin/wit" })`
and `export!(PluginName)`. The generated core module embeds a
`component-type` section, so `wasm-tools component new` lifts it into a
component without adapters (the world imports no wasi interfaces).

End-to-end demo of the HTTP acquisition layer (chapter mode + file mode):

```sh
python3 scripts/mock-source.py 8765 &           # the remote source
cargo run -p mandara-server                   # the server
./scripts/e2e-http-plugins.sh                   # full automated check
```

The host unit tests embed `plugins-built/hello.wasm` as a fixture — after
changing the WIT or the hello guest, rebuild with
`./scripts/build-plugins.sh hello` to refresh it.