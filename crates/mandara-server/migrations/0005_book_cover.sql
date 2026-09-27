-- Stored cover images (extracted from uploaded epubs; see
-- GET /api/books/{id}/cover). `cover_url` keeps remote/plugin-provided
-- covers; these columns hold the actual bytes when the source file
-- shipped them.
ALTER TABLE books ADD COLUMN cover BLOB;
ALTER TABLE books ADD COLUMN cover_mime TEXT;
