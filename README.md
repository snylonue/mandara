# Mandara · Light-novel reading website

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
  single SQLite database (`data/mandara.db`); backup = copy one file.
- 🧩 wasm plugin system: `data/plugins/*.wasm` are loaded at startup as
  WebAssembly components (wasmtime + component model). Plugins implement the
  `mandara:plugin` world to offer their own catalogs; chapters are
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
| Plugin host | wasmtime 48 (component model), WIT in `crates/mandara-plugin/wit/` |
| Frontend | Vite 8 + React 19 + TypeScript (npm), plain CSS, react-router 7 |
| Environment | Nix flake (flake-parts + rust-overlay): dev shell, packages, modular service |
| Language | UI strings and book examples are Chinese; code comments and docs are English |

## Repository layout

```
├── flake.nix                    # dev shell, packages, overlay, modular service
├── nix/
│   ├── packages.nix             # server / frontend / wasm plugin builds
│   ├── module.nix               # package.services.default (modular service, hardening)
│   └── tests/mandara-vm.nix   # end-to-end NixOS VM test of the module
├── Cargo.toml                   # cargo workspace
├── crates/
│   ├── mandara-core/          # domain models + BookSource trait (plugin seam)
│   ├── mandara-formats/       # epub / txt parsing
│   ├── mandara-plugin/        # wasmtime component host + WIT interface
│   └── mandara-server/        # axum app: routes, auth, library, migrations/
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
cargo run -p mandara-server                 # serves frontend/dist (SPA) at /
```

> If `npm install` fails with EACCES because `~/.npm` contains root-owned
> files, use `npm_config_cache=/tmp/npm-cache`.

## Deployment on NixOS

