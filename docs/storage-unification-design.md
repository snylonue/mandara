# Unified book storage format design

Status: **proposal 2026-08-24** — awaiting owner decisions on §7 (storage
location for originals, migration of the `format` column, styling of
converted text chapters).

## 1. Problem

Book content reaches the library through four paths, and each one stores
a different shape:

| # | Source | Stored as | Original bytes |
|---|---|---|---|
| 1 | epub upload | per-chapter **sanitized XHTML** (`chapters.format='html'`, images inlined as data URIs) | **discarded** |
| 2 | txt upload | **plain text** (`format='text'`, one row per detected chapter) | **discarded** |
| 3 | plugin chapter-mode | **plain text** (`format='text'`), lazily materialized from `get-chapter` | n/a (plugin is the source) |
| 4 | plugin file-mode | fetched epub/txt bytes run through the upload parser → same as 1/2 | **discarded** |

On top of these four there is an **implicit fifth format**: wenku8 text
chapters carry illustration references as line conventions (`N. <url>`
lists in plate chapters, inline `[插图NN] <url>` marks in prose), which
only the frontend's `TextChapter` regex knows about.

Consequences observed in practice:

1. **Originals are unrecoverable.** Parser improvements cannot be applied
   to existing books — this already bit us twice (the toc `remap_branches`
   fixes required re-uploading the same epub). There is also no way to
   download/export what was uploaded.
2. **Two render paths + a regex hack.** The reader branches on
   `chapter.format` (`html` → `dangerouslySetInnerHTML`, `text` →
   pre-wrap `<p>`), and text chapters get a second, source-specific
   parsing layer in the browser (`TEXT_IMAGE_RE`). Every new source
   convention means another frontend change.
3. **Content conventions are out-of-band.** The `[插图NN] url` contract
   lives in the wenku8 plugin's source code and the frontend regex — not
   in the WIT world, not in the API contract, not in any doc.
4. **Layout-dependent progress.** `sessions.fraction` is scroll-fraction
   based; the html/text split produces two different layout engines, so
   the same fraction means different places in different editions.

## 2. Goals

- **G1 — one canonical chapter format.** Every chapter row stores
  sanitized HTML, regardless of where the content came from. All
  source-specific conventions are expanded **at the ingest boundary**
  (host side), never in the reader.
- **G2 — originals retained.** Uploaded epub/txt bytes (and file-mode
  plugin pulls) are persisted, enabling re-parse with improved parsers
  and a download endpoint.
- **G3 — single frontend render path.** `TextChapter` and the format
  branch disappear; the reader renders one kind of chapter.
- **G4 — in-place migration.** Existing libraries convert without
  re-uploading; sessions/shares/progress survive.

## 3. Non-goals

- Reworking cover/image blob storage (the `books.cover` BLOB and data-URI
  inlining are orthogonal; a shared image store can come later).
- Reading positions finer than scroll fraction (char `offset` stays
  reserved for non-web readers).
- Content-addressed dedup of identical files across entries.
- Changing the WIT content model: plugins keep emitting plain text
  (chapter-mode); normalization is host work (§4.3).

## 4. Design

### 4.1 Canonical chapter format: sanitized HTML

`chapters.content` becomes **always** sanitized HTML; `chapters.format`
is retired (see §6). Ingest converters, all host-side in
`bookshelf-formats` / the server service layer:

- **epub** — unchanged; already emits sanitized XHTML.
- **txt** — HTML-escape, split on blank lines, wrap each paragraph in
  `<p>`; dialogue-heavy light-novel text maps cleanly. Chapter titles
  stay in `chapters.title` (no `<h1>` inlining, matching current epub
  behavior).
- **plugin text** — the txt converter, plus expansion of the *text
  chapter conventions*, which this design promotes to a documented host
  contract (docs/plugins.md):
  - a `[插图] 共 N 张` head line is dropped (redundant metadata);
  - a line `N. <url>` (plate list) becomes `<figure><img src="…"></figure>`;
  - an inline `[插图NN] <url>` mark becomes the same `<figure>` at that
    position in the paragraph flow.
  URLs are validated (`https?` only) and escaped like any other text.
  The frontend regex (`TEXT_IMAGE_RE`) is deleted — the transformation
  happened once, at materialization, not on every render.

The reader renders every chapter through the existing `.epub-content`
path. Converted-from-text chapters get a marker class (e.g.
`data-origin="text"` on the container) so CSS can give them the
roomier paragraph rhythm readers are used to from txt (§7.3).

### 4.2 Original file retention

- **Storage: filesystem, not BLOB.** `data/files/{file_id}.{ext}` next
  to the DB, with metadata columns on `book_files`:
  `orig_ext`, `orig_sha256`, `orig_size`. Rationale: the `data/`
  directory is already the backup unit (SQLite + plugins live there);
  multi-MB epubs as BLOBs bloat the DB, VACUUMs and page cache.
  (Alternative considered and rejected: `orig BLOB` column — single-file
  backup story, but worse memory behavior and no streaming download.)
