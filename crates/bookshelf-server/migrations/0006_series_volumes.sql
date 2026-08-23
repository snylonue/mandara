-- no-transaction
-- Series + per-volume book files (docs/series-design.md).
--
-- `book_files` is rebuilt: its table-level `UNIQUE(source, external_id)`
-- must become `UNIQUE(source, external_id, volume_no)` so a multi-volume
-- plugin book can live as N files sharing one source external id (one
-- 卷 per file). Rebuilding a referenced table requires foreign keys off;
-- sqlx wraps migrations in a transaction by default, and `PRAGMA
-- foreign_keys` is a no-op inside a transaction — hence the
-- `-- no-transaction` marker + an explicit BEGIN/COMMIT here. The PRAGMA
-- is connection-scoped and restored right after; startup runs migrations
-- before any traffic, so no other connection can interleave.
PRAGMA foreign_keys = OFF;
BEGIN;

-- A series: metadata grouping for the volumes of one publication family.
-- Member books carry `series_id` + `volume_no` (1-based order).
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

-- book_files rebuilt: same columns plus `volume_no` (0 = whole book) and
-- `volume_offset` (flat chapter index of this volume's first chapter
-- inside the source, persisted because the plugin is stateless).
CREATE TABLE book_files_new (
    id            TEXT PRIMARY KEY NOT NULL,
    book_id       TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    -- 'local' for uploaded files, otherwise the plugin id providing it
    source        TEXT NOT NULL,
    -- id of the book inside its source (for local files, same as `id`)
    external_id   TEXT NOT NULL,
    -- chapter content indirection (metadata/content source separation)
    content_source TEXT,
    content_external_id TEXT,
    -- 'epub' | 'txt' for uploads, 'plugin' for plugin-provided books
    format        TEXT NOT NULL DEFAULT 'epub',
    -- short human label for this edition, e.g. "epub 校对版" / "第一卷"
    label         TEXT NOT NULL DEFAULT '',
    visibility    TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'public')),
    owner_id      TEXT REFERENCES users(id) ON DELETE SET NULL,
    chapter_count INTEGER NOT NULL DEFAULT 0,
    -- hierarchical TOC (JSON array of TocNode), '' = synthesize flat
    toc           TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    -- volume slice of the source book this file covers (0 = whole book)
    volume_no     INTEGER NOT NULL DEFAULT 0,
    -- flat index of this volume's first chapter inside the source
    volume_offset INTEGER NOT NULL DEFAULT 0
);

INSERT INTO book_files_new (
    id, book_id, source, external_id, content_source, content_external_id,
    format, label, visibility, owner_id, chapter_count, toc, created_at,
    volume_no, volume_offset
)
SELECT id, book_id, source, external_id, content_source, content_external_id,
       format, label, visibility, owner_id, chapter_count, toc, created_at,
       0, 0
FROM book_files;

DROP TABLE book_files;
ALTER TABLE book_files_new RENAME TO book_files;

CREATE INDEX idx_book_files_book ON book_files(book_id);
CREATE INDEX idx_book_files_owner ON book_files(owner_id);
-- one plugin book (source + external id) may now have one row per volume
CREATE UNIQUE INDEX idx_book_files_source_ext_vol ON book_files(source, external_id, volume_no);

CREATE INDEX idx_books_series ON books(series_id);

COMMIT;
PRAGMA foreign_keys = ON;