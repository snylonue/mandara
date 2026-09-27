-- Extended metadata (bangumi/douban-style fields), one JSON object
-- column per table (docs/metadata-ext-design.md). Every known key is
-- optional inside the object and unknown keys are preserved verbatim,
-- so future fields never need another migration. '{}' = empty.
ALTER TABLE books ADD COLUMN ext_meta TEXT NOT NULL DEFAULT '{}';
ALTER TABLE series ADD COLUMN ext_meta TEXT NOT NULL DEFAULT '{}';
