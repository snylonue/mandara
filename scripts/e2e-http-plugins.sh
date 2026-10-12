#!/usr/bin/env bash
# Isolated HTTP acquisition regression runner; only its own child processes are stopped.
set -euo pipefail
cd "$(dirname "$0")/.."
exec python3 scripts/e2e-http-plugins.py "$@"
