# Plugin System v2 — Design & Development Plan

Status: **proposal** (dev phase; no backward compatibility constraints).
Supersedes the v1 interface in `crates/mandara-plugin/wit/mandara.wit`
and the authoring guide `docs/plugins.md` (rewritten at implementation time).

## 1. Requirements summary

Collected from the project owner (dev phase, breaking changes allowed):

| # | Requirement | Current (v1) status |
|---|---|---|
| R1 | Plugins should be **declarative** where possible: catalogs/contents as data, not code | not met — imperative `list-books`/`chapter-titles`/`get-chapter`, N+1 sync |
| R2 | Plugins must be **stateless**: no state across calls, no host-side mutable state handed to plugins | met — fresh wasmtime `Store` per call; keep |
| R3 | **Flexible, strongly typed configuration** per plugin | not met — no configuration channel at all |
| R4 | **Large sources**: catalog fetched by id or by **search** with pagination, never fully enumerated; no eager materialization of huge catalogs | not met — `list-books` is unparameterized, startup sync materializes everything; `get-book(id)`-by-scan |
| R5 | **Metadata and chapter content fetched independently**: e.g. metadata from a wiki plugin, chapter content from a reading-site plugin | not met — one plugin is both; `book_files.source/external_id` drives every path |

Non-goals (dev phase): no compatibility with v1 plugins, v1 `mandara:plugin@0.1.0`
package, or the v1 sync policy. `local` uploads and the unified books →
book_files → chapters model stay untouched.

## 2. Architecture overview

```
data/plugins/*.wasm
   │  loaded once per file
   ▼
mandara-plugin host                plugin_instances (SQLite, new)
  WasmPlugin per registered instance ── id = source id
  ├─ Fresh Store per call            ── config validated & stored here
  ├─ configure(values) before call   ── injected per call (stateless)
  ├─ epoch-deadline / limits         ── trap runaway plugins
  ▼
Library (books / book_files / chapters)
  ├─ metadata path:  source / external_id            (unchanged semantics)
  ├─ content path:   content_source / content_external_id  (new, NULL = same)
  ▼
HTTP API: /api/plugins/* (browse, search, materialize, configure)
  ▼
Frontend: plugin browser (search → “加入书架”), admin config form
```

Design decisions:

- **One world, required exports, stubbed**: wasmtime 48's WIT parser
  (wit-parser 0.254) does **not** support the `optional-export` keyword, and
  `bindgen!` extracts all world exports at instantiation. So v2 keeps every
  export required; unused features return `none`/empty and are *declared* via
  `capabilities() -> list<string>`. The host keys all policy off
  `capabilities` (one call at load time).
- **Capabilities** (`declare`, `search`, `lookup`, `identify`, `content`):
  - `declare` — static full-catalog plugin: startup sync, inline content ok
  - `search` — catalog browsed with `search-books(query, offset, limit)`
  - `lookup` — single book by id (`get-book`), used by refresh & materialize
  - `identify` — upload identification (v1 behavior)
  - `content` — provides `chapter-titles`/`get-chapter`
  - Metadata-only plugins declare everything except `content`.
- **Statelessness preserved**: fresh `Store` per call; config re-injected via
  the `configure` import at the start of every call; plugins never see host
  globals. All durable state (instances, config, catalog, chapters) lives in
  SQLite under the host's control.

## 3. WIT v2 (`mandara:plugin@0.2.0`)

```wit
package mandara:plugin@0.2.0;

interface types {
    record book-entry {
        id: string,                       // stable id within this plugin
        title: string,
        authors: list<string>,
        description: option<string>,
        cover-url: option<string>,
        // Chapters come from another plugin instance when set.
        content-source: option<string>,   // instance/source id
        content-id: option<string>,       // book id inside content source (default: `id`)
    }

    record chapter {
        title: string,
        // Empty string = title-only placeholder (body fetched lazily via
        // get-chapter); non-empty = can be materialized eagerly.
        content: string,
    }

    record declared-book {
        book: book-entry,
        chapters: list<chapter>,
    }

    record search-result {
        total: u64,                       // source-side total (may be capped)
        items: list<book-entry>,
    }
}

interface store {
    /// Append a line to the server log, tagged with the instance id.
    log: func(message: string);
}

interface config {
    record config-field {
        key: string,                      // stable key
        label: string,                    // zh-CN display label
        kind: config-kind,
        default: option<string>,          // serialized default value
        required: bool,
        hint: option<string>,
    }
    variant config-kind {
        string, number, boolean,
        enum(list<string>),               // options; config-value = enum-index
        list-of-string,                   // newline/comma separated in UI
    }
    variant config-value {
        string(string), number(f64), boolean(bool),
        enum-index(u32), list(list<string>),
    }
}

world mandara-plugin {
    import types;
    import store;
    import config;

    use types.{book-entry, chapter, declared-book, search-result};
    use config.{config-field, config-value};

    /// Human-readable name of this plugin.
    export name: func() -> string;

    /// Configuration schema; the host renders an admin form from it,
    /// validates submitted values against it, and stores the result.
    export config-schema: func() -> list<config-field>;

    /// Catalogs access patterns implemented: subset of
    /// ["declare", "search", "lookup", "identify", "content"].
    /// Host policy: declare ⇒ startup sync; search/lookup-only ⇒ lazy.
    export capabilities: func() -> list<string>;

    /// Full static catalog, when it is small enough to enumerate eagerly.
    export declare: func() -> option<list<declared-book>>;

    /// Paginated catalog search (large sources).
    export search-books: func(query: string, offset: u32, limit: u32) -> search-result;

    /// Single book by id (materialize-on-demand, refresh).
    export get-book: func(book-id: string) -> option<book-entry>;

    /// Chapter titles of a book in reading order; indices match get-chapter.
    export chapter-titles: func(book-id: string) -> list<string>;

    /// Fetch one chapter. `none` = not found.
    export get-chapter: func(book-id: string, index: u32) -> option<chapter>;

    /// Identify an uploaded file (`filename`, sha-256 hex digest). First
    /// `some` across instances wins and supplies the upload's metadata.
    export identify-upload: func(filename: string, file-hash: string) -> option<book-entry>;
}
```

