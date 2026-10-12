-- Preserve plugin-local series identities independently of host UUIDs.
ALTER TABLE series ADD COLUMN meta_source TEXT;
ALTER TABLE series ADD COLUMN meta_external_id TEXT;
CREATE UNIQUE INDEX idx_series_provenance ON series(meta_source, meta_external_id);
