# Bookshelf · Light-novel reading website

Self-hosted light-novel reading website: **Rust backend + web frontend**.
Supports **epub / txt**, multiple users, unified storage of books and metadata,
per-file public/private visibility, multi-session reading progress, sharing,
and a **wasm plugin system** for user-provided book and metadata sources.

## Features

- 📖 epub (HTML → plain text) and txt (UTF-8 / UTF-16 / GB18030 detection,
  CJK/western chapter-heading split) upload and reading
- 👥 Multi-user with JWT + Argon2 auth; `admin` / `user` roles
- 🔒 **Permission model**: every uploaded file is either `private` (visible
  only to its uploader and admins) or `public` (visible to every logged-in
  user) — the uploader chooses at upload time and can toggle it later.
  The whole auth layer can additionally be switched off
  (`BOOKSHELF_AUTH_ENABLED=false` → single-user local mode).
- 📚 **One metadata entry ↔ many files**: a book record (title, authors,
  description) can hold several files — different formats, editions or
  translations. Chapters, sessions and shares attach to files, since
  different files may split chapters differently.
- 🗄️ Unified storage: all metadata, files and chapter content live in a
  single SQLite database (`data/bookshelf.db`); backup = copy one file.
- 🧩 wasm plugin system: `data/plugins/*.wasm` are loaded at startup as
  WebAssembly components (wasmtime + component model). Plugins implement the
  `bookshelf:plugin` world to offer their own catalogs; chapters are
  materialized into the central store on first read.
- 📑 Multi-session progress: several reading sessions per file (phone /
  tablet / computer...), each with independent progress
  (chapter + scroll fraction).