- **Uploads**: bytes are written before parsing; on parse failure the
  file is removed. The stored bytes are exactly the uploaded payload.
- **Plugin file-mode**: the bytes pulled via `get-book-file` are cached
  as the original too — the source may disappear later, and re-parse
  should not depend on re-fetching.
- **Plugin chapter-mode**: no original (the plugin *is* the source);
  columns stay NULL.
- **`GET /api/files/{id}/download`** — streams the original with its
  natural mime; authorization identical to chapter reads
  (owner/admin/public). `Content-Disposition` uses the book title.
- **`POST /api/files/{id}/reparse`** (owner/admin, requires stored
  original): re-runs the current parser over the stored bytes and
  replaces `chapters` + `book_files.toc` in one transaction. Sessions
  are kept; `chapter_idx`/`fraction` are clamped to the new chapter
  count. This is the tool for "parser got better, refresh old books"
  (and per-book, unlike the old re-upload workaround).

### 4.3 What plugins see

Nothing changes in WIT: `get-chapter` still returns text; `get-book-file`
still returns bytes. The text-chapter conventions move from "thing the
frontend happens to parse" to a documented host-side normalization rule
(docs/plugins.md §chapters). A future `chapter.format='html'` escape
hatch (already an open question in docs/plugin-http-api-design.md) can
slot in later without touching this design.

## 5. API contract changes (docs/api/openapi.yaml)

- `Chapter.format` — removed (breaking; frontend types regenerated via
  `npm run api-types`). One release note: "all chapters are html now".
- `FileMeta` — gains `original: {size, sha256} | null` so the UI can
  show a download button only when an original exists.
- New `GET /api/files/{id}/download` (binary response) and
  `POST /api/files/{id}/reparse` (returns the refreshed `FileDetail`).

## 6. Migration

Migration `0007_storage_unification.sql` + a Rust startup backfill:

1. **0007 (SQL)**: add `orig_ext/orig_sha256/orig_size` to `book_files`
   (NULL = no original retained). Existing uploads cannot be backfilled —
   their bytes are gone; the columns simply stay NULL and
   download/reparse answer 409 with a clear message ("pre-retention
   upload; re-upload to enable"). Plugin chapter-mode rows are NULL by
   design.
2. **Startup backfill (Rust, idempotent, batched)**: for every row with
   `chapters.format='text'`, run the §4.1 converter and rewrite the
   content as HTML, then set `format='html'`. Runs after migrations on
   every boot; cheap no-op when nothing matches. Kept out of SQL because
   the conversion is real code, not expressions.
3. **`format` column removal**: after the backfill has shipped in one
   release, migration `0008` drops `chapters.format`
   (SQLite 3.35+ `DROP COLUMN`) and the API field. Until then the column
   is write-once `'html'` and ignored by the reader.

Sessions/shares need no migration: they reference files, and fraction
progress tolerates the (small) layout shift of txt chapters gaining real
paragraph margins.

## 7. Open questions (owner decisions)

1. **Originals on disk vs BLOB** — §4.2 recommends `data/files/` on
   disk; BLOB keeps the "everything in one SQLite file" property if the
   owner values that more.
2. **Reparse trigger** — manual per-book button (recommended) vs
   automatic reparse-all on version upgrade (riskier: a parser regression
   would rewrite whole libraries at once).
3. **Styling of converted text chapters** — keep the txt reading feel
   (`data-origin="text"` → wider line-height, no justify) or unify
   visually with epub styling (justified paragraphs)? Recommend the
   former; the flag costs one attribute.
4. **Size cap for retained originals** — reuse the existing upload size
   limit (recommended) or a separate `BOOKSHELF_ORIG_MAX_BYTES`.

## 8. Implementation plan

- **P1 — originals (G2, independent value):** migration 0007 columns,
  write-on-upload, file-mode cache, `download` + `reparse` endpoints,
  OpenAPI + generated types, frontend download/reparse buttons on
  managed file cards, e2e (upload → download roundtrip sha-equal →
  reparse → chapters/toc replaced → sessions clamped; 409 for
  pre-retention rows; authz matrix).
- **P2 — canonical ingest (G1):** txt→html and plugin-text→html
  converters in `bookshelf-formats` (unit tests: escaping, blank-line
  paragraphs, convention expansion, URL validation), wired into upload
  parsing and plugin materialization; new rows all `'html'`.
- **P3 — backfill + single render path (G3/G4):** startup backfill,
  `Chapter.format` removed from the contract, frontend `TextChapter` +
  format branch deleted, `.epub-content`/`data-origin` styling, e2e
  regression on the wenku8 illustration flow (plate chapter + inline
  marks render as images from stored HTML).
- **P4 — cleanup:** migration 0008 drops `chapters.format`; dead
  `ChapterFormat` model code removed; docs/plugins.md documents the text
  conventions as host-normalized; AGENTS.md progress log updated.

Each phase is one commit (repo rule: one feature = one commit), `cargo
fmt` + clippy + `tsc`/`vite build` green before each.
