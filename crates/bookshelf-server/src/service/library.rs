//! The library service: central storage for book metadata, files and
//! chapters.
//!
//! Two-level model: `books` are pure metadata entries, `book_files` are
//! the actual books (uploads or virtual plugin books). Files from every
//! source are catalogued here; plugin chapters are materialized into the
//! same tables on first access, so readers see one unified store.
//!
//! Plugin system v2: sources are *instances* registered in
//! `plugin_instances` (see [`PluginService`]). Catalog access is lazy —
//! `declare`-capable instances sync their static catalog at startup,
//! `search`-capable instances are browsed on demand and single books are
//! materialized via `get-book`. A file's chapters may come from a
//! different instance than its metadata (`content_source` /
//! `content_external_id`, metadata/content separation).
//!
//! Visibility is per file: `private` = owner + admins only, `public` =
//! visible to every logged-in user.

use std::sync::Arc;

use sqlx::SqlitePool;
use tracing::{info, warn};

use bookshelf_core::model::{
    BookMeta, Chapter, ChapterFormat, ChapterMeta, FileMeta, TocNode, User, Visibility,
};
use bookshelf_core::source::{SourceBook, SourceBookFile, SourceChapter};

use crate::error::ApiError;
use crate::rows::{BookRow, ChapterRow, ChapterTitleRow, FileRow};
use crate::service::plugins::{DeclaredCatalog, PluginService};

/// Where the metadata of an upload comes from.
pub enum UploadMetadata {
    /// Attach the file to an existing metadata entry (its metadata wins).
    Attach { book_id: String },
    /// Metadata taken from a plugin source by id. `book_id_in_source`
    /// must be offered by that source's `get-book`.
    Plugin {
        source: String,
        book_id_in_source: String,
    },
    /// Ask every enabled plugin instance to identify the file (first
    /// match wins), falling back to metadata parsed from the file itself.
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
    plugins: Arc<PluginService>,
}

impl Library {
    pub fn new(db: SqlitePool, plugins: Arc<PluginService>) -> Self {
        Library { db, plugins }
    }

    /// The plugin instance service (instance management, config, calls).
    pub fn plugins(&self) -> &PluginService {
        &self.plugins
    }

    // ---- plugin sync -----------------------------------------------------

    /// Re-catalogue every enabled `declare`-capable instance into the
    /// library. Returns the number of books (re-)synced.
    pub async fn sync_plugins(&self) -> Result<usize, ApiError> {
        let mut total = 0;
        for catalog in self.plugins.declared_catalogs().await? {
            total += self.sync_catalog(&catalog).await?;
        }
        Ok(total)
    }

    /// Re-catalogue one instance (used after config changes / enable).
    pub async fn sync_instance(&self, id: &str) -> Result<usize, ApiError> {
        match self.plugins.declared_catalog(id).await? {
            Some(catalog) => self.sync_catalog(&catalog).await,
            None => Ok(0),
        }
    }

    async fn sync_catalog(&self, catalog: &DeclaredCatalog) -> Result<usize, ApiError> {
        let mut total = 0;
        for book in &catalog.books {
            let titles: Vec<String> = book.chapters.iter().map(|c| c.title.clone()).collect();
            let (book_id, file_id) = self
                .ensure_plugin_book(&catalog.instance, None, &book.book, &titles)
                .await?;
            // Eager bodies: only when this instance also provides the
            // content (a `content-source` pointer defers to the target).
            if book.book.content_source.is_none() {
                self.upsert_chapters(&file_id, &book.chapters).await?;
            }
            info!(
                source = %catalog.instance,
                book = %book_id,
                chapters = titles.len(),
                "declared book synced"
            );
            total += 1;
        }
        Ok(total)
    }

    // ---- plugin catalog / materialization ----------------------------------

