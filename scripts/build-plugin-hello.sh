#!/usr/bin/env bash
# Build the hello-plugin example into a loadable wasm component.
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build -p hello-plugin --release --target wasm32-unknown-unknown

mkdir -p plugins-built
wasm-tools component new \
    target/wasm32-unknown-unknown/release/hello_plugin.wasm \
    -o plugins-built/hello.wasm
wasm-tools validate --features component-model plugins-built/hello.wasm

echo ""
echo "built: plugins-built/hello.wasm"
echo "deploy: cp plugins-built/hello.wasm data/plugins/"