- 🔗 Sharing: file share links (anonymous read without login) and session
  share links (follow another session's live progress).

## Tech stack

| Layer | Tech |
|---|---|
| Backend | Rust 2021, axum 0.8, sqlx 0.9 (SQLite, runtime queries only), jsonwebtoken, argon2 |
| Plugin host | wasmtime 48 (component model), WIT in `crates/bookshelf-plugin/wit/` |
| Frontend | Vite 8 + React 19 + TypeScript (npm), plain CSS, react-router 7 |
| Environment | Nix (flake-parts + rust-overlay); one-off tools via `nix run` |
| Language | UI strings and book examples are Chinese; code comments and docs are English |

## Repository layout

```
├── flake.nix                    # dev env: rust + wasm targets, node, wasm-tools
├── Cargo.toml                   # cargo workspace
├── crates/
│   ├── bookshelf-core/          # domain models + BookSource trait (plugin seam)
│   ├── bookshelf-formats/       # epub / txt parsing
│   ├── bookshelf-plugin/        # wasmtime component host + WIT interface
│   └── bookshelf-server/        # axum app: routes, auth, library, migrations/
├── plugins/
│   └── hello-plugin/            # example plugin (wasm component)
├── frontend/                    # React app (npm)
├── docs/plugins.md              # plugin authoring guide
└── justfile                     # common commands
```

## Quick start

```sh
# 1. Enter the dev environment (rust + node + wasm-tools; first run downloads
#    the toolchain)
nix develop

# 2. (optional) build and deploy the example wasm plugin
just plugin-build                 # produces plugins-built/hello.wasm
cp plugins-built/hello.wasm data/plugins/

# 3. start the backend (http://127.0.0.1:8080, data in data/)
just dev

# 4. in another terminal, start the frontend (http://localhost:5173,
#    /api proxied to the backend)
just dev-web
```

Single-binary deployment: build the frontend and let the backend serve it.

```sh
cd frontend && npm install && npm run build   # produces frontend/dist
cargo run -p bookshelf-server                 # serves frontend/dist (SPA) at /
```

> If `npm install` fails with EACCES because `~/.npm` contains root-owned
> files, use `npm_config_cache=/tmp/npm-cache`.

## Configuration (env vars / CLI flags)

| Variable | Default | Meaning |
|---|---|---|
| `BOOKSHELF_ADDR` | `127.0.0.1:8080` | listen address |
| `BOOKSHELF_DB` | `data/bookshelf.db` | SQLite database (unified storage) |
| `BOOKSHELF_DATA_DIR` | db's directory | runtime data directory |
| `BOOKSHELF_PLUGINS_DIR` | `data/plugins` | directory scanned for `*.wasm` plugins |
| `BOOKSHELF_JWT_SECRET` | `dev-only-change-me` | JWT secret (change in production) |
| `BOOKSHELF_AUTH_ENABLED` | `true` | `false` disables auth/permissions (single-user mode) |
| `BOOKSHELF_ALLOW_REGISTER` | `true` | allow new user registration |
| `BOOKSHELF_MAX_UPLOAD_MB` | `64` | max upload size |
| `BOOKSHELF_FRONTEND_DIR` | `frontend` | frontend dir (its `dist/` is served at `/` if present) |

Copy `.env.example` to `.env` to override defaults.

## Data model & progress/share semantics

```
users  ─┬─< books      (metadata: title, authors, ...)
        │
        └─< book_files (one file per format/edition; visibility per file)
               │
               ├─< chapters  (file_id, idx, title, content)
               ├─< sessions  (user_id, file_id, label, position)
               └─< shares    (token, kind=book|session, file_id, session_id)
```

- **Position** = chapter index + char offset within the chapter + scroll
  fraction (0..1). The API accepts all three and clamps them to the file's
  chapter count.
- **Sessions**: multiple named sessions per (user, file); unique constraint
  `(user_id, file_id, label)` — reusing a label resumes that session.
- **Shares**: `book` kind → anonymous read-only link to a file; `session`
  kind → follow a session's live progress (percent + chapter + updated_at).
- **Permissions**: file `private` → owner + admins only; `public` → all
  logged-in users. Ownerless files (plugin catalogs) are public and
  shareable by anyone, but only admins can manage them. Session/share
  deletion follows the usual owner-or-admin rule.

## API overview

```
POST /api/auth/register|login            -> {token, user}
GET  /api/auth/me
GET  /api/health                         -> {auth_enabled, allow_register, ...}

GET  /api/books?q=&source=               -> [{book, files:[...visible files]}]
POST /api/books                          <- multipart: file + visibility + label
                                           (new metadata + first file)
GET  /api/books/{id}                     -> {book, files}
PATCH/DELETE /api/books/{id}             (metadata creator/admin)
POST /api/books/{id}/files               <- multipart: attach another file
                                           (different format/edition)

GET  /api/files/{id}                     -> {file, book, chapters}
PATCH /api/files/{id}                    -> {visibility?, label?} (owner/admin)
DELETE /api/files/{id}                   (owner/admin)
GET  /api/files/{id}/chapters/{idx}      -> chapter content (plugins lazily
                                           materialize on first read)
GET/POST /api/files/{id}/sessions        -> my sessions / create {label}
PUT/DELETE /api/sessions/{id}            -> update position / delete

POST /api/files/{id}/shares              -> {kind:"book"|"session", session_id?,
                                           expires_days?}
GET  /api/shares/{token}                 -> share info (session snapshot)
GET  /api/shares/{token}/book            -> anonymous read: book + file +
                                           chapter titles
GET  /api/shares/{token}/chapters/{idx}  -> anonymous read: chapter
DELETE /api/shares/{token}               (creator/admin)

GET  /api/plugins                        -> loaded plugins
POST /api/plugins/sync                   (admin) re-sync plugin catalogs
```

## wasm plugins

Write a component implementing the `bookshelf:plugin` world and drop it into
`data/plugins/`. Full guide: **[docs/plugins.md](docs/plugins.md)**;
in-repo example: `plugins/hello-plugin`.

```sh
just plugin-build          # cargo build --target wasm32-unknown-unknown
                           # + wasm-tools component new
```

## Development commands

```sh
just dev          # start the backend
just dev-web      # start the frontend dev server
just check        # cargo check + clippy
just test         # cargo test --workspace
just plugin-build # build the example plugin component
```

## License

MIT OR Apache-2.0