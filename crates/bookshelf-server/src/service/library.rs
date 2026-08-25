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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use sqlx::SqlitePool;
use tracing::{info, warn};

use bookshelf_core::model::{
    BookMeta, Chapter, ChapterMeta, FileMeta, Role, SeriesMeta, TocNode, User, Visibility,
};
use bookshelf_core::source::{SourceBook, SourceBookFile, SourceChapter, SourceVolume};
use bookshelf_formats::ParsedBook;

use crate::error::ApiError;
use crate::rows::{BookRow, ChapterRow, ChapterTitleRow, FileRow, SeriesRow};
use crate::service::plugins::{DeclaredCatalog, PluginService};

/// Where the *content* of a library addition comes from. Every addition
/// to the library is "acquire content, then attach metadata": this is the
/// content half, orthogonal to how the metadata entry is resolved (see
/// [`AcquireMetadata`]).
pub enum AcquireContent<'a> {
    /// An uploaded local file (bytes in memory).
    File { bytes: &'a [u8], filename: &'a str },
    /// A book offered by a plugin source. Whole-book files are fetched
    /// via `get-book-file` and run through the upload parser (file mode,
    /// `book-file` capability); otherwise a virtual chapter-mode file is
    /// created whose bodies materialize lazily on first read.
    Plugin {
        source: &'a str,
        book_id_in_source: &'a str,
    },
}

