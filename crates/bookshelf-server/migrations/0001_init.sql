-- Initial schema: users, books (unified metadata storage), chapters
-- (unified content storage), reading sessions and share tokens.
--
-- Books from all sources (uploaded files and wasm plugins) end up in the
-- same tables; `books.source` records where a book came from.

CREATE TABLE users (
    id            TEXT PRIMARY KEY NOT NULL,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user')),
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE books (
    id            TEXT PRIMARY KEY NOT NULL,
    -- 'local' for uploaded files, otherwise the plugin id that provides it
    source        TEXT NOT NULL,
    -- id of the book inside its source (for local books, same as `id`)
    external_id   TEXT NOT NULL,
    title         TEXT NOT NULL,
    authors       TEXT NOT NULL DEFAULT '[]',
    description   TEXT,
    cover_url     TEXT,
    visibility    TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'public')),
    owner_id      TEXT REFERENCES users(id) ON DELETE SET NULL,
    chapter_count INTEGER NOT NULL DEFAULT 0,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (source, external_id)
);

CREATE INDEX idx_books_title ON books(title);
CREATE INDEX idx_books_owner ON books(owner_id);

CREATE TABLE chapters (
    book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    idx     INTEGER NOT NULL,
    title   TEXT NOT NULL,
    content TEXT NOT NULL,
    PRIMARY KEY (book_id, idx)
);

-- A reading session = one device context reading one book.
-- A user may have several sessions per book (phone, laptop, ...), each with
-- its own progress.
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY NOT NULL,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id     TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    label       TEXT NOT NULL DEFAULT 'default',
    chapter_idx INTEGER NOT NULL DEFAULT 0,
    offset      INTEGER NOT NULL DEFAULT 0,
    fraction    REAL NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (user_id, book_id, label)
);

CREATE INDEX idx_sessions_user_book ON sessions(user_id, book_id);
CREATE INDEX idx_sessions_updated ON sessions(updated_at);

-- Share tokens: either a book share (public read link) or a session share
-- (follow someone's progress).
CREATE TABLE shares (
    token      TEXT PRIMARY KEY NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('book', 'session')),
    mode       TEXT NOT NULL DEFAULT 'read' CHECK (mode IN ('read', 'progress')),
    book_id    TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
    created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
    expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_shares_book ON shares(book_id);
CREATE INDEX idx_shares_created_by ON shares(created_by);