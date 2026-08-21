//! The library service: central storage for book metadata, files and
//! chapters.
//!
//! Two-level model: `books` are pure metadata entries, `book_files` are the
//! actual books (uploads or virtual plugin books). Files from every source
//! are catalogued here; plugin chapters are materialized into the same
//! tables on first access, so readers see one unified store.
//!
//! Visibility is per file: `private` = owner + admins only, `public` =
//! visible to every logged-in user.

use std::sync::Arc;

use sqlx::SqlitePool;
use tracing::info;

use bookshelf_core::error::Result as CoreResult;
use bookshelf_core::model::{
    BookMeta, Chapter, ChapterFormat, ChapterMeta, FileMeta, TocNode, User, Visibility,
};
use bookshelf_core::source::{BookSource, SourceBook};

use crate::error::ApiError;
use crate::rows::{BookRow, ChapterRow, ChapterTitleRow, FileRow};

/// Where the metadata of an upload comes from.
pub enum UploadMetadata {
    /// Attach the file to an existing metadata entry (its metadata wins).
    Attach { book_id: String },
    /// Metadata taken from a plugin source. `book_id_in_source` must be an
    /// id offered by that plugin's catalog.
    Plugin {
        source: String,
        book_id_in_source: String,
    },
    /// Ask every loaded plugin to identify the file (first match wins),
    /// falling back to metadata parsed from the file itself.
    Auto,
}

/// User-supplied fields that override the metadata produced by the file
/// parser or a plugin source.
#[derive(Default)]
pub struct MetadataOverrides {
    pub title: Option<String>,
    pub authors: Option<Vec<String>>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
}

impl MetadataOverrides {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.authors.is_none()
            && self.description.is_none()
            && self.cover_url.is_none()
    }
}

/// The uploaded file itself: bytes, original name and the storage options
/// chosen by the uploader.
pub struct UploadInput<'a> {
    pub bytes: &'a [u8],
    pub filename: &'a str,
    pub visibility: Visibility,
    pub label: &'a str,
}

pub struct Library {
    db: SqlitePool,
    sources: Vec<Arc<dyn BookSource>>,
}

impl Library {
    pub fn new(db: SqlitePool, sources: Vec<Arc<dyn BookSource>>) -> Self {
        Library { db, sources }
    }

    // ---- sources ---------------------------------------------------------

    pub fn plugin_ids(&self) -> Vec<String> {
        self.sources.iter().map(|s| s.id().to_string()).collect()
    }

    /// Re-catalogue all plugin books into the library: metadata into
    /// `books`, a virtual public file into `book_files`.
    /// Returns the number of books upserted.
    pub async fn sync_plugins(&self) -> Result<usize, ApiError> {
        let mut total = 0;
        for source in &self.sources {
            let books = source.list_books().await?;
            let count = books.len();
            for book in &books {
                let titles = source.chapter_titles(&book.id).await?;
                self.ensure_plugin_book(source.id(), None, book, &titles)
                    .await?;
            }
            info!(source = source.id(), books = count, "plugin catalog synced");
            total += count;
        }
        Ok(total)
    }