Notes:

- `configure` (in the `config` interface, like `log` in `store`) is a
  **host-provided import**: the host validates config against
  `config-schema` and injects the values at the start of every call
  (stores are per-call, so nothing persists in the guest).
- `book-entry.content-*`: metadata-only plugins set these to point at
  another instance; content plugins leave them `none`.
- v1's `list-books` is replaced by `declare` (small) + `search`/`get-book`
  (large). No pagination on `declare` — declared catalogs must fit.

## 4. Host model

### 4.1 Instances & lifecycle

New table:

```sql
CREATE TABLE plugin_instances (
    id         TEXT PRIMARY KEY NOT NULL,   -- source id (book_files.source)
    wasm_file  TEXT NOT NULL,               -- file under data/plugins/
    config     TEXT NOT NULL DEFAULT '{}',  -- validated config JSON
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
```

- Startup: every `data/plugins/*.wasm` is compiled once; one `WasmPlugin`
  is registered **per instance row** referencing that file (same wasm,
  multiple instances = different configs/source ids, e.g. one “web source”
  plugin pointed at two sites).
- Registration happens via admin API (see §6), not by dropping files only —
  files provide code, DB provides instances/config.
- Load order = instance creation order (stable for `identify-upload`).

### 4.2 Per-call protocol (unchanged shape, extended)

```
call(export, args…):
  store = Store::new(engine, HostState { name, config_values })
  bindings = instantiate(store, component, linker)     // per call
  configure(store, bindings, config_values)            // inject config
  bindings.call_<export>(store, args…)                 // epoch-deadline armed
```

### 4.3 Guards (new, required for R4)

- `Config::epoch_interruption(true)`; per-call `store.set_epoch_deadline(1)`
  and a background/incremental epoch pump; trap → `Plugin timeout`.
- Host caps: `search-books` `limit` ≤ 50, `offset` ≤ 10 000; `declare`
  catalog ≤ 10 000 books and ≤ 100 000 chapters (error beyond, warn beyond
  10% of cap); `get-chapter` content ≤ 2 MiB per call (checked after lift).

## 5. Data model (migration 0004_plugin_v2.sql)

```sql
-- (plugin_instances table as in §4.1)

-- Content indirection: chapters of a virtual plugin file may come from a
-- different instance. NULL = same as `source` / `external_id`.
ALTER TABLE book_files ADD COLUMN content_source TEXT;
ALTER TABLE book_files ADD COLUMN content_external_id TEXT;
```

Semantics:

| Path | Columns | Used by |
|---|---|---|
| Metadata | `source`, `external_id` (UNIQUE pair) | sync, refresh, identify, catalog picker, dedup |
| Content | `content_source ?? source`, `content_external_id ?? external_id` | `ensure_titles`, `materialize_from_source`, chapter reads |

`chapters` stay keyed by `file_id` — unchanged. Local uploads unaffected
(both new columns NULL).

## 6. Sync & materialization policy

| Instance capabilities | Startup behavior | Catalog entry points |
|---|---|---|
| `declare` | full sync via `declare` (metadata + title rows; eager body materialization for non-empty `content`) | n/a — everything is in the library |
| `search` (+`lookup`) | **no listing, nothing materialized** | `GET /api/plugins/{id}/search` → user picks a book → `POST /api/books` (`plugin_source` + `plugin_book_id`) materializes just that book |
| none of the above | only virtual metadata (plugin without books) | n/a |

Materialization of one book (unified acquisition: `POST /api/books`
with `plugin_source` + `plugin_book_id` as the content, 获取书籍 →
添加元数据):

