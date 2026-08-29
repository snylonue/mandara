# Series & multi-volume acquisition design

Status: **approved 2026-08-23** — owner decisions recorded in §3a:
(1) auto-split whenever the source declares >1 volume; (2) series page in P0;
(3) manual series management (create / assign / reorder, incl. uploads) in P0;
(4) volume entries keep the plain series title, members distinguished by
`volume_no` (`第N卷` badge).

## 1. Problem

Two gaps reported by the owner:

1. **No series concept.** The library is a flat list of `books` entries.
   A light-novel series (《刀剑神域》 卷1..20) has no grouping entity;
   there is nothing to tie its volumes together.
2. **Multi-volume sources merge.** wenku8 (and any chapter-mode plugin
   source) advertises one book id per series. Its TOC (`reader.php?aid=`)
   has 卷 rows, but `book-entry`/`chapter-titles`/`get-chapter` only know a
   *flat, volume-independent* chapter list. Acquisition therefore creates
   exactly one library book whose chapter list is 卷1..卷N concatenated
   (with `第N卷 ` title prefixes on repeated 插图 chapters) — volumes are
   not first-class, progress is one "book" wide, and volume titles/layout
   are lost.

## 2. Goals (P0)

- A **series** = a publication family: `{title, authors, description,
  cover_url}` + member books ordered by volume number.
- Acquiring a multi-volume plugin book auto-splits: **one series + N
  book entries + N virtual files** (one per volume).
- Each volume is an ordinary library book downstream: own metadata entry,
  own file, own TOC slice, own sessions/shares/progress. Nothing else in
  the reader/session/share model changes.
- Single-volume books behave exactly as today (`volume_no = 0`, no series).
- Uploads (epub/txt) are not volume-split in this round (a parsed file is
  one complete edition).

## 3. Non-goals (follow-ups)

- Series cover bytes (BLOB). P0 stores `cover_url` from the source only;
  volume cards already show covers via `GET /api/books/{id}/cover`.
- Volume splitting for **file-mode** (`get-book-file`) sources: a
  downloaded epub/txt is inherently a single file; wenku8's downloads are
  login-walled anyway.
- Deleting/merging volumes after acquisition (delete per book already
  works via existing book/file deletion).

## 3a. Owner decisions (2026-08-23)

1. **Auto-split**: whenever the source declares >1 volume, acquisition
   splits automatically — no opt-in checkbox in the add-book dialog.
2. **Series page** is in P0: `/series/:id` shows the series header,
   all volumes (ordered, with covers/第N卷/章节数) and, for the series
   creator/admin, manage controls (edit metadata, reorder volumes,
   add/remove books, delete series).
3. **Manual series management** is in P0 and covers uploads too: create a
   series, assign any (managed) book into a series with a volume number,
   move/reorder/remove — via the series page and a per-book
   "加入系列" dialog.
4. **Volume titles**: every volume entry keeps the plain series title
   (`books.volume_no` distinguishes them; UI renders `第N卷`). Manual
   per-volume title overrides via the existing book patch still work.

## 4. Data model (migration `0006_series_volumes.sql`)

```sql
CREATE TABLE series (
    id          TEXT PRIMARY KEY NOT NULL,
    title       TEXT NOT NULL,
    authors     TEXT NOT NULL DEFAULT '[]',
    description TEXT,
    cover_url   TEXT,
    created_by  TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

ALTER TABLE books ADD COLUMN series_id TEXT REFERENCES series(id) ON DELETE SET NULL;
ALTER TABLE books ADD COLUMN volume_no  INTEGER NOT NULL DEFAULT 0;

-- book_files: one row per volume slice of a plugin book.
ALTER TABLE book_files ADD COLUMN volume_no    INTEGER NOT NULL DEFAULT 0;
ALTER TABLE book_files ADD COLUMN volume_offset INTEGER NOT NULL DEFAULT 0;
DROP INDEX IF EXISTS sqlite_autoindex_book_files_1;
CREATE UNIQUE INDEX idx_book_files_source_ext_vol
    ON book_files(source, external_id, volume_no);
```

- `books.volume_no`: ordinal within the series (1-based when part of a
  series, else 0). `books.series_id` NULL + `volume_no = 0` = standalone.
