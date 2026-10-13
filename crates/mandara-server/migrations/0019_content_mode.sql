-- Content loading is selected once when a content item is added.
ALTER TABLE book_files ADD COLUMN content_mode TEXT NOT NULL DEFAULT 'chapters'
    CHECK (content_mode IN ('chapters', 'file'));
-- The built-in upload provider supplies whole files.
UPDATE book_files SET content_mode = 'file' WHERE source = 'local';
DROP INDEX idx_book_files_source_ext_vol;
CREATE INDEX idx_book_files_source_ext ON book_files(book_id, source, external_id);
ALTER TABLE book_files DROP COLUMN volume_no;
ALTER TABLE book_files DROP COLUMN volume_offset;
