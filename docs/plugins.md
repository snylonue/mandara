# Plugin authoring guide (v2)

Bookshelf's wasm plugin system lets users provide custom book and metadata
sources. Plugins run as **WebAssembly components** inside the host sandbox
(wasmtime): they can only call the host's `store::log` and
`config::configure` — no host filesystem, no network.

See `docs/plugin-v2-design.md` for the full design (status: implemented).
This guide is for plugin authors.

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
  guest.

## Lifecycle

- Startup: wasm files compiled; instances loaded from `plugin_instances`.
- `declare`-capable instances are **fully synced** at startup and on
  `POST /api/plugins/sync` (metadata + title rows; eager bodies for
  non-empty `content`).
- `search`/`lookup`-capable instances are **lazy**: nothing is
  materialized until a user picks a book in the source browser
  (`POST /api/plugins/{id}/books` → `get-book` + title placeholders).
- Chapter bodies materialize into the central DB on first read; afterwards
  the book behaves like any uploaded file (readable even if the plugin
  file is removed).
- `identify`-capable instances are asked to recognize uploaded files
  (`identify-upload`, first match wins, in instance creation order).
- `POST /api/books/{id}/refresh` re-pulls **metadata only** via
  `get-book` (never a catalog scan); title placeholders come from the
  content instance; materialized chapter bodies are never overwritten.

## Interface (WIT)

Interface file: `crates/bookshelf-plugin/wit/bookshelf.wit`, world
`bookshelf:plugin/bookshelf-plugin@0.2.0`. Every export is **required**
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
}

interface store { log: func(message: string); }

interface config {
    record config-field { key, label, kind, default, required, hint }
    variant config-kind { text, number, boolean, enum-options(list<string>), list-of-string }
    variant config-value { text(string), number(f64), boolean(bool), enum-index(u32), string-list(list<string>) }
    configure: func() -> list<config-value>;   // host-provided
}
```

World exports:

| Export | Required by | Purpose |
|---|---|---|
| `name()` | always | Human-readable name |
| `config-schema()` | always | Config fields the host renders as an admin form |
| `capabilities()` | always | `["declare","search","lookup","identify","content"]` subset |
| `declare()` | always | Small static catalog (`None` for large sources) |
| `search-books(query, offset, limit)` | `search` | Paginated catalog browsing |
| `get-book(book-id)` | `lookup` | Single book (materialize on demand, refresh) |
| `chapter-titles(book-id)` | `content` | Titles in reading order |
| `get-chapter(book-id, index)` | `content` | One chapter body |
| `identify-upload(filename, file-hash)` | `identify` | Upload recognition |

Host caps: `search-books` limit ≤ 50 / offset ≤ 10 000 (400 beyond);
`declare` ≤ 10 000 books / 100 000 chapters (error beyond, warn past 90%);
chapter content ≤ 2 MiB per call. Runaway plugins are trapped by the epoch
deadline (a call running for more than a few 200 ms pump intervals is
interrupted — keep catalog/chapter generation fast, or split it across
calls).

## Configuration

`config-schema()` returns the fields; values live in JSON keyed by field
key. The host validates (unknown keys rejected, `required` enforced, enum
indices range-checked), stores the validated JSON, and **injects the
values at the start of every call** — call `config::configure()` first in
your exports and map the returned list back by schema order:

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
- `plugins/wiki-plugin/` — metadata only: `search` + `lookup`, no
  `declare`, no `content`; every entry points `content-source: "reader"`.
- `plugins/reader-plugin/` — reading-site source: `search` + `lookup` +
  `content`; serves titles and bodies for `r-N` books.

## Building

```sh
./scripts/build-plugins.sh hello wiki reader   # -> plugins-built/*.wasm
cp plugins-built/*.wasm data/plugins/
```

The guest crates use `wit-bindgen = "0.60"` with
`wit_bindgen::generate!({ world: "bookshelf-plugin", path: "../../crates/bookshelf-plugin/wit" })`
and `export!(PluginName)`. The generated core module embeds a
`component-type` section, so `wasm-tools component new` lifts it into a
component without adapters (the world imports no wasi interfaces).

The host unit tests embed `plugins-built/hello.wasm` as a fixture — after
changing the WIT or the hello guest, rebuild with
`./scripts/build-plugins.sh hello` to refresh
`crates/bookshelf-plugin/tests/fixtures/hello.wasm`.