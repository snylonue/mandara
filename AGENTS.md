# AGENTS.md

Guidance for AI coding assistants (and humans) working in this repository.

## Project

**Bookshelf** — a self-hosted light-novel reading website with a Rust backend
and a web frontend. Supports epub/txt books, multiple users, unified storage of
books and metadata, optional permission control, reading progress
management/sharing, and a wasm plugin system for user-provided book and
metadata sources.

---

## Requirements (from the project owner)

1. Light-novel reading website; Rust backend + web frontend.
2. Support **epub** and **txt** text formats.
3. Multi-user backend with **unified storage** for books and metadata.
4. **Permissions per uploaded book**: the uploader chooses to publish or
   hide each uploaded book (per-file `public`/`private` visibility). A single
   metadata entry can be linked to **multiple book files** (formats,
   editions, translations).
5. **wasm plugin system** for user-defined book and metadata sources.
6. Frontend: reading, **progress management**, and **sharing**.
7. Progress management must support **multiple sessions** (e.g. several
   devices reading the same book simultaneously, each with its own progress).
8. Nix-managed development environment:
   - Temporary packages are used via `nix run`.
   - Project dependencies are declared in `flake.nix`.
   - Language dependencies use their native package managers
     (cargo for Rust, npm for JS, uv for Python — none used yet).
9. Record progress in commit messages, not in AGENTS.md or any TODO/progress file: write a <= 72-char imperative summary plus few lines of body covering only what changed and the minimal context needed to understand why. Omit next steps, reasoning, per-file listings, and any signatures or tool attribution; to recover prior context, read git log.
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
| Backend | Rust 2024, cargo workspace | 4 crates, see layout below |
| Web framework | axum 0.8 | `{param}` path syntax, multipart upload |
| DB | SQLite via diesel | runtime queries only so no `DATABASE_URL` is needed at compile time (nix-friendly). Migrations in `crates/bookshelf-server/migrations/`. Two-level model: `books` = pure metadata, `book_files` = actual files (uploads or virtual plugin books); chapters/sessions/shares attach to files |
| Auth | JWT (jsonwebtoken 11, `rust_crypto` backend) + Argon2 | roles `admin`/`user`; the first registered account becomes admin |
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
- [ ] Plugin resources: no wasi (network/fs) host imports exposed yet;
      the controlled `http.fetch` import (v3) is the only network path
      (docs/plugin-http-api-design.md open questions: per-instance QPS
      token bucket, HTTPS proxy support, `chapter.format` for
      page-scraping sources, guest-side caching are all still open).
- [ ] No rate limiting, no session/TTL cleanup job, no share listing endpoint.
- [ ] Frontend has no tests; backend unit tests only in `bookshelf-formats`.
- [ ] Reader saves only `fraction` (not char `offset`, which is reserved for
      non-web readers).

## Conventions

- **Environment**: `nix develop` (direnv: `use flake` already set up). For
  one-off tools use `nix run` (per requirement 8).
- **Language deps**: cargo / npm only — never commit `node_modules` or
  `target`; `frontend/dist` is gitignored (built artifact).
- **DB**: SQLite at `data/bookshelf.db`; runtime queries only (no compile-time
  `query!` macros → no `DATABASE_URL`).
- **ApiError** implements `std::error::Error`; new domain errors should plug
  into the `ApiError::*` variants in `crates/bookshelf-server/src/error.rs`.
- **Admin bootstrap**: the first registered account gets the `admin` role
  (there is no other way to create an admin).
- **Checks before commit**: `just fmt` (cargo fmt --all), then `just check`
  (cargo check + clippy -D warnings),
  `just test`, `cd frontend && npm run typecheck` (use
  `npm_config_cache=/tmp/npm-cache` if `~/.npm` has root-owned files),
  then commit with a detailed message (see requirement 9).
- **Diesel schema** (`crates/bookshelf-server/src/schema.rs`): regenerate
  after EVERY new migration with `just schema`. The diesel CLI ships in
  the dev shell (`flake.nix`, sqlite-only via `pkgs.diesel-cli.override`);
  run `nix develop`, then `just schema`, then re-apply the documented
  deviations (header comment, `Integer`→`BigInt` widening, the
  `#[sql_name = "offset"]` attribute). The file must stay byte-identical
  to `diesel print-schema` output (modulo the header comment and the
  `#[sql_name = "offset"]` attribute).