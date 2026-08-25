-- Image store v2: the server is a pure content-addressed store; producers
-- (wasm plugins via the `store-image` import, epub upload ingest) fetch
-- and attach images themselves.
--
-- The `url` provenance column is dropped (dedup key is the id = sha256 of
-- the bytes; origin tracking lives with the producer). Chapter bodies the
-- old host-side localization rewrote to `/api/images/{id}` are stale under
-- the new model — they are cleared back to title-only placeholders so the
-- next read lazily re-materializes them from their plugin source
-- (`chapters` rows keep title/count consistency; empty content triggers
-- the lazy pull).
--
-- Old epub uploads keep self-contained `data:` URIs and render as before.

DROP TABLE images;

CREATE TABLE images (
    id         TEXT PRIMARY KEY NOT NULL, -- sha256 of the bytes (hex)
    mime       TEXT NOT NULL,
    size       INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

UPDATE chapters SET content = '' WHERE content LIKE '%/api/images/%';
