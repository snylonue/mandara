-- Reject existing partial originals without discarding or inventing metadata.
CREATE TEMP TABLE _mandara_original_metadata_check (
    valid INTEGER NOT NULL
        CONSTRAINT book_files_original_all_or_none CHECK (valid = 1)
);
INSERT INTO _mandara_original_metadata_check (valid)
SELECT NOT EXISTS (
    SELECT 1 FROM book_files
    WHERE ((orig_ext IS NULL) + (orig_sha256 IS NULL) + (orig_size IS NULL)) NOT IN (0, 3)
);
DROP TABLE _mandara_original_metadata_check;

-- SQLite cannot add a table CHECK without rebuilding this referenced parent
-- table. Triggers enforce the same invariant without touching its dependents.
CREATE TRIGGER book_files_original_insert
BEFORE INSERT ON book_files
WHEN ((NEW.orig_ext IS NULL) + (NEW.orig_sha256 IS NULL) + (NEW.orig_size IS NULL)) NOT IN (0, 3)
BEGIN
    SELECT RAISE(ABORT, 'original metadata must be all NULL or all non-NULL');
END;

CREATE TRIGGER book_files_original_update
BEFORE UPDATE OF orig_ext, orig_sha256, orig_size ON book_files
WHEN ((NEW.orig_ext IS NULL) + (NEW.orig_sha256 IS NULL) + (NEW.orig_size IS NULL)) NOT IN (0, 3)
BEGIN
    SELECT RAISE(ABORT, 'original metadata must be all NULL or all non-NULL');
END;
