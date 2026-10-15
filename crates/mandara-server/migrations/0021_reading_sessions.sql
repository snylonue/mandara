-- Global reading contexts. The legacy sessions table keeps per-file progress
-- ids so existing public progress links remain valid.
CREATE TABLE reading_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (user_id, label)
);
CREATE UNIQUE INDEX idx_reading_sessions_default ON reading_sessions(user_id) WHERE is_default = 1;
INSERT INTO reading_sessions (id, user_id, label, is_default)
SELECT lower(hex(randomblob(16))), id, 'default', 1 FROM users;
INSERT INTO reading_sessions (id, user_id, label)
SELECT lower(hex(randomblob(16))), user_id, label FROM sessions
WHERE label <> 'default' GROUP BY user_id, label;
ALTER TABLE sessions ADD COLUMN reading_session_id TEXT REFERENCES reading_sessions(id) ON DELETE CASCADE;
UPDATE sessions SET reading_session_id = (
    SELECT id FROM reading_sessions WHERE reading_sessions.user_id = sessions.user_id AND reading_sessions.label = sessions.label
);
ALTER TABLE sessions ADD COLUMN position_kind TEXT NOT NULL DEFAULT 'fraction' CHECK (position_kind IN ('fraction', 'offset'));
UPDATE sessions SET position_kind = 'offset' WHERE offset > 0 AND fraction = 0;
CREATE UNIQUE INDEX idx_sessions_context_file ON sessions(reading_session_id, file_id);
-- SQLite cannot add a non-null foreign-key column to a populated table.
CREATE TRIGGER sessions_require_context BEFORE INSERT ON sessions
WHEN NEW.reading_session_id IS NULL OR NOT EXISTS (
    SELECT 1 FROM reading_sessions WHERE id = NEW.reading_session_id AND user_id = NEW.user_id AND label = NEW.label
) BEGIN SELECT RAISE(ABORT, 'invalid reading context'); END;
CREATE TRIGGER sessions_preserve_context BEFORE UPDATE OF reading_session_id, user_id, label ON sessions
WHEN NEW.reading_session_id IS NULL OR NOT EXISTS (
    SELECT 1 FROM reading_sessions WHERE id = NEW.reading_session_id AND user_id = NEW.user_id AND label = NEW.label
) BEGIN SELECT RAISE(ABORT, 'invalid reading context'); END;
CREATE TRIGGER users_default_session AFTER INSERT ON users BEGIN
    INSERT INTO reading_sessions (id, user_id, label, is_default) VALUES (lower(hex(randomblob(16))), NEW.id, 'default', 1);
END;
