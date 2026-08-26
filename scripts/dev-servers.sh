#!/usr/bin/env bash
# Dev helper: start/stop the bookshelf server + mock source for e2e runs.
# Usage: scripts/dev-servers.sh start|stop|status   [server-args...]
set -euo pipefail
cd "$(dirname "$0")/.."

case "${1:-}" in
start)
    shift
    for pid in $(pgrep -f 'target/debug/bookshelf-server' || true); do kill "$pid" 2>/dev/null || true; done
    for pid in $(pgrep -f 'mock-source.py' || true); do kill "$pid" 2>/dev/null || true; done
    sleep 0.5
    python3 scripts/mock-source.py 8765 > /tmp/mock-source.log 2>&1 &
    BOOKSHELF_DB="${BOOKSHELF_DB:-/tmp/e2e.db}" \
    BOOKSHELF_PLUGINS_DIR="${BOOKSHELF_PLUGINS_DIR:-$PWD/plugins-built}" \
    nohup target/debug/bookshelf-server > /tmp/bookshelf-server.log 2>&1 &
    echo "started mock source + bookshelf server (pid $!)"
    for i in $(seq 1 20); do
        if grep -q 'listening on' /tmp/bookshelf-server.log 2>/dev/null; then break; fi
        sleep 0.5
    done
    grep -E 'listening on|fetch policy' /tmp/bookshelf-server.log || true
    ;;
stop)
    for pid in $(pgrep -f 'target/debug/bookshelf-server' || true); do kill "$pid" 2>/dev/null || true; done
    for pid in $(pgrep -f 'mock-source.py' || true); do kill "$pid" 2>/dev/null || true; done
    echo "stopped"
    ;;
status)
    pgrep -af 'target/debug/bookshelf-server' || echo "server not running"
    pgrep -af 'mock-source.py' || echo "mock not running"
    ;;
*) echo "usage: $0 start|stop|status"; exit 1 ;;
esac