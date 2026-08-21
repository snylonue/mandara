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
4. **Optional permission control** (can be switched off for single-user use).
5. **wasm plugin system** for user-defined book and metadata sources.
6. Frontend: reading, **progress management**, and **sharing**.
7. Progress management must support **multiple sessions** (e.g. several
   devices reading the same book simultaneously, each with its own progress).
8. Nix-managed development environment:
   - Temporary packages are used via `nix run`.
   - Project dependencies are declared in `flake.nix`.
   - Language dependencies use their native package managers
     (cargo for Rust, npm for JS, uv for Python — none used yet).
9. Write progress updates to this file after each work session
   ("每次的进度都要同步过去") — keep the [Progress Log](#progress-log) current.

## Tech Stack & Decisions

| Layer | Choice | Notes |
|---|---|---|
| Backend | Rust 2021, cargo workspace | 4 crates, see layout below |
| Web framework | axum 0.8 | `{param}` path syntax, multipart upload |
| DB | SQLite via sqlx 0.9 | runtime queries only (`sqlx::query*`, no `query!` macros) so no `DATABASE_URL` is needed at compile time (nix-friendly). Migrations in `crates/bookshelf-server/migrations/` |
| Auth | JWT (jsonwebtoken 11, `rust_crypto` backend) + Argon2 | roles `admin`/`user`; optional via `BOOKSHELF_AUTH_ENABLED` |
| Plugin host | wasmtime 48 (component model) | WIT world `bookshelf:plugin/bookshelf-plugin` in `crates/bookshelf-plugin/wit/`; plugins loaded from `data/plugins/*.wasm`; chapters materialized into the central DB on first read |
| Plugin guest | wit-bindgen 0.60 (`generate!` + `export!`), `wasm32-unknown-unknown` | module is lifted with `wasm-tools component new` (no adapter needed, world imports no wasi) |
| Formats | `epub` (epub-rs) + `html2text`/`scraper`; custom txt parser | txt: UTF-8/UTF-16/GB18030 detection + chapter-heading split (第X章 / Chapter N / VOL.N / 序章 etc.) |
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
- [x] Unified storage: `books`/`chapters`/`sessions`/`shares`/`users` tables;
      plugin catalogs upserted into `books` on startup (source = plugin id),
      chapters materialized lazily into `chapters` on first read.
- [x] Auth: register/login/me (JWT + Argon2), admin/user roles, `local` admin
      user seeded when auth is disabled; books `private`/`public` visibility.
- [x] Books API: list/search, multipart upload of epub/txt, detail + chapter
      titles, chapter content, patch/delete (owner/admin).
- [x] Sessions API: list/create/update/delete; per-(user, book, label) unique;
      position = `{chapter_idx, offset, fraction}` clamped to chapter count.
- [x] Shares API: book shares (anonymous read of catalog + chapters),
      session shares (progress snapshot with percent & owner), expiry support,
      delete by creator/admin.
- [x] Plugin system: `hello-plugin` example builds to a valid component
      (25 KB), loads at startup, catalog sync works, lazy chapter
      materialization works; `GET /api/plugins`, admin `POST /api/plugins/sync`.
- [x] Frontend: library (search + upload), book detail (session management,
      share link creation, TOC), reader (chapter nav, debounced scroll-fraction
      progress save, session switcher, new session), public share viewer
      (session snapshot banner + read-only reader), login/register, local-mode
      banner. `tsc` + `vite build` green.
- [x] Server serves `frontend/dist` at `/` with SPA fallback when present.
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
| 2026-08-21 | Initial project setup: requirements, tech choices, workspace, backend (core/formats/plugin/server), example wasm plugin, React frontend, migrations, docs; all compile/test/e2e verified; repo committed. |

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
- **Checks before commit**: `just check` (cargo check + clippy -D warnings),
  `just test`, `cd frontend && npm run typecheck` (use
  `npm_config_cache=/tmp/npm-cache` if `~/.npm` has root-owned files),
  then **update this file's Progress Log**.