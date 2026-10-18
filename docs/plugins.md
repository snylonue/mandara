# Plugin authoring guide (v3)

Mandara's wasm plugin system lets users provide custom book and metadata
sources. Plugins run as **WebAssembly components** inside the host sandbox
(wasmtime): network access is available only through the host's
`http.fetch` import, with timeout/size caps. Host imports also provide
instance-scoped persistent state and content-addressed image storage.
No WASI, no filesystem, no ambient network.

See `docs/plugin-v2-design.md` (design, implemented) and
`docs/plugin-http-api-design.md` (v3 HTTP acquisition, implemented) for
the full design. This guide is for plugin authors.

## Concepts

- **Wasm file** (`data/plugins/*.wasm`) — the compiled plugin code. The
  server compiles every file at startup and exposes it for registration.
- **Instance** — one registered configuration of a wasm file, with its own
  id (the source id in `book_files.source`), stored config, and enable
  flag. One wasm file can back several instances (e.g. one "web source"
  plugin pointed at two sites). Instances are registered via the admin UI
  or `POST /api/plugins/instances`; instance ids default to a uuid, or an
  admin-chosen stable id (needed when metadata plugins point at content
  plugins).
- **Capabilities** — `capabilities()` declares which catalog patterns the
  plugin implements (see below). The host keys all policy off this list.
- **Guest memory** — a fresh store per call; business exports receive
  configuration as an explicit parameter. Metadata exports receive none.
  Globals do not survive calls. Use the instance-scoped `state` import for durable plugin data;
  materialized books and chapter bodies remain the host library cache.

Wenku8 and Novelia cache successfully stored illustration references in instance
state under `image:v1:<absolute-url>`. Repeated URLs reuse the local image
across chapters and restarts without fetching the bytes again. Failed downloads
and unsupported image payloads are not cached. State read/write errors degrade
to uncached behavior; simultaneous cache misses may still download twice because
the state API has no compare-and-swap. URLs are assumed immutable; delete the
corresponding state entry to refresh an image whose upstream bytes changed.

## Host ownership

The Rust host separates four lifetimes:

- `PluginRuntime` owns one shared Wasmtime engine/linker, the HTTP policy
  (stored by value), and the shared image store.
- `CompiledPlugin` contains only a file identity and compiled component.
  `PluginRuntime::load_dir` scans files and `compile` compiles their bytes;
  neither executes guest exports.
- `PluginInstance` binds a registered ID, compiled code, stored configuration,
  and its state store, but does not retain the executor or host services.
  The server restores these objects at startup and creates them after
  inserting a new registration.
- `HostState` holds only shared instance-ID/file strings, the instance-bound
  state-store handle, a cheap clone of `HostServices`, and per-call image
  accounting. It retains neither the complete instance nor its compiled
  component or configuration. The runtime derives positional configuration
  locally and passes it directly to the business export.
  Identity handles share their strings without copying them per call, and the
  state handle is cloned from the bound instance, never constructed per call.
  Cloning services copies the HTTP policy and clones only the shared image-store
  handle, without an additional reference-counted service wrapper.

Guest exports are methods on `PluginRuntime`, for example
`runtime.get_chapter(&instance, content_id, index)`. Calls receive the bound
instance, never separately selected configuration and storage. The call store
uses the compiled component's engine directly, while the runtime supplies
the linker and shared host services. Components are bound through their
compiling runtime; calls do not repeat that engine-identity check.

Configuration saves publish a new immutable snapshot while retaining the
same state store. In-flight calls keep their original configuration snapshot;
subsequent calls use the saved configuration. Unregistering removes the live
instance and its database-owned state.

## Persistent plugin state

The state import extends protocol v0.13.0 without changing existing exports:

```wit
interface state {
    get: func(key: string) -> result<option<list<u8>>, string>;
    set: func(key: string, value: list<u8>) -> result<_, string>;
    delete: func(key: string) -> result<bool, string>;
}
```

The host chooses the namespace from the registered instance ID, not the
Wasm filename or configuration. Instances sharing a Wasm file cannot read
one another's data. SQLite keeps values across calls, server restarts,
configuration changes, disabling/re-enabling an instance, and replacement
of its Wasm file. Deleting the instance deletes all its state; writes from
still-running calls cannot recreate state for a deleted instance.

