-- Initial schema.
--
-- Two-level model:
--   books      – pure metadata entries (title, authors, description...)
--   book_files – actual book files (epub/txt uploads, or virtual files
--                provided by wasm plugins). Several files can share one
--                metadata entry (e.g. different formats or editions).
--
-- Permission model: each *file* is either `private` (visible only to its
-- owner and admins) or `public` (visible to every logged-in user). On top
-- of that the whole auth layer can be switched off (single-user mode).
--
-- Chapters/sessions/shares all attach to a file, because different files
-- of the same book can have different chapter splits.

CREATE TABLE users (
    id            TEXT PRIMARY KEY NOT NULL,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user')),
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Book metadata. One entry can be linked to any number of files.
CREATE TABLE books (
    id          TEXT PRIMARY KEY NOT NULL,
    title       TEXT NOT NULL,
    authors     TEXT NOT NULL DEFAULT '[]',
    description TEXT,
    cover_url   TEXT,
    created_by  TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_books_title ON books(title);

-- Actual book files (or virtual plugin-provided books).
CREATE TABLE book_files (
    id            TEXT PRIMARY KEY NOT NULL,
    book_id       TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    -- 'local' for uploaded files, otherwise the plugin id providing it
    source        TEXT NOT NULL,
    -- id of the book inside its source (for local files, same as `id`)
    external_id   TEXT NOT NULL,
    -- 'epub' | 'txt' for uploads, 'plugin' for plugin-provided books
    format        TEXT NOT NULL DEFAULT 'epub',
    -- short human label for this edition, e.g. "epub 校对版"
    label         TEXT NOT NULL DEFAULT '',
    visibility    TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'public')),
    owner_id      TEXT REFERENCES users(id) ON DELETE SET NULL,
    chapter_count INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (source, external_id)
);

CREATE INDEX idx_book_files_book ON book_files(book_id);
CREATE INDEX idx_book_files_owner ON book_files(owner_id);

-- Chapter content belongs to a file (different files may split chapters
-- differently).
CREATE TABLE chapters (
    file_id TEXT NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    idx     INTEGER NOT NULL,
    title   TEXT NOT NULL,
    content TEXT NOT NULL,
    PRIMARY KEY (file_id, idx)
);

-- A reading session = one device context reading one file.
-- A user may have several sessions per file (phone, laptop, ...), each with
-- its own progress.
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY NOT NULL,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    file_id     TEXT NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    label       TEXT NOT NULL DEFAULT 'default',
    chapter_idx INTEGER NOT NULL DEFAULT 0,
    offset      INTEGER NOT NULL DEFAULT 0,
    fraction    REAL NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (user_id, file_id, label)
);

CREATE INDEX idx_sessions_user_file ON sessions(user_id, file_id);
CREATE INDEX idx_sessions_updated ON sessions(updated_at);

-- Share tokens: either a file share (anonymous read link) or a session
-- share (follow someone's progress).
CREATE TABLE shares (
    token      TEXT PRIMARY KEY NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('book', 'session')),
    mode       TEXT NOT NULL DEFAULT 'read' CHECK (mode IN ('read', 'progress')),
    file_id    TEXT NOT NULL REFERENCES book_files(id) ON DELETE CASCADE,
    session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
    created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_shares_file ON shares(file_id);
CREATE INDEX idx_shares_created_by ON shares(created_by);