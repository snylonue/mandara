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
#   4. refresh pulls metadata only (materialized bodies untouched).
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
    [ -f /tmp/e2e-http-server.pid ] && kill "$(cat /tmp/e2e-http-server.pid)" 2>/dev/null || true
    [ -f /tmp/e2e-http-mock.pid ] && kill "$(cat /tmp/e2e-http-mock.pid)" 2>/dev/null || true
    remove_pid_stale() {
        # kill leftover mock/servers still bound to our e2e database
        for pid in $(pgrep -f 'mock-source.py' || true); do kill "$pid" 2>/dev/null || true; done
        for pid in $(pgrep -f 'bookshelf-server' || true); do
            if tr '\0' ' ' < /proc/$pid/environ 2>/dev/null | grep -q "BOOKSHELF_DB=.*e2e-http"; then
                kill "$pid" 2>/dev/null || true
            fi
        done
    }
    remove_pid_stale
    rm -f "$DB" /tmp/e2e-http-server.pid /tmp/e2e-http-mock.pid
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
    stop_server
    rm -f "$DB"
    BOOKSHELF_AUTH_ENABLED=false \
    BOOKSHELF_DB="$DB" \
    BOOKSHELF_PLUGINS_DIR="$PWD/plugins-built" \
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

# Post multipart fields to the unified acquisition endpoint
# (POST /api/books): api_form <path> <field=value> [<field=value> ...]
api_form() { # api_form <path> fields...
    local path="$1"; shift
    local args=()
    for f in "$@"; do args+=(-F "$f"); done
    curl -sf -X POST "$BASE$path" "${args[@]}"
}

# Acquire a plugin book through the unified endpoint (public, like the
# old one-click materialize).
acquire() { # acquire <plugin> <book_id>
    api_form /api/books "plugin_source=$1" "plugin_book_id=$2" "visibility=public"
}

jq_field() { # jq-ish helper: python -c reading stdin
    python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"
}

echo "== e2e: plugin HTTP acquisition (docs/plugin-http-api-design.md) =="
[ "$BUILD" = 1 ] && ./scripts/build-plugins.sh > /dev/null
cargo build -p bookshelf-server > /dev/null 2>&1 || fail "server build"

start_mock
start_server

# --- register sources -----------------------------------------------------
api POST /api/plugins/instances '{"id":"wiki","wasm_file":"wiki.wasm"}' > /dev/null
# reader: raise the chapter cap so the multi-volume demo book (r-9, 4
# chapters) is served whole — the default max-chapters=3 would cut 卷 2.
api POST /api/plugins/instances '{"id":"reader","wasm_file":"reader.wasm","config":{"max-chapters":10}}' > /dev/null
pass "registered wiki + reader instances"

# --- chapter mode: browse + materialize through the HTTP source ----------
TOTAL=$(api GET "/api/plugins/wiki/search?limit=3" | jq_field "d['total']")
[ "$TOTAL" = "8" ] || fail "wiki search total=$TOTAL (expected 8)"
pass "wiki search: $TOTAL books served over HTTP"

BOOK=$(acquire wiki w-1 | jq_field "d['books'][0]['book']['id']")
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
COUNT2=$(acquire reader r-1 | jq_field "d['books'][0]['files'][0]['chapter_count']")
[ "$COUNT2" = "3" ] || fail "r-1 chapter_count=$COUNT2"
pass "materialized r-1 directly via reader"

# refresh pulls metadata only; bodies stay materialized
api POST "/api/books/$BOOK/refresh" | grep -q "星海拾遗" || fail "refresh lost metadata"
api GET "/api/files/$FILE/chapters/0" | grep -q "mock-source.py" || fail "refresh wiped a materialized body"
pass "refresh: metadata from source, bodies untouched"

# --- file mode: get-book-file -> epub through the upload parser ----------
BFILE=$(acquire reader r-2 | jq_field "d['books'][0]['files'][0]['id']")
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
api GET "/api/files/$BFILE/chapters/0" | grep -q "<p>" || fail "sanitized HTML missing"
pass "file-mode r-2 materialized through the upload parser (epub, nested TOC, html chapters)"

# --- unified acquisition matrix (docs/api/openapi.yaml POST /api/books) -
# Every combination of content source (file | plugin) and metadata mode
# (auto | attach | manual overrides) goes through the one endpoint.
printf '第一章 测试章节\n正文内容一行\n' > /tmp/e2e-upload.txt

# file + auto: a plain upload parses its own metadata
UP=$(api_form /api/books "file=@/tmp/e2e-upload.txt" "visibility=public")
UPBOOK=$(echo "$UP" | jq_field "d['books'][0]['book']['id']")
UPC=$(echo "$UP" | jq_field "d['books'][0]['files'][0]['chapter_count']")
[ "$UPC" -ge 1 ] 2>/dev/null || fail "file+auto chapter_count=$UPC"
pass "file+auto: uploaded txt parsed into a new metadata entry"

# file + attach: a second file joins the same metadata entry
UP2=$(api_form /api/books "file=@/tmp/e2e-upload.txt" "book_id=$UPBOOK" "visibility=public")
[ "$(echo "$UP2" | jq_field "d['books'][0]['book']['id']")" = "$UPBOOK" ] || fail "file+attach book mismatch"
pass "file+attach: second file attached under the same metadata"