- Missing keys return `Ok(None)`; an empty byte buffer is `Ok(Some(vec![]))`.
- `set` replaces the whole value; `delete` returns whether the key existed.
- Each operation is atomic. A successful write persists even if the rest
  of that plugin call later fails. Concurrent writes are last-writer-wins;
  `get` followed by `set` is **not** an atomic read/modify/write operation.
- No TTL, revisions, CAS, or automatic invalidation. Plugins choose their
  own keys and serialization and explicitly overwrite/delete stale data.
- Keys are arbitrary strings, including empty strings. The server's SQLite
  `StateStore` limits values to 256 KiB, and each instance to 4096 keys and
  16 MiB of key + value bytes. Storage implementations enforce their own
  quotas; the Wasm host does not impose a value size limit.
  A failed or over-quota write preserves the previous value.
- Validation, quota, and storage errors are `Err(string)`, not guest traps.
  Every export, including `name`, `config-schema`, `capabilities`, and
  `source-info`, runs with the registered instance's state store. Loading a
  Wasm file only compiles it and does not execute guest functions.

Rust guest example (choose serialization appropriate for the data):

```rust
use mandara::plugin::state;

fn cached_index(key: &str, fetch: impl FnOnce() -> Result<Vec<u8>, String>)
    -> Result<Vec<u8>, String>
{
    if let Some(bytes) = state::get(key)? {
        return Ok(bytes);
    }
    let bytes = fetch()?;
    state::set(key, &bytes)?;
    Ok(bytes)
}

// Explicit invalidation:
// state::delete("chapter-index:123")?;
```

Existing v0.13.0 components remain compatible without a rebuild. Guests
that use the new state import must be rebuilt with `./scripts/build-plugins.sh`;
rebuilding does not deploy them to `data/plugins/` automatically.

## Lifecycle

- Startup: wasm files are compiled without executing plugin code. Existing
  instances are restored from `plugin_instances`; plugin information is read
  only in an instance context, with its own state store.
- Registration only selects a Wasm file and creates an instance (with an
  optional explicit ID). Configuration starts as `{}`. The UI then reads
  the instance's configuration schema; values are validated when saved.
  Unsaved fields use schema defaults during calls, without rejecting
  missing required fields at registration.
- `declare`-capable instances are **fully synced** at startup and on
  `POST /api/plugins/sync` (metadata + title rows; eager bodies for
  every supplied `content`, including empty strings).
- `search`/`lookup`-capable instances are **lazy**: nothing is
  materialized until a user acquires a book through the unified
  acquisition flow (`POST /api/books` with `plugin_source` +
  `plugin_book_id` — 获取书籍 → 添加元数据).
- Metadata and content have independent IDs and interfaces. `get-book`
  returns only metadata; `get-content(content-id)` returns one declaration
  containing identity, loading form and chapter titles with optional bodies. Its `none` means that content ID is unavailable, not a metadata
  classification. Content attachment never requires `get-book`. The host stores
  that form on the content item at acquisition; reads never infer it from
  capabilities. Plugins own any upstream fallback within the declared form.
  Uploaded files provide `file` content through the same parser, cache and
  completion pipeline. Chapter-mode content is queued for background loading.
- Content acquisition returns without waiting for all chapter
  bodies. Background downloads fill missing bodies in chapter order, with
  at most two background fetches active globally. Cached bodies are skipped;
  failed chapters are logged without blocking subsequent chapters.
- Refresh queues newly discovered chapters; rematerialize clears cached
  bodies and queues a download of the same content item. Existing downloads
  are joined rather than restarted; deletion does not wait for remote requests.
- Readers and background downloads use the same start-or-join operation for
  each cache key. Its short critical section only selects one writer and
  registers completion waiters; it does not hold cache data or cache states.
  Chapter keys are independent; whole-file downloads use one key per content
  item. Database reads, writes and plugin work never hold this lock. Writers
  finish their cache writes before delivering normalized content through the
  channel. Chapter downloads return a chapter; whole-file downloads return all
  chapters so each waiter selects its own index without rereading the database.
  Database bodies use NULL for uncached title placeholders; an empty string is
  a cached empty chapter. Cache presence never depends on body length.
  A missing chapter is an error; an existing chapter may have an empty body. Channels carry
  only not-found or failed download errors; detailed causes are logged by the
  writer, not shared API errors. Failures release waiters without automatic retries.
  Canceling a reader does not cancel the writer. Existing cached bodies are
  returned directly, without joining a download or requiring an available plugin.
