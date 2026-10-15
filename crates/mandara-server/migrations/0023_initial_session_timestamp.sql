-- The initial reading context shares the account's native UTC timestamp.
DROP TRIGGER users_initial_session;
CREATE TRIGGER users_initial_session AFTER INSERT ON users BEGIN
    INSERT INTO reading_sessions (id, user_id, label, created_at)
    VALUES (lower(hex(randomblob(16))), NEW.id, 'default', NEW.created_at);
END;
