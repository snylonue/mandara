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

# Build the demo wasm plugins -> plugins-built/{hello,wiki,reader}.wasm
# Deploy: cp plugins-built/*.wasm data/plugins/
plugin-build:
    ./scripts/build-plugins.sh

# Build the frontend into frontend/dist (served by the backend)
web-build:
    cd frontend && npm install && npm run build