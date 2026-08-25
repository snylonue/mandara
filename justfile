set shell := ["bash", "-uc"]

# Start the backend dev server (default http://127.0.0.1:8080, data in data/)
dev:
    cargo run -p bookshelf-server

# Start the frontend dev server (http://localhost:5173, /api proxied to the backend)
dev-web:
    cd frontend && npm run dev

# Check + test
check:
    cargo check --workspace
    # --all-targets so test/example/bench code is linted too
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

fmt:
    cargo fmt --all

# Regenerate crates/bookshelf-server/src/schema.rs from the SQL migrations
# (run after EVERY new migration; the file is the Diesel compile-time schema).
schema:
	#!/usr/bin/env bash
	set -euo pipefail
	db=$(mktemp -d)/schema.db
	for f in crates/bookshelf-server/migrations/*.sql; do sqlite3 "$db" < "$f"; done
	cd crates/bookshelf-server && DATABASE_URL="$db" diesel print-schema > src/schema.rs 2>/dev/null \
		|| echo "diesel CLI not found — update src/schema.rs by hand (see the header note)"
	echo "schema.rs regenerated"

# Build the demo wasm plugins -> plugins-built/{hello,wiki,reader}.wasm
# Deploy: cp plugins-built/*.wasm data/plugins/
plugin-build:
    ./scripts/build-plugins.sh

# Build the frontend into frontend/dist (served by the backend)
web-build:
    cd frontend && npm install && npm run build