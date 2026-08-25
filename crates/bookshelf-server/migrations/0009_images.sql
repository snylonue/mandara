-- Localized chapter images (storage unification follow-up): plugin
-- chapter text may reference remote illustration URLs (e.g. wenku8's
-- pic CDN, which browsers may fail to load directly). At materialization
-- the host downloads them into `data/files/images/{sha256}` and rewrites
-- the chapter HTML to `GET /api/images/{id}`; this table maps the
-- original URL to the stored image (dedup across chapters/books) and
-- carries the mime type for serving.
CREATE TABLE images (
    id         TEXT PRIMARY KEY NOT NULL, -- sha256 of the bytes (hex)
    url        TEXT NOT NULL UNIQUE,
    mime       TEXT NOT NULL,
    size       INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
