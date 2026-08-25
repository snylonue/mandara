-- Storage unification P4: every chapter is canonical HTML (converted at
-- the ingest boundary; legacy plain-text rows were backfilled by the
-- startup step that runs before this migration — see
-- crate::service::library::backfill_text_chapters). The format column is
-- now constant 'html' and no longer read.
--
-- NOTE: SQLite 3.35+ (DROP COLUMN); sqlx bundles a recent version.
ALTER TABLE chapters DROP COLUMN format;