# plugin + attach: plugin content joins an existing metadata entry
UP3=$(api_form /api/books "plugin_source=wiki" "plugin_book_id=w-2" "book_id=$UPBOOK" "visibility=public")
[ "$(echo "$UP3" | jq_field "d['books'][0]['book']['id']")" = "$UPBOOK" ] || fail "plugin+attach book mismatch"
W3C=$(echo "$UP3" | jq_field "d['books'][0]['files'][0]['chapter_count']")
[ "$W3C" = "3" ] || fail "plugin+attach chapter_count=$W3C (expected 3: reader r-2 content)"
pass "plugin+attach: wiki w-2 content attached under existing metadata"

# plugin content + manual overrides: the plugin entry with a user title
OV=$(api_form /api/books "plugin_source=wiki" "plugin_book_id=w-3" "title=自定义书题" "visibility=public")
echo "$OV" | grep -q "自定义书题" || fail "plugin+overrides title lost"
pass "plugin+overrides: plugin entry overridden with a manual title"

# one plugin book = one library entry: attaching it under another entry
# must conflict
UP4=$(api_form /api/books "file=@/tmp/e2e-upload.txt" "visibility=public")
UPBOOK4=$(echo "$UP4" | jq_field "d['books'][0]['book']['id']")
if api_form /api/books "plugin_source=wiki" "plugin_book_id=w-2" "book_id=$UPBOOK4" "visibility=public" > /dev/null 2>&1; then
    fail "re-attaching an already materialized plugin book should 409"
fi
pass "plugin+attach conflict: w-2 already materialized under another entry -> 409"

# --- multi-volume split + series (docs/series-design.md) ----------------
# reader r-9 declares 2 volumes (2 + 2 chapters): acquisition must create
# one series + one book per 卷, each with its own file, and lazy chapter
# pulls must shift by the volume's flat offset.
SPLIT=$(acquire reader r-9)
SERIES_ID=$(echo "$SPLIT" | jq_field "d['series']['id']")
[ -n "$SERIES_ID" ] || fail "multi-volume acquisition produced no series"
NVOL=$(echo "$SPLIT" | jq_field "len(d['books'])")
[ "$NVOL" = "2" ] || fail "split volume count=$NVOL (expected 2)"
V1=$(echo "$SPLIT" | jq_field "d['books'][0]['book']['volume_no']")
V2=$(echo "$SPLIT" | jq_field "d['books'][1]['book']['volume_no']")
[ "$V1" = "1" ] && [ "$V2" = "2" ] || fail "volume numbers $V1/$V2 (expected 1/2)"
SV1=$(echo "$SPLIT" | jq_field "len(d['books'][0]['files'])")
[ "$SV1" = "1" ] || fail "each volume must own exactly one file"
C1=$(echo "$SPLIT" | jq_field "d['books'][0]['files'][0]['chapter_count']")
C2=$(echo "$SPLIT" | jq_field "d['books'][1]['files'][0]['chapter_count']")
[ "$C1" = "2" ] && [ "$C2" = "2" ] || fail "per-volume chapter counts $C1/$C2 (expected 2/2)"
F2=$(echo "$SPLIT" | jq_field "d['books'][1]['files'][0]['id']")
OFF2=$(echo "$SPLIT" | jq_field "d['books'][1]['files'][0]['volume_offset']")
[ "$OFF2" = "2" ] || fail "volume 2 flat offset=$OFF2 (expected 2)"
# offset mapping: volume 2's chapter 0 is the source's flat chapter 3
api GET "/api/files/$F2/chapters/0" | grep -q "第三章 觉醒" || fail "vol2 ch0 should be the source's 第三章 觉醒"
api GET "/api/files/$F2/chapters/1" | grep -q "第四章 归航" || fail "vol2 ch1 should be the source's 第四章 归航"
pass "split: series + 2 books, per-volume files with correct offsets and lazy chapters"
# one plugin book = one series: re-acquiring conflicts
if acquire reader r-9 > /dev/null 2>&1; then
    fail "re-acquiring an already split book should 409"
fi
pass "split conflict: re-acquire r-9 -> 409"
# series endpoints: list, detail (volume titles), volume file labels
api GET /api/series | grep -q "$SERIES_ID" || fail "created series not listed in /api/series"
api GET "/api/series/$SERIES_ID" | grep -q "第一卷 相遇" || fail "series detail misses the volume-title label"
api GET "/api/files/$F2" | grep -q "第二卷 觉醒" || fail "volume 2 file label should carry its volume title"
pass "series list/detail + volume labels"
# manual series management (uploads): create, assign with a volume number,
# reorder via PUT members, unassign, delete
MS=$(api POST /api/series '{"title":"测试系列","authors":["某作者"]}')
MS_ID=$(echo "$MS" | jq_field "d['id']")
api PATCH "/api/books/$UPBOOK" "{\"series_id\":\"$MS_ID\",\"volume_no\":1}" > /dev/null || fail "manual series assignment"
api GET "/api/series/$MS_ID" | grep -q "$UPBOOK" || fail "assigned book not a member of the series"
api PUT "/api/series/$MS_ID/members" '{"book_ids":[]}' > /dev/null || fail "clear members via PUT"
api PATCH "/api/books/$UPBOOK" '{"series_id":null}' > /dev/null || fail "unassign via PATCH null"
api DELETE "/api/series/$MS_ID" > /dev/null || fail "delete series"
pass "manual series: create / assign / empty members / unassign / delete"

rm -f /tmp/e2e-upload.txt

echo ""
echo "ALL E2E CHECKS PASSED"