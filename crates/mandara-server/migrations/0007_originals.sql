-- Retained original file bytes (storage unification, design
-- docs/storage-unification-design.md §4.2). The bytes themselves live on
-- disk under `data/files/{file_id}.{orig_ext}`; these columns carry the
-- metadata needed to serve them (`GET /api/files/{id}/download`) and to
-- re-parse with a newer parser (`--reparse-originals`).
--
-- NULL = no original retained: plugin chapter-mode files (the plugin is
-- the source), and uploads made before this migration (their bytes were
-- discarded at parse time).
ALTER TABLE book_files ADD COLUMN orig_ext TEXT;
ALTER TABLE book_files ADD COLUMN orig_sha256 TEXT;
ALTER TABLE book_files ADD COLUMN orig_size INTEGER;
