-- Plugin system v2: instance registry + metadata/content source separation.
-- See docs/plugin-v2-design.md.

-- One registered plugin instance. `id` is the source id used in
-- `book_files.source`; `wasm_file` is the basename of a compiled
-- component under data/plugins/ (one wasm file can back several instances
-- with different configurations); `config` is the validated JSON object
-- keyed by config-schema field keys.
CREATE TABLE plugin_instances (
    id         TEXT PRIMARY KEY NOT NULL,
    wasm_file  TEXT NOT NULL,
    config     TEXT NOT NULL DEFAULT '{}',
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

-- Content indirection: the chapters of a virtual plugin file may come from
-- a different plugin instance (metadata/content source separation). NULL =
-- same as `source` / `external_id`. Local uploads keep both columns NULL.
ALTER TABLE book_files ADD COLUMN content_source TEXT;
ALTER TABLE book_files ADD COLUMN content_external_id TEXT;