- `book_files.volume_no`: which slice of the source book this file is
  (0 = the whole book, today's semantics).
- `book_files.volume_offset`: flat chapter index of this volume's first
  chapter inside the source (`get-chapter(source, volume_offset + idx)`),
  persisted because the plugin is stateless. `chapter_count` keeps its
  meaning as *this file's* (volume-local) chapter count.
- The old `UNIQUE (source, external_id)` index must be dropped and
  replaced — SQLite auto-indexes the table-level `UNIQUE` constraint, so
  the constraint itself is re-created as a unique index.

Existing rows migrate with defaults (standalone, volume 0, offset 0).
Books already materialized from wenku8 stay merged; deleting and
re-acquiring re-splits them (documented).

## 5. WIT v0.5.0 — `book-entry.volumes`

```wit
/// One volume of a multi-volume source book (e.g. a light-novel series).
///
/// The volume chapter lists are consecutive slices of the flat
/// `chapter-titles` order: volume k starts at
/// `sum(volumes[0..k].chapter-count)`. The host slices titles and maps
/// per-volume indices to flat indices using these counts + the persisted
/// `volume-offset`; plugins must keep the sum consistent with
/// `chapter-titles` (the host clamps/handles mismatch defensively).
record volume-info {
    title: string,
    /// Number of chapters in this volume (in the flat reading order).
    chapter-count: u32,
}
```

- `book-entry` gains `volumes: option<list<volume-info>>`
  (`none`/empty = single volume = current behavior).
- **Index semantics unchanged**: `chapter-titles` and `get-chapter`
  indices stay flat over the whole source book. The host derives the
  volume structure from `volumes` alone, so statelessness, lazy chapter
  pulls and the content-target indirection are untouched.
- Version stays in the v-dev series (no compat); all four demo plugins
  are rebuilt (they set `volumes: None`; wenku8 populates it).

## 6. wenku8 plugin (P0)

- `get_book` additionally fetches the (small) TOC and returns
  `volumes: Some(...)`:
  - volume title = the `vcss` row text when non-empty, else `第N卷`;
  - `chapter-count` per volume computed under the configured 插图
    `placement()` policy (`build_order` output, must match
    `chapter-titles`), i.e. Skip drops the plate counts.
- `chapter_titles`/`get_chapter` keep the exact flat order of today.
- The `第N卷 ` prefixes on repeated 插图 titles become redundant once the
  host slices per-volume files, but stay harmless (each volume file's
  slice contains exactly one such chapter). Left untouched for P0.
- Registering an instance / protocol stays identical.

## 7. Host & library service

### 7.1 `SourceBook` model

`bookshelf_core::source::SourceBook` gains
`pub volumes: Vec<SourceVolume>` where `SourceVolume { title, chapter_count }`.
The WIT→model conversion populates it (`none` → empty vec).

### 7.2 Split acquisition (`acquire_book`)

New path, active when: content = plugin chapter-mode, **metadata is
`New` or `Plugin`** (not `Attach`), and `entry.volumes.len() > 1`.

1. `plugin_book_entry` (as today) to get the entry + volumes.
2. `content_target` → flat `chapter_titles` (as today).
3. Compute volume slices from `volumes` (sums; clamp `chapter_count`
   against the flat length; drop degenerate empty volumes; a book with
   ≤1 volume falls back to the current single-entry path).
4. In one transaction:
   - create `series` row (title/authors/description/cover_url from the
     entry);
   - per volume `v` (1-based): create `books` row (`series_id`, `volume_no
     = v`, title = entry title, authors/description/cover_url copied),
     create virtual `book_files` row (`source`, `external_id`,
     `volume_no = v`, `volume_offset = slice_start`, `chapter_count =
     slice.len()`, public, owner = caller), insert placeholder titles
     (*sliced*) via the existing `upsert_placeholder_titles`.
5. Response: `AcquireResult { series, books: [BookDetail, ...] }`.

Conflict rule update (one source book = one series): the existing
"plugin book already under metadata X" 409 check becomes volume-aware —
any row `(source, external_id)` with any `volume_no` that belongs to a
different book than the target triggers 409. This also rejects
re-acquiring a book that a pre-upgrade merged row still references
(message hints: delete the old entry first to re-split).

`Attach` mode + multi-volume entry → 400 with a clear message
("a multi-volume source book cannot be attached to a single metadata
entry; acquire it as a new book"). `identify-upload` results keep
single-book behavior (an uploaded file is one edition), documented.

### 7.3 Lazy content + refresh

- `get_chapter`: plugin index becomes `file.volume_offset + idx` when
  `volume_no > 0` (offset 0 for volume 1/standalone — semantics
  unchanged in the common case).
- `ensure_titles` / placeholder creation / `refresh_book`: call the
  content instance for the flat list, then **slice** per
  `volume_offset .. volume_offset + chapter_count` instead of inserting
  the whole list into every volume file. `refresh_book` keeps
  re-pulling metadata per entry; volume re-slicing happens on the same
  pass (the plugin must re-declare the same structure; drift is
  documented, not auto-fixed in P0).

## 8. API contract (`docs/api/openapi.yaml`)

- New schema `SeriesBrief { id, title, authors, description, cover_url,
  volume_count, created_by, created_at }`.
- `BookMeta` += `series_id: string|null`, `volume_no: integer`.
- `FileMeta` += `volume_no: integer` (0 = whole book) and
  `volume_offset: integer` (persisted flat start — needed to map lazy
  chapter pulls; harmless to expose).
- `BookDetail` += `series: SeriesBrief|null`.
- `BookListEntry` += `series: SeriesBrief|null` — the library groups
  client-side without extra round trips.
- `POST /api/books` 201 body becomes
  `AcquireResult { series: SeriesBrief|null, books: [BookDetail] }`
  (`books` always non-empty; 1 element unless the acquisition split).
  Frontend regenerates `src/api/schema.d.ts` with `npm run api-types`.

New series endpoints:

- `GET  /api/series` → `[SeriesBrief]` — series visible to the caller
  (creator, or ≥1 visible member book; admins see all).
- `POST /api/series` `{title, authors?, description?}` → 201 `SeriesBrief`
  (any logged-in user; the creator manages it).
- `GET  /api/series/{id}` → `SeriesDetail { series, books: [BookDetail] }`
  — member books ordered by `volume_no`, files filtered by visibility
  (empty `files` = no visible file).
- `PATCH /api/series/{id}` `{title?, authors?, description?}` →
  `SeriesBrief` (creator/admin).
- `PUT  /api/series/{id}/members` `{book_ids: [string]}` → `SeriesDetail`
  — atomic reorder: the listed books become the members in order
  (`volume_no` = index+1); former members not listed are unassigned.
  Caller must manage the series *and* every listed book (403 otherwise).
- `DELETE /api/series/{id}` → 204 — unassigns all member books then
  deletes the series (creator/admin).
- `PATCH /api/books/{id}` gains optional `series_id: string|null` and
  `volume_no: integer|null|absent` — assign to a series (auto = next
  free volume), move (volume number; 409 on duplicate), or `null`
  `series_id` to unassign. Book creator/admin *and* series
  creator/admin required.

`BookMeta`/`FileMeta`/`SeriesBrief` changes flow into every response that
embeds them; the frontend regenerates `src/api/schema.d.ts`.

## 9. Frontend (P0)

- **Library**: group cards by `entry.series.id`; each group renders a
  header (series title, `N 卷` chip, authors, link to `/series/:id`)
  above its member cards. Member cards show a `第N卷` badge.
  Standalone entries render exactly as today; add-book dialog unchanged
  (user still enters one wenku8 id).
- **Series page** `/series/:id`: series header (title, authors,
  description, N 卷, covers from member books), volume grid ordered by
  `volume_no` (each card → `/book/:id`, shows `第N卷` + chapter count).
  Manage mode for creator/admin: edit title/authors/description
  (PATCH), reorder volumes with ↑/↓ and save manually (PUT members),
  remove a member, add books (picker of caller's unassigned books),
  delete the series.
- **Book detail**: header shows a series chip (series title · 第N卷)
  linking to the series page, plus an 加入系列/管理 dialog
  (existing series + volume number, or create a new series) →
  `PATCH /api/books/{id}`.
- **AddBookDialog**: handles the new `AcquireResult` body; on split,
  toast `已创建系列《title》· N 卷` (new i18n keys), otherwise the
  regular added toast; then refresh the library.
- i18n: new keys under `library.*` / `book.*` / `series.*` (zh-CN).

## 10. Tests / verification

- Host: WIT roundtrip with `volumes` (incl. `none`), slice computation
  (sums, clamp, degenerate empty volume collapse), conflict 409,
  `get-chapter` offset mapping, refresh slice — unit tests in the
  bookshelf-server / bookshelf-plugin crates; existing 44 backend tests
  keep passing.
- wenku8 guest: `volumes` shape from a fixture TOC (vcss titles incl.
  empty titles → `第N卷` fallback; placement Skip changes counts).
- E2E (dev instance + live wenku8): acquire
  3617 → series + N books (each `第N卷` volume, plate chapters at
  volume-local positions), chapters lazy-fetch per volume with correct
  offsets, sessions per volume, refresh, 409 on re-acquire, and the
  existing e2e matrix (file+attach / plugin+attach single-volume
  sources, share regression).
- `cargo fmt --all` + clippy -D warnings (workspace baseline known:
  clippy is clean since c7700af), frontend `tsc` + `vite build`.

## 11. Implementation order (one feature = one commit)

1. `docs:` this design doc.
2. `feat(plugin)`: WIT v0.5.0 `volume-info`/`book-entry.volumes`,
   `SourceBook.volumes` + conversion, rebuild all demo plugins
   (`volumes: None`), wenku8 guest populates volumes (+ guest tests);
   host tests updated.
3. `feat(api)`: migration 0006 (series table, book/file columns,
   `book_files` unique-key rebuild), split acquisition + volume-aware
   409, volume-offset mapping in lazy materialization / refresh,
   series endpoints + book-patch extension, model/OpenAPI updates.
4. `feat(web)`: library series grouping, series page with manage UI,
   book-detail series dialog, AddBookDialog result handling, i18n,
   regenerated types (`npm run api-types`).
5. e2e verification + progress log.