/// How the metadata entry of a library addition is resolved (the
/// metadata half; orthogonal to the content source).
pub enum AcquireMetadata<'a> {
    /// New metadata entry. For local files the entry is parsed from the
    /// file (plugins may identify it first via `identify-upload`); for
    /// plugin content it is the plugin's own `book-entry`. `overrides`
    /// correct the produced metadata.
    New { overrides: &'a MetadataOverrides },
    /// Metadata from a named plugin source's `get-book` (e.g. a plugin
    /// catalog picker). The content may be an uploaded file (the classic
    /// "metadata from a plugin, file from disk" mode) or the same
    /// plugin's book.
    Plugin {
        source: &'a str,
        book_id_in_source: &'a str,
        overrides: &'a MetadataOverrides,
    },
    /// Attach to an existing metadata entry. The existing metadata wins;
    /// `overrides` are ignored.
    Attach { book_id: &'a str },
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

/// Result of one acquisition: the series created by a volume split (when
/// the source declared >1 volumes) plus the books that entered the
/// library, each with the file stored under it.
#[derive(Debug, Clone)]
pub struct AcquireOutcome {
    pub series: Option<SeriesMeta>,
    /// Non-empty; one element unless the acquisition split volumes.
    pub books: Vec<(BookMeta, FileMeta)>,
}

pub struct Library {
    db: SqlitePool,
    plugins: Arc<PluginService>,
    /// Directory for retained original file bytes
    /// (`data/files/{file_id}.{ext}`; see docs/storage-unification-design.md).
    files_dir: PathBuf,
}

impl Library {
    pub fn new(db: SqlitePool, plugins: Arc<PluginService>, files_dir: PathBuf) -> Self {
        Library {
            db,
            plugins,
            files_dir,
        }
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

    /// Materialize exactly one book of a plugin source into the library
    /// (convenience used by `POST /api/plugins/{id}/books`; equivalent to
    /// the unified acquisition endpoint with plugin content + new
    /// metadata, public visibility).
    pub async fn materialize_plugin_book(
        &self,
        user: &User,
        instance: &str,
        book_id_in_source: &str,
    ) -> Result<(BookMeta, FileMeta), ApiError> {
        let outcome = self
            .acquire_book(
                user,
                AcquireContent::Plugin {
                    source: instance,
                    book_id_in_source,
                },
                AcquireMetadata::New {
                    overrides: &MetadataOverrides::default(),
                },
                Visibility::Public,
                "",
            )
            .await?;
        outcome
            .books
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("acquisition produced no books")))
    }

    /// File-mode materialization: store a `get-book-file` result like a
    /// local upload (format `epub`/`txt` from the file name, chapters +
    /// hierarchical TOC + sanitized HTML). The metadata comes from the
    /// source's `book-entry`; afterwards the DB is the source of truth
    /// and the plugin is not consulted for content.
    async fn materialize_file_mode(
        &self,
        source: &str,
        entry: &SourceBook,
        book_file: &SourceBookFile,
        book_id: &str,
    ) -> Result<FileMeta, ApiError> {
        let mut parsed = bookshelf_formats::parse(&book_file.bytes, &book_file.filename)?;
        ingest_parsed_book(&self.db, &self.files_dir, &mut parsed).await?;
        // The virtual row for (source, external_id) under `book_id` was
        // created by `ensure_plugin_book` / `attach_plugin_file`; rewrite
        // it into a stored file: self-contained (no content indirection),
        // real format/toc/chapter count.
        let file_id: String =
            sqlx::query_scalar("SELECT id FROM book_files WHERE source = ? AND external_id = ?")
                .bind(source)
                .bind(&entry.id)
                .fetch_one(&self.db)
                .await?;
        let format = detect_format(&book_file.filename);
        let toc_json = serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());
        let label = format!("{source} 下载");
        // Cache the fetched bytes as the file's original: the source may
        // disappear later; download/reparse should not depend on it.
        let (orig_ext, orig_sha256, orig_size) = self
            .store_original(&file_id, format, &book_file.bytes)
            .await?;

        // Rewrite the virtual row into a stored file: self-contained
        // (no content indirection), real format/toc/chapter count.
        sqlx::query(
            "UPDATE book_files SET format = ?, label = ?, toc = ?, content_source = NULL, \
             content_external_id = NULL, chapter_count = ?, orig_ext = ?, orig_sha256 = ?, orig_size = ? \
             WHERE id = ?",
        )
        .bind(format)
        .bind(&label)
        .bind(toc_json)
        .bind(parsed.chapters.len() as i64)
        .bind(&orig_ext)
        .bind(&orig_sha256)
        .bind(orig_size)
        .bind(&file_id)
        .execute(&self.db)
        .await?;
        sqlx::query("DELETE FROM chapters WHERE file_id = ?")
            .bind(&file_id)
            .execute(&self.db)
            .await?;
        for (idx, chapter) in parsed.chapters.iter().enumerate() {
            sqlx::query("INSERT INTO chapters (file_id, idx, title, content) VALUES (?, ?, ?, ?)")
                .bind(&file_id)
                .bind(idx as i64)
                .bind(&chapter.title)
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

        self.get_file(&file_id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))
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
            let content = bookshelf_formats::htmlize::plugin_text_to_html(&chapter.content);
            sqlx::query(
                "INSERT INTO chapters (file_id, idx, title, content) VALUES (?, ?, ?, ?) \
                 ON CONFLICT (file_id, idx) DO UPDATE SET \
                   title = excluded.title, \
                   content = CASE WHEN excluded.content = '' THEN chapters.content ELSE excluded.content END",
            )
            .bind(file_id)
            .bind(idx as i64)
            .bind(chapter.title.trim())
            .bind(&content)
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
             label, visibility, owner_id, chapter_count, created_at, volume_no, volume_offset, \
             orig_ext, orig_sha256, orig_size \
             FROM book_files WHERE source = ? AND external_id = ?",
        )
        .bind(source)
        .bind(external_id)
        .fetch_optional(&self.db)
        .await?;
        row.map(FileRow::into_model).transpose()
    }

    // ---- acquisition (获取书籍 → 添加元数据) --------------------------------

    /// Add a book to the library: one unified flow for every content
    /// source. [`AcquireContent`] is where the book's content comes from
    /// (uploaded file or plugin source), [`AcquireMetadata`] how the
    /// metadata entry is resolved (new / from a plugin / attach to
    /// existing). Storage options (`visibility`, `label`) apply to both.
    ///
    /// When a plugin source declares >1 volumes for its book, the
    /// acquisition auto-splits into one series + one book per volume
    /// (see [`Self::try_split_acquire`]); otherwise a single book is
    /// created/attached, exactly as before.
    pub async fn acquire_book(
        &self,
        user: &User,
        content: AcquireContent<'_>,
        metadata: AcquireMetadata<'_>,
        visibility: Visibility,
        label: &str,
    ) -> Result<AcquireOutcome, ApiError> {
        // Multi-volume plugin acquisition: the source declares the book
        // spans several 卷 — split into a series + one book per volume.
        if let AcquireContent::Plugin {
            source,
            book_id_in_source,
        } = &content
            && let AcquireMetadata::New { .. } | AcquireMetadata::Plugin { .. } = &metadata
            && let Some(outcome) = self
                .try_split_acquire(
                    user,
                    source,
                    book_id_in_source,
                    &metadata,
                    visibility,
                    label,
                )
                .await?
        {
            return Ok(outcome);
        }

        let (book_id, file) = match (content, metadata) {
            // ---- attach an uploaded file to existing metadata -----------
            (AcquireContent::File { bytes, filename }, AcquireMetadata::Attach { book_id }) => {
                let mut parsed = bookshelf_formats::parse(bytes, filename)?;
                ingest_parsed_book(&self.db, &self.files_dir, &mut parsed).await?;
                // The caller must be able to see the book (its owner, an
                // admin, or via a public file).
                let _ = self
                    .get_book(book_id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("book"))?;
                if self.files_of_book(book_id, user).await?.is_empty() {
                    return Err(ApiError::Forbidden);
                }
                // The attached edition may be the first one carrying a
                // real cover — fill it in when the entry has none yet.
                if let Some(cover) = &parsed.cover {
                    self.store_cover_if_missing(book_id, cover).await?;
                }
                let file = self
                    .store_local_file(
                        &parsed, bytes, book_id, &user.id, filename, visibility, label,
                    )
                    .await?;
                (book_id.to_string(), file)
            }

            // ---- attach a plugin book's content to existing metadata ---
            (
                AcquireContent::Plugin {
                    source,
                    book_id_in_source,
                },
                AcquireMetadata::Attach { book_id },
            ) => {
                let _ = self
                    .get_book(book_id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("book"))?;
                if self.files_of_book(book_id, user).await?.is_empty() {
                    return Err(ApiError::Forbidden);
                }
                let entry = self.plugin_book_entry(source, book_id_in_source).await?;
                // A multi-volume source book cannot be squeezed into one
                // metadata entry: each 卷 is its own library book under a
                // series (acquire it as a new book instead).
                if entry.volumes.len() > 1 {
                    return Err(ApiError::bad_request(
                        "this plugin book spans several volumes and cannot be attached to a single \
                         metadata entry — acquire it as a new book instead (one book per volume)",
                    ));
                }
                // One plugin book = one library entry: it must not already
                // live under a different metadata entry.
                let existing: Option<String> = sqlx::query_scalar(
                    "SELECT book_id FROM book_files WHERE source = ? AND external_id = ?",
                )
                .bind(source)
                .bind(&entry.id)
                .fetch_optional(&self.db)
                .await?;
                if let Some(other) = existing
                    && other != book_id
                {
                    return Err(ApiError::Conflict(format!(
                        "plugin book `{book_id_in_source}` is already in the library under \
                         metadata `{other}`; attach another source instead"
                    )));
                }
                self.attach_plugin_file(source, Some(&user.id), &entry, book_id)
                    .await?;
                let file = self
                    .materialize_plugin_content(source, &entry, book_id)
                    .await?;
                (book_id.to_string(), file)
            }

            // ---- metadata from a named plugin source --------------------
            (
                content,
                AcquireMetadata::Plugin {
                    source,
                    book_id_in_source,
                    overrides,
                },
            ) => {
                let entry = self.plugin_book_entry(source, book_id_in_source).await?;
                let (content_inst, content_id) = self.plugins.content_target(source, &entry);
                let titles = self
                    .plugins
                    .chapter_titles(content_inst, content_id)
                    .await?;
                let (book_id, _) = self
                    .ensure_plugin_book(source, Some(&user.id), &entry, &titles)
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
                match content {
                    AcquireContent::File { bytes, filename } => {
                        let mut parsed = bookshelf_formats::parse(bytes, filename)?;
                        ingest_parsed_book(&self.db, &self.files_dir, &mut parsed).await?;
                        let file = self
                            .store_local_file(
                                &parsed, bytes, &book_id, &user.id, filename, visibility, label,
                            )
                            .await?;
                        (book_id, file)
                    }
                    AcquireContent::Plugin { .. } => {
                        let file = self
                            .materialize_plugin_content(source, &entry, &book_id)
                            .await?;
                        (book_id, file)
                    }
                }
            }

            // ---- new metadata from an uploaded file ---------------------
            // Auto: let plugins identify the file (first match wins),
            // otherwise create metadata parsed from the file. `overrides`
            // correct the produced metadata in both cases.
            (AcquireContent::File { bytes, filename }, AcquireMetadata::New { overrides }) => {
                let mut parsed = bookshelf_formats::parse(bytes, filename)?;
                ingest_parsed_book(&self.db, &self.files_dir, &mut parsed).await?;
                let hash = sha256_hex(bytes);
                let book_id = match self.plugins.identify_upload(filename, &hash).await? {
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
                };
                let file = self
                    .store_local_file(
                        &parsed, bytes, &book_id, &user.id, filename, visibility, label,
                    )
                    .await?;
                (book_id, file)
            }

            // ---- new metadata from a plugin source's own entry ----------
            // (plugin content; there is no file to parse/identify — the
            // plugin's `book-entry` is the natural metadata)
            (
                AcquireContent::Plugin {
                    source,
                    book_id_in_source,
                },
                AcquireMetadata::New { overrides },
            ) => {
                let entry = self.plugin_book_entry(source, book_id_in_source).await?;
                let (content_inst, content_id) = self.plugins.content_target(source, &entry);
                let titles = self
                    .plugins
                    .chapter_titles(content_inst, content_id)
                    .await?;
                let (book_id, _) = self
                    .ensure_plugin_book(source, Some(&user.id), &entry, &titles)
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
                let file = self
                    .materialize_plugin_content(source, &entry, &book_id)
                    .await?;
                (book_id, file)
            }
        };

        let book = self
            .get_book(&book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))?;
        Ok(AcquireOutcome {
            series: None,
            books: vec![(book, file)],
        })
    }

    /// Multi-volume plugin acquisition (series split). Returns `Some`
    /// when the source's `book-entry` declared >1 volumes and the split
    /// was performed; `None` when the book is effectively single-volume.
    ///
    /// Only plugin *content* splits (an uploaded file is one complete
    /// edition and never does): the guard in [`Self::acquire_book`]
    /// limits this path to `AcquireContent::Plugin` + new/plugin
    /// metadata.
    async fn try_split_acquire(
        &self,
        user: &User,
        source: &str,
        book_id_in_source: &str,
        metadata: &AcquireMetadata<'_>,
        visibility: Visibility,
        label: &str,
    ) -> Result<Option<AcquireOutcome>, ApiError> {
        let _ = label; // the volume's label is its volume title, not a user string
        let entry = self.plugin_book_entry(source, book_id_in_source).await?;
        if entry.volumes.len() <= 1 {
            return Ok(None);
        }
        let (content_inst, content_id) = self.plugins.content_target(source, &entry);
        let titles = self
            .plugins
            .chapter_titles(content_inst, content_id)
            .await?;
        let Some(slices) = volume_slices(&entry.volumes, titles.len() as u32) else {
            // Declared volumes collapsed (all empty): fall back to today's
            // single-book acquisition.
            return Ok(None);
        };
        let overrides = match metadata {
            AcquireMetadata::New { overrides } | AcquireMetadata::Plugin { overrides, .. } => {
                overrides
            }
            AcquireMetadata::Attach { .. } => unreachable!("guarded by the caller"),
        };

        // One plugin book = one library series: reject materializing the
        // same source book twice (incl. pre-split merged entries).
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT book_id FROM book_files WHERE source = ? AND external_id = ? LIMIT 1",
        )
        .bind(source)
        .bind(&entry.id)
        .fetch_optional(&self.db)
        .await?;
        if let Some(existing_id) = existing {
            return Err(ApiError::Conflict(format!(
                "plugin book `{}` is already in the library (as book `{existing_id}`); \
                 delete the existing entry first to re-acquire it split into volumes",
                entry.id
            )));
        }

        let title = overrides
            .title
            .clone()
            .unwrap_or_else(|| entry.title.clone());
        let authors_json =
            serde_json::to_string(overrides.authors.as_ref().unwrap_or(&entry.authors))
                .unwrap_or_else(|_| "[]".into());
        let description = overrides
            .description
            .clone()
            .or_else(|| entry.description.clone());
        let cover_url = overrides
            .cover_url
            .clone()
            .or_else(|| entry.cover_url.clone());

        let series_id = uuid::Uuid::new_v4().simple().to_string();
        let mut tx = self.db.begin().await?;
        sqlx::query(
            "INSERT INTO series (id, title, authors, description, cover_url, created_by) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&series_id)
        .bind(&title)
        .bind(&authors_json)
        .bind(&description)
        .bind(&cover_url)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;

        let mut books = Vec::with_capacity(slices.len());
        for (index, (vol_title, start, count)) in slices.iter().enumerate() {
            let volume_no = (index + 1) as u32;
            let book_id = uuid::Uuid::new_v4().simple().to_string();
            sqlx::query(
                "INSERT INTO books (id, title, authors, description, cover_url, series_id, \
                 volume_no, created_by) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&book_id)
            .bind(&title)
            .bind(&authors_json)
            .bind(&description)
            .bind(&cover_url)
            .bind(&series_id)
            .bind(volume_no as i64)
            .bind(&user.id)
            .execute(&mut *tx)
            .await?;

            // One virtual chapter-mode file per volume; the volume's
            // chapter range is a slice of the source's flat list, so lazy
            // `get-chapter(source, volume_offset + idx)` works.
            let file_label = if vol_title.is_empty() {
                format!("第{volume_no}卷")
            } else {
                vol_title.clone()
            };
            let file_id = uuid::Uuid::new_v4().simple().to_string();
            sqlx::query(
                "INSERT INTO book_files (id, book_id, source, external_id, content_source, \
                 content_external_id, format, label, visibility, owner_id, chapter_count, \
                 volume_no, volume_offset) \
                 VALUES (?, ?, ?, ?, ?, ?, 'plugin', ?, 'public', ?, ?, ?, ?)",
            )
            .bind(&file_id)
            .bind(&book_id)
            .bind(source)
            .bind(&entry.id)
            .bind(&entry.content_source)
            .bind(&entry.content_id)
            .bind(&file_label)
            .bind(&user.id)
            .bind(*count as i64)
            .bind(volume_no as i64)
            .bind(*start as i64)
            .execute(&mut *tx)
            .await?;

            // Title-only placeholder rows for this volume's slice.
            let slice = &titles[*start as usize..(*start + *count) as usize];
            for (idx, t) in slice.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO chapters (file_id, idx, title, content) VALUES (?, ?, ?, '')",
                )
                .bind(&file_id)
                .bind(idx as i64)
                .bind(t.trim())
                .execute(&mut *tx)
                .await?;
            }

            books.push((
                BookMeta {
                    id: book_id.clone(),
                    title: title.clone(),
                    authors: parse_authors(&authors_json),
                    description: description.clone(),
                    cover_url: cover_url.clone(),
                    created_by: Some(user.id.clone()),
                    created_at: String::new(),
                    series_id: Some(series_id.clone()),
                    volume_no,
                },
                FileMeta {
                    id: file_id,
                    book_id,
                    source: source.to_string(),
                    external_id: entry.id.clone(),
                    content_source: entry.content_source.clone(),
                    content_external_id: entry.content_id.clone(),
                    format: "plugin".into(),
                    label: file_label,
                    visibility,
                    owner_id: Some(user.id.clone()),
                    chapter_count: *count,
                    created_at: String::new(),
                    volume_no,
                    volume_offset: *start,
                    original: None,
                },
            ));
        }
        tx.commit().await?;

        let series = SeriesMeta {
            id: series_id,
            title: title.clone(),
            authors: parse_authors(&authors_json),
            description: description.clone(),
            cover_url: cover_url.clone(),
            created_by: Some(user.id.clone()),
            created_at: String::new(),
        };
        info!(
            source = %source,
            book = %entry.id,
            volumes = books.len(),
            series = %series.id,
            "acquired multi-volume plugin book as a series"
        );
        Ok(Some(AcquireOutcome {
            series: Some(series),
            books,
        }))
    }

    /// Fetch one entry from a plugin source by id (`lookup` capability).
    async fn plugin_book_entry(&self, source: &str, book_id: &str) -> Result<SourceBook, ApiError> {
        let entry = self.plugins.get_book(source, book_id).await?;
        entry
            .map(SourceBook::from)
            .ok_or_else(|| ApiError::bad_request(format!("plugin does not offer book `{book_id}`")))
    }

    /// The virtual file row of a plugin book attached under an existing
    /// metadata entry (no metadata is created; `ensure_plugin_book` is
    /// bypassed so the entry the user chose wins). The caller must have
    /// checked the plugin book is not already materialized elsewhere.
    async fn attach_plugin_file(
        &self,
        source: &str,
        owner: Option<&str>,
        book: &SourceBook,
        book_id: &str,
    ) -> Result<String, ApiError> {
        if let Some(file_id) =
            sqlx::query_scalar("SELECT id FROM book_files WHERE source = ? AND external_id = ?")
                .bind(source)
                .bind(&book.id)
                .fetch_optional(&self.db)
                .await?
        {
            return Ok(file_id);
        }
        let file_id = uuid::Uuid::new_v4().simple().to_string();
        sqlx::query(
            "INSERT INTO book_files (id, book_id, source, external_id, content_source, content_external_id, format, label, visibility, owner_id, chapter_count) \
             VALUES (?, ?, ?, ?, ?, ?, 'plugin', '', 'public', ?, 0)",
        )
        .bind(&file_id)
        .bind(book_id)
        .bind(source)
        .bind(&book.id)
        .bind(&book.content_source)
        .bind(&book.content_id)
        .bind(owner)
        .execute(&self.db)
        .await?;
        Ok(file_id)
    }

    /// Materialize a plugin book's *content* under an existing metadata
    /// entry. File mode (`get-book-file` through the upload parser,
    /// `book-file` capability) wins; chapter mode (virtual file + title
    /// placeholders, bodies lazy until first read) is the fallback.
    async fn materialize_plugin_content(
        &self,
        source: &str,
        entry: &SourceBook,
        book_id: &str,
    ) -> Result<FileMeta, ApiError> {
        if self.plugins.declares(source, "book-file").await?
            && let Some(book_file) = self.plugins.get_book_file(source, &entry.id).await?
        {
            return self
                .materialize_file_mode(source, entry, &book_file, book_id)
                .await;
        }

        // Chapter mode: virtual file row + title placeholders. The row
        // may be freshly attached (chapter_count 0 — titles were only
        // ensured for metadata-creating paths), so fetch titles directly
        // in that case.
        let file = self
            .plugin_file(source, &entry.id)
            .await?
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("plugin file vanished")))?;
        if file.chapter_count == 0 {
            let (content_inst, content_id) = self.plugins.content_target(source, entry);
            let titles = self
                .plugins
                .chapter_titles(content_inst, content_id)
                .await?;
            if !titles.is_empty() {
                sqlx::query(
                    "UPDATE book_files SET chapter_count = ?, content_source = ?, content_external_id = ? \
                     WHERE id = ?",
                )
                .bind(titles.len() as i64)
                .bind(&entry.content_source)
                .bind(&entry.content_id)
                .bind(&file.id)
                .execute(&self.db)
                .await?;
                self.upsert_placeholder_titles(&file.id, &titles).await?;
            }
        } else {
            self.ensure_titles(&file).await?;
        }
        self.get_file(&file.id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))
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
            "INSERT INTO books (id, title, authors, description, cover_url, cover, cover_mime, created_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(parsed.cover.as_ref().map(|c| c.bytes.clone()))
        .bind(parsed.cover.as_ref().map(|c| c.mime.clone()))
        .bind(owner_id)
        .execute(&self.db)
        .await?;
        Ok(book_id)
    }

    /// Store cover bytes on the metadata entry when it has none yet
    /// (attach-mode uploads; creation always overwrites via INSERT).
    async fn store_cover_if_missing(
        &self,
        book_id: &str,
        cover: &bookshelf_formats::CoverImage,
    ) -> Result<(), ApiError> {
        sqlx::query(
            "UPDATE books SET cover = ?, cover_mime = ? \
             WHERE id = ? AND cover IS NULL",
        )
        .bind(&cover.bytes)
        .bind(&cover.mime)
        .bind(book_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// The stored cover image of a metadata entry (bytes, mime), if any.
    pub async fn get_cover(&self, book_id: &str) -> Result<Option<(Vec<u8>, String)>, ApiError> {
        let row: Option<(Option<Vec<u8>>, Option<String>)> =
            sqlx::query_as("SELECT cover, cover_mime FROM books WHERE id = ?")
                .bind(book_id)
                .fetch_optional(&self.db)
                .await?;
        Ok(row.and_then(|(bytes, mime)| match (bytes, mime) {
            (Some(bytes), Some(mime)) if !bytes.is_empty() => Some((bytes, mime)),
            _ => None,
        }))
    }

    /// Retain the original file bytes on disk
    /// (`data/files/{file_id}.{ext}`, storage-unification design §4.2)
    /// and return the `book_files` columns `orig_ext, orig_sha256,
    /// orig_size`.
    async fn store_original(
        &self,
        file_id: &str,
        ext: &str,
        bytes: &[u8],
    ) -> Result<(String, String, i64), ApiError> {
        tokio::fs::create_dir_all(&self.files_dir)
            .await
            .with_context(|| format!("create files dir {}", self.files_dir.display()))?;
        let path = self.original_path(file_id, ext);
        tokio::fs::write(&path, bytes)
            .await
            .with_context(|| format!("write original {}", path.display()))?;
        Ok((ext.to_string(), sha256_hex(bytes), bytes.len() as i64))
    }

    /// Disk path of a file's retained original bytes.
    fn original_path(&self, file_id: &str, ext: &str) -> PathBuf {
        original_path(&self.files_dir, file_id, ext)
    }

    /// Load a file's retained original bytes (for download / reparse).
    pub async fn get_original(
        &self,
        file: &FileMeta,
    ) -> Result<Option<(Vec<u8>, String)>, ApiError> {
        let Some(orig) = &file.original else {
            return Ok(None);
        };
        let path = self.original_path(&file.id, &orig.ext);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some((bytes, orig.ext.clone()))),
            // A DB row without its bytes (deleted by hand, restore from
            // backup mismatch) is a recoverable 409, not a 500.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                warn!(file = %file.id, path = %path.display(), "original missing on disk");
                Err(ApiError::Conflict(
                    "original file missing on disk (restore data/files/ from backup)".into(),
                ))
            }
            Err(e) => Err(anyhow::Error::new(e).context("read original").into()),
        }
    }

    /// Store an uploaded file (chapters + TOC) under the given metadata
    /// entry. The file's `external_id` equals its own id for local files.
    /// The raw upload bytes are retained on disk (original retention).
    #[allow(clippy::too_many_arguments)]
    async fn store_local_file(
        &self,
        parsed: &bookshelf_formats::ParsedBook,
        bytes: &[u8],
        book_id: &str,
        owner_id: &str,
        filename: &str,
        visibility: Visibility,
        label: &str,
    ) -> Result<FileMeta, ApiError> {
        let file_id = uuid::Uuid::new_v4().simple().to_string();
        let format = detect_format(filename);
        let toc_json = serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());
        let (orig_ext, orig_sha256, orig_size) =
            self.store_original(&file_id, format, bytes).await?;

        let mut tx = self.db.begin().await?;
        sqlx::query(
            "INSERT INTO book_files (id, book_id, source, external_id, format, label, visibility, owner_id, chapter_count, toc, orig_ext, orig_sha256, orig_size) \
             VALUES (?, ?, 'local', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(&orig_ext)
        .bind(&orig_sha256)
        .bind(orig_size)
        .execute(&mut *tx)
        .await?;

        for (idx, chapter) in parsed.chapters.iter().enumerate() {
            sqlx::query("INSERT INTO chapters (file_id, idx, title, content) VALUES (?, ?, ?, ?)")
                .bind(&file_id)
                .bind(idx as i64)
                .bind(&chapter.title)
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

        // Static SQL with parameterized switches (no dynamic assembly):
        // `source` filters only when non-null; the visibility clause is
        // bypassed for admins via an always-true bound flag. Series first
        // (in volume order), then standalone books, newest first.
        let rows: Vec<BookRow> = sqlx::query_as(
            "SELECT DISTINCT b.id, b.title, b.authors, b.description, b.cover_url, b.created_by, \
             b.created_at, b.series_id, b.volume_no FROM books b JOIN book_files f ON f.book_id = b.id \
             WHERE b.title LIKE ? \
             AND (f.source = ? OR ? IS NULL) \
             AND (? OR f.visibility = 'public' OR f.owner_id = ?) \
             ORDER BY (b.series_id IS NOT NULL) DESC, b.series_id, b.volume_no, b.created_at DESC",
        )
        .bind(pattern)
        .bind(source)
        .bind(source)
        .bind(is_admin)
        .bind(&user.id)
        .fetch_all(&self.db)
        .await?;

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
            "SELECT id, title, authors, description, cover_url, created_by, created_at, series_id, \
             volume_no FROM books WHERE id = ?",
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
        let plugin_files: Vec<(String, String, String, i64, i64, i64)> = sqlx::query_as(
            "SELECT id, source, external_id, volume_no, volume_offset, chapter_count FROM book_files \
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

        for (file_id, source_id, external_id, volume_no, volume_offset, chapter_count) in
            &plugin_files
        {
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
            // Refresh the chapter-title placeholder rows (never overwrite
            // materialized chapter content). A volume file only owns its
            // slice of the source's flat list.
            let slice = title_slice(
                &titles,
                *volume_no as u32,
                *volume_offset as u32,
                *chapter_count as u32,
            );
            sqlx::query(
                "UPDATE book_files SET chapter_count = ?, content_source = ?, content_external_id = ? \
                 WHERE id = ?",
            )
            .bind(slice.len() as i64)
            .bind(&entry.content_source)
            .bind(&entry.content_id)
            .bind(file_id)
            .execute(&self.db)
            .await?;
            if !slice.is_empty() {
                self.upsert_placeholder_titles(file_id, slice).await?;
            }
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

    // ---- series -----------------------------------------------------------

    /// Member books of a series in volume order (all members — file
    /// visibility is applied per book by the caller; the series itself
    /// is already visibility-checked).
    pub async fn series_member_books(&self, id: &str) -> Result<Vec<BookMeta>, ApiError> {
        let rows: Vec<BookRow> = sqlx::query_as(
            "SELECT id, title, authors, description, cover_url, created_by, created_at, series_id, \
             volume_no FROM books WHERE series_id = ? ORDER BY volume_no",
        )
        .bind(id)
        .fetch_all(&self.db)
        .await?;
        rows.into_iter().map(BookRow::into_model).collect()
    }

    /// Is the series visible to the caller (creator, admin, or ≥1 member
    /// book with a visible file)?
    pub async fn series_visible_to(&self, user: &User, series_id: &str) -> Result<bool, ApiError> {
        if user.role == Role::Admin {
            return Ok(true);
        }
        let creator: Option<String> =
            sqlx::query_scalar("SELECT created_by FROM series WHERE id = ?")
                .bind(series_id)
                .fetch_optional(&self.db)
                .await?;
        let creator = creator.ok_or_else(|| ApiError::not_found("series"))?;
        if creator == user.id {
            return Ok(true);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM books b JOIN book_files f ON f.book_id = b.id \
             WHERE b.series_id = ? AND (f.visibility = 'public' OR f.owner_id = ?)",
        )
        .bind(series_id)
        .bind(&user.id)
        .fetch_one(&self.db)
        .await?;
        Ok(count > 0)
    }

    pub async fn get_series(&self, id: &str) -> Result<Option<SeriesMeta>, ApiError> {
        let row: Option<SeriesRow> = sqlx::query_as(
            "SELECT id, title, authors, description, cover_url, created_by, created_at \
             FROM series WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db)
        .await?;
        Ok(row.map(SeriesRow::into_model))
    }

    /// Number of member books of a series (any visibility).
    pub async fn series_volume_count(&self, id: &str) -> Result<u32, ApiError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE series_id = ?")
            .bind(id)
            .fetch_one(&self.db)
            .await?;
        Ok(count.max(0) as u32)
    }

    /// All series the caller can see (creator, or ≥1 visible member
    /// file; admins see all), each with its member count.
    pub async fn list_series(&self, user: &User) -> Result<Vec<(SeriesMeta, u32)>, ApiError> {
        let rows: Vec<SeriesRow> = if user.role == Role::Admin {
            sqlx::query_as(
                "SELECT id, title, authors, description, cover_url, created_by, created_at \
                 FROM series ORDER BY created_at DESC",
            )
            .fetch_all(&self.db)
            .await?
        } else {
            sqlx::query_as(
                "SELECT s.id, s.title, s.authors, s.description, s.cover_url, s.created_by, \
                 s.created_at FROM series s WHERE s.created_by = ? OR EXISTS ( \
                     SELECT 1 FROM books b JOIN book_files f ON f.book_id = b.id \
                     WHERE b.series_id = s.id AND (f.visibility = 'public' OR f.owner_id = ?) \
                 ) ORDER BY s.created_at DESC",
            )
            .bind(&user.id)
            .bind(&user.id)
            .fetch_all(&self.db)
            .await?
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let series = row.into_model();
            let count = self.series_volume_count(&series.id).await?;
            out.push((series, count));
        }
        Ok(out)
    }

    /// Create a series (any logged-in user; the creator manages it).
    pub async fn create_series(
        &self,
        user: &User,
        title: &str,
        authors: Option<&Vec<String>>,
        description: Option<&str>,
    ) -> Result<SeriesMeta, ApiError> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let authors_json =
            authors.map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".into()));
        sqlx::query(
            "INSERT INTO series (id, title, authors, description, cover_url, created_by) \
             VALUES (?, ?, ?, ?, NULL, ?)",
        )
        .bind(&id)
        .bind(title)
        .bind(authors_json)
        .bind(description)
        .bind(&user.id)
        .execute(&self.db)
        .await?;
        self.get_series(&id)
            .await?
            .ok_or_else(|| ApiError::not_found("series"))
    }

    /// Edit series metadata (creator/admin).
    pub async fn update_series(
        &self,
        user: &User,
        id: &str,
        title: Option<&str>,
        authors: Option<&Vec<String>>,
        description: Option<&str>,
    ) -> Result<SeriesMeta, ApiError> {
        let series = self
            .get_series(id)
            .await?
            .ok_or_else(|| ApiError::not_found("series"))?;
        if series.created_by.as_deref() != Some(user.id.as_str()) && user.role != Role::Admin {
            return Err(ApiError::Forbidden);
        }
        let authors_json =
            authors.map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".into()));
        sqlx::query(
            "UPDATE series SET title = COALESCE(?, title), description = COALESCE(?, description), \
             authors = COALESCE(?, authors) WHERE id = ?",
        )
        .bind(title)
        .bind(description)
        .bind(authors_json)
        .bind(id)
        .execute(&self.db)
        .await?;
        self.get_series(id)
            .await?
            .ok_or_else(|| ApiError::not_found("series"))
    }

    /// Delete a series: member books are unassigned (their entries stay).
    pub async fn delete_series(&self, user: &User, id: &str) -> Result<(), ApiError> {
        let series = self
            .get_series(id)
            .await?
            .ok_or_else(|| ApiError::not_found("series"))?;
        if series.created_by.as_deref() != Some(user.id.as_str()) && user.role != Role::Admin {
            return Err(ApiError::Forbidden);
        }
        let mut tx = self.db.begin().await?;
        sqlx::query("UPDATE books SET series_id = NULL, volume_no = 0 WHERE series_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM series WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Replace the member set + order of a series in one call: the given
    /// books (in order) become the volumes 1..N; former members not in
    /// the list are unassigned. The caller must manage the series and
    /// every listed book.
    pub async fn set_series_members(
        &self,
        user: &User,
        id: &str,
        book_ids: &[String],
    ) -> Result<(), ApiError> {
        let series = self
            .get_series(id)
            .await?
            .ok_or_else(|| ApiError::not_found("series"))?;
        if series.created_by.as_deref() != Some(user.id.as_str()) && user.role != Role::Admin {
            return Err(ApiError::Forbidden);
        }
        let mut seen = std::collections::HashSet::new();
        for bid in book_ids {
            if !seen.insert(bid) {
                return Err(ApiError::bad_request(format!(
                    "book `{bid}` appears more than once"
                )));
            }
            let book = self
                .get_book(bid)
                .await?
                .ok_or_else(|| ApiError::bad_request(format!("no such book `{bid}`")))?;
            let owned = book.created_by.as_deref() == Some(user.id.as_str());
            if !owned && user.role != Role::Admin {
                return Err(ApiError::Forbidden);
            }
        }
        let mut tx = self.db.begin().await?;
        sqlx::query("UPDATE books SET series_id = NULL, volume_no = 0 WHERE series_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for (i, bid) in book_ids.iter().enumerate() {
            sqlx::query("UPDATE books SET series_id = ?, volume_no = ? WHERE id = ?")
                .bind(id)
                .bind((i + 1) as i64)
                .bind(bid)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Assign / move / unassign a book's series membership (per-book
    /// `PATCH /api/books/{id}`). Both the book and the target series
    /// must be manageable by the caller. `series_id: Some(None)` (JSON
    /// `null`) unassigns; a missing `series_id` keeps the current series
    /// (a `volume_no` then renumbers within it); without a `volume_no`
    /// the next free volume is used.
    pub async fn set_book_series(
        &self,
        user: &User,
        book_id: &str,
        series_id: Option<Option<String>>,
        volume_no: Option<Option<u32>>,
    ) -> Result<BookMeta, ApiError> {
        let book = self
            .get_book(book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))?;
        let owned = book.created_by.as_deref() == Some(user.id.as_str());
        if !owned && user.role != Role::Admin {
            return Err(ApiError::Forbidden);
        }

        let (target_series, target_vol) = match series_id {
            Some(Some(id)) => (Some(id), volume_no),
            Some(None) => (None, None),
            None => {
                let current = book.series_id.clone();
                if current.is_none() && matches!(volume_no, Some(Some(_))) {
                    return Err(ApiError::bad_request(
                        "book is not part of a series; pass `series_id` to assign it",
                    ));
                }
                (current, volume_no)
            }
        };

        let Some(sid) = target_series else {
            sqlx::query("UPDATE books SET series_id = NULL, volume_no = 0 WHERE id = ?")
                .bind(book_id)
                .execute(&self.db)
                .await?;
            return self
                .get_book(book_id)
                .await?
                .ok_or_else(|| ApiError::not_found("book"));
        };

        let series = self
            .get_series(&sid)
            .await?
            .ok_or_else(|| ApiError::not_found("series"))?;
        let owned_series = series.created_by.as_deref() == Some(user.id.as_str());
        if !owned_series && user.role != Role::Admin {
            return Err(ApiError::Forbidden);
        }
        let vol = match target_vol {
            Some(Some(n)) => n,
            Some(None) | None => {
                let max: Option<i64> = sqlx::query_scalar(
                    "SELECT MAX(volume_no) FROM books WHERE series_id = ? AND id != ?",
                )
                .bind(&sid)
                .bind(book_id)
                .fetch_one(&self.db)
                .await?;
                (max.unwrap_or(0).max(0) as u32).saturating_add(1)
            }
        };
        // Volume numbers within a series are unique.
        let taken: Option<String> = sqlx::query_scalar(
            "SELECT id FROM books WHERE series_id = ? AND volume_no = ? AND id != ?",
        )
        .bind(&sid)
        .bind(vol as i64)
        .bind(book_id)
        .fetch_optional(&self.db)
        .await?;
        if let Some(other) = taken {
            return Err(ApiError::Conflict(format!(
                "第{vol}卷 already belongs to book `{other}`; pick another volume number"
            )));
        }
        sqlx::query("UPDATE books SET series_id = ?, volume_no = ? WHERE id = ?")
            .bind(&sid)
            .bind(vol as i64)
            .bind(book_id)
            .execute(&self.db)
            .await?;
        self.get_book(book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))
    }

    // ---- files -----------------------------------------------------------

    /// Files of a book the user is allowed to see.
    pub async fn files_of_book(
        &self,
        book_id: &str,
        user: &User,
    ) -> Result<Vec<FileMeta>, ApiError> {
        let is_admin = user.role == bookshelf_core::model::Role::Admin;
        // Static SQL; the visibility clause is bypassed for admins via an
        // always-true bound flag.
        let rows: Vec<FileRow> = sqlx::query_as(
            "SELECT id, book_id, source, external_id, content_source, content_external_id, format, label, visibility, owner_id, \
             chapter_count, created_at, volume_no, volume_offset, orig_ext, orig_sha256, orig_size FROM book_files WHERE book_id = ? \
             AND (? OR visibility = 'public' OR owner_id = ?) ORDER BY created_at DESC",
        )
        .bind(book_id)
        .bind(is_admin)
        .bind(&user.id)
        .fetch_all(&self.db)
        .await?;
        rows.into_iter().map(FileRow::into_model).collect()
    }

    pub async fn get_file(&self, id: &str) -> Result<Option<FileMeta>, ApiError> {
        let row: Option<FileRow> = sqlx::query_as(
            "SELECT id, book_id, source, external_id, content_source, content_external_id, format, label, visibility, owner_id, \
             chapter_count, created_at, volume_no, volume_offset, orig_ext, orig_sha256, orig_size FROM book_files WHERE id = ?",
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
        // Remove the retained original first (best effort — a leftover
        // disk file without a row is harmless garbage).
        if let Some(file) = self.get_file(id).await? {
            let orig = file.original.map(|o| self.original_path(id, &o.ext));
            if let Some(path) = orig
                && let Err(e) = tokio::fs::remove_file(&path).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!(file = %id, path = %path.display(), error = %e, "failed to remove original");
            }
        }
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
            "SELECT idx, title, content FROM chapters WHERE file_id = ? AND idx = ?",
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
        // Volume files (multi-volume splits) stay chapter-mode: their
        // `volume_offset` slices of the source's flat list are only valid
        // in chapter mode — a whole-book file rewrite would merge the
        // volumes back together.
        if file.format == "plugin" && file.content_source.is_none() && file.volume_no == 0 {
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
                let parsed = bookshelf_formats::parse(&book_file.bytes, &book_file.filename)
                    .inspect_err(|e| warn!(error = %e, "file-mode reparse failed"))
                    .ok();
                if let (Some(entry), Some(mut parsed)) = (entry.as_ref(), parsed) {
                    ingest_parsed_book(&self.db, &self.files_dir, &mut parsed).await?;
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
                            "INSERT INTO chapters (file_id, idx, title, content) VALUES (?, ?, ?, ?)",
                        )
                        .bind(&file.id)
                        .bind(i as i64)
                        .bind(&chapter.title)
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
            "SELECT idx, title, content FROM chapters WHERE file_id = ? AND idx = ?",
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
        // For a volume file, the plugin index is the volume's chapter
        // shifted by the persisted flat `volume_offset`. Plugin text is
        // normalized to canonical HTML at the ingest boundary (including
        // the illustration conventions; see bookshelf_formats::htmlize).
        let Some(chapter) = self
            .plugins
            .get_chapter(source_id, external_id, idx + file.volume_offset)
            .await?
        else {
            return Ok(None);
        };
        let content = bookshelf_formats::htmlize::plugin_text_to_html(&chapter.content);
        sqlx::query(
            "INSERT INTO chapters (file_id, idx, title, content) VALUES (?, ?, ?, ?) \
             ON CONFLICT (file_id, idx) DO UPDATE SET title = excluded.title, content = excluded.content",
        )
        .bind(&file.id)
        .bind(idx as i64)
        .bind(&chapter.title)
        .bind(&content)
        .execute(&self.db)
        .await?;
        Ok(Some(Chapter {
            idx,
            title: chapter.title,
            content,
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
                "INSERT INTO chapters (file_id, idx, title, content) VALUES (?, ?, ?, '') \
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
        // A volume file only owns its slice of the source's flat chapter
        // list; ensure_titles must not import the other volumes.
        let slice = title_slice(
            &titles,
            file.volume_no,
            file.volume_offset,
            file.chapter_count,
        );
        if slice.is_empty() {
            return Ok(());
        }
        self.upsert_placeholder_titles(&file.id, slice).await?;
        // Placeholder rows beyond the volume's range are stale (previous
        // content source, or a chapter count that shrank); drop the
        // empty ones. Materialized bodies stay untouched.
        sqlx::query("DELETE FROM chapters WHERE file_id = ? AND content = '' AND idx >= ?")
            .bind(&file.id)
            .bind(slice.len() as i64)
            .execute(&self.db)
            .await?;
        if slice.len() != file.chapter_count as usize {
            sqlx::query("UPDATE book_files SET chapter_count = ? WHERE id = ?")
                .bind(slice.len() as i64)
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

/// The flat chapter list of a plugin book, restricted to the range a file
/// owns: `[volume_offset, volume_offset + chapter_count)` for volume
/// files, the whole list otherwise. Clamped defensively (a source may
/// have restructured since acquisition).
fn title_slice(
    titles: &[String],
    volume_no: u32,
    volume_offset: u32,
    chapter_count: u32,
) -> &[String] {
    if volume_no == 0 {
        return titles;
    }
    let start = (volume_offset as usize).min(titles.len());
    let end = start
        .saturating_add(chapter_count as usize)
        .min(titles.len());
    &titles[start..end]
}

/// Consecutive volume slices of a source book's flat chapter list:
/// `(volume title, start, count)`. Defensive: zero-chapter volumes
/// collapse, declared counts are clamped to the flat list, and leftover
/// chapters (declarations running out early) extend the last volume.
/// Returns `None` when the book is effectively single-volume.
fn volume_slices(volumes: &[SourceVolume], total: u32) -> Option<Vec<(String, u32, u32)>> {
    if volumes.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut start = 0u32;
    for v in volumes {
        let count = v.chapter_count.min(total.saturating_sub(start));
        if count > 0 {
            out.push((v.title.clone(), start, count));
            start += count;
        }
    }
    if out.len() < 2 || start == 0 {
        return None;
    }
    if start < total
        && let Some(last) = out.last_mut()
    {
        last.2 += total - start;
    }
    Some(out)
}

/// Disk path of a file's retained original bytes
/// (`{files_dir}/{file_id}.{ext}`).
fn original_path(files_dir: &Path, file_id: &str, ext: &str) -> PathBuf {
    files_dir.join(format!("{file_id}.{ext}"))
}

// ---- chapter image localization ------------------------------------------

/// Store image bytes in the content-addressed image store: id = sha256 hex,
/// bytes at `{files_dir}/images/{id}`, row in the `images` table. Duplicate
/// bytes are a no-op (same id). Returns the id for referencing the image as
/// `/api/images/{id}`.
async fn store_image(
    db: &SqlitePool,
    files_dir: &Path,
    bytes: &[u8],
    mime: &str,
) -> anyhow::Result<String> {
    let id = sha256_hex(bytes);
    let dir = files_dir.join("images");
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create images dir {}", dir.display()))?;
    let path = dir.join(&id);
    if !path.exists() {
        tokio::fs::write(&path, bytes)
            .await
            .with_context(|| format!("write image {}", path.display()))?;
    }
    sqlx::query("INSERT OR IGNORE INTO images (id, mime, size) VALUES (?, ?, ?)")
        .bind(&id)
        .bind(mime)
        .bind(bytes.len() as i64)
        .execute(db)
        .await?;
    Ok(id)
}

/// Ingest boundary for parsed book files: stores every extracted image in
/// the image store and rewrites the parser's ingest placeholders
/// (`src="image:{n}"`) to canonical `/api/images/{id}` references, so all
/// stored chapter HTML — uploads and plugin materializations alike — uses
/// one image reference format.
pub async fn ingest_parsed_book(
    db: &SqlitePool,
    files_dir: &Path,
    parsed: &mut ParsedBook,
) -> anyhow::Result<()> {
    if parsed.images.is_empty() {
        return Ok(());
    }
    let mut ids = Vec::with_capacity(parsed.images.len());
    for image in &parsed.images {
        ids.push(store_image(db, files_dir, &image.bytes, &image.mime).await?);
    }
    for chapter in &mut parsed.chapters {
        for (n, id) in ids.iter().enumerate() {
            let placeholder = format!("src=\"image:{n}\"");
            let reference = format!("src=\"/api/images/{id}\"");
            if chapter.content.contains(&placeholder) {
                chapter.content = chapter.content.replace(&placeholder, &reference);
            }
        }
    }
    Ok(())
}

impl Library {
    /// The stored mime type of a stored image (by sha256 hex id).
    pub async fn image_mime(&self, id: &str) -> Result<Option<String>, ApiError> {
        let mime: Option<String> = sqlx::query_scalar("SELECT mime FROM images WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.db)
            .await?;
        Ok(mime)
    }

    /// Read a localized image's bytes (by sha256 hex id).
    pub async fn image_bytes(&self, id: &str) -> Result<Option<Vec<u8>>, ApiError> {
        match tokio::fs::read(self.files_dir.join("images").join(id)).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(anyhow::Error::new(e).context("read image").into()),
        }
    }
}

/// Startup backfill (storage unification P3): convert legacy plain-text
/// chapter rows to canonical HTML. Idempotent — after one pass no row has
/// `format='text'` and subsequent boots are a no-op. Returns the number
/// of converted rows. Removed together with the `chapters.format`
/// column in migration 0008 (P4).
pub async fn backfill_text_chapters(db: &SqlitePool) -> anyhow::Result<usize> {
    let mut total = 0usize;
    loop {
        let rows: Vec<(String, i64, String)> = sqlx::query_as(
            "SELECT file_id, idx, content FROM chapters WHERE format = 'text' LIMIT 500",
        )
        .fetch_all(db)
        .await?;
        if rows.is_empty() {
            return Ok(total);
        }
        let mut tx = db.begin().await?;
        for (file_id, idx, content) in &rows {
            let html = bookshelf_formats::htmlize::plugin_text_to_html(content);
            sqlx::query(
                "UPDATE chapters SET format = 'html', content = ? WHERE file_id = ? AND idx = ?",
            )
            .bind(&html)
            .bind(file_id)
            .bind(idx)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        total += rows.len();
    }
}

/// One-shot upgrade migration (`--reparse-originals`, owner decision:
/// manual, no HTTP endpoint): re-run the current parser over every
/// retained original and replace the stored chapters + toc in one
/// transaction per file. Sessions are kept; `chapter_idx` is clamped to
/// the new chapter count. Returns (reparsed, failed).
pub async fn reparse_originals(
    db: &SqlitePool,
    files_dir: &Path,
) -> anyhow::Result<(usize, usize)> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, orig_ext FROM book_files WHERE orig_ext IS NOT NULL ORDER BY created_at",
    )
    .fetch_all(db)
    .await?;
    let mut ok = 0usize;
    let mut failed = 0usize;
    for (file_id, ext) in rows {
        let path = original_path(files_dir, &file_id, &ext);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(file = %file_id, path = %path.display(), error = %e, "reparse: original unreadable");
                failed += 1;
                continue;
            }
        };
        let mut parsed = match bookshelf_formats::parse(&bytes, &format!("original.{ext}")) {
            Ok(parsed) => parsed,
            Err(e) => {
                warn!(file = %file_id, ext = %ext, error = %e, "reparse: parse failed");
                failed += 1;
                continue;
            }
        };
        if let Err(e) = ingest_parsed_book(db, files_dir, &mut parsed).await {
            warn!(file = %file_id, ext = %ext, error = %e, "reparse: image ingest failed");
            failed += 1;
            continue;
        }
        let toc_json = serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());
        let count = parsed.chapters.len() as i64;
        let mut tx = db.begin().await?;
        sqlx::query("DELETE FROM chapters WHERE file_id = ?")
            .bind(&file_id)
            .execute(&mut *tx)
            .await?;
        for (idx, chapter) in parsed.chapters.iter().enumerate() {
            sqlx::query("INSERT INTO chapters (file_id, idx, title, content) VALUES (?, ?, ?, ?)")
                .bind(&file_id)
                .bind(idx as i64)
                .bind(&chapter.title)
                .bind(&chapter.content)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("UPDATE book_files SET chapter_count = ?, toc = ? WHERE id = ?")
            .bind(count)
            .bind(&toc_json)
            .bind(&file_id)
            .execute(&mut *tx)
            .await?;
        // Keep every session; clamp positions into the new chapter range.
        sqlx::query(
            "UPDATE sessions SET chapter_idx = MIN(chapter_idx, ?), fraction = MIN(fraction, 1.0) \
             WHERE file_id = ?",
        )
        .bind((count - 1).max(0))
        .bind(&file_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        info!(file = %file_id, ext = %ext, chapters = count, "reparse: replaced chapters + toc");
        ok += 1;
    }
    Ok((ok, failed))
}

/// Parse a stored `authors` JSON array (as read back from the DB).
fn parse_authors(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;
    use bookshelf_formats::ParsedImage;
    use std::str::FromStr;

    fn vol(title: &str, n: u32) -> SourceVolume {
        SourceVolume {
            title: title.into(),
            chapter_count: n,
        }
    }

    #[test]
    fn single_volume_declaration_does_not_split() {
        assert!(volume_slices(&[], 10).is_none());
        assert!(volume_slices(&[vol("", 10)], 10).is_none());
    }

    #[test]
    fn two_volumes_split_flat_list() {
        let slices = volume_slices(&[vol("第一卷", 2), vol("第二卷", 3)], 5).unwrap();
        assert_eq!(
            slices,
            vec![("第一卷".to_string(), 0, 2), ("第二卷".to_string(), 2, 3)]
        );
    }

    #[test]
    fn declared_counts_are_clamped_to_the_flat_list() {
        // declarations claim more than the source offers: one volume
        // swallows everything → no split
        assert!(volume_slices(&[vol("一", 9), vol("二", 9)], 5).is_none());
        // declarations claim less: the last volume absorbs the rest
        let slices = volume_slices(&[vol("一", 2), vol("二", 1)], 7).unwrap();
        assert_eq!(
            slices,
            vec![("一".to_string(), 0, 2), ("二".to_string(), 2, 5)]
        );
    }

    #[test]
    fn zero_chapter_volumes_collapse() {
        let slices = volume_slices(&[vol("一", 0), vol("二", 3), vol("三", 2)], 5).unwrap();
        assert_eq!(
            slices,
            vec![("二".to_string(), 0, 3), ("三".to_string(), 3, 2)]
        );
        // enough zeroes → effectively single volume → no split
        assert!(volume_slices(&[vol("一", 0), vol("二", 0)], 5).is_none());
        assert!(volume_slices(&[vol("一", 5), vol("二", 0)], 5).is_none());
        assert!(volume_slices(&[vol("一", 0), vol("二", 5)], 5).is_none());
    }

    #[test]
    fn title_slice_is_the_volume_range() {
        let titles: Vec<String> = (0..6).map(|i| format!("c{i}")).collect();
        let whole = FileMeta {
            volume_no: 0,
            volume_offset: 0,
            chapter_count: 6,
            ..dummy_file()
        };
        assert_eq!(
            title_slice(
                &titles,
                whole.volume_no,
                whole.volume_offset,
                whole.chapter_count
            ),
            &titles[..]
        );
        let vol2 = FileMeta {
            volume_no: 2,
            volume_offset: 3,
            chapter_count: 2,
            ..dummy_file()
        };
        assert_eq!(
            title_slice(
                &titles,
                vol2.volume_no,
                vol2.volume_offset,
                vol2.chapter_count
            ),
            &["c3", "c4"]
        );
        // clamped when the source shrank
        let vol2shrank = FileMeta {
            volume_no: 2,
            volume_offset: 4,
            chapter_count: 3,
            ..dummy_file()
        };
        assert_eq!(
            title_slice(
                &titles,
                vol2shrank.volume_no,
                vol2shrank.volume_offset,
                vol2shrank.chapter_count
            ),
            &["c4", "c5"]
        );
    }

    fn dummy_file() -> FileMeta {
        FileMeta {
            id: String::new(),
            book_id: String::new(),
            source: String::new(),
            external_id: String::new(),
            content_source: None,
            content_external_id: None,
            format: "plugin".into(),
            label: String::new(),
            visibility: Visibility::Public,
            owner_id: None,
            chapter_count: 0,
            created_at: String::new(),
            volume_no: 0,
            volume_offset: 0,
            original: None,
        }
    }

    #[tokio::test]
    async fn ingest_stores_images_and_rewrites_placeholders() {
        let opts = sqlx::sqlite::SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let db = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE images (id TEXT PRIMARY KEY NOT NULL, mime TEXT NOT NULL, \
             size INTEGER NOT NULL, created_at TEXT NOT NULL DEFAULT '')",
        )
        .execute(&db)
        .await
        .unwrap();

        let dir = std::env::temp_dir().join(format!(
            "bookshelf-ingest-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        // Two distinct images; the first is referenced twice (dedup by id).
        let img_a = vec![1u8, 2, 3, 4];
        let img_b = vec![9u8, 8, 7];
        let mut parsed = ParsedBook {
            title: "t".into(),
            authors: vec![],
            description: None,
            cover_url: None,
            cover: None,
            images: vec![
                ParsedImage {
                    bytes: img_a.clone(),
                    mime: "image/png".into(),
                },
                ParsedImage {
                    bytes: img_b,
                    mime: "image/jpeg".into(),
                },
            ],
            chapters: vec![bookshelf_formats::ParsedChapter {
                title: "c0".into(),
                content: "<p><img src=\"image:1\"/></p><figure><img src=\"image:0\"/></figure>\
                     <p><img src=\"image:0\"/></p>"
                    .into(),
            }],
            toc: vec![],
        };
        ingest_parsed_book(&db, &dir, &mut parsed).await.unwrap();

        // Both placeholders rewritten to the canonical reference form.
        let content = &parsed.chapters[0].content;
        assert!(!content.contains("src=\"image:"));
        assert_eq!(content.matches("/api/images/").count(), 3);

        // Rows + bytes stored, deduped by content (two ids total).
        let rows: Vec<(String, String, i64)> =
            sqlx::query_as("SELECT id, mime, size FROM images ORDER BY id")
                .fetch_all(&db)
                .await
                .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(_, _, size)| *size > 0));
        for row in &rows {
            assert!(dir.join("images").join(&row.0).exists());
        }
        let id_a = sha256_hex(&img_a);
        assert!(content.contains(&format!("/api/images/{id_a}")));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