    /// Search one instance's catalog (design R4: paginated, never fully
    /// enumerated server-side). Each entry is annotated with the library
    /// metadata entry it is already synced into (when it is).
    pub async fn search_plugin(
        &self,
        instance: &str,
        query: &str,
        offset: u32,
        limit: u32,
    ) -> Result<(u64, Vec<(SourceBook, Option<String>)>), ApiError> {
        let result = self
            .plugins
            .search_books(instance, query, offset, limit)
            .await?;
        let mut items = Vec::with_capacity(result.items.len());
        for entry in result.items {
            let book = SourceBook::from(entry);
            let book_id: Option<String> = sqlx::query_scalar(
                "SELECT book_id FROM book_files WHERE source = ? AND external_id = ?",
            )
            .bind(instance)
            .bind(&book.id)
            .fetch_optional(&self.db)
            .await?;
            items.push((book, book_id));
        }
        Ok((result.total, items))
    }

    /// Materialize exactly one book of a searchable source
    /// (`get-book` + chapter-title placeholders; bodies stay lazy until
    /// first read). The caller claims the metadata (becomes its creator),
    /// like an upload identified as this plugin book.
    ///
    /// v3 acquisition modes (docs/plugin-http-api-design.md §4.2): an
    /// instance declaring `book-file` is materialized in **file mode**
    /// first — the whole file is fetched via `get-book-file` and run
    /// through the upload parser (chapters + hierarchical TOC + sanitized
    /// HTML, stored like a local upload). Chapter mode (titles + lazy
    /// bodies) is the fallback when the source has no file.
    pub async fn materialize_plugin_book(
        &self,
        user: &User,
        instance: &str,
        book_id_in_source: &str,
    ) -> Result<(BookMeta, FileMeta), ApiError> {
        let entry = self.plugins.get_book(instance, book_id_in_source).await?;
        let entry = entry.ok_or_else(|| {
            ApiError::NotFound(format!(
                "source `{instance}` does not offer book `{book_id_in_source}`"
            ))
        })?;
        let entry = SourceBook::from(entry);

        // File mode first: a declared `book-file` capability means the
        // source offers whole files; we fall back to chapter mode when
        // `get-book-file` returns none/errors.
        if self.plugins.declares(instance, "book-file").await?
            && let Some(book_file) = self.plugins.get_book_file(instance, &entry.id).await?
        {
            return self
                .materialize_file_mode(user, instance, &entry, &book_file)
                .await;
        }

        // Chapter mode: titles (and bodies) come from the *content*
        // instance.
        let (content_inst, content_id) = self.plugins.content_target(instance, &entry);
        let titles = self
            .plugins
            .chapter_titles(content_inst, content_id)
            .await?;
        let (book_id, _) = self
            .ensure_plugin_book(instance, Some(&user.id), &entry, &titles)
            .await?;
        let file = self
            .plugin_file(instance, &entry.id)
            .await?
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("plugin file vanished")))?;
        // Title placeholders; bodies stay lazy (first read).
        self.ensure_titles(&file).await?;
        let book = self
            .get_book(&book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))?;
        Ok((book, file))
    }

    /// File-mode materialization: store a `get-book-file` result like a
    /// local upload (format `epub`/`txt` from the file name, chapters +
    /// hierarchical TOC + sanitized HTML). The metadata comes from the
    /// source's `book-entry`; afterwards the DB is the source of truth
    /// and the plugin is not consulted for content.
    async fn materialize_file_mode(
        &self,
        user: &User,
        source: &str,
        entry: &SourceBook,
        book_file: &SourceBookFile,
    ) -> Result<(BookMeta, FileMeta), ApiError> {
        let parsed = bookshelf_formats::parse(&book_file.bytes, &book_file.filename)?;
        let (book_id, file_id) = self
            .ensure_plugin_book(source, Some(&user.id), entry, &[])
            .await?;
        let format = detect_format(&book_file.filename);
        let toc_json = serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());
        let label = format!("{source} 下载");

        // Rewrite the virtual row into a stored file: self-contained
        // (no content indirection), real format/toc/chapter count.
        sqlx::query(
            "UPDATE book_files SET format = ?, label = ?, toc = ?, content_source = NULL, \
             content_external_id = NULL, chapter_count = ? WHERE id = ?",
        )
        .bind(format)
        .bind(&label)
        .bind(toc_json)
        .bind(parsed.chapters.len() as i64)
        .bind(&file_id)
        .execute(&self.db)
        .await?;
        sqlx::query("DELETE FROM chapters WHERE file_id = ?")
            .bind(&file_id)
            .execute(&self.db)
            .await?;
        for (idx, chapter) in parsed.chapters.iter().enumerate() {
            sqlx::query(
                "INSERT INTO chapters (file_id, idx, title, format, content) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&file_id)
            .bind(idx as i64)
            .bind(&chapter.title)
            .bind(chapter.format.as_str())
            .bind(&chapter.content)
            .execute(&self.db)
            .await?;
        }
        info!(
            source = %source,
            book = %book_id,
            file = %file_id,
            format,
            chapters = parsed.chapters.len(),
            "plugin book materialized in file mode (upload parser)"
        );

        let file = self
            .get_file(&file_id)
            .await?
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("plugin file vanished")))?;
        let book = self
            .get_book(&book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))?;
        Ok((book, file))
    }

    /// Create/update the metadata entry and virtual file of a plugin book.
    /// Chapters/sessions don't exist yet; `titles` only fixes the
    /// `chapter_count` (placeholder rows are ensured separately).
    ///
    /// `owner` claims the metadata when it has no creator yet (uploads
    /// identified as this plugin book, or a user picking the book from
    /// the source browser) and becomes the owner of a newly created
    /// virtual file.
    async fn ensure_plugin_book(
        &self,
        source_id: &str,
        owner: Option<&str>,
        book: &SourceBook,
        titles: &[String],
    ) -> Result<(String, String), ApiError> {
        let authors = serde_json::to_string(&book.authors).unwrap_or_else(|_| "[]".into());

        // Reuse the metadata entry when this plugin file was synced
        // before, otherwise create both the book metadata and its virtual
        // file.
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
                    "INSERT INTO book_files (id, book_id, source, external_id, content_source, content_external_id, format, label, visibility, owner_id, chapter_count) \
                     VALUES (?, ?, ?, ?, ?, ?, 'plugin', '', 'public', ?, ?)",
                )
                .bind(uuid::Uuid::new_v4().simple().to_string())
                .bind(&book_id)
                .bind(source_id)
                .bind(&book.id)
                .bind(&book.content_source)
                .bind(&book.content_id)
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
        // Content indirection comes from the entry; the metadata instance
        // owns the `books` row, the content instance the chapters.
        sqlx::query(
            "UPDATE book_files SET chapter_count = ?, content_source = ?, content_external_id = ? \
             WHERE source = ? AND external_id = ?",
        )
        .bind(titles.len() as i64)
        .bind(&book.content_source)
        .bind(&book.content_id)
        .bind(source_id)
        .bind(&book.id)
        .execute(&self.db)
        .await?;

        let file_id: String =
            sqlx::query_scalar("SELECT id FROM book_files WHERE source = ? AND external_id = ?")
                .bind(source_id)
                .bind(&book.id)
                .fetch_one(&self.db)
                .await?;
        Ok((book_id, file_id))
    }

    /// Store declared chapter rows (title + body, eager for non-empty
    /// content). Bodies of chapters that are declared empty stay lazy.
    async fn upsert_chapters(
        &self,
        file_id: &str,
        chapters: &[SourceChapter],
    ) -> Result<(), ApiError> {
        let mut tx = self.db.begin().await?;
        for (idx, chapter) in chapters.iter().enumerate() {
            sqlx::query(
                "INSERT INTO chapters (file_id, idx, title, format, content) VALUES (?, ?, ?, 'text', ?) \
                 ON CONFLICT (file_id, idx) DO UPDATE SET \
                   title = excluded.title, \
                   content = CASE WHEN excluded.content = '' THEN chapters.content ELSE excluded.content END",
            )
            .bind(file_id)
            .bind(idx as i64)
            .bind(chapter.title.trim())
            .bind(&chapter.content)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The virtual file row of a plugin book.
    async fn plugin_file(
        &self,
        source: &str,
        external_id: &str,
    ) -> Result<Option<FileMeta>, ApiError> {
        let row: Option<FileRow> = sqlx::query_as(
            "SELECT id, book_id, source, external_id, content_source, content_external_id, format, \
             label, visibility, owner_id, chapter_count, created_at \
             FROM book_files WHERE source = ? AND external_id = ?",
        )
        .bind(source)
        .bind(external_id)
        .fetch_optional(&self.db)
        .await?;
        row.map(FileRow::into_model).transpose()
    }

    // ---- uploads ---------------------------------------------------------

    /// Ingest an uploaded epub/txt file. The metadata comes from one of
    /// [`UploadMetadata`]'s modes (`attach`/`plugin`/`auto`), possibly
    /// with user `overrides` on top of whatever the file parser or
    /// plugin produced. Returns the metadata entry and the stored file.
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
            // explicit plugin reference), resolved via `get-book`.
            UploadMetadata::Plugin {
                source,
                book_id_in_source,
            } => {
                let book_entry = self.plugin_book_entry(&source, &book_id_in_source).await?;
                let (content_inst, content_id) = self.plugins.content_target(&source, &book_entry);
                let titles = self
                    .plugins
                    .chapter_titles(content_inst, content_id)
                    .await?;
                let (book_id, _) = self
                    .ensure_plugin_book(&source, Some(&user.id), &book_entry, &titles)
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
                match self.plugins.identify_upload(upload.filename, &hash).await? {
                    Some((source, entry)) => {
                        let (content_inst, content_id) =
                            self.plugins.content_target(&source, &entry);
                        let titles = self
                            .plugins
                            .chapter_titles(content_inst, content_id)
                            .await?;
                        let (book_id, _) = self
                            .ensure_plugin_book(&source, Some(&user.id), &entry, &titles)
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

    /// Fetch one entry from a plugin source by id (`lookup` capability).
    async fn plugin_book_entry(&self, source: &str, book_id: &str) -> Result<SourceBook, ApiError> {
        let entry = self.plugins.get_book(source, book_id).await?;
        entry
            .map(SourceBook::from)
            .ok_or_else(|| ApiError::bad_request(format!("plugin does not offer book `{book_id}`")))
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

    /// Re-pull metadata from the plugin source(s) backing this book via
    /// `get-book` (design: refresh never scans a catalog). Title
    /// placeholders come from the *content* instance; materialized
    /// chapter bodies are never overwritten. The creator of the metadata
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
            let entry = self.plugins.get_book(source_id, external_id).await?;
            let entry = entry.ok_or_else(|| {
                ApiError::NotFound(format!(
                    "source `{source_id}` no longer offers book `{external_id}`"
                ))
            })?;
            let entry = SourceBook::from(entry);

            // Metadata always from the metadata instance; titles from the
            // content instance.
            let (content_inst, content_id) = self.plugins.content_target(source_id, &entry);
            let titles = self
                .plugins
                .chapter_titles(content_inst, content_id)
                .await
                .unwrap_or_else(|e| {
                    warn!(source = %content_inst, book = %content_id, "chapter titles unavailable: {e}");
                    Vec::new()
                });

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
            sqlx::query(
                "UPDATE book_files SET chapter_count = ?, content_source = ?, content_external_id = ? \
                 WHERE id = ?",
            )
            .bind(titles.len() as i64)
            .bind(&entry.content_source)
            .bind(&entry.content_id)
            .bind(file_id)
            .execute(&self.db)
            .await?;

            // Refresh the chapter-title placeholder rows (never overwrite
            // materialized chapter content).
            self.upsert_placeholder_titles(file_id, &titles).await?;
            info!(book = %book_id, source = %source_id, "book metadata refreshed from plugin source");
        }

        self.get_book(book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))
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
            "SELECT id, book_id, source, external_id, content_source, content_external_id, format, label, visibility, owner_id, \
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
            "SELECT id, book_id, source, external_id, content_source, content_external_id, format, label, visibility, owner_id, \
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

    /// Rebind a file's chapter source (metadata/content separation). The
    /// target instance must exist; when it has the `lookup` capability
    /// the book must resolve via `get-book`. Materialized chapter bodies
    /// are cleared so the next read re-materializes from the new source.
    pub async fn set_content_source(
        &self,
        file_id: &str,
        content_source: &str,
        content_external_id: Option<String>,
    ) -> Result<FileMeta, ApiError> {
        let file = self
            .get_file(file_id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))?;
        let content_external_id = content_external_id.unwrap_or_else(|| file.external_id.clone());
        self.plugins
            .check_content_target(content_source, &content_external_id)
            .await?;
        sqlx::query(
            "UPDATE book_files SET content_source = ?, content_external_id = ? WHERE id = ?",
        )
        .bind(content_source)
        .bind(&content_external_id)
        .bind(file_id)
        .execute(&self.db)
        .await?;
        // Bodies from the previous source must not leak through; keep the
        // title rows as placeholders for the new source.
        sqlx::query("UPDATE chapters SET content = '' WHERE file_id = ?")
            .bind(file_id)
            .execute(&self.db)
            .await?;
        let file = self
            .get_file(file_id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))?;
        self.ensure_titles(&file).await?;
        Ok(file)
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
        if !raw.trim().is_empty()
            && let Ok(toc) = serde_json::from_str::<Vec<TocNode>>(&raw)
        {
            return Ok(toc);
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

    /// Fetch a chapter, materializing it from its content source on first
    /// access (or after a rebind cleared the bodies).
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
        // file's content source.
        let (source_id, external_id) = file_content_target(file);
        if source_id == "local" {
            return Ok(None);
        }

        // v3 first-access file mode: a virtual row whose source offers
        // whole book files is materialized *as a file* on first read
        // (once; the DB becomes the source of truth afterwards). The
        // upload parser gives it a real TOC + sanitized HTML for free.
        if file.format == "plugin" && file.content_source.is_none() {
            let entry = self
                .plugins
                .get_book(source_id, external_id)
                .await
                .map_err(|e| warn!(source = %source_id, book = %external_id, "file-mode metadata unavailable: {e}"))
                .ok()
                .flatten()
                .map(SourceBook::from);
            if self.plugins.declares(source_id, "book-file").await?
                && let Some(book_file) = self.plugins.get_book_file(source_id, external_id).await?
            {
                if let (Some(entry), Ok(parsed)) = (
                    entry.as_ref(),
                    bookshelf_formats::parse(&book_file.bytes, &book_file.filename),
                ) {
                    // Rewrite this row like `materialize_file_mode`
                    // (metadata stays; content becomes self-contained).
                    let toc_json =
                        serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());
                    let format = detect_format(&book_file.filename);
                    let label = format!("{source_id} 下载");
                    sqlx::query(
                        "UPDATE book_files SET format = ?, label = ?, toc = ?, \
                             content_source = NULL, content_external_id = NULL, \
                             chapter_count = ? WHERE id = ?",
                    )
                    .bind(format)
                    .bind(&label)
                    .bind(toc_json)
                    .bind(parsed.chapters.len() as i64)
                    .bind(&file.id)
                    .execute(&self.db)
                    .await?;
                    sqlx::query("DELETE FROM chapters WHERE file_id = ?")
                        .bind(&file.id)
                        .execute(&self.db)
                        .await?;
                    for (i, chapter) in parsed.chapters.iter().enumerate() {
                        sqlx::query(
                            "INSERT INTO chapters (file_id, idx, title, format, content) \
                                 VALUES (?, ?, ?, ?, ?)",
                        )
                        .bind(&file.id)
                        .bind(i as i64)
                        .bind(&chapter.title)
                        .bind(chapter.format.as_str())
                        .bind(&chapter.content)
                        .execute(&self.db)
                        .await?;
                    }
                    // Keep the metadata fresh from the source entry.
                    let authors =
                        serde_json::to_string(&entry.authors).unwrap_or_else(|_| "[]".into());
                    sqlx::query(
                        "UPDATE books SET title = ?, authors = ?, description = ?, cover_url = ? \
                             WHERE id = ?",
                    )
                    .bind(&entry.title)
                    .bind(&authors)
                    .bind(&entry.description)
                    .bind(&entry.cover_url)
                    .bind(&file.book_id)
                    .execute(&self.db)
                    .await?;
                    info!(
                        source = %source_id,
                        book = %file.book_id,
                        file = %file.id,
                        format,
                        chapters = parsed.chapters.len(),
                        "plugin book materialized in file mode on first access"
                    );
                } else {
                    warn!(
                        source = %source_id,
                        book = %external_id,
                        "could not materialize book file (metadata or parse); \
                         falling back to chapter mode"
                    );
                }
            }
        }

        // Re-read after a possible file-mode materialization.
        if let Some(row) = sqlx::query_as::<_, ChapterRow>(
            "SELECT idx, title, format, content FROM chapters WHERE file_id = ? AND idx = ?",
        )
        .bind(&file.id)
        .bind(idx as i64)
        .fetch_optional(&self.db)
        .await?
            && !row.content.is_empty()
        {
            return Ok(Some(row.into_model()));
        }

        // Chapter mode (lazy per-chapter pulls from the content source).
        let Some(chapter) = self
            .plugins
            .get_chapter(source_id, external_id, idx)
            .await?
        else {
            return Ok(None);
        };
        sqlx::query(
            "INSERT INTO chapters (file_id, idx, title, format, content) VALUES (?, ?, ?, 'text', ?) \
             ON CONFLICT (file_id, idx) DO UPDATE SET title = excluded.title, content = excluded.content",
        )
        .bind(&file.id)
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

    /// Upsert title rows, never overwriting materialized bodies.
    async fn upsert_placeholder_titles(
        &self,
        file_id: &str,
        titles: &[String],
    ) -> Result<(), ApiError> {
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
        Ok(())
    }

    /// Ensure the chapter title rows of a plugin file exist (so that
    /// `chapter_titles` works for books never read yet). Titles come from
    /// the file's *content* instance.
    pub async fn ensure_titles(&self, file: &FileMeta) -> Result<(), ApiError> {
        if file.chapter_count == 0 {
            return Ok(());
        }
        let (source_id, external_id) = file_content_target(file);
        if source_id == "local" {
            return Ok(());
        }
        let titles = self
            .plugins
            .chapter_titles(source_id, external_id)
            .await
            .unwrap_or_else(|e| {
                warn!(source = %source_id, book = %external_id, "chapter titles unavailable: {e}");
                Vec::new()
            });
        if titles.is_empty() {
            return Ok(());
        }
        self.upsert_placeholder_titles(&file.id, &titles).await?;
        // Placeholder rows beyond the new title set are stale (previous
        // content source, or a chapter count that shrank); drop the
        // empty ones. Materialized bodies stay untouched.
        sqlx::query("DELETE FROM chapters WHERE file_id = ? AND content = '' AND idx >= ?")
            .bind(&file.id)
            .bind(titles.len() as i64)
            .execute(&self.db)
            .await?;
        if titles.len() != file.chapter_count as usize {
            sqlx::query("UPDATE book_files SET chapter_count = ? WHERE id = ?")
                .bind(titles.len() as i64)
                .bind(&file.id)
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }
}

/// Effective (instance, book id) for chapter reads of a file: the file's
/// content columns when set, otherwise its own source/external id.
fn file_content_target(file: &FileMeta) -> (&str, &str) {
    (
        file.content_source.as_deref().unwrap_or(&file.source),
        file.content_external_id
            .as_deref()
            .unwrap_or(&file.external_id),
    )
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
