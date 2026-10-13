-- NULL means the chapter body has not been cached; an empty string is cached content.
-- Legacy remote empty bodies were placeholders. Local imports have no lazy bodies,
-- so preserve their empty chapters as cached. Keep keys, titles and reading order.
CREATE TABLE chapters_with_nullable_content (
    file_id TEXT NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    title TEXT NOT NULL,
    content TEXT,
    linear INTEGER NOT NULL DEFAULT 1 CHECK (linear IN (0, 1)),
    PRIMARY KEY (file_id, idx)
);
INSERT INTO chapters_with_nullable_content (file_id, idx, title, content, linear)
SELECT c.file_id, c.idx, c.title,
       CASE WHEN (SELECT source FROM book_files WHERE id = c.file_id) = 'local'
            THEN c.content ELSE nullif(c.content, '') END,
       c.linear
FROM chapters c;
DROP TABLE chapters;
ALTER TABLE chapters_with_nullable_content RENAME TO chapters;
