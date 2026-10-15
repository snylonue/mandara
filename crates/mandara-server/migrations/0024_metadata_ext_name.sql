-- Use the same metadata field name in SQLite, Rust, and the HTTP API.
ALTER TABLE books RENAME COLUMN ext_meta TO ext;
ALTER TABLE series RENAME COLUMN ext_meta TO ext;
