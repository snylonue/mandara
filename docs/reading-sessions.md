# Reading sessions

A reading session is a user-owned context (for example, a phone or a reread),
not a bookmark for a single book. It contains independent progress for any
number of book files. Progress stays file-specific because different editions
can have different chapter layouts.

Account creation automatically creates one ordinary session. No session is
marked as default or protected from deletion, and deleting all sessions does
not create a replacement. Creating a named session starts with no progress;
an existing session with the same trimmed label is reused.

## Web behavior

The shelf header and reader settings select the same session. The browser
stores its id separately for each account and restores it across navigation
and reloads. If the saved session no longer exists, the first (oldest) session
is selected. When none remain, the user can create a session before reading.
An explicit reader session query adopts that session as the browser selection.
Shelf and series progress use only this selected context.

In reader settings, choose a source context and confirm the per-book sync.
Only the currently opened file is copied, including its chapter and in-chapter
position. This is an explicit overwrite, even if the source is further behind.
Other books and the source context remain unchanged. Refresh the source list
when another device has added progress since opening the reader.

The reader flushes pending saves before switching or synchronizing, and remounts
at the restored position so stale scroll writes cannot undo the sync.

## API and migration

See [the API contract](api/openapi.yaml) for request and response schemas.

- `GET /api/sessions` lists global contexts; `POST` creates one.
- `GET /api/sessions/{id}/progress` lists a context's file progress.
- `PUT /api/sessions/{id}/files/{file_id}/progress` upserts one position.
- `POST /api/sessions/{id}/files/{file_id}/sync` copies only that file from
  `source_session_id`. Both contexts must belong to the caller and the file
  must remain visible. Missing source progress is an error, not a reset.
- `DELETE /api/sessions/{id}` deletes an owned context and its progress.

The old per-file creation and position-update endpoints are replaced by these
context-scoped endpoints. Legacy progress is grouped by owner and label; ids,
positions, timestamps and existing public progress links are preserved.
The internal legacy `sessions` table stores file progress while
`reading_sessions` stores global contexts.

`ReadingProgress.session_id` is the global context id. Its `id` is a stable
per-file progress id. The existing share API's `session_id` continues to accept
that per-file progress id, so sharing one book never exposes the rest of a
context's reading history.
