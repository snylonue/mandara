#!/usr/bin/env bash
# End-to-end check of the plugin HTTP acquisition layer
# (docs/plugin-http-api-design.md, P2 + P3): the demo plugins now fetch
# everything from scripts/mock-source.py through the host's http.fetch
# import, under the host fetch policy.
#
# Flow:
#   1. register wiki + reader instances (HTTP sources);
#   2. browse / materialize books through them (chapter mode),
#      assert chapters arrive lazily from the mock source;
#   3. materialize a book in FILE mode (get-book-file -> epub download ->
#      upload parser): real TOC + sanitized html chapters like an upload;
#   4. refresh pulls metadata only (materialized bodies untouched);
#   5. with an EMPTY fetch allow list every plugin fetch is denied and
#      the sources degrade to empty results (server logs the denials).
#
# Usage: ./scripts/e2e-http-plugins.sh [--build] [--keep]
set -euo pipefail
cd "$(dirname "$0")/.."

BUILD=0
KEEP=0
for arg in "$@"; do
    case "$arg" in
    --build) BUILD=1 ;;
    --keep) KEEP=1 ;;
    *) echo "unknown arg: $arg" >&2; exit 1 ;;
    esac
done

PORT=8765
BASE=http://127.0.0.1:8080
DB=data/e2e-http.db
SERVER_LOG=/tmp/e2e-http-server.log
MOCK_LOG=/tmp/e2e-http-mock.log

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok: $*"; }

cleanup() {
    [ "$KEEP" = 1 ] && return
    for pid in $(pgrep -f 'e2e-http' || true); do kill "$pid" 2>/dev/null || true; done
    for pid in $(pgrep -f 'mock-source.py' || true); do kill "$pid" 2>/dev/null || true; done
    rm -f "$DB"
}
trap cleanup EXIT

stop_server() {
    for pid in $(pgrep -f 'bookshelf-server' || true); do
        # only ours (the one using the e2e database)
        if tr '\0' ' ' < /proc/$pid/environ 2>/dev/null | grep -q "BOOKSHELF_DB=.*e2e-http"; then
            kill "$pid" 2>/dev/null || true
        fi
    done
    sleep 0.5
}

start_mock() {
    python3 scripts/mock-source.py "$PORT" > "$MOCK_LOG" 2>&1 &
    echo $! > /tmp/e2e-http-mock.pid
    sleep 0.8
}

start_server() {
    local allowed_hosts="$1"
    stop_server
    rm -f "$DB"
    BOOKSHELF_AUTH_ENABLED=false \
    BOOKSHELF_DB="$DB" \
    BOOKSHELF_PLUGINS_DIR="$PWD/plugins-built" \
    BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS="$allowed_hosts" \
    nohup target/debug/bookshelf-server > "$SERVER_LOG" 2>&1 &
    echo $! > /tmp/e2e-http-server.pid
    for i in $(seq 1 30); do
        grep -q 'listening on' "$SERVER_LOG" 2>/dev/null && return 0
        sleep 0.5
    done
    fail "server did not start; log: $(tail -5 "$SERVER_LOG")"
}

api() { # api <method> <path> [json body] -> prints response body
    local method="$1" path="$2" body="${3:-}"
    if [ -n "$body" ]; then
        curl -sf -X "$method" "$BASE$path" -H 'content-type: application/json' -d "$body"
    else
        curl -sf -X "$method" "$BASE$path"
    fi
}

jq_field() { # jq-ish helper: python -c reading stdin
    python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"
}

echo "== e2e: plugin HTTP acquisition (docs/plugin-http-api-design.md) =="
[ "$BUILD" = 1 ] && ./scripts/build-plugins.sh > /dev/null
cargo build -p bookshelf-server > /dev/null 2>&1 || fail "server build"

start_mock
start_server "127.0.0.1:${PORT}"

# --- register sources -----------------------------------------------------
api POST /api/plugins/instances '{"id":"wiki","wasm_file":"wiki.wasm"}' > /dev/null
api POST /api/plugins/instances '{"id":"reader","wasm_file":"reader.wasm"}' > /dev/null
pass "registered wiki + reader instances"