- `identify`-capable instances are asked to recognize uploaded files
  (`identify-upload`, first match wins, in instance creation order).
- `POST /api/books/{id}/refresh` re-pulls **metadata only** via
  `get-book` (never a catalog scan); file-mode rows are never re-parsed;
  materialized chapter bodies are never overwritten.

## Interface (WIT)

Interface file: `crates/mandara-plugin/wit/mandara.wit`, world
`mandara:plugin/mandara-plugin@0.13.0`. Every export is **required**
(the WIT parser has no optional exports) — unsupported features return
`none`/empty and are declared via `capabilities()`:

```wit
interface types {
    enum content-mode { chapters, file }
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
    record chapter { title: string, content: string, format: string }
    record declared-chapter { title: string, content: option<string>, format: string }
    record declared-content { id: string, mode: content-mode, chapters: list<declared-chapter> }
    record declared-book { book: book-entry, content: option<declared-content> }
    variant catalog-entry { single(book-entry), series(series-entry) }
    record search-result { total: u64, items: list<catalog-entry> }
    record series-result { series: series-entry, books: list<book-entry> }
    record book-file { filename: string, mime: string, bytes: list<u8> }
    record config-field { key, label, kind, default, required, hint }
    variant config-kind { text, number, boolean, enum-options(list<string>), list-of-string }
    variant config-value { text(string), number(f64), boolean(bool), enum-index(u32), string-list(list<string>) }
}

interface store { log: func(message: string); }

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

**Independent declarations (v0.13.0).** `book-entry` is pure metadata.
`get-content` returns `declared-content` with identity, loading form and
chapter declarations in a single call; `none`
only means content is unavailable. `declared-content` carries its own ID,
loading form and optional chapter bodies. A catalog envelope explicitly
associates metadata and content without requiring their IDs to match. The
hello plugin demonstrates `hello-1` metadata with `hello-content-1` content.
Chapter-mode plugins declare the complete chapter directory in `get-content`
when content is created. The host saves those chapter declarations with the
content identity and loading form before acquisition returns. Missing bodies
are downloaded separately in the background; they do not make the directory
incomplete. For file-mode content, the host parses and saves the directory and
TOC during creation. Reading uses this saved directory without querying the
plugin; only an explicit user refresh updates it. The host never switches to
chapters on file failure. Rebuild all components with the current WIT.

**Declared bodies (v0.10.0).** `declared-chapter.content` is explicitly optional:
`none` omits the body for background downloading during acquisition or explicit
refresh, while `some("")` supplies a valid empty chapter.
The host normalizes every supplied body and preserves its cache presence even
when normalization produces an empty string. `chapter.content` returned by
`get-chapter` remains a required string; a missing chapter is returned as `none`.
This replaces the v0.9 empty-string sentinel and changes the component ABI;
rebuild all plugins with the current WIT before deploying the new host.

World exports:

| Export | Required by | Purpose |
|---|---|---|
| `name()` | always | Human-readable name |
| `config-schema()` | always | Config fields the host renders as an admin form |
| `capabilities()` | always | `["declare","search","lookup","identify","content","book-file"]` subset |
| `declare(config)` | always | Small static catalog (`None` for large sources) |
| `search-books(config, query, offset, limit)` | `search` | Paginated catalog browsing |
| `get-book(config, book-id)` | `lookup` | Single book (materialize on demand, refresh) |
| `get-content(config, content-id)` | content acquisition | Identity, loading form and chapter declarations together |
| `get-series(config, series-id)` | `lookup` | Series metadata plus independently addressable member books |
| `get-chapter(config, content-id, index)` | `content` | One chapter body |
| `identify-upload(config, filename, file-hash)` | `identify` | Upload recognition |
| `get-book-file(config, content-id)` | `book-file` | Whole book file (epub/txt download) |

**Explicit configuration.** Business exports take `config: list<config-value>`
as their first parameter, in `config-schema` field order. `name`, `source-info`,
`capabilities`, and `config-schema` remain parameterless. Configuration is not
a host import. The package remains v0.13.0, but these signatures change the
component ABI: rebuild every plugin before using it with the updated host.

**One volume = one book (v0.9.0).** A plugin adapts its source into
explicit series and individual books. `get-series` returns series metadata
and member `BookEntry` values; member IDs are used by `get-book`. Content
IDs are selected independently and used by the content exports.
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

**Upgrade:** rebuild all components after the WIT change. Metadata
provenance and content identity remain independent.

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
usually applies first). Guest computation is bounded by 1 billion Wasmtime
fuel units per call; a runaway guest traps when its fuel is exhausted. Host
I/O does not consume fuel, and calls have **no aggregate wall-clock
deadline**. `MANDARA_PLUGIN_FETCH_TIMEOUT_MS` (default 30 s) caps each
individual HTTP request, not the sum of requests in an export. Plugin
calls run on blocking threads, so a slow source never stalls a tokio worker.

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

**Image latency note.** Serial page and image requests do not share a
wall-clock deadline. Each `fetch` has its own timeout, but synchronous
image downloads still delay the chapter response: Wenku8 verification
measured ~7 s per plate, approximately 2 min for a 16-plate illustration
chapter. Removing the aggregate deadline does not make image storage
asynchronous.

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
absent), stores the validated JSON, and **passes the values explicitly to
each business export**. Map the parameter list back by schema order; pass a
slice to helpers that need configuration, instead of reading ambient host state:

```rust
const FIELDS: [&str; 2] = ["site-name", "page-size"];