The flake provides packages and an overlay. The combined package exports a
[Modular Service](https://nixos.org/manual/nixos/stable/#modular-services)
as `services.default`, for NixOS 26.05 or newer:

| Output | What it is |
|---|---|
| `packages.<system>.mandara` | server binary + built frontend (the default), with `services.default` |
| `packages.<system>.mandara-server` | server binary only |
| `packages.<system>.mandara-frontend` | the Vite build output |
| `packages.<system>.mandara-plugin-<name>` | one wasm component per in-repo plugin |
| `packages.<system>.mandara-plugins` | all in-repo wasm components in one directory |
| `overlays.default` | the same as `pkgs.mandara*`, plus the `pkgs.mandaraPackages` scope |

```nix
# flake.nix of the host
inputs.mandara = {
  url = "github:example/mandara";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

```nix
# A host module receiving inputs through specialArgs
{ inputs, pkgs, ... }:
let
  packages = inputs.mandara.packages.${pkgs.stdenv.hostPlatform.system};
in
{
  system.services.mandara = {
    imports = [ packages.mandara.services.default ];
    mandara = {
      address = "127.0.0.1";  # TLS terminates in the reverse proxy
      cookieSecure = true;
      # openssl rand -base64 32 > /run/secrets/mandara-jwt-secret
      jwtSecretFile = "/run/secrets/mandara-jwt-secret";
      plugins = [
        packages.mandara-plugin-hello
        packages.mandara-plugin-wenku8
      ];
      # ...or drop *.wasm files into /var/lib/mandara/plugins
    };
  };

  services.nginx.virtualHosts."books.example.com" = {
    forceSSL = true;
    enableACME = true;
    locations."/".proxyPass = "http://127.0.0.1:8080";
  };
}
```

With `nixpkgs.overlays = [ inputs.mandara.overlays.default ]`, import
`pkgs.mandara.services.default` instead. The module defaults `mandara.package`
to the package exporting it, so importing an overridden package's service
also uses that package. The server, frontend and plugins are independent
derivations composed by `lib.makeScope`:

```nix
system.services.mandara.imports = [
  (pkgs.mandaraPackages.overrideScope (final: prev: {
    frontend = prev.frontend.override { src = /path/to/frontend; };
  })).mandara.services.default
];
```

Individual packages can also be overridden with
`pkgs.mandara.override { frontend = …; }` or
`pkgs.mandara-server.override { src = …; }`. Plugins are selected separately
through `system.services.<name>.mandara.plugins`.

The instance name determines the unit name, default state directory and
service user. For example, a second instance named `library` can import the
same module and set `mandara.port = 8081`; it uses `library.service`,
`/var/lib/library` and `library-reparse-originals.service`. Set a distinct
`mandara.stateDirectory` explicitly when nesting instances under
`system.services.<name>.services` to avoid sharing state with another
instance of the same local name.

The service:

- runs under an unprivileged systemd `DynamicUser` with a sandboxed unit
  (read-only `/`, empty capability set, and a seccomp filter including the
  `memfd_create` needed by wasmtime's JIT),
- keeps mutable state in `/var/lib/<instance>`: the SQLite database, retained
  originals, images and plugin drop-ins. Stop the service before copying
  this directory, or use `sqlite3 … ".backup"` for the WAL-mode database,
- passes the JWT secret through systemd `LoadCredential=`, so the source
  secret file only needs to be readable by root,
- provides `systemctl start <instance>-reparse-originals` to re-run the
  current parser over retained originals after an upgrade.

Service-specific options live under `system.services.<name>.mandara`:
`port`, `allowRegister`, `maxUploadMb`, `frontendDir`, `environment`,
`environmentFile` and `extraArgs`. Registration defaults to true because
**the first registered account becomes the admin**; disable it once that
account exists. Compose additional service modules with `imports`, extend
`process.argv`, or customize `systemd.services.""` and
`systemd.services.reparse-originals` inside the instance.

To migrate from the old NixOS module, remove the `nixosModules.default`
import and replace `services.mandara = { enable = true; …; }` with the
instance configuration above. The default `mandara` instance retains
`/var/lib/mandara`; systemd manages its ownership with `DynamicUser`.
For a custom persistent user/group, create those accounts in the host
configuration, set `mandara.user`/`mandara.group`, and disable `DynamicUser`
on both units. Replace `openFirewall = true` with the host setting
`networking.firewall.allowedTCPPorts = [ 8080 ];` when exposing the server
directly. A reverse proxy deployment only needs its public HTTP(S) ports.

`nix build .#checks.x86_64-linux.mandara-vm` boots the service in a VM and
checks multiple instances, state isolation, re-parsing, the frontend,
plugin loading, registration and the upload/read path.

## Configuration (env vars / CLI flags)

| Variable | Default | Meaning |
|---|---|---|
| `MANDARA_ADDR` | `127.0.0.1:8080` | listen address |
| `MANDARA_DB` | `data/mandara.db` | SQLite database (unified storage) |
| `MANDARA_DATA_DIR` | db's directory | runtime data directory |
| `MANDARA_PLUGINS_DIR` | `data/plugins` | directory scanned for `*.wasm` plugins |
| `MANDARA_JWT_SECRET` | `dev-only-change-me` | JWT secret (change in production) |
| `MANDARA_JWT_SECRET_FILE` | — | file holding the JWT secret (wins over `MANDARA_JWT_SECRET`) |
| `MANDARA_ALLOW_REGISTER` | `true` | allow new user registration |
| `MANDARA_COOKIE_SECURE` | `false` | mark the session cookie `Secure` (TLS deployments) |
| `MANDARA_MAX_UPLOAD_MB` | `64` | max upload size |
| `MANDARA_FRONTEND_DIR` | `frontend` | frontend dir (its `dist/` is served at `/` if present) |
| `MANDARA_REPARSE_ORIGINALS` | `false` | re-parse every retained original, then exit (upgrade task) |

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

Write a component implementing the `mandara:plugin` world and drop it into
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
`nix build .#mandara-plugin-hello` builds one wasm component and
`nix build .#mandara-plugins` builds them all.

## License

MIT OR Apache-2.0
