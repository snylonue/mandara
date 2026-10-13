-- Content items have one immutable source identity. Preserve legacy cached
-- bodies, sessions, shares and retained originals under their existing IDs.
-- Former overrides become the item's actual source; metadata provenance stays
-- on books. Keep colliding legacy items rather than discarding user data.
DROP INDEX idx_book_files_source_ext_vol;
UPDATE book_files SET
    source = coalesce(content_source, source),
    external_id = coalesce(content_external_id, external_id),
    volume_no = CASE WHEN content_source IS NOT NULL THEN 0 ELSE volume_no END,
    volume_offset = CASE WHEN content_source IS NOT NULL THEN 0 ELSE volume_offset END;
ALTER TABLE book_files DROP COLUMN content_source;
ALTER TABLE book_files DROP COLUMN content_external_id;
CREATE INDEX idx_book_files_source_ext_vol ON book_files(book_id, source, external_id, volume_no);
