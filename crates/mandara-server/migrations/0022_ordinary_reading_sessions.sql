-- The account's initial session is an ordinary, deletable context.
-- Keep this separate so already-applied migration checksums remain valid.
DROP TRIGGER users_default_session;
DROP INDEX idx_reading_sessions_default;
ALTER TABLE reading_sessions DROP COLUMN is_default;
CREATE TRIGGER users_initial_session AFTER INSERT ON users BEGIN
    INSERT INTO reading_sessions (id, user_id, label) VALUES (lower(hex(randomblob(16))), NEW.id, 'default');
END;
