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
  The first registered account becomes the `admin`.
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
| Environment | Nix flake (flake-parts + rust-overlay): dev shell, packages, NixOS module |
| Language | UI strings and book examples are Chinese; code comments and docs are English |

## Repository layout

```
├── flake.nix                    # dev shell, packages, overlay, NixOS module
├── nix/
│   ├── packages.nix             # server / frontend / wasm plugin builds
│   ├── module.nix               # services.bookshelf (systemd unit, hardening)
│   └── tests/bookshelf-vm.nix   # end-to-end NixOS VM test of the module
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

## Deployment on NixOS

The flake provides the packages, an overlay and a NixOS module:

| Output | What it is |
|---|---|
| `packages.<system>.bookshelf` | server binary + built frontend (the default) |
| `packages.<system>.bookshelf-server` | server binary only |
| `packages.<system>.bookshelf-frontend` | the Vite build output |
| `packages.<system>.bookshelf-plugins` | the in-repo wasm components |
| `overlays.default` | the same as `pkgs.bookshelf*` |
| `nixosModules.default` | `services.bookshelf` (also applies the overlay) |

```nix
# flake.nix of the host
inputs.bookshelf = {
  url = "github:example/bookshelf";
  inputs.nixpkgs.follows = "nixpkgs";
};

# ... in the nixosSystem modules:
modules = [ inputs.bookshelf.nixosModules.default ./configuration.nix ];
```

```nix
# configuration.nix
services.bookshelf = {
  enable = true;
  address = "127.0.0.1";  # TLS terminates in the reverse proxy
  cookieSecure = true;    # session cookie only over https
  # openssl rand -base64 32 > /run/secrets/bookshelf-jwt-secret
  jwtSecretFile = "/run/secrets/bookshelf-jwt-secret";
  plugins = [ pkgs.bookshelf-plugins ];  # or drop *.wasm into the state dir
};

services.nginx.virtualHosts."books.example.com" = {
  forceSSL = true;
  enableACME = true;
  locations."/".proxyPass = "http://127.0.0.1:8080";
};
```

What the module does:

- runs the server under a dedicated unprivileged system user and a
  sandboxed unit (read-only `/`, empty capability set, seccomp filter with
  the `memfd_create` wasmtime's JIT needs),
- keeps every piece of mutable state in `/var/lib/bookshelf` (SQLite
  database, retained originals, image store, and the plugin drop-in
  directory when `plugins` is empty), so a backup is a copy of that one
  directory,
- passes the JWT secret through systemd `LoadCredential=`, so the secret
  file only has to be readable by root,
- ships `systemctl start bookshelf-reparse-originals`: a one-shot task that
  re-runs the current parser over every retained original (use it after an
  upgrade that improves chapter splitting).

Useful options: `port`, `openFirewall`, `allowRegister` (defaults to true,
because the **first registered account becomes the admin** — turn it off
once that account exists), `maxUploadMb`, `frontendDir`, `environment`,
`environmentFile`, `extraArgs`; every option is documented.

`nix build .#checks.x86_64-linux.bookshelf-vm` boots the module in a VM and
checks the frontend, plugin loading, registration and the upload/read path.

## Configuration (env vars / CLI flags)

| Variable | Default | Meaning |
|---|---|---|
| `BOOKSHELF_ADDR` | `127.0.0.1:8080` | listen address |
| `BOOKSHELF_DB` | `data/bookshelf.db` | SQLite database (unified storage) |
| `BOOKSHELF_DATA_DIR` | db's directory | runtime data directory |
| `BOOKSHELF_PLUGINS_DIR` | `data/plugins` | directory scanned for `*.wasm` plugins |
| `BOOKSHELF_JWT_SECRET` | `dev-only-change-me` | JWT secret (change in production) |
| `BOOKSHELF_JWT_SECRET_FILE` | — | file holding the JWT secret (wins over `BOOKSHELF_JWT_SECRET`) |
| `BOOKSHELF_ALLOW_REGISTER` | `true` | allow new user registration |
| `BOOKSHELF_COOKIE_SECURE` | `false` | mark the session cookie `Secure` (TLS deployments) |
| `BOOKSHELF_MAX_UPLOAD_MB` | `64` | max upload size |
| `BOOKSHELF_FRONTEND_DIR` | `frontend` | frontend dir (its `dist/` is served at `/` if present) |
| `BOOKSHELF_REPARSE_ORIGINALS` | `false` | re-parse every retained original, then exit (upgrade task) |

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
GET  /api/health                         -> {allow_register, ...}

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
just fmt          # cargo fmt --all
just fmt-nix      # format the nix files (nixfmt-tree, same as `nix fmt`)
```

`nix run .#` starts the packaged server (binary + built frontend) directly;
`nix build .#bookshelf-plugins` builds the wasm components.

## License

MIT OR Apache-2.0