    /// Find the metadata entry of a plugin book, creating it (plus the
    /// plugin's virtual public file) when missing. Metadata keeps being
    /// refreshed from the entry on every call.
    ///
    /// `owner` claims the metadata when it has no creator yet (uploads
    /// identified as this plugin book), and becomes the owner of a newly
    /// created virtual file.
    async fn ensure_plugin_book(
        &self,
        source_id: &str,
        owner: Option<&str>,
        book: &SourceBook,
        titles: &[String],
    ) -> Result<String, ApiError> {
        let authors = serde_json::to_string(&book.authors).unwrap_or_else(|_| "[]".into());

        // Reuse the metadata entry when this plugin file was synced before,
        // otherwise create both the book metadata and its virtual file.
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT book_id FROM book_files WHERE source = ? AND external_id = ?")
                .bind(source_id)
                .bind(&book.id)
                .fetch_optional(&self.db)
                .await?;

        let book_id = match existing {
            Some((book_id,)) => book_id,
            None => {
                let book_id = uuid::Uuid::new_v4().simple().to_string();
                let mut tx = self.db.begin().await?;
                sqlx::query(
                    "INSERT INTO books (id, title, authors, description, cover_url, created_by) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(&book_id)
                .bind(&book.title)
                .bind(&authors)
                .bind(&book.description)
                .bind(&book.cover_url)
                .bind(owner)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO book_files (id, book_id, source, external_id, format, label, visibility, owner_id, chapter_count) \
                     VALUES (?, ?, ?, ?, 'plugin', '', 'public', ?, ?)",
                )
                .bind(uuid::Uuid::new_v4().simple().to_string())
                .bind(&book_id)
                .bind(source_id)
                .bind(&book.id)
                .bind(owner)
                .bind(titles.len() as i64)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                book_id
            }
        };

        // Keep metadata fresh on every sync/claim. When a user claims an
        // upload as this plugin book, the metadata gains that user as its
        // creator (so they can maintain and refresh it).
        sqlx::query(
            "UPDATE books SET title = ?, authors = ?, description = ?, cover_url = ?, \
             created_by = COALESCE(created_by, ?) WHERE id = ?",
        )
        .bind(&book.title)
        .bind(&authors)
        .bind(&book.description)
        .bind(&book.cover_url)
        .bind(owner)
        .bind(&book_id)
        .execute(&self.db)
        .await?;
        sqlx::query("UPDATE book_files SET chapter_count = ? WHERE source = ? AND external_id = ?")
            .bind(titles.len() as i64)
            .bind(source_id)
            .bind(&book.id)
            .execute(&self.db)
            .await?;
        Ok(book_id)
    }

    pub fn source(&self, id: &str) -> Option<Arc<dyn BookSource>> {
        self.sources.iter().find(|s| s.id() == id).cloned()
    }

    // ---- uploads ---------------------------------------------------------

