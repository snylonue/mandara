-- Hierarchical table of contents per file (JSON array of TocNode:
-- {"title": str, "idx": int|null, "children": [...]}).
-- Empty string = no TOC stored (synthesize a flat one from chapters).
ALTER TABLE book_files ADD COLUMN toc TEXT NOT NULL DEFAULT '';