1. `get-book(book-id)` → `book-entry` (+ `chapter-titles`) — 404 when `none`
2. `ensure_plugin_book` creates/updates `books` row + virtual `book_files`
   row with `content_source`/`content_external_id` from the entry
3. Chapter title placeholders inserted; bodies stay lazy (first read)

`POST /api/books/{id}/refresh` re-pulls **metadata only** via `get-book`
(v1 scanned the whole catalog — removed). Titles of a plugin file come from
the **content** instance; metadata conflicts always resolve in favour of the
metadata instance (it owns the `books` row).

## 7. Metadata / content separation (R5)

Scenario: wiki plugin `wiki` knows the book as `bk-42`; reading-site plugin
`reader` knows the same book as `r-1337`.

- `wiki` returns `book-entry { id: "bk-42", …, content-source: "reader",
  content-id: "r-1337" }` → the virtual file is created with
  `content_source = 'reader'`, `content_external_id = 'r-1337'`; first read
  calls `reader.get-chapter("r-1337", idx)`.
- Rebinding (`POST /api/files/{id}/content-source`, owner/admin): switch a
  book's chapters to another instance manually — covers “I found the same
  book on the reading site, add it there” when the wiki does not know the
  mapping. Validates the target instance exists; optional `get-book` check.
- A plugin that is both metadata and content source does nothing new
  (content-source = none ⇒ self).

## 8. HTTP API additions (OpenAPI)

```
GET    /api/plugins                        # instances + capabilities (+ config schema refs)
POST   /api/plugins/sync                   # (existing) resync declare-capable instances
POST   /api/plugins/instances              # admin: register instance {wasm_file, config?}
DELETE /api/plugins/instances/{id}         # admin
PUT    /api/plugins/instances/{id}/enabled # admin
GET    /api/plugins/{id}/config-schema     # render the admin form
PUT    /api/plugins/{id}/config            # validate + store; triggers re-sync for declare
GET    /api/plugins/{id}/search?q=&limit=&offset=
POST   /api/books                          # unified acquisition: content = file | plugin_source+plugin_book_id; metadata = book_id | plugin | auto (+ overrides)
POST   /api/files/{id}/content-source      # owner/admin: rebind content instance
```

Config validation errors are structured (`[{field, message}]`) for inline
form display; unknown keys rejected; `required` enforced; enum indices range
checked.

## 9. Frontend

- **管理**: plugin instances list (enable/disable/delete), schema-rendered
  config form, "重新同步" per instance.
- **图书馆**: “源浏览器” dialog — pick instance → search box → paginated
  book cards → “加入书架” (materialize) → lands in the library.
- **书详情**: show metadata vs content source; “更换内容源” action for
  owner/admin.
- Upload dialog (attach/auto/manual) keeps working; the manual plugin
  picker now uses `get-book` instead of full catalog scans.

## 10. Implementation plan

| Phase | Scope | Acceptance |
|---|---|---|
| P0 | WIT@0.2 rewrite (`mandara.wit`), host `bindgen!` regen + `WasmPlugin` rework (per-instance config, `configure` injection, capabilities call, epoch deadline/limits), rewrite `hello-plugin` as data-driven demo, delete v1-only plumbing (`list-books`/scan-based `plugin_entry`) | `cargo test` green (config roundtrip, capability stubs, timeout trap); wasm-tools lift still works; `cargo fmt` before commit |
| P1 | `plugin_instances` migration; register/enable/delete APIs; config-schema + validation + PUT config; admin UI form | e2e: register instance → schema GET → bad config 400 with field errors → good config → plugin log shows injected values |
| P2 | `declare` + `search-books` + `get-book` host plumbing; lazy sync policy (no startup listing for search-only); `/search` + `/books` endpoints; library source-browser UI; refresh via `get-book` | e2e: search-only plugin → empty library until user picks a book; caps enforced (400 beyond limit) |
| P3 | `content_source`/`content_external_id` columns; content routing in `ensure_titles`/`materialize_from_source`; rebind endpoint + book-detail UI | e2e with two demo plugins: `wiki-plugin` (declare/search, metadata only) + `reader-plugin` (content; searchable) → metadata from wiki, chapters materialized from reader; rebind switches reader book |
| P4 | Guards hardening (epoch pump scheduling, caps audit), rewrite `docs/plugins.md` for v2, AGENTS.md progress log, e2e regression of uploads + shares + sessions | full justfile checks green; demos documented |

Two demo plugins land in P3 as the reference implementation of R5
(see `plugins/wiki-plugin/`, `plugins/reader-plugin/`).

## 11. Open questions (decide during P1)

- Cover images: `cover-url` is still metadata-only; storing images is out of
  scope (existing known limitation) — confirmed.
- Hot-reload of wasm files: instance registry makes file replacement
  detectable at next restart only; a `?reload=1` sync is a cheap follow-up —
  deferred.
- `identify-upload` ordering across *instances* of the same wasm — resolve
  by instance creation order (matches v1 load order).