    /// Ingest an uploaded epub/txt file. The metadata comes from one of
    /// [`UploadMetadata`]'s modes (`attach`/`plugin`/`auto`), possibly with
    /// user `overrides` on top of whatever the file parser or plugin
    /// produced. Returns the metadata entry and the stored file.
    pub async fn upload_file(
        &self,
        user: &User,
        upload: UploadInput<'_>,
        mode: UploadMetadata,
        overrides: &MetadataOverrides,
    ) -> Result<(BookMeta, FileMeta), ApiError> {
        // Chapter extraction is needed in every mode, so parse first.
        let parsed = bookshelf_formats::parse(upload.bytes, upload.filename)?;

        let book_id = match mode {
            // Attach to existing metadata: the caller must be able to see
            // the book (its owner, an admin, or via a public file).
            UploadMetadata::Attach { book_id } => {
                let _ = self
                    .get_book(&book_id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("book"))?;
                if self.files_of_book(&book_id, user).await?.is_empty() {
                    return Err(ApiError::Forbidden);
                }
                book_id
            }
            // Metadata from a named plugin source (manual picker or
            // explicit plugin reference).
            UploadMetadata::Plugin {
                source,
                book_id_in_source,
            } => {
                let book_entry = self.plugin_entry(&source, &book_id_in_source).await?;
                let book_id = self
                    .ensure_plugin_book(
                        &source,
                        Some(&user.id),
                        &book_entry,
                        &self.plugin_titles(&source, &book_id_in_source).await?,
                    )
                    .await?;
                // User-provided fields correct the plugin metadata.
                if !overrides.is_empty() {
                    self.update_book(
                        &book_id,
                        overrides.title.as_deref(),
                        overrides.description.as_deref(),
                        overrides.authors.as_ref(),
                        overrides.cover_url.as_deref(),
                    )
                    .await?;
                }
                book_id
            }
            // Auto: let plugins identify the file (first match wins),
            // otherwise create metadata parsed from the file.
            UploadMetadata::Auto => {
                let hash = sha256_hex(upload.bytes);
                let mut matched = None;
                for source in &self.sources {
                    if let Some(entry) = source.identify_upload(upload.filename, &hash).await? {
                        matched = Some((source.id().to_string(), entry));
                        break;
                    }
                }
                match matched {
                    Some((source, entry)) => {
                        let book_id = self
                            .ensure_plugin_book(
                                &source,
                                Some(&user.id),
                                &entry,
                                &self.plugin_titles(&source, &entry.id).await?,
                            )
                            .await?;
                        if !overrides.is_empty() {
                            self.update_book(
                                &book_id,
                                overrides.title.as_deref(),
                                overrides.description.as_deref(),
                                overrides.authors.as_ref(),
                                overrides.cover_url.as_deref(),
                            )
                            .await?;
                        }
                        book_id
                    }
                    None => self.create_book(&parsed, &user.id, overrides).await?,
                }
            }
        };

        let file = self
            .store_local_file(
                &parsed,
                &book_id,
                &user.id,
                upload.filename,
                upload.visibility,
                upload.label,
            )
            .await?;
        let book = self
            .get_book(&book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))?;
        Ok((book, file))
    }

    /// Create a fresh metadata entry from the parsed file, applying user
    /// overrides on top.
    async fn create_book(
        &self,
        parsed: &bookshelf_formats::ParsedBook,
        owner_id: &str,
        overrides: &MetadataOverrides,
    ) -> Result<String, ApiError> {
        let book_id = uuid::Uuid::new_v4().simple().to_string();
        let authors = match &overrides.authors {
            Some(a) => serde_json::to_string(a).unwrap_or_else(|_| "[]".into()),
            None => serde_json::to_string(&parsed.authors).unwrap_or_else(|_| "[]".into()),
        };
        sqlx::query(
            "INSERT INTO books (id, title, authors, description, cover_url, created_by) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&book_id)
        .bind(
            overrides
                .title
                .clone()
                .unwrap_or_else(|| parsed.title.clone()),
        )
        .bind(authors)
        .bind(
            overrides
                .description
                .clone()
                .or_else(|| parsed.description.clone()),
        )
        .bind(
            overrides
                .cover_url
                .clone()
                .or_else(|| parsed.cover_url.clone()),
        )
        .bind(owner_id)
        .execute(&self.db)
        .await?;
        Ok(book_id)
    }

    /// Store an uploaded file (chapters + TOC) under the given metadata
    /// entry. The file's `external_id` equals its own id for local files.
    async fn store_local_file(
        &self,
        parsed: &bookshelf_formats::ParsedBook,
        book_id: &str,
        owner_id: &str,
        filename: &str,
        visibility: Visibility,
        label: &str,
    ) -> Result<FileMeta, ApiError> {
        let file_id = uuid::Uuid::new_v4().simple().to_string();
        let format = detect_format(filename);
        let toc_json = serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());

        let mut tx = self.db.begin().await?;
        sqlx::query(
            "INSERT INTO book_files (id, book_id, source, external_id, format, label, visibility, owner_id, chapter_count, toc) \
             VALUES (?, ?, 'local', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(book_id)
        .bind(&file_id)
        .bind(format)
        .bind(label)
        .bind(visibility.as_str())
        .bind(owner_id)
        .bind(parsed.chapters.len() as i64)
        .bind(toc_json)
        .execute(&mut *tx)
        .await?;

        for (idx, chapter) in parsed.chapters.iter().enumerate() {
            sqlx::query(
                "INSERT INTO chapters (file_id, idx, title, format, content) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&file_id)
            .bind(idx as i64)
            .bind(&chapter.title)
            .bind(chapter.format.as_str())
            .bind(&chapter.content)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        self.get_file(&file_id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))
    }

    /// Fetch one entry from a plugin source's catalog.
    async fn plugin_entry(&self, source: &str, book_id: &str) -> Result<SourceBook, ApiError> {
        let source = self
            .source(source)
            .ok_or_else(|| ApiError::bad_request(format!("unknown plugin source `{source}`")))?;
        let books = source.list_books().await?;
        books
            .into_iter()
            .find(|b| b.id == book_id)
            .ok_or_else(|| ApiError::bad_request(format!("plugin does not offer book `{book_id}`")))
    }

    /// Chapter titles of a plugin book (empty when the source errors).
    async fn plugin_titles(&self, source: &str, book_id: &str) -> Result<Vec<String>, ApiError> {
        match self.source(source) {
            Some(s) => Ok(s.chapter_titles(book_id).await.unwrap_or_default()),
            None => Ok(Vec::new()),
        }
    }

    // ---- books -----------------------------------------------------------

    /// List metadata entries that own at least one file the user can see,
    /// with the visible files attached. Admins see everything.
    pub async fn list_books(
        &self,
        query: Option<&str>,
        source: Option<&str>,
        user: &User,
    ) -> Result<Vec<(BookMeta, Vec<FileMeta>)>, ApiError> {
        let pattern = format!("%{}%", query.unwrap_or("").trim());
        let is_admin = user.role == bookshelf_core::model::Role::Admin;

        let mut q = sqlx::QueryBuilder::new(
            "SELECT DISTINCT b.id, b.title, b.authors, b.description, b.cover_url, b.created_by, b.created_at \
             FROM books b JOIN book_files f ON f.book_id = b.id WHERE b.title LIKE ",
        );
        q.push_bind(pattern);
        if let Some(src) = source {
            q.push(" AND f.source = ").push_bind(src);
        }
        if is_admin {
            q.push(" ORDER BY b.created_at DESC");
        } else {
            q.push(" AND (f.visibility = 'public' OR f.owner_id = ")
                .push_bind(&user.id)
                .push(") ORDER BY b.created_at DESC");
        }
        let rows: Vec<BookRow> = q.build_query_as().fetch_all(&self.db).await?;

        let mut books = Vec::new();
        for row in rows {
            let book = row.into_model()?;
            let files = self.files_of_book(&book.id, user).await?;
            if !files.is_empty() {
                books.push((book, files));
            }
        }
        Ok(books)
    }

    pub async fn get_book(&self, id: &str) -> Result<Option<BookMeta>, ApiError> {
        let row: Option<BookRow> = sqlx::query_as(
            "SELECT id, title, authors, description, cover_url, created_by, created_at \
             FROM books WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db)
        .await?;
        row.map(BookRow::into_model).transpose()
    }

    pub async fn update_book(
        &self,
        id: &str,
        title: Option<&str>,
        description: Option<&str>,
        authors: Option<&Vec<String>>,
        cover_url: Option<&str>,
    ) -> Result<BookMeta, ApiError> {
        let authors_json =
            authors.map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".into()));
        sqlx::query(
            "UPDATE books SET title = COALESCE(?, title), description = COALESCE(?, description), \
             authors = COALESCE(?, authors), cover_url = COALESCE(?, cover_url) WHERE id = ?",
        )
        .bind(title)
        .bind(description)
        .bind(authors_json)
        .bind(cover_url)
        .bind(id)
        .execute(&self.db)
        .await?;
        self.get_book(id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))
    }

    /// Re-pull metadata from every plugin source backing this book
    /// (metadata update from plugin sources). The creator of the metadata
    /// or an admin may refresh.
    pub async fn refresh_book(&self, user: &User, book_id: &str) -> Result<BookMeta, ApiError> {
        let book = self
            .get_book(book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))?;
        let owned = book.created_by.as_deref() == Some(user.id.as_str());
        if !owned && user.role != bookshelf_core::model::Role::Admin {
            return Err(ApiError::Forbidden);
        }

        // Files of this book backed by a plugin source.
        let plugin_files: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, source, external_id FROM book_files \
             WHERE book_id = ? AND source != 'local'",
        )
        .bind(book_id)
        .fetch_all(&self.db)
        .await?;
        if plugin_files.is_empty() {
            return Err(ApiError::bad_request(
                "book metadata has no plugin source to refresh from",
            ));
        }

        for (file_id, source_id, external_id) in &plugin_files {
            let source = self.source(source_id).ok_or_else(|| {
                ApiError::bad_request(format!("plugin source `{source_id}` is not loaded"))
            })?;
            let entries = source.list_books().await?;
            let entry = entries
                .iter()
                .find(|b| &b.id == external_id)
                .ok_or_else(|| {
                    ApiError::NotFound(format!(
                        "source `{source_id}` no longer offers book `{external_id}`"
                    ))
                })?;
            let titles = source.chapter_titles(external_id).await.unwrap_or_default();

            let authors = serde_json::to_string(&entry.authors).unwrap_or_else(|_| "[]".into());
            sqlx::query(
                "UPDATE books SET title = ?, authors = ?, description = ?, cover_url = ? \
                 WHERE id = ?",
            )
            .bind(&entry.title)
            .bind(authors)
            .bind(&entry.description)
            .bind(&entry.cover_url)
            .bind(book_id)
            .execute(&self.db)
            .await?;
            sqlx::query("UPDATE book_files SET chapter_count = ? WHERE id = ?")
                .bind(titles.len() as i64)
                .bind(file_id)
                .execute(&self.db)
                .await?;

            // Refresh the chapter-title placeholder rows (never overwrite
            // materialized chapter content).
            let mut tx = self.db.begin().await?;
            for (idx, title) in titles.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO chapters (file_id, idx, title, format, content) \
                     VALUES (?, ?, ?, 'text', '') \
                     ON CONFLICT (file_id, idx) DO UPDATE SET title = excluded.title \
                     WHERE chapters.content = ''",
                )
                .bind(file_id)
                .bind(idx as i64)
                .bind(title.trim())
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            info!(book = %book_id, source = %source_id, "book metadata refreshed from plugin source");
        }

        self.get_book(book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))
    }

    /// The catalog a plugin source offers right now, each entry annotated
    /// with the metadata entry it is already synced into (if any).
    pub async fn plugin_catalog(
        &self,
        plugin_id: &str,
    ) -> Result<Vec<(SourceBook, Option<String>)>, ApiError> {
        let source = self
            .source(plugin_id)
            .ok_or_else(|| ApiError::not_found("plugin"))?;
        let books = source.list_books().await?;
        let mut out = Vec::with_capacity(books.len());
        for book in &books {
            let book_id: Option<String> = sqlx::query_scalar(
                "SELECT book_id FROM book_files WHERE source = ? AND external_id = ?",
            )
            .bind(plugin_id)
            .bind(&book.id)
            .fetch_optional(&self.db)
            .await?;
            out.push((book.clone(), book_id));
        }
        Ok(out)
    }

    /// Delete a metadata entry. File deletion is deliberately a separate
    /// operation: while the book still owns files this returns a conflict
    /// listing them, so metadata deletion never cascades silently.
    pub async fn delete_book(&self, id: &str) -> Result<(), ApiError> {
        let files: Vec<String> = sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ?")
            .bind(id)
            .fetch_all(&self.db)
            .await?;
        if !files.is_empty() {
            return Err(ApiError::ConflictWithFiles(files));
        }
        let result = sqlx::query("DELETE FROM books WHERE id = ?")
            .bind(id)
            .execute(&self.db)
            .await?;
        if result.rows_affected() == 0 {
            return Err(ApiError::not_found("book"));
        }
        Ok(())
    }

    // ---- files -----------------------------------------------------------

    /// Files of a book the user is allowed to see.
    pub async fn files_of_book(
        &self,
        book_id: &str,
        user: &User,
    ) -> Result<Vec<FileMeta>, ApiError> {
        let is_admin = user.role == bookshelf_core::model::Role::Admin;
        let mut q = sqlx::QueryBuilder::new(
            "SELECT id, book_id, source, external_id, format, label, visibility, owner_id, \
             chapter_count, created_at FROM book_files WHERE book_id = ",
        );
        q.push_bind(book_id);
        if !is_admin {
            q.push(" AND (visibility = 'public' OR owner_id = ")
                .push_bind(&user.id)
                .push(")");
        }
        q.push(" ORDER BY created_at DESC");
        let rows: Vec<FileRow> = q.build_query_as().fetch_all(&self.db).await?;
        rows.into_iter().map(FileRow::into_model).collect()
    }

    pub async fn get_file(&self, id: &str) -> Result<Option<FileMeta>, ApiError> {
        let row: Option<FileRow> = sqlx::query_as(
            "SELECT id, book_id, source, external_id, format, label, visibility, owner_id, \
             chapter_count, created_at FROM book_files WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db)
        .await?;
        row.map(FileRow::into_model).transpose()
    }

    pub async fn update_file(
        &self,
        id: &str,
        visibility: Option<Visibility>,
        label: Option<&str>,
    ) -> Result<FileMeta, ApiError> {
        sqlx::query(
            "UPDATE book_files SET visibility = COALESCE(?, visibility), label = COALESCE(?, label) WHERE id = ?",
        )
        .bind(visibility.map(|v| v.as_str()))
        .bind(label)
        .bind(id)
        .execute(&self.db)
        .await?;
        self.get_file(id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))
    }

    pub async fn delete_file(&self, id: &str) -> Result<(), ApiError> {
        let result = sqlx::query("DELETE FROM book_files WHERE id = ?")
            .bind(id)
            .execute(&self.db)
            .await?;
        if result.rows_affected() == 0 {
            return Err(ApiError::not_found("file"));
        }
        Ok(())
    }

    // ---- chapters --------------------------------------------------------

    pub async fn chapter_titles(&self, file_id: &str) -> Result<Vec<ChapterMeta>, ApiError> {
        let rows: Vec<ChapterTitleRow> =
            sqlx::query_as("SELECT idx, title FROM chapters WHERE file_id = ? ORDER BY idx")
                .bind(file_id)
                .fetch_all(&self.db)
                .await?;
        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    /// The file's hierarchical TOC. Files stored without one (plugin
    /// books, legacy uploads) get a flat tree synthesized from the chapter
    /// title rows.
    pub async fn file_toc(&self, file_id: &str) -> Result<Vec<TocNode>, ApiError> {
        let raw: String = sqlx::query_scalar("SELECT toc FROM book_files WHERE id = ?")
            .bind(file_id)
            .fetch_one(&self.db)
            .await?;
        if !raw.trim().is_empty() {
            if let Ok(toc) = serde_json::from_str::<Vec<TocNode>>(&raw) {
                return Ok(toc);
            }
        }
        let rows: Vec<ChapterTitleRow> =
            sqlx::query_as("SELECT idx, title FROM chapters WHERE file_id = ? ORDER BY idx")
                .bind(file_id)
                .fetch_all(&self.db)
                .await?;
        Ok(rows
            .into_iter()
            .map(|r| TocNode {
                title: r.title,
                idx: Some(r.idx.max(0) as u32),
                frag: None,
                children: Vec::new(),
            })
            .collect())
    }

    /// Fetch a chapter, materializing it from its plugin source on first
    /// access.
    pub async fn get_chapter(
        &self,
        file: &FileMeta,
        idx: u32,
    ) -> Result<Option<Chapter>, ApiError> {
        if idx as i64 >= file.chapter_count as i64 {
            return Ok(None);
        }
        let row: Option<ChapterRow> = sqlx::query_as(
            "SELECT idx, title, format, content FROM chapters WHERE file_id = ? AND idx = ?",
        )
        .bind(&file.id)
        .bind(idx as i64)
        .fetch_optional(&self.db)
        .await?;

        if let Some(row) = row {
            // Title-only placeholder rows are materialized by
            // `ensure_titles`; fetch the real content lazily.
            if !row.content.is_empty() {
                return Ok(Some(row.into_model()));
            }
        }

        // Not stored yet (or placeholder): lazily materialize from the
        // plugin source.
        let Some(source) = self.source(&file.source) else {
            return Ok(None);
        };
        self.materialize_from_source(source, &file.external_id, &file.id, idx)
            .await
    }

    async fn materialize_from_source(
        &self,
        source: Arc<dyn BookSource>,
        external_id: &str,
        file_id: &str,
        idx: u32,
    ) -> Result<Option<Chapter>, ApiError> {
        let Some(chapter) = source.get_chapter(external_id, idx).await? else {
            return Ok(None);
        };
        sqlx::query(
            "INSERT INTO chapters (file_id, idx, title, format, content) VALUES (?, ?, ?, 'text', ?) \
             ON CONFLICT (file_id, idx) DO UPDATE SET title = excluded.title, content = excluded.content",
        )
        .bind(file_id)
        .bind(idx as i64)
        .bind(&chapter.title)
        .bind(&chapter.content)
        .execute(&self.db)
        .await?;
        Ok(Some(Chapter {
            idx,
            title: chapter.title,
            format: ChapterFormat::Text,
            content: chapter.content,
        }))
    }

    /// Ensure the chapter title rows of a plugin file exist (so that
    /// `chapter_titles` works for books never read yet).
    pub async fn ensure_titles(&self, file: &FileMeta) -> Result<(), ApiError> {
        if file.source == "local" || file.chapter_count == 0 {
            return Ok(());
        }
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapters WHERE file_id = ?")
            .bind(&file.id)
            .fetch_one(&self.db)
            .await?;
        if stored > 0 {
            return Ok(());
        }
        if let Some(source) = self.source(&file.source) {
            let titles: CoreResult<Vec<String>> = source.chapter_titles(&file.external_id).await;
            if let Ok(titles) = titles {
                let mut tx = self.db.begin().await?;
                for (idx, title) in titles.into_iter().enumerate() {
                    sqlx::query(
                        "INSERT OR IGNORE INTO chapters (file_id, idx, title, format, content) \
                         VALUES (?, ?, ?, 'text', '')",
                    )
                    .bind(&file.id)
                    .bind(idx as i64)
                    .bind(title.trim())
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
            }
        }
        Ok(())
    }
}

fn detect_format(filename: &str) -> &'static str {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".epub") {
        "epub"
    } else {
        "txt"
    }
}

/// Lowercase hex sha-256 digest, passed to plugins for upload
/// identification.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}
