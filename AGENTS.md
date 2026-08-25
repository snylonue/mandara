# AGENTS.md

Guidance for AI coding assistants (and humans) working in this repository.

## Project

**Bookshelf** — a self-hosted light-novel reading website with a Rust backend
and a web frontend. Supports epub/txt books, multiple users, unified storage of
books and metadata, optional permission control, reading progress
management/sharing, and a wasm plugin system for user-provided book and
metadata sources.

**Status: initial scaffolding complete (verified end-to-end).** See
[Current Progress](#current-progress).

---

## Requirements (from the project owner)

1. Light-novel reading website; Rust backend + web frontend.
2. Support **epub** and **txt** text formats.
3. Multi-user backend with **unified storage** for books and metadata.
4. **Permissions per uploaded book**: the uploader chooses to publish or
   hide each uploaded book (per-file `public`/`private` visibility). A single
   metadata entry can be linked to **multiple book files** (formats,
   editions, translations). The whole auth layer can additionally be switched
   off for single-user use (`BOOKSHELF_AUTH_ENABLED=false`).
5. **wasm plugin system** for user-defined book and metadata sources.
6. Frontend: reading, **progress management**, and **sharing**.
7. Progress management must support **multiple sessions** (e.g. several
   devices reading the same book simultaneously, each with its own progress).
8. Nix-managed development environment:
   - Temporary packages are used via `nix run`.
   - Project dependencies are declared in `flake.nix`.
   - Language dependencies use their native package managers
     (cargo for Rust, npm for JS, uv for Python — none used yet).
9. Write progress updates to this file after each work session — keep the
   [Progress Log](#progress-log) current.
10. Docs and code comments are written in English; UI strings and book
    example content stay Chinese (zh-CN is the product language).
11. **One feature = one commit.** Commits land as soon as a feature is
    finished; never bundle multiple features into a single commit.
12. **Frontend/backend contract via API docs.** The OpenAPI description in
    `docs/api/openapi.yaml` is the single source of truth for the HTTP API.
    The frontend generates its request/response types from it
    (`openapi-typescript`, see `frontend/package.json`); components must not
    depend on backend implementation details beyond that contract.
13. Frontend UI is **i18n-ready**: all UI strings live in
    `frontend/src/i18n/locales/zh-CN.json` (i18next). Only zh-CN is shipped
    for now; adding a locale = adding a JSON file.
14. **Run `cargo fmt --all` before every commit** — the tree must stay
    rustfmt-clean (`cargo fmt --check` passes). Formatting changes from
    `cargo fmt` land in their own `chore:` commit, never mixed into feature
    commits.

## Tech Stack & Decisions

| Layer | Choice | Notes |
|---|---|---|
| Backend | Rust 2021, cargo workspace | 4 crates, see layout below |
| Web framework | axum 0.8 | `{param}` path syntax, multipart upload |
| DB | SQLite via sqlx 0.9 | runtime queries only (`sqlx::query*`, no `query!` macros) so no `DATABASE_URL` is needed at compile time (nix-friendly). Migrations in `crates/bookshelf-server/migrations/`. Two-level model: `books` = pure metadata, `book_files` = actual files (uploads or virtual plugin books); chapters/sessions/shares attach to files |
| Auth | JWT (jsonwebtoken 11, `rust_crypto` backend) + Argon2 | roles `admin`/`user`; optional via `BOOKSHELF_AUTH_ENABLED` |
| Plugin host | wasmtime 48 (component model) | WIT world `bookshelf:plugin/bookshelf-plugin` in `crates/bookshelf-plugin/wit/`; plugins loaded from `data/plugins/*.wasm`; chapters materialized into the central DB on first read |
| Plugin guest | wit-bindgen 0.60 (`generate!` + `export!`), `wasm32-unknown-unknown` | module is lifted with `wasm-tools component new` (no adapter needed, world imports no wasi) |
| Formats | `epub` (epub-rs) + `html2text`/`scraper`; custom txt parser | txt: UTF-8/UTF-16/GB18030 detection + chapter-heading split (CJK ordinal-marker headings, Chapter N, VOL.N, prologues, ...) |
| Frontend | Vite 8 + React 19 + TypeScript 5.9, npm | plain CSS, react-router 7; dev proxy `/api` → 127.0.0.1:8080 |
| Env | Nix flake (flake-parts + rust-overlay) | devShell: rust stable + wasm32 targets, nodejs, wasm-tools, sqlite, just, pi |

### Key WIT gotchas (learned the hard way)
- `book` is a reserved word in WIT — records are named `book-entry`.
- A world that uses types from an imported interface must `use types.{...}`
  explicitly, otherwise parsing fails with "name `x` does not exist".
- wasmtime 48: sync host functions return `()` (no `Result`); world bindings
  are `BookshelfPlugin` itself; `add_to_linker::<_, HasSelf<_>>`.

## Repository Layout

```
flake.nix                          # toolchain: rust + wasm targets, node, wasm-tools
Cargo.toml                         # workspace root (shared deps)
crates/bookshelf-core/             # domain models + BookSource trait (plugin seam)
crates/bookshelf-formats/          # epub / txt parsing → ParsedBook
crates/bookshelf-plugin/           # wasmtime host + WIT interface
crates/bookshelf-server/           # axum app, migrations/, routes/, service/
plugins/hello-plugin/              # example wasm plugin (guest)
frontend/                          # React app (npm)
docs/plugins.md                    # plugin authoring guide
justfile                           # dev / check / test / plugin-build / web-build
scripts/build-plugin-hello.sh      # build example plugin → plugins-built/
```

## Current Progress

### Done (verified)
- [x] Flake: rust stable (1.98) + `wasm32-unknown-unknown`/`wasip1`/`wasip2`
      targets, nodejs 24, wasm-tools; `nix develop` works; dirs live in `data/`
      (gitignored).
- [x] Workspace + 4 crates compile with zero warnings; `cargo test --workspace`
      green (6 format-parsing unit tests).
- [x] Unified storage: `books` (metadata) + `book_files` (uploads / virtual
      plugin files) + `chapters`/`sessions`/`shares`/`users`; plugin catalogs
      upserted on startup (metadata reused across syncs), chapters
      materialized lazily into `chapters` on first read.
- [x] Auth: register/login/me (JWT + Argon2), admin/user roles, `local` admin
      user seeded when auth is disabled.
- [x] Permissions: every uploaded file is private by default; uploader picks
      `public`/`private` per file at upload time and can toggle it later
      (owner/admin). Public files are visible/shared by all logged-in users;
      ownerless plugin files are public, manageable by admins only.
- [x] One metadata entry ↔ many files: upload creates metadata + first file;
      `POST /api/books/{id}/files` attaches more editions to existing
      metadata; metadata edit/delete by its creator or admin.
- [x] Books/Files API: list/search (books + visible files), multipart upload
      (new book or attach to existing), file detail + chapter titles,
      chapter content with lazy plugin materialization, visibility toggle,
      patch/delete (owner/admin).
- [x] Sessions API: list/create/update/delete; per-(user, book, label) unique;
      position = `{chapter_idx, offset, fraction}` clamped to chapter count.
- [x] Shares API: book shares (anonymous read of catalog + chapters),
      session shares (progress snapshot with percent & owner), expiry support,
      delete by creator/admin.
- [x] Plugin system: v2 (design `docs/plugin-v2-design.md` implemented):
      WIT `bookshelf:plugin@0.2.0` (world `bookshelf-plugin`); per-instance
      registry (`plugin_instances`), stable/auto instance ids, one wasm file
      backs many instances; `config-schema`/`configure` config channel with
      structured validation (400 `{errors:[{field,message}]}`);
      capabilities-gated lazy catalog (`declare` eager sync / `search` +
      `lookup` on demand / `identify` upload recognition / `content`
      chapters); metadata↔content separation via
      `book_files.content_source/content_external_id` + rebind endpoint;
      epoch-deadline traps (200 ms pump, ±2 ticks) + host caps (search
      limit/offset, declare catalog, 2 MiB chapter); demos
      hello/wiki/reader plugins; host tests green (config roundtrip,
      capability stubs, timeout trap, introspection stability).
- [x] Frontend: plugin admin page (instances, enable/delete/resync,
      schema-rendered config form), library “源浏览器” (search→materialize),
      upload manual-mode picker via search, book-detail content-source
      display + “更换内容源” rebind dialog; OpenAPI contract + generated
      types updated.
- [x] Metadata management: one upload endpoint with attach-to-existing /
      auto (plugin `identify-upload` identification with parsed fallback) /
      manual (field overrides, plugin catalog picker) modes; distinct
      metadata vs file deletion (409 while files remain); per-book
      `POST /api/books/{id}/refresh` re-pulls metadata from plugin sources;
      `GET /api/plugins/{id}/catalog` for picking plugin metadata.
- [x] Frontend: library (search + upload), book detail (session management,
      share link creation, TOC), reader (chapter nav, debounced scroll-fraction
      progress save, session switcher, new session), public share viewer
      (session snapshot banner + read-only reader), login/register, local-mode
      banner. `tsc` + `vite build` green.
- [x] Server serves `frontend/dist` at `/` with SPA fallback when present.
- [x] Series + volumes: `series` metadata group; every book can carry
      `series_id` + `volume_no` (standalone = 0). Plugin sources may
      declare multi-volume structure (`book-entry.volumes`, WIT v0.5.0)
      and acquisition auto-splits into **one series + one book (and
      virtual file) per 卷** instead of merging the series into one
      book — per-volume chapter counts, flat-offset lazy pulls
      (`book_files.volume_offset`), per-volume sessions/shares. Manual
      series management (create / assign / reorder / unassign / delete)
      via `POST|GET|PATCH|DELETE /api/series`, `PUT
      /api/series/{id}/members`, extended `PATCH /api/books/{id}`;
      `POST /api/books` returns `AcquireResult {series, books}`.
      Frontend: library groups cards by series, `/series/:id` page with
      manage UI, book-detail series chip + 加入系列 dialog, volume
      badges, split-aware add-book toast; e2e (mock r-9 two-卷 book) +
      live wenku8 3617 verified (series of 2 volumes, real vcss volume
      titles, 8+7 chapters).
- [x] API contract: `docs/api/openapi.yaml` covers the whole HTTP API;
      frontend generates typed schemas from it (`npm run api-types` →
      `src/api/schema.d.ts`) instead of hand-mirroring backend types.
- [x] Frontend i18n: i18next + react-i18next wired in; all UI strings
      extracted to `src/i18n/locales/zh-CN.json`; only zh-CN shipped.
- [x] End-to-end smoke test (curl): register → plugin book visible →
      chapter content → session create/update → book share (anonymous) →
      session share → txt upload → all HTTP 200/201.
- [x] Git repo initialized, initial commits made.

### Known limitations / next steps
- [ ] Server-side frontend prod build packaging (nix package of server+web is
      only a stub in flake).
- [ ] No cover image storage (parsed but not stored yet).
- [ ] Plugin resources: no wasi (network/fs) host imports exposed yet;
      the controlled `http.fetch` import (v3) is the only network path
      (docs/plugin-http-api-design.md open questions: per-instance QPS
      token bucket, HTTPS proxy support, `chapter.format` for
      page-scraping sources, guest-side caching are all still open).
- [ ] No rate limiting, no session/TTL cleanup job, no share listing endpoint.
- [ ] Frontend has no tests; backend unit tests only in `bookshelf-formats`.
- [ ] Reader saves only `fraction` (not char `offset`, which is reserved for
      non-web readers).

## Progress Log

| Date | Entry |
|---|---|
| 2026-08-21 | Initial project setup: requirements, tech choices, workspace, backend (core/formats/plugin/server), example wasm plugin, React frontend, migrations, docs; all compile/test/e2e verified. |
| 2026-08-21 | Rework #2 per owner feedback: (a) git history cleaned (`.direnv` removed, now gitignored); (b) all docs/comments converted to English (UI strings stay zh-CN); (c) permission model reworked: per-file public/private chosen by uploader, metadata (`books`) decoupled from files (`book_files`) so one metadata entry holds multiple files; chapters/sessions/shares now attach to files; API moved to `/api/files/*`, `POST /api/books/{id}/files` for extra editions; frontend updated (file-based library cards, visibility toggle, attach-upload, reader per file). E2E re-verified including visibility semantics (403 on hidden files for other users). |
| 2026-08-21 | New working agreements recorded (req. 11–13) in AGENTS.md: one feature = one commit; API contract via `docs/api/openapi.yaml` (frontend types generated with openapi-typescript); frontend i18n via i18next (zh-CN only for now). |
| 2026-08-21 | feat(api): OpenAPI 3.1 contract (`docs/api/openapi.yaml`, 22 endpoints, full schemas/security) as single source of truth; frontend generates `src/api/schema.d.ts` via `npm run api-types`; `src/types.ts` is now a thin alias layer over generated types. |
| 2026-08-21 | feat(web): i18n via i18next + react-i18next; all UI strings extracted to `src/i18n/locales/zh-CN.json` (84 keys, no hardcoded strings remain, verified no missing keys); adding a locale = one JSON file. |
| 2026-08-21 | feat(toc): hierarchical table of contents end-to-end. Parser keeps the nav/NCX tree (EPUB3 nav nested `ol>li`, EPUB2 NCX nested navPoints rebuilt despite html5ever's HTML-mode `<content/>` hoisting via nearest-navPoint-ancestor logic); image-only pages (插图 plates) are no longer chapters; `book_files.toc` JSON column (migration 0003) stores the tree (`idx` references chapters, `null` = group); `FileDetail.toc` + `ShareBookResponse.toc` in the API contract (flat synthesis fallback for plugin books/legacy uploads); reader replaces the flat `<select>` with a collapsible tree TOC panel (recursive `TocTree`, groups expandable, current chapter highlighted), wired in ReaderPage + SharePage. Verified e2e with a generated nested-TOC EPUB (卷→话 tree, illustration page dropped). |
| 2026-08-21 | feat(epub): parse per the EPUB standard. Reading order = OPF spine (auxiliary `linear="no"` items and front matter without TOC entries/headings no longer become chapters); chapter titles come from the EPUB3 nav doc or EPUB2 NCX (fallback: first heading, then numbered); chapter content is stored as sanitized XHTML (whitelist-built via html5ever/scraper, images inlined as data URIs, scripts/styles/on* attributes stripped, cross-doc links dropped) instead of html2text plain text; `chapters.format` column (`html`/`text`, migration 0002) + `Chapter.format` in the API contract; frontend renders HTML chapters via `dangerouslySetInnerHTML` + dedicated `.epub-content` styles (ruby/table/image/blockquote, …) and keeps pre-wrap text rendering for txt/plugin chapters; `html2text` dependency removed. Verified e2e with a real Chinese EPUB (27 spine docs → 24 chapters; title page / blank / book-TOC pages dropped; TOC titles correct) and txt/plugin chapters still `text`. |
| 2026-08-21 | feat(metadata): upload-time metadata management. `POST /api/books` is now one endpoint for all three modes: attach (`book_id` → existing metadata, dedup), auto (plugins asked to identify via new WIT `identify-upload(filename, sha256)` — first match becomes the metadata, keeping parsed fallback) and manual (`title`/`authors` JSON/`description`/`cover_url` overrides; a plugin catalog picker `GET /api/plugins/{id}/catalog` lets users source metadata from a plugin and stay linked to it). Plugin books are created via a shared `ensure_plugin_book` (metadata + virtual public file); a user claiming an upload as a plugin book becomes the metadata's creator (`COALESCE`), so it is manageable/refreshable. Metadata and file deletion are now distinct: `DELETE /api/books/{id}` returns 409 + file ids while files remain (no silent cascade; files deleted individually via `DELETE /api/files/{id}`). `POST /api/books/{id}/refresh` re-pulls metadata + chapter-title placeholders from the plugin source(s) (creator/admin; never overwrites materialized chapter content). Frontend: upload dialog with the three modes + plugin picker, per-book refresh + delete-metadata (with delete-all-files-then-metadata flow on 409). OpenAPI updated (upload form fields, 409 schema, refresh + catalog endpoints, `PluginCatalogEntry`); verified e2e: attach→plugin book dedup, hello-named upload auto-identified, manual fields, plugin-ref override + refresh restores plugin title, catalog book_id, 400 on no-plugin refresh, 409→file delete→metadata delete. |
| 2026-08-21 | requirement 14 recorded: run `cargo fmt --all` before every commit (tree must stay rustfmt-clean; formatting lands in its own `chore:` commit). Workspace made rustfmt-clean as a baseline (`chore: rustfmt-clean the workspace`, formatting only). |
| 2026-08-21 | fix(toc) follow-up: the real-world epub (汪晖《去政治化的政治》) still lost the 编 level on re-upload. Root cause: part divider pages are auxiliary (`linear="no"`) or entirely absent from the spine, and `remap_branches` spliced away every branch whose document was not kept as a chapter — so 编 groups lost their node and their essays lifted to the top (DB showed 13 top-level chapters, zero groups). Now any branch without a chapter of its own (no target, or its doc not kept) becomes a pure structure group (`idx: null`) when it has surviving children; only childless junk (cover/插图 leaves) disappears. Regression test `part_pages_become_group_nodes` (dividers linear=no + one absent from spine + image-only leaf). 13 unit tests green. Note: previously uploaded books keep their stored (flat) toc — re-upload needed. |
| 2026-08-21 | feat(toc): calibre-style fragment entries (`file.html#section`) now survive the remap instead of being dropped as same-document duplicates. Real-world epub (汪晖《去政治化的政治》): 61 of 84 navPoints are fragment entries nested up to 4 levels; the whole tree (chapter → section → subsection, 3 levels) now round-trips, each section carrying `TocNode.frag` so the reader scrolls to the element id after loading the chapter (ids survive sanitization; same-chapter clicks scroll directly). Duplicate drop is now an exact-target check (ancestor vs descendant), so sibling sections stay. Backend (`remap_branches` keyed by full href, `frag` in model + API contract, flat fallbacks emit `frag: null`), reader (`pendingFragRef` + `scrollToFragId`, `goto(idx, frag)`), openapi + generated types updated; unit test `keeps_fragment_sections` (EPUB2+EPUB3); verified against the real epub file (84 nodes / 61 frags / 24 chapters). 14 unit tests green. |
| 2026-08-21 | fix(web): file manage actions (visibility toggle / share / delete) used to render for every local file on the book detail page, including other users' public uploads — clicking them failed with 403. UI now mirrors the backend's owner-or-admin rule (`canManage = admin || f.owner_id === user.id`); other users' uploads show a read-only hint (他人上传 · 仅可阅读, new i18n key). |
| 2026-08-21 | docs: plugin system v2 design (`docs/plugin-v2-design.md`) — requirements from owner feedback: declarative/stateless plugins (R1/R2, statelessness already enforced via fresh Store per call), flexible strongly-typed config (R3, missing → `config-schema`/`configure` design), large sources with search/get-book pagination + lazy metadata materialization (R4, missing today — v1 sync scans `list-books` fully), metadata/content source separation (R5, missing → `content_source`/`content_external_id` columns + `book-entry.content-source` pointer). WIT rewritten as `bookshelf:plugin@0.2.0` (dev phase, no compat): world keeps required exports only (wasmtime 48's wit-parser lacks `optional-export`) with `capabilities()` declaration + stubs; per-call `configure` keeps statelessness; epoch-interruption timeouts + caps for runaway plugins; phased plan P0–P4 with two demo plugins (wiki metadata-only + reader content) as e2e proof of R5. |
| 2026-08-21 | feat(toc): calibre-style fragment entries (`file.html#section`) now survive the remap instead of being dropped as same-document duplicates. Real-world epub (汪晖《去政治化的政治》): 61 of 84 navPoints are fragment entries nested up to 4 levels; the whole tree (chapter → section → subsection, 3 levels) now round-trips, each section carrying `TocNode.frag` so the reader scrolls to the element id after loading the chapter (ids survive sanitization; same-chapter clicks scroll directly). Duplicate drop is now an exact-target check (ancestor vs descendant), so sibling sections stay. Backend (`remap_branches` keyed by full href, `frag` in model + API contract, flat fallbacks emit `frag: null`), reader (`pendingFragRef` + `scrollToFragId`, `goto(idx, frag)`), openapi + generated types updated; unit test `keeps_fragment_sections` (EPUB2+EPUB3); verified against the real epub file (84 nodes / 61 frags / 24 chapters). 14 unit tests green. |
| 2026-08-22 | plugin v2 implemented (`docs/plugin-v2-design.md`, P0–P4): WIT rewritten (config channel, capabilities, declare/search/lookup/identify/content), host rework (per-instance config injection, epoch-deadline guard with 200 ms pump + 2-tick margin, search/declare/chapter caps), instance registry + admin APIs (register with stable id / enable / delete-with-409 / config-schema / PUT config + re-sync), lazy catalog (startup sync only for `declare`; `GET /api/plugins/{id}/search` + `POST /api/plugins/{id}/books` for search/lookup sources), refresh via `get-book`, metadata/content separation (`content_source/content_external_id` columns + `POST /api/files/{id}/content-source` rebind with get-book validation; bodies cleared on rebind; stale placeholders pruned), demo plugins `wiki-plugin` (metadata only, entries point `content-source: "reader"`) + `reader-plugin` (content) + data-driven `hello-plugin` rework, `scripts/build-plugins.sh`, frontend: admin plugin page + source browser + upload picker + content-source UI, i18n + OpenAPI updates. E2e verified: declare sync with injected config, structured config errors, search caps, materialize → titles/bodies from reader, rebind switches chapters, disable/409 semantics, identify upload, shares/sessions regression, guest `store::log` shows injected values. Backend unit tests: 23 green (14 formats + 8 host + 1 introspection stability); `cargo fmt --all` run before commit. |
| 2026-08-22 | docs: plugin HTTP & book-file API design (`docs/plugin-http-api-design.md`) — proposal for the acquisition layer the v2 system still lacks (plugins are pure computations: only `store::log` + `config::configure` imports, no network/fs). Adds a single controlled import `http.fetch` (request/response records, typed `fetch-error` with permanent/transient split) under host policy (allow-list hosts, resolve-time SSRF check incl. redirect hops, hard timeout/size caps, 5-hop redirects with cross-host header stripping, query-stripped logging), an optional `book-file`/`get-book-file` export so whole-book sources flow through the existing upload parser (chapters + TOC + sanitized HTML for free; file mode preferred over chapter mode), capability `"book-file"`, config-driven base-url/API-keys per instance, WIT 0.3.0, P1–P4 implementation outline and five open questions (rate limiting, proxy, http-by-default, chapter format field, caching). No code changes — WIT/host/plugins untouched. |
| 2026-08-22 | plugin v3 HTTP acquisition implemented (`docs/plugin-http-api-design.md` P1–P3): WIT 0.3.0 (`http` interface + `book-file` record + `get-book-file` export); host `http.fetch` under policy — `BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS` (empty = denied; `host`/`host:port` entries), scheme gate (`BOOKSHELF_PLUGIN_FETCH_HTTP`), SSRF at resolve time via a ureq resolver (same addresses the connection uses, no DNS-rebinding window) + re-checked per redirect hop (5 max, credential-ish headers dropped cross-origin, curl-style method downgrade), overall deadline = min(requested, `BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS`), size cap `BOOKSHELF_PLUGIN_FETCH_MAX_BYTES` (no partial data), host-managed headers never forwarded, 4xx/5xx are responses not errors, logs `scheme://host/path` only; plugin calls moved onto `spawn_blocking` (slow sources never stall a worker); demo plugins rewritten to fetch everything over HTTP (`base-url`/`api-key` config; mock source `scripts/mock-source.py`, dev servers helper, `scripts/e2e-http-plugins.sh`); file-mode acquisition: `get-book-file` → the bytes run through the existing upload parser (epub/txt format, real TOC, sanitized HTML, stored like a local upload; file mode wins over chapter mode, chapter mode is the fallback, also on first access of virtual rows; refresh pulls metadata only for file-mode rows); host fix: optional config fields without defaults validated correctly; workspace upgraded to Rust edition 2024 (chore). 20 fetch-policy unit tests (allow-list, SSRF loopback, timeout, size-limit, redirect-limit, cross/same-host header rules) + full e2e green (chapter mode → lazy bodies from the mock, refresh, file-mode epub with nested TOC + html chapters, deny-all allow list). |
| 2026-08-23 | 统一获取流程 + 插件交互抽象 (owner feedback round: 1. 上传与插件取书流程不统一 → 统一为 获取书籍→添加元数据; 2. 前端插件交互只适配 wenku8 → 抽象插件交互模型). **feat(api) 统一获取端点**: `POST /api/books` 是唯一的入库入口——内容来源与元数据来源正交: 内容 = `file`(上传) 或 `plugin_source`+`plugin_book_id`(插件书: file-mode 走 `get-book-file` 过上传解析器, 否则 chapter-mode 虚拟文件惰性拉取); 元数据 = 自动(解析/identify 或插件自身 entry) / `book_id` 关联已有 / 手动覆盖(`title`/`authors`/`description`/`cover_url`). 服务层 `UploadMetadata`/`UploadInput` 重构为 `AcquireContent`/`AcquireMetadata` + `acquire_book()`; `materialize_file_mode` 改为在已建元数据条目下落地(attach 场景); 插件内容 attach 到已有条目时, 已 materialize 到其他条目的同一插件书返回 409(一个插件书 = 一个图书馆条目). `POST /api/plugins/{id}/books` 被取代并移除(路由/服务/OpenAPI/e2e 同步更新), 设计文档更新. **feat(web) 统一添加书籍对话框**: 图书馆工具栏两个入口(上传书籍/源浏览器)合并为一个 添加书籍 三步对话框——①获取书籍(上传文件标签页 / 插件源标签页, 选择即记录不立即入库) ②添加元数据(自动/关联已有/手动) ③文件设置(可见性+备注); 手动输入 ID 下沉到源面板内(无 search 能力的源自动显示). **feat(plugin) WIT v4 `source-info`**: 插件声明交互模型(kind: search/manual-id/browse, id-kind, zh-CN id/search 提示), host 加载时取一次并经 `GET /api/plugins` 透出; wenku8 声明 manual-id + 数字书号提示, wiki/reader search, hello browse; 4 个 demo 插件全部重建 + fixture 刷新. **feat(web) 交互抽象**: `sourceInteraction()` 按 source-info + capabilities 解析交互形态并渲染对应面板(显式 kind 与能力不匹配时降级推导, 未来的新 kind 不会破坏 UI), 搜索/手动 ID 占位文案由插件声明提供, 移除写死的 wenku8 文案与 dead i18n keys. e2e 新增统一获取矩阵(file+auto / file+attach / plugin+attach / plugin+overrides / 409 冲突) 全绿; 后端 44 个测试 green; `cargo fmt --all` + clippy -D warnings clean; 前端 `tsc` + `vite build` green. |
| 2026-08-22 | feat(wenku8): real-world plugin for 轻小说文库 (`plugins/wenku8-plugin/`, `lookup`+`content`) + the two host/frontend gaps it uncovered. (a) **Epoch deadline now scales with the fetch budget** (`crates/bookshelf-plugin/src/host.rs` `call_deadline_ticks`: `ceil(BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS/200ms) + 2` instead of a flat 2 ticks ≈ 400 ms): the old guard interrupted every call blocked in `http.fetch` for longer than ~400 ms — fine for the localhost mock the demos were tested against, fatal for any real site (wenku8 answers in ~1 s; observed `wasm trap: interrupt` at the import-return boundary). Network waits are bounded by the fetch-timeout cap anyway, so calls may now spend up to that budget; compute-only calls keep the tight minimum. The runaway-trap test now constructs its plugin with a tight 100 ms policy so the spin loop still dies within a few pump ticks (test stays ~1 s). (b) **Source browser manual-id entry** (`frontend/src/pages/LibraryPage.tsx` + `SourceSearch`): the library 源浏览器 filter dropped the `search` requirement (now `lookup`), gained `allowManualId` (the dialog already supported it), and no-search instances no longer render/hit the search box (new i18n key `plugin.noSearch`). (c) **wenku8 plugin**: GBK decoding via `encoding_rs` (pages are GBK), hand-rolled XHTML parsing (depth-aware `#content` extraction, entity decode, per-line tidy); book page `/book/{id}.htm` → title/author/description/cover (https-ified); TOC `reader.php?aid=` → `vcss`/`ccss` rows (unlinked spacer rows skipped; same-host `Referer` sent, 403 otherwise); per-volume `插图` plates detected by title and moved to the volume front by default (like the physical book / linovelib2epub; config `illustration-placement` = front/end/skip, `第N卷` prefix disambiguates repeats in the flat plugin TOC), bodies rendered as numbered image-URL lines (plugin chapters are text only); `get-chapter` re-fetches the (small) TOC to map index→cid (statelessness); one retry on transient 403/5xx/transport; wenku8 search/login/fetch-downloads are login-walled so no `search` capability — books are materialized by numeric id. Verified e2e against the live site (`BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS=www.wenku8.net`): materialize 3617 → correct metadata (GBK), 15 chapters with `第1卷 插图`/`第2卷 插图` at volume front (16 plate URLs each), prose chapter text clean (no contentdp ad, entities decoded), refresh works; books 2455/3696/2759 parse (spacer row excluded, single/multi-volume alike); placement=end/skip reorder/drop the plates as configured. Owner feedback round 2: wenku8 prose chapters carry `（插图NNN）` marks where the print book has a plate — the plugin now resolves them inline to the matching plate of that volume's 插图 chapter (`[插图005] https://pic.777743.xyz/…`, 001 → first plate; out-of-range marks fall back to appearance order, unresolved marks stay). `get-chapter` keeps the raw TOC volumes (`fetch_toc` → `TocInfo`) so marks resolve against the right volume; `OrderedChapter.volume` carries the volume index; `collect_image_urls`/`format_image_list` split the plate extractor; 5 guest unit tests (mark parsing, halfwidth/traditional forms, replacement, fallback). Rate limiting: wenku8 answers bursts with HTTP 429 — transient 403/429/5xx/transport now retried up to three times per request (docs/plugins.md note; verified end-to-end on the dev instance: 第一章 005-008 → 208489-208492, 第二章 009-011, 第三章 012-014, 第四章 015-016 all inline; chapters without marks unchanged; plate chapters unchanged, still at volume end). Owner feedback: the default was changed to **keep the 插图 chapters at their original position in the volume (end of the volume, matching the wenku8 TOC)** — `illustration-placement` enum reordered so index 0 = `end` (default), 1 = `front`, 2 = `skip` (the 8080 temp instance was stopped; the 9092 dev instance re-materialized 3617 with plates at idx 7/14). Backend tests: 43 green (incl. the deadline-scaled runaway trap); `cargo fmt --all` + clippy -D warnings clean; frontend `tsc` + `vite build` green. |

| 2026-08-23 | **feat(web) UI 改进第二轮**（owner 反馈：配色/交互/信息展示糟糕，见 data/screenshot/）。(1) `feat(web)` 书籍详情页重构为封面 hero 布局：左侧 2:3 封面（存储字节 → `cover_url` → 首字回退，抽成共享 `components/BookCover.tsx`），右侧系列 chip + 标题 + 作者/版本 meta + 简介（超 120 字折叠可展开）+ 真实操作按钮（开始阅读/分享/从插件源更新/加入系列/删除元数据），取代原先散落的文字链接；文件版本改为独立卡片（格式徽章 + 紧凑 mini-btn 操作），删去「文件版本/分享」两段裸露说明文案（i18n 同步清理）；新增 mini-btn / chip / detail-hero / panel-box 等样式与 `ghost.danger-text` 按钮变体。(2) `fix(web)` 系列页卷卡片只试了封面端点、没回退 `cover_url` 导致全部显示首字占位符——改用共享 BookCover；头部同样 hero 化（首卷封面 + 作者 chip + 卷/章统计 + 编辑系列/添加书籍/保存顺序/删除系列真按钮）；作者 chip 在 flex column 里被拉伸成整行（`.chip { align-self: flex-start }`）。(3) `style(web)` 视觉层次：body 顶部光晕渐变（dark/light 各自 token）、导航毛玻璃半透明、搜索框胶囊化、图书馆去掉常驻的「添加书籍 = …」解释行（admin 提示居中弱化）。无头 Chromium 截图验证三页（本地模式 8080 实例）；tsc + vite build green。 |
| 2026-08-23 | fix(api): `list_books()` 的 QueryBuilder 动态拼接丢了可见性过滤的右括号——非管理员用户带搜索词查书全部 500（SQL 语法错误）。不补字符串，而是把 `list_books()` 和 `files_of_book()` 两处动态 SQL 全部改为静态语句：可选 source 过滤写成 `(f.source = ? OR ? IS NULL)`，admin 绕过用绑定的布尔开关 `(? OR visibility = 'public' OR owner_id = ?)`。service 层回到全静态 SQL，消除括号/AND 拼接这类 bug。e2e 验证：非管理员搜自己的私有书、管理员绕过、source 过滤（local/不存在源）、carol 隔离与公开后可见、files_of_book 可见性；工作区 49 测试 green。 |
| 2026-08-24 | **feat(web) UI 改进第三轮**（自查截图评审后)：(1) 图书馆/系列页容器加宽到 1280px（`.content:has(.book-grid)`），卡片 minmax 168px、标题放开两行截断，管理员提示从居中悬浮行改为搜索栏旁的胶囊徽章；(2) 插件管理页对齐新视觉语言：实例改为独立卡片（file-card + mini-btn 操作），能力以「wasm 文件 · caps」副行展示（去掉「能力: 无」噪音），注册表单收进 panel-box；(3) 登录/本地模式提示居中卡片化（品牌 logo + 主按钮 CTA，新增 `a.btn-primary`），登录/注册表单顶部加 logo；系列卷卡片管理按钮统一为 mini-btn；清理死 i18n key（`plugin.noCapabilities`）。无头 Chromium 截图验证图书馆/插件/系列/登录四页；tsc + vite build green。 |
| 2026-08-23 | ui-improvement plan implemented (`docs/ui-improvement.md`, 6 commits): (1) `style(web)` design tokens — `:root` completed with the previously undefined `--panel`/`--card-bg` plus semantic palette/spacing/type scales, light-color fallbacks removed, button variants (primary/default/ghost/danger × sm/md) and `:focus-visible` rings; (2) `feat(web)` shared `Modal` component (backdrop, scroll lock, ESC/backdrop close, focus trap) — source browser + upload dialog modalized, single-dialog state so only one is ever open, upload form restructured into 元数据/文件 sections; (3) `feat(web)` reader: settings panel (font size ±, line height, serif/sans, column width, dark/sepia/light themes, localStorage-persisted), fixed top bar (目录/返回 · title · chapter · percent), TOC replaced by a left slide-in drawer with current-chapter highlight + auto-scroll, single bottom prev/next set with next-chapter preview, body typography 42em/line-height 2.0/0.8em paragraphs, no forced centered h1; (4) `feat(web)` library cards: 2:3 cover (stored bytes → `cover_url` → initial fallback), icon badges (公开/隐藏/plugin), skeleton loaders, empty state; (5) `feat(api/cover)` migration 0005 (`books.cover` BLOB + `cover_mime`), `ParsedBook.cover` from the EPUB3 `cover-image` property / EPUB2 meta, stored on creation + attach-if-missing, unauthenticated img-friendly `GET /api/books/{id}/cover`, openapi + regenerated types; (6) `feat(web)` toast system (success/error/warning, replaces scattered inline error boxes; inline stays for form validation), nav with SVG logo + 书架/插件管理 items + user dropdown, inline SVG icon set replacing ←/→/＋/📍 glyphs, `data-theme` light/dark switch on the tokens. E2e verified against a real epub: cover served as image/jpeg (75 KB), txt books 404 → placeholder, attaching an epub edition fills the missing cover of an existing entry. All workspace tests green (formats now 15 incl. new cover test); frontend tsc + vite build green. Known pre-existing issue: clippy -D warnings fails in bookshelf-plugin tests on the current toolchain (present on the base commit, untouched here). |
| 2026-08-23 | **feat(series): 系列 + 按卷拆分** (owner feedback: 没有系列概念，wenku8 多卷会合并成一卷; decisions: auto-split / series page / manual management now / keep series title on volumes). (1) `docs:` design `docs/series-design.md` (approved). (2) `feat(plugin)` WIT v0.5.0 `volume-info`/`book-entry.volumes` — flat chapter list stays the source of truth, volumes are consecutive slices; core `SourceVolume`/`SourceBook.volumes` + host conversion; guests rebuilt (`volumes: None`), wenku8 `get_book` fetches the TOC and declares 卷 (vcss titles, 第N卷 fallback, counts per the 插图 placement policy); new guest test (volume titles + per-volume counts under end/skip). (3) `feat(api)` migration 0006 (`-- no-transaction` + FK-off table rebuild of `book_files`: unique `(source, external_id, volume_no)`; `series` table; `books.series_id/volume_no`, `book_files.volume_no/volume_offset`), split acquisition (plugin chapter-mode + new/plugin metadata → series + N books/N virtual files in one tx; volume-aware 409; attach of a multi-volume book → 400), offset mapping in lazy `get_chapter`, `ensure_titles`/`refresh` slice per volume, series endpoints (list/create/detail/patch/delete/members), `PATCH /api/books/{id}` series fields (assign/move/unassign, auto next-free volume, unique volume_no), OpenAPI (`SeriesBrief`/`SeriesDetail`/`AcquireResult`, Book/FileMeta volume fields), 5 server unit tests for slice math. (4) `feat(web)` library series grouping + volume badges, `/series/:id` page with manage UI (edit / reorder ↑↓ / add-remove members / delete), book-detail 加入系列 dialog, AddBookDialog handles `AcquireResult` (新增系列 toast), i18n zh-CN, regenerated schema types; tsc + vite build green. (5) `fix(api)` volume files skip the whole-book file-mode rewrite on first chapter access (was re-merging the volumes from `get-book-file`); `AcquireResult.series` is the full `SeriesBrief`. (6) `test(e2e)` mock r-9 two-卷 book + reader-plugin volumes passthrough; e2e matrix extended: split counts/offsets/lazy pulls, 409 on re-acquire, series list/detail/labels, manual create/assign/empty members/unassign/delete. (7) `docs:` docs/plugins.md v0.5.0 volumes section + this log. Verified live: wenku8 3617 → series 《魔法使的搬运铺…》 2 卷（真实 vcss 卷标题）8+7 章, vol2 ch0 offset 8 lazy pull, per-volume sessions, refresh, library grouped. All 55 workspace tests green; `cargo fmt --all` + clippy -D warnings clean; frontend `tsc` + `vite build` green. |
| 2026-08-24 | fix(web): wenku8 插图没渲染——插件把插图存成纯文本（插图章 `N. <url>` 列表 + 正文内联 `[插图NNN] <url>` 标记），Reader 对 `format: "text"` 章节只渲染一个 pre-wrap `<p>`，URL 全部显示为文字。新增 `TextChapter` 渲染器：正则识别两种引用形式并渲染为懒加载 `<img>`（`referrerPolicy="no-referrer"`，wenku8 CDN 无 Referer 校验，直连可加载），其余文本保持原 pre-wrap 布局；无引用的纯文本章节渲染路径不变。新增 `.chapter-img` 样式。真实 wenku8 3617 端到端截图验证：插图章 16 张彩页全部成图、正文第一章 4 处内联插图渲染在对应段落位置；tsc + vite build green。 |
| 2026-08-25 | **存储格式统一**（`docs/storage-unification-design.md`，owner 拍板：原件存磁盘 / reparse 为升级时手动命令不留接口 / text 转换并入 epub 样式 / 复用上传大小限制；5 个 commit）：
**P1 feat(api) 原件保留**（migration 0007：`book_files.orig_ext/sha256/size`；上传与插件 file-mode 拉取的字节存 `data/files/{file_id}.{ext}`，chapter-mode 与旧上传为 null；`GET /api/files/{id}/download` 与章节读取同权限；删除文件时清理原件；`FileMeta.original` + 前端下载按钮；e2e：sha 往返一致、权限矩阵 private 403/public 200/匿名 401/owner 200）。**P1b feat(server) `--reparse-originals`**：一次性 CLI 迁移，用当前解析器重解析全部原件并替换 chapters+toc（单事务/文件，session chapter_idx 截断），在插件加载前运行后退出；e2e：清空的正文/TOC 从原件重建、越界 session 截断。**P2 feat(formats) 统一 HTML ingest**：新增 `bookshelf_formats::htmlize`（`text_to_html` 空行分段/转义/单换行→`<br/>`；`plugin_text_to_html` 额外展开插图约定：`[插图] 共 N 张` 头行丢弃、`N. url` 列表行与内联 `[插图NN] url` 标记→`<figure><img>`、非 http URL 与无 URL 括号文本保持原样，9 个单测）；txt 解析器直接产出 HTML 章节；插件 chapter-mode 惰性物化与 `upsert_chapters` 在 ingest 边界规范化；标题占位符存 'html'。**P3 feat(web) 单渲染路径 + 回填**：启动回填把遗留 `format='text'` 章节转为 HTML（幂等分批）；`Chapter.format` 移出 API 契约（重新生成类型）；Reader 删除 `TextChapter` 正则与 format 分支，全部走 `.epub-content`；figure/img 样式并入 epub 样式（owner 决定）；e2e：旧文本行启动即转换、真实 wenku8 3617 重新获取——插图章存为 `<figure>` 序列、内联标记在正文流中成图（截图验证）；插件插图页瞬时拉取失败会留下未解析标记，重拉即解析。**P4 refactor(db) 删列**：migration 0008 DROP `chapters.format`，所有章节 INSERT/SELECT 去掉 format，`ChapterFormat`/`ParsedChapter.format` 从模型移除；回填移入 `db::connect` 在迁移**之前**执行（新库无表、已迁库无列均容忍），保证 pre-0008 库先转换再删列；`docs/plugins.md` 把文本章节约定（列表行/内联标记/头行丢弃）记为宿主 ingest 规范化。e2e 迁移三场景：全新库（1–8 全过）、旧库含 text 行（回填→删列→API 无 format 字段）、二次启动无错误。64 个工作区测试 green；`cargo fmt` + clippy -D warnings + tsc + vite build green。 |
| 2026-08-25 | feat(api): **章节图片本地化**（owner 反馈：前端显示不出 pic.777743.xyz 的图，解析时先把图片下载）：插件章节物化时宿主把 HTML 里的远程 `<img src>` 下载到 `data/files/images/{sha256}`（migration 0009 `images` 表 url→sha/mime/size，跨章节/书籍按 URL 去重），章节 HTML 重写为新的公开端点 `GET /api/images/{id}`（与封面同模型：`<img>` 无法带 Authorization，id 为字节 sha256 不可猜测）。下载 SSRF 防护（resolve 时拒绝非公网地址）、单张 20 MiB/30s 上限、并发 4；失败保留远程 URL，下次重新物化重试。e2e（真实 wenku8 3617）：插图章首次阅读本地化全部 16 张（约 7.7s），磁盘直出 image/jpeg，二次阅读 14ms，正文内联标记经 URL 去重复用已存图片；阅读器截图确认从本地端点渲染。`docs/plugins.md` 补充图片本地化说明；66 个工作区测试 green。 |
| 2026-08-25 | refactor: **去掉请求检查，插件只保留联网能力**（owner 反馈两轮）：① `download_image` 的手搓 URL 解析 + SSRF resolve 检查删除——过度设计，URL 校验交给 ureq（手搓还弄坏了 IPv6 字面量）；审查全仓库：`epub.rs` 的 scheme 前缀判断改过一版大小写不敏感后又按 owner 指示回退（epub 图片都是容器本地的，不用管），`bookshelf-plugin/http_fetch` 本来就用 `url::Url` 正规解析，wenku8 的 `rsplit("cid=")` 是刮削本职不动。② **wasm 插件请求不再检查**：`http_fetch` 从 509 行减到 ~230 行——allow-list（`BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS`）、scheme 门（`FETCH_HTTP`）、SSRF resolver/blocked_nets/ipnet、重定向逐跳复查、跨域凭据头剥离全部删除；只保留运营上限（timeout 钳制、64 MiB 截断、≤5 重定向由 ureq 内建、日志去 query）。WIT `denied` 变体保留兼容但不再产生；`ipnet`/`url` 依赖移除；main.rs 启动警告改为打印 caps；e2e 脚本删掉 deny-all 段落和残留的 `format` 断言；`docs/plugins.md`/`docs/plugin-http-api-design.md`（状态注明策略已移除）同步。20 个策略测试缩成 7 个行为测试；完整 HTTP 插件 e2e 无任何环境变量通过，真实 wenku8 3617 零配置可拉取。52 个工作区测试 green。 |
| 2026-08-26 | **feat(server) Diesel 基建（重构方案 P1）**：workspace 引入 diesel 2.3（sqlite）+ diesel-async 0.9（sqlite/deadpool/sync-connection-wrapper）；`src/schema.rs` 入库——由 `diesel print-schema` 生成并机器验证与迁移一致（抓出两处手写遗漏：book_files 的 source/external_id 列、4 个 →users joinable），仅保留 header 注释与 `#[sql_name = "offset"]` 两处偏离；`db::connect_diesel()` 建 deadpool 池（SyncConnectionWrapper 走 spawn_blocking），custom_setup 在每个连接上跑 PRAGMA（busy_timeout/WAL/foreign_keys/**synchronous=NORMAL**，P6 项提前落地）；`Library` 增加 `diesel_db` 字段与旧 sqlx 池并存（P6 才退役）；`ApiError` 补 diesel result/pool error 映射；单测验证池查询 + WAL/FK pragma 生效；justfile 新增 `schema` 配方（迁移入临时库→print-schema），AGENTS.md 记录再生成约定。54 测试 green；启动冒烟通过。 |
| 2026-08-26 | **feat(api/plugin) 统一 image store（重构方案 P0，三个 commit）**：① `feat(api)` 内容寻址图片库——epub sanitizer 不再内联 data URI，改为抽取到 `ParsedBook.images`（按容器路径去重）+ 发 `src="image:{n}"` 占位符；新增 `ingest_parsed_book()` 在全部 6 个解析边界（上传获取×3、file-mode 物化、首访重写、reparse CLI）落库并把占位符重写为 `/api/images/{id}`；迁移 0010 重建 images 表（去 url 列）并把已本地化章节正文清回占位行（惰性重新物化）；formats 去掉 base64 依赖。② `refactor(api)` 删除宿主侧 localize_images/download_image/remote_image_urls 及 ureq 依赖——最后一个「持 SQLite 写事务跨网络 I/O」的 P0 问题随之消失。③ `feat(plugin)` WIT v0.6.0 `store.store-image` 导入（单图 20 MiB/单次调用 96 MiB 上限，fresh HostState 每调用使计数免费；超限 trap）；host 新增 `ImageStore` trait，服务端 `DbImageStore` 在阻塞插件线程上 block_on 异步 sqlx 存储；htmlize 约定扩展：列表行与内联插图标记接受 `/api/images/{id}` 引用；wenku8 插件自行下载彩页（magic-byte 嗅探 mime，同页重试策略）并以站内引用输出插图章与内联标记（失败回退远程 URL）；demo 插件重建、hello fixture 刷新。53 测试 green，clippy/fmt clean。**live 验证（wenku8 3617，隔离实例 8931）**：获取→系列+2 卷正常；第1卷插图章首次读取 16/16 彩页经 store-image 入库、正文全部为 `/api/images/{id}` 站内引用（0 远程）；`GET /api/images/{id}` 直出 image/jpeg + immutable 缓存头；磁盘 16 文件 2.2 MiB、DB 16 行去重一致；二次读取 43ms；第一章 4 处内联标记在段落原位解析为已存图片且与插图章共享 id（去重生效）。耗时注意：慢 CDN 下每图 ~7s、16 图章 ~2min，默认 30s epoch 预算会 trap——已在 docs/plugins.md 注明需调大 `BOOKSHELF_PLUGIN_FETCH_TIMEOUT_MS`。 |
| 2026-08-26 | docs: SQL 层重构方案（`docs/sql-refactor-plan.md`）：手写 sqlx 运行时查询（~106 处）迁移到 Diesel 2.3 + diesel-async 0.7（SyncConnectionWrapper + deadpool，schema.rs 提交入库 + `just schema` 再生成，迁移仍走 sqlx::migrate!）。owner 拍板：删除 localize_images（顺带消除 P0——upsert_chapters 持写事务跨网络下载阻塞全站写）、翻译期一并修掉两个 N+1（list_books 逐书查 files / list_series 逐系列 COUNT，改 IN 批量/分组聚合）、图片改为统一 image store 模型（owner 二次细化：服务端只做内容寻址存储，产出方负责取图与引用；否决了插件侧 data-URI 内联——受 2 MiB 章节上限/零去重/无缓存三重劣化）：宿主 `store_image(bytes)→sha256 id` + 规范引用 `<img src="/api/images/{id}">`；插件侧 WIT v6 新增 `store-image` 导入（插件自己 http.fetch 下载后入库），epub 上传侧 sanitizer 停止 data-URI 内联、改发 `image:{n}` 占位符 + 并行字节列表，在 ingest 边界经同一服务落库重写；迁移 0010 重建 images 表（去 url 列）并清除已本地化的旧章节（不兼容旧内容直接删，惰性重新物化）。CASE WHEN upsert 守卫改 coalesce(nullif(excluded…))（sql_function! 声明 nullif），sessions 的 strftime 显式更新改为 Rust 侧生成同格式时间戳。分 P0–P7：P0 拆三个 commit（formats/api 图片 store+epub 占位符改写 → 删 localize_images+迁移 0010 → WIT v6 store-image + wenku8 插件自下载彩页），后续 Diesel 基建 → auth → sessions/shares → plugins → library（含 N+1）→ 退役运行时 sqlx（synchronous=NORMAL、ensure_titles 补事务）→ 文档约定。已知取舍：SyncConnectionWrapper future 非取消安全（diesel-async#281）。
| 2026-08-26 | **SQL→Diesel 重构实施完成（P1–P7，7 个 commit）**：P1 `feat(server)` Diesel 基建——diesel 2.3 + diesel-async 0.9（sqlite/deadpool/sync-connection-wrapper），schema.rs 生成入库（机器 diff 验证；后续把 INTEGER/REAL 统一放宽为 BigInt/Double 以匹配行结构 i64/f64，免去边界强转）；deadpool 池 custom_setup 每连接跑 PRAGMA（busy_timeout/WAL/foreign_keys/synchronous=NORMAL）。P2 `refactor(auth)` users 查询（require_user/register 唯一冲突→409/login/seed）。P3 `refactor(api)` sessions+shares 全部翻译；session updated_at 改 Rust 侧生成同格式时间戳；LEFT JOIN 进度视图右表列用 `.nullable()`。P4 `refactor(api)` 插件实例 CRUD（rowid tiebreak 移除）。P5 `refactor(api)` library 服务 ~70 处全部翻译——`list_books` 改为 JOIN 条件组装（diesel into_boxed 替代参数开关）+ 单条 IN 批量取 files（N+1 修复）；`list_series` 分组 COUNT（N+1 修复）；CASE-WHEN upsert 守卫用 define_sql_function 声明的 NULLIF/COALESCE 表达；placeholder 标题的 DO UPDATE WHERE 守卫移到 Rust 侧过滤（Diesel 类型系统要求 WHERE 谓词为 Bool 类型）；删除已死的 backfill_text_chapters。P6 `refactor(server)` 退役运行时 sqlx——Library/PluginService/AppState 只留 DieselDb，sqlx 仅剩 migrate!（macros feature），rows.rs 去 sqlx derives。P7 文档+本日志。验证：55 workspace 测试 green、clippy/fmt clean；live e2e（wenku8 3617 系列 split 获取/vol2 offset 惰性拉取/16 彩页 image store/refresh/series 列表/txt+epub 上传含封面/搜索/可见性/409-删除流/share 匿名读）全绿，无 panic。经验教训：diesel-async 事务闭包与循环体 HRTB 不兼容→多语句获取改自动提交并注释取舍； diesel::prelude::* 不能与 diesel_async::RunQueryDsl 同时导入（execute/first 二义性），统一只导 async 版 + 具名 QueryDsl/ExpressionMethods/OptionalExtension/SelectableHelper。 |
| 2026-08-24 | docs: 统一存储格式方案（`docs/storage-unification-design.md`，待 owner 拍板）：现状是四种内容形态并存（epub→清洗 XHTML / txt→纯文本 / 插件 chapter-mode→纯文本 / file-mode→走上传解析器），原件字节全部丢弃（已两次因解析器改进被迫重新上传），前端 html/text 双渲染路径 + wenku8 插图行约定只存在于前端正则。方案：① 章节统一为清洗后 HTML，txt 段落化、插件文本约定（插图列表/内联标记）在宿主 ingest 时展开为 `<figure><img>`，删除前端 `TextChapter` 正则；② `book_files` 增加 `orig_ext/sha256/size`，原件存 `data/files/{file_id}.{ext}`（含 file-mode 拉取缓存），新增 `GET /api/files/{id}/download` + `POST /api/files/{id}/reparse`（重解析换新解析器，sessions 截断保护）；③ 迁移 0007 加列 + Rust 启动时批量回填 text→html，`chapters.format` 一个版本后由 0008 移除；④ 分 P1–P4 四个独立 commit。待决：原件磁盘 vs BLOB、reparse 手动/自动、text 章节样式、大小上限。 |

## Conventions

- **Environment**: `nix develop` (direnv: `use flake` already set up). For
  one-off tools use `nix run` (per requirement 8).
- **Language deps**: cargo / npm only — never commit `node_modules` or
  `target`; `frontend/dist` is gitignored (built artifact).
- **DB**: SQLite at `data/bookshelf.db`; runtime queries only (no compile-time
  `query!` macros → no `DATABASE_URL`).
- **ApiError** implements `std::error::Error`; new domain errors should plug
  into the `ApiError::*` variants in `crates/bookshelf-server/src/error.rs`.
- **Auth disabled mode**: `seed_local_user` inserts the `local` admin row so
  FKs on sessions/shares keep working — don't remove.
- **Checks before commit**: `just fmt` (cargo fmt --all), then `just check`
  (cargo check + clippy -D warnings),
  `just test`, `cd frontend && npm run typecheck` (use
  `npm_config_cache=/tmp/npm-cache` if `~/.npm` has root-owned files),
  then **update this file's Progress Log**.
- **Diesel schema** (`crates/bookshelf-server/src/schema.rs`): regenerate
  after EVERY new migration with `just schema`. Requires the diesel CLI
  (`cargo install diesel_cli --no-default-features --features sqlite`; add
  it to the flake when convenient). The file must stay byte-identical to
  `diesel print-schema` output (modulo the header comment and the
  `#[sql_name = "offset"]` attribute).