# --- chapter mode: browse + materialize through the HTTP source ----------
TOTAL=$(api GET "/api/plugins/wiki/search?limit=3" | jq_field "d['total']")
[ "$TOTAL" = "8" ] || fail "wiki search total=$TOTAL (expected 8)"
pass "wiki search: $TOTAL books served over HTTP"

BOOK=$(api POST /api/plugins/wiki/books '{"book_id":"w-1"}' | jq_field "d['book']['id']")
FILE=$(api GET "/api/books/$BOOK" | jq_field "d['files'][0]['id']")
COUNT=$(api GET "/api/books/$BOOK" | jq_field "d['files'][0]['chapter_count']")
[ "$COUNT" = "3" ] || fail "w-1 chapter_count=$COUNT (expected 3)"
pass "materialized w-1 via wiki -> 3 chapter placeholders"

CHAPTER=$(api GET "/api/files/$FILE/chapters/0")
echo "$CHAPTER" | grep -q "mock-source.py" || fail "chapter 0 does not come from the mock source"
echo "$CHAPTER" | grep -q "第一章 回声信标" || fail "chapter 0 title wrong"
pass "chapter 0 materialized lazily from the HTTP source"

# wiki's content-source indirection: chapters come from the reader instance
CS=$(api GET "/api/books/$BOOK" | jq_field "d['files'][0]['content_source']")
[ "$CS" = "reader" ] || fail "content_source=$CS (expected reader)"
pass "content indirection wiki -> reader intact"

# independent materialize through the reader instance
COUNT2=$(api POST /api/plugins/reader/books '{"book_id":"r-1"}' | jq_field "d['files'][0]['chapter_count']")
[ "$COUNT2" = "3" ] || fail "r-1 chapter_count=$COUNT2"
pass "materialized r-1 directly via reader"

# refresh pulls metadata only; bodies stay materialized
api POST "/api/books/$BOOK/refresh" | grep -q "星海拾遗" || fail "refresh lost metadata"
api GET "/api/files/$FILE/chapters/0" | grep -q "mock-source.py" || fail "refresh wiped a materialized body"
pass "refresh: metadata from source, bodies untouched"

# --- file mode: get-book-file -> epub through the upload parser ----------
BFILE=$(api POST /api/plugins/reader/books '{"book_id":"r-2"}' | jq_field "d['files'][0]['id']")
api GET "/api/files/$BFILE" > /tmp/e2e-file.json
BFMT=$(python3 -c "import json; d=json.load(open('/tmp/e2e-file.json')); print(d['file']['format'])")
[ "$BFMT" = "epub" ] || fail "file-mode format=$BFMT (expected epub)"
BCOUNT=$(python3 -c "import json; d=json.load(open('/tmp/e2e-file.json')); print(d['file']['chapter_count'])")
[ "$BCOUNT" = "3" ] || fail "file-mode chapter_count=$BCOUNT"
TOC_DEPTH=$(python3 -c "
import json
d = json.load(open('/tmp/e2e-file.json'))
def depth(ns):
    if not ns:
        return 0
    return 1 + max((depth(n.get('children', [])) for n in ns), default=0)
print(depth(d['toc']))
")
[ "$TOC_DEPTH" = "2" ] || fail "file-mode TOC depth=$TOC_DEPTH (expected 2: 卷 -> chapters)"
HFMT=$(api GET "/api/files/$BFILE/chapters/0" | jq_field "d['format']")
[ "$HFMT" = "html" ] || fail "file-mode chapter format=$HFMT (expected html)"
api GET "/api/files/$BFILE/chapters/0" | grep -q "<p>" || fail "sanitized HTML missing"
pass "file-mode r-2 materialized through the upload parser (epub, nested TOC, html chapters)"

# --- policy: empty allow list denies every plugin fetch -------------------
start_server ""   # restart with the allow list emptied
api POST /api/plugins/instances '{"id":"wiki","wasm_file":"wiki.wasm"}' > /dev/null
TOTAL=$(api GET "/api/plugins/wiki/search?limit=3" | jq_field "d['total']")
[ "$TOTAL" = "0" ] || fail "deny-all search total=$TOTAL (expected 0)"
grep -q "denied" "$SERVER_LOG" || fail "server log does not mention the denial"
pass "empty allow list denies plugin fetches; sources degrade to empty"

echo ""
echo "ALL E2E CHECKS PASSED"