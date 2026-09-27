-- Record which plugin source provided a book's metadata, independent of
-- the content sources of its files: one metadata entry can hold files
-- from many plugins (cross-plugin acquisition), and refresh must pull
-- metadata from this recorded source only — never from the files'
-- sources (which would overwrite it). NULL = no plugin metadata source
-- (local uploads without plugin identification).
ALTER TABLE books ADD COLUMN meta_source TEXT;
ALTER TABLE books ADD COLUMN meta_external_id TEXT;

-- Backfill legacy plugin-materialized books: their metadata came from
-- their own plugin file source.
UPDATE books SET
    meta_source = (
        SELECT bf.source FROM book_files bf
        WHERE bf.book_id = books.id AND bf.source <> 'local'
        ORDER BY bf.rowid LIMIT 1
    ),
    meta_external_id = (
        SELECT bf.external_id FROM book_files bf
        WHERE bf.book_id = books.id AND bf.source <> 'local'
        ORDER BY bf.rowid LIMIT 1
    )
WHERE EXISTS (
    SELECT 1 FROM book_files bf
    WHERE bf.book_id = books.id AND bf.source <> 'local'
);
