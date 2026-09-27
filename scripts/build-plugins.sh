#!/usr/bin/env bash
# Build the plugins into loadable wasm components.
#
# Usage: ./scripts/build-plugins.sh [name ...]
#   builds the named plugins (default: every plugins/*-plugin crate) into
#   plugins-built/, validates the components, and copies hello.wasm into the
#   plugin crate's test fixture directory (the host tests embed it).
#
# The default MUST cover every plugin crate: a wasm left over from an older
# WIT version fails the host's export type-check and silently loses all its
# capabilities ("could not introspect plugin").
set -euo pipefail

cd "$(dirname "$0")/.."

NAMES=("$@")
if [ "${#NAMES[@]}" -eq 0 ]; then
    NAMES=()
    for dir in plugins/*-plugin; do
        NAMES+=("$(basename "$dir" -plugin)")
    done
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
mkdir -p crates/mandara-plugin/tests/fixtures
cp plugins-built/hello.wasm crates/mandara-plugin/tests/fixtures/hello.wasm

echo ""
echo "deploy: cp plugins-built/*.wasm data/plugins/"