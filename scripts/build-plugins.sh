#!/usr/bin/env bash
# Build the demo plugins into loadable wasm components.
#
# Usage: ./scripts/build-plugins.sh [name ...]
#   builds the named plugins (default: hello wiki reader) into plugins-built/,
#   validates the components, and copies hello.wasm into the plugin crate's
#   test fixture directory (the host tests embed it).
set -euo pipefail

cd "$(dirname "$0")/.."

NAMES=("$@")
if [ "${#NAMES[@]}" -eq 0 ]; then
    NAMES=(hello wiki reader)
fi

mkdir -p plugins-built

for name in "${NAMES[@]}"; do
    cargo build -p "${name}-plugin" --release --target wasm32-unknown-unknown
    wasm-tools component new \
        "target/wasm32-unknown-unknown/release/${name}_plugin.wasm" \
        -o "plugins-built/${name}.wasm"
    wasm-tools validate --features component-model "plugins-built/${name}.wasm"
    echo "built: plugins-built/${name}.wasm"
done

# The host unit tests embed the hello component as a fixture; keep it in
# sync whenever the WIT or the hello guest changes.
mkdir -p crates/bookshelf-plugin/tests/fixtures
cp plugins-built/hello.wasm crates/bookshelf-plugin/tests/fixtures/hello.wasm

echo ""
echo "deploy: cp plugins-built/*.wasm data/plugins/"