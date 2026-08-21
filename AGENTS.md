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
- [x] Plugin system: `hello-plugin` example builds to a valid component
      (25 KB), loads at startup, catalog sync works, lazy chapter
      materialization works; `GET /api/plugins`, admin `POST /api/plugins/sync`.
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
      wasip2 + `wasmtime-wasi` path documented but not implemented.
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
| 2026-08-21 | fix(toc): hierarchical TOC only showed one level — three bugs, all in group handling. (a) `remap_branches` spliced pure group branches (no target path) away instead of keeping them, flattening 卷/话 span-group trees to leaves (doc comment said the opposite of the code); (b) `collect_nav_branches` picked the first descendant `a[href]`, so an href-less group `<li>` stole its first child entry's link/title; (c) missing group links also concatenated all descendant titles into the group title (`li.text()`), and the NCX twin of (b): `collect_ncx_branches` picked the first descendant `<content>` (html5ever hoists child navPoints into the previous `<content/>`), giving group navPoints their child's target. Fixed: groups are kept as structure nodes when they have surviving children; an entry's own link/content is the first one before the nested list / nested navPoint; group titles take the li's own text excluding the nested `ol`. Fixtures are now recursive (卷→话→章, href-less groups at every level) with regressions for both EPUB3 nav and EPUB2 NCX. Reader side: `TocTree` groups were rendered as `<details open={depth === 0}>` — only the top level started expanded, so deeper levels were invisible; now `open` for every group (full hierarchy visible, still collapsible). Verified: 12 unit tests green + e2e upload of a 3-level deep3.epub → API returns 卷→话→章 with clean titles. |

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