fn site_name(config: &[ConfigValue]) -> String {
    let idx = FIELDS.iter().position(|k| *k == "site-name").unwrap();
    match config.get(idx) {
        Some(ConfigValue::Text(s)) if !s.is_empty() => s.clone(),
        _ => "默认站点".into(),
    }
}
```

Metadata and schema queries do not receive configuration. `HostState` stores
no configuration, and there is no `config` import or `configure` function.
Configuration snapshots and persistent state remain instance-owned.

Defaults are serialized JSON (`"8"`, `"false"`, `"\"x\""`); a text field
whose default is not valid JSON is treated as the literal string.
URL templates, api keys and page sizes are all config values, so one wasm
file serves many sites (per-instance config).

## Metadata / content separation (R5)

Plugins describe only their own source. Users independently choose metadata
and content in the acquisition flow; the host records both source ids and
source-local book ids. Plugins never reference another instance. A book
holds multiple independent content items, each with its own immutable
source and book ID, chapter cache, reading sessions and shares. Users select
an item to read; deleting one does not affect the others. There is no global
content-source setting or source-rebinding endpoint. Migration 0017 folds
legacy overrides into each item's recorded source, retaining item IDs, caches,
sessions, shares and archived original bytes. Colliding legacy items are kept.

## Example plugins

- `plugins/novelia-plugin/` — Novelia web novels (`search` + `lookup` +
  chapter content) and content-only Wenku downloads (`book-file`), identified by
  upstream volumeId. Configurable providers, translation and account login. See
  [Novelia setup and limitations](novelia-plugin.md).

- `plugins/hello-plugin/` — small `declare` + `identify` + `content`
  plugin; config-driven (site name, content variant, verbose logging, a
  deliberate infinite-loop switch for testing fuel exhaustion).
- `plugins/wiki-plugin/` — metadata-only over HTTP: `search` + `lookup`,
  config `base-url`/`api-key`/`site-name`/`page-size`; users choose
  the reader source separately. Try it with `scripts/mock-source.py`.
- `plugins/bangumi-plugin/` — a **real-world** metadata-only source:
  Bangumi (`https://bgm.tv`) book subjects via the official v0 API.
  `search` + `lookup`, no content — acquired entries are pure metadata;
  attach separate content items by uploading files or selecting a content plugin.
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
  `lookup` + `content` + `book-file`; the upstream `content` endpoint
  supplies loading and chapter declarations in one response. Metadata responses carry no
  content availability or loading fields.
- `plugins/wenku8-plugin/` — a **real-world** source: 轻小说文库
  (`https://www.wenku8.net/`, GBK-encoded pages). `lookup` + `content`
  only: wenku8's search and listings are login-walled, so books are
  browsed by their numeric series id (the number in the book URL, e.g.
  `3617`) and acquired by selecting an independently identified volume — the library source browser's manual-id entry, and the
  add-content dialog, support lookup-only sources. Config: `base-url`
  (mirror-switchable), optional `referer` override, `user-agent` (used
  for every request, including images; omitted or empty uses the default
  browser UA), and `illustration-placement` — wenku8 appends an `插图`
  (color plates) chapter at the *end of every volume*; the plugin keeps it **in that
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
