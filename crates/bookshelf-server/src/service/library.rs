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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use tracing::{info, warn};

use bookshelf_core::model::{
    BookMeta, Chapter, ChapterMeta, FileFormat, FileMeta, ImageId, Role, SeriesMeta, TocNode, User,
    Visibility,
};
use bookshelf_core::source::{SourceBook, SourceBookFile, SourceChapter, SourceVolume};
use bookshelf_core::{BookExt, SeriesExt};
use bookshelf_formats::ParsedBook;

// `NULLIF(a, b)` — not built into Diesel's DSL; declared once so upsert
// guards stay pure expressions (no raw SQL strings).
diesel::define_sql_function! {
    #[sql_name = "NULLIF"]
    fn nullif(x: diesel::sql_types::Text, y: diesel::sql_types::Text) -> diesel::sql_types::Nullable<diesel::sql_types::Text>;
}

// `COALESCE(?, col)` for optional string updates: `None` keeps the
// stored value (nullable target column).
diesel::define_sql_function! {
    #[sql_name = "COALESCE"]
    fn coalesce_opt(
        x: diesel::sql_types::Nullable<diesel::sql_types::Text>,
        y: diesel::sql_types::Nullable<diesel::sql_types::Text>,
    ) -> diesel::sql_types::Nullable<diesel::sql_types::Text>;
}

// Same, for NOT NULL columns.
diesel::define_sql_function! {
    #[sql_name = "COALESCE"]
    fn coalesce_opt_nn(
        x: diesel::sql_types::Nullable<diesel::sql_types::Text>,
        y: diesel::sql_types::Text,
    ) -> diesel::sql_types::Text;
}

// `LEAST(a, b)` — declared like the other SQLite builtins Diesel lacks.
diesel::define_sql_function! {
    #[sql_name = "MIN"]
    fn least(x: diesel::sql_types::BigInt, y: diesel::sql_types::BigInt) -> diesel::sql_types::BigInt;
}

diesel::define_sql_function! {
    #[sql_name = "MIN"]
    fn least_double(x: diesel::sql_types::Double, y: diesel::sql_types::Double) -> diesel::sql_types::Double;
}

// `COALESCE(a, b)` — also declared, for the same reason.
diesel::define_sql_function! {
    #[sql_name = "COALESCE"]
    fn coalesce(
        x: diesel::sql_types::Nullable<diesel::sql_types::Text>,
        y: diesel::sql_types::Text,
    ) -> diesel::sql_types::Nullable<diesel::sql_types::Text>;
}

// Same function, non-nullable result (for upsert guards over NOT NULL
// columns).
diesel::define_sql_function! {
    #[sql_name = "COALESCE"]
    fn coalesce_nn(
        x: diesel::sql_types::Nullable<diesel::sql_types::Text>,
        y: diesel::sql_types::Text,
    ) -> diesel::sql_types::Text;
}

/// Validate + normalize a source's extended metadata object and render it
/// as the stored column value. Invalid payloads fail the acquisition —
/// bad data never lands (docs/metadata-ext-design.md).
fn source_ext_column(ext: Option<&serde_json::Value>) -> Result<String, ApiError> {
    match ext {
        None => Ok("{}".into()),
        Some(v) => BookExt::from_value(v.clone())
            .map(|e| e.to_column())
            .map_err(ApiError::bad_request),
    }
}

// Diesel translation (docs/sql-refactor-plan.md): module-wide traits so
// individual functions stay readable; only the async RunQueryDsl is
// imported (the sync one would be ambiguous on execute/first/load).
use crate::db::DieselDb;
use crate::schema;
use diesel::BoolExpressionMethods as _;
use diesel::ExpressionMethods as _;
use diesel::OptionalExtension as _;
use diesel::QueryDsl as _;
use diesel::expression_methods::NullableExpressionMethods as _;
use diesel::expression_methods::TextExpressionMethods as _;
use diesel::prelude::SelectableHelper as _;
use diesel_async::RunQueryDsl as _;

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
    /// No content: add a metadata entry on its own (a book without any
    /// file yet). Only valid with [`AcquireMetadata::Plugin`] (metadata
    /// from a plugin catalog) or [`AcquireMetadata::New`] (a manually
    /// described entry, which needs a title); [`AcquireMetadata::Attach`]
    /// has nothing to attach and is rejected.
    None,
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
    /// catalog picker). The content is independent of this source: an
    /// uploaded file, another plugin's book, or none (metadata only).
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
/// parser or a plugin source. `ext` overrides per-key (see
/// [`BookExt::apply_override`]).
#[derive(Default)]
pub struct MetadataOverrides {
    pub title: Option<String>,
    pub authors: Option<Vec<String>>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub ext: Option<BookExt>,
}

impl MetadataOverrides {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.authors.is_none()
            && self.description.is_none()
            && self.cover_url.is_none()
            && self.ext.is_none()
    }
}

/// Result of one acquisition: the series created by a volume split (when
/// the source declared >1 volumes) plus the books that entered the
/// library, each with the file stored under it (`None` for a
/// metadata-only addition, which has no content yet).
#[derive(Debug, Clone)]
pub struct AcquireOutcome {
    pub series: Option<SeriesMeta>,
    /// Non-empty; one element unless the acquisition split volumes.
    pub books: Vec<(BookMeta, Option<FileMeta>)>,
}

pub struct Library {
    diesel_db: crate::db::DieselDb,
    plugins: Arc<PluginService>,
    /// Directory for retained original file bytes
    /// (`data/files/{file_id}.{ext}`; see docs/storage-unification-design.md).
    files_dir: PathBuf,
}

impl Library {
    pub fn new(
        diesel_db: crate::db::DieselDb,
        plugins: Arc<PluginService>,
        files_dir: PathBuf,
    ) -> Self {
        Library {
            diesel_db,
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
        {
            let mut conn = self.diesel_db.get().await?;
            for entry in result.items {
                let book = SourceBook::try_from(entry)
                    .map_err(|e| ApiError::bad_request(format!("invalid search result: {e}")))?;
                let book_id: Option<String> = schema::book_files::table
                    .filter(
                        schema::book_files::source
                            .eq(instance)
                            .and(schema::book_files::external_id.eq(&book.id)),
                    )
                    .select(schema::book_files::book_id)
                    .first(&mut conn)
                    .await
                    .optional()?;
                items.push((book, book_id));
            }
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
        let (book, file) =
            outcome.books.into_iter().next().ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("acquisition produced no books"))
            })?;
        let file = file
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("acquisition produced no file")))?;
        Ok((book, file))
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
        ingest_parsed_book(&self.diesel_db, &self.files_dir, &mut parsed).await?;
        // The virtual row for (source, external_id) under `book_id` was
        // created by `ensure_plugin_book` / `attach_plugin_file`; rewrite
        // it into a stored file: self-contained (no content indirection),
        // real format/toc/chapter count.
        let file_id: String = schema::book_files::table
            .filter(
                schema::book_files::source
                    .eq(source)
                    .and(schema::book_files::external_id.eq(&entry.id)),
            )
            .select(schema::book_files::id)
            .first(&mut self.diesel_db.get().await?)
            .await?;
        let format = detect_format(&book_file.filename);
        let toc_json = serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());
        let label = format!("{source} 下载");
        // Cache the fetched bytes as the file's original: the source may
        // disappear later; download/reparse should not depend on it.
        let (orig_ext, orig_sha256, orig_size) = self
            .store_original(&file_id, format.as_ref(), &book_file.bytes)
            .await?;

        // Rewrite the virtual row into a stored file: self-contained
        // (no content indirection), real format/toc/chapter count.
        diesel::update(schema::book_files::table.find(&file_id))
            .set((
                schema::book_files::format.eq(format.as_ref()),
                schema::book_files::label.eq(&label),
                schema::book_files::toc.eq(toc_json),
                schema::book_files::content_source.eq(Option::<String>::None),
                schema::book_files::content_external_id.eq(Option::<String>::None),
                schema::book_files::chapter_count.eq(parsed.chapters.len() as i64),
                schema::book_files::orig_ext.eq(&orig_ext),
                schema::book_files::orig_sha256.eq(&orig_sha256),
                schema::book_files::orig_size.eq(orig_size),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        diesel::delete(schema::chapters::table.filter(schema::chapters::file_id.eq(&file_id)))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        {
            let mut conn = self.diesel_db.get().await?;
            for (idx, chapter) in parsed.chapters.iter().enumerate() {
                diesel::insert_into(schema::chapters::table)
                    .values((
                        schema::chapters::file_id.eq(&file_id),
                        schema::chapters::idx.eq(idx as i64),
                        schema::chapters::title.eq(&chapter.title),
                        schema::chapters::content.eq(&chapter.content),
                    ))
                    .execute(&mut conn)
                    .await?;
            }
        }
        info!(
            source = %source,
            book = %book_id,
            file = %file_id,
            format = %format,
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
        let ext_json = source_ext_column(book.ext.as_ref())?;

        // Reuse the metadata entry when this plugin file was synced
        // before, otherwise create both the book metadata and its virtual
        // file.
        let existing: Option<String> = schema::book_files::table
            .filter(
                schema::book_files::source
                    .eq(source_id)
                    .and(schema::book_files::external_id.eq(&book.id)),
            )
            .select(schema::book_files::book_id)
            .first::<String>(&mut self.diesel_db.get().await?)
            .await
            .optional()?;

        let book_id = match existing {
            Some(book_id) => book_id,
            None => {
                let book_id = uuid::Uuid::new_v4().simple().to_string();
                let mut conn = self.diesel_db.get().await?;
                diesel::insert_into(schema::books::table)
                    .values((
                        schema::books::id.eq(&book_id),
                        schema::books::title.eq(&book.title),
                        schema::books::authors.eq(&authors),
                        schema::books::description.eq(&book.description),
                        schema::books::cover_url.eq(&book.cover_url),
                        schema::books::ext_meta.eq(&ext_json),
                        schema::books::created_by.eq(owner),
                    ))
                    .execute(&mut conn)
                    .await?;
                diesel::insert_into(schema::book_files::table)
                    .values((
                        schema::book_files::id.eq(uuid::Uuid::new_v4().simple().to_string()),
                        schema::book_files::book_id.eq(&book_id),
                        schema::book_files::source.eq(source_id),
                        schema::book_files::external_id.eq(&book.id),
                        schema::book_files::content_source.eq(&book.content_source),
                        schema::book_files::content_external_id.eq(&book.content_id),
                        schema::book_files::format.eq(FileFormat::Plugin.as_ref()),
                        schema::book_files::visibility.eq("public"),
                        schema::book_files::owner_id.eq(owner),
                        schema::book_files::chapter_count.eq(titles.len() as i64),
                    ))
                    .execute(&mut conn)
                    .await?;
                book_id
            }
        };

        // Keep metadata fresh on every sync/claim. When a user claims an
        // upload as this plugin book, the metadata gains that user as its
        // creator (so they can maintain and refresh it). Without an owner
        // the column is left untouched — writing `''` would violate the
        // `created_by REFERENCES users(id)` foreign key.
        diesel::update(schema::books::table.find(&book_id))
            .set((
                schema::books::title.eq(&book.title),
                schema::books::authors.eq(&authors),
                schema::books::description.eq(&book.description),
                schema::books::cover_url.eq(&book.cover_url),
                // Refresh replaces the extended metadata wholesale (same
                // overwrite rule as title/description/cover).
                schema::books::ext_meta.eq(&ext_json),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        if let Some(owner) = owner {
            diesel::update(schema::books::table.find(&book_id))
                .set(schema::books::created_by.eq(coalesce(schema::books::created_by, owner)))
                .execute(&mut self.diesel_db.get().await?)
                .await?;
        }
        // Content indirection comes from the entry; the metadata instance
        // owns the `books` row, the content instance the chapters.
        diesel::update(
            schema::book_files::table.filter(
                schema::book_files::source
                    .eq(source_id)
                    .and(schema::book_files::external_id.eq(&book.id)),
            ),
        )
        .set((
            schema::book_files::chapter_count.eq(titles.len() as i64),
            schema::book_files::content_source.eq(&book.content_source),
            schema::book_files::content_external_id.eq(&book.content_id),
        ))
        .execute(&mut self.diesel_db.get().await?)
        .await?;

        let file_id: String = schema::book_files::table
            .filter(
                schema::book_files::source
                    .eq(source_id)
                    .and(schema::book_files::external_id.eq(&book.id)),
            )
            .select(schema::book_files::id)
            .first(&mut self.diesel_db.get().await?)
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
        let mut conn = self.diesel_db.get().await?;
        for (idx, chapter) in chapters.iter().enumerate() {
            let content = annotate_image_dims(
                &self.diesel_db,
                &bookshelf_formats::htmlize::plugin_text_to_html(&chapter.content),
            )
            .await;
            // Empty bodies never overwrite materialized content: keep the
            // stored text when the incoming chapter is a lazy placeholder.
            diesel::insert_into(schema::chapters::table)
                .values((
                    schema::chapters::file_id.eq(file_id),
                    schema::chapters::idx.eq(idx as i64),
                    schema::chapters::title.eq(chapter.title.trim()),
                    schema::chapters::content.eq(&content),
                ))
                .on_conflict((schema::chapters::file_id, schema::chapters::idx))
                .do_update()
                .set((
                    schema::chapters::title.eq(diesel::upsert::excluded(schema::chapters::title)),
                    schema::chapters::content.eq(coalesce_nn(
                        nullif(diesel::upsert::excluded(schema::chapters::content), ""),
                        schema::chapters::content,
                    )),
                ))
                .execute(&mut conn)
                .await?;
        }
        Ok(())
    }

    /// The virtual file row of a plugin book.
    async fn plugin_file(
        &self,
        source: &str,
        external_id: &str,
    ) -> Result<Option<FileMeta>, ApiError> {
        let mut conn = self.diesel_db.get().await?;
        let row: Option<FileRow> = schema::book_files::table
            .filter(
                schema::book_files::source
                    .eq(source)
                    .and(schema::book_files::external_id.eq(external_id)),
            )
            .select(FileRow::as_select())
            .first(&mut conn)
            .await
            .optional()?;
        drop(conn);
        row.map(FileRow::into_model).transpose()
    }

    /// Insert a bare metadata entry (no files yet) — a book added on its
    /// own (`AcquireContent::None`) so its content can be attached later.
    #[allow(clippy::too_many_arguments)]
    async fn create_metadata_entry(
        &self,
        title: &str,
        authors: &[String],
        description: Option<&str>,
        cover_url: Option<&str>,
        ext: Option<&BookExt>,
        owner_id: &str,
    ) -> Result<String, ApiError> {
        let book_id = uuid::Uuid::new_v4().simple().to_string();
        let authors_json = serde_json::to_string(authors).unwrap_or_else(|_| "[]".into());
        let mut ext = ext.cloned().unwrap_or_default();
        ext.sanitize().map_err(ApiError::bad_request)?;
        diesel::insert_into(schema::books::table)
            .values((
                schema::books::id.eq(&book_id),
                schema::books::title.eq(title),
                schema::books::authors.eq(authors_json),
                schema::books::description.eq(description.map(str::to_string)),
                schema::books::cover_url.eq(cover_url.map(str::to_string)),
                schema::books::ext_meta.eq(ext.to_column()),
                schema::books::created_by.eq(owner_id),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        Ok(book_id)
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
                ingest_parsed_book(&self.diesel_db, &self.files_dir, &mut parsed).await?;
                // The caller must be able to see the book (its owner, an
                // admin, via a public file, or as a metadata-only entry
                // they created).
                let book = self
                    .get_book(book_id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("book"))?;
                let owns = user.role == Role::Admin
                    || book.created_by.as_deref() == Some(user.id.as_str());
                if !owns && self.files_of_book(book_id, user).await?.is_empty() {
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
                (book_id.to_string(), Some(file))
            }

            // ---- attach a plugin book's content to existing metadata ---
            (
                AcquireContent::Plugin {
                    source,
                    book_id_in_source,
                },
                AcquireMetadata::Attach { book_id },
            ) => {
                let book = self
                    .get_book(book_id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("book"))?;
                let owns = user.role == Role::Admin
                    || book.created_by.as_deref() == Some(user.id.as_str());
                if !owns && self.files_of_book(book_id, user).await?.is_empty() {
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
                let existing: Option<String> = schema::book_files::table
                    .filter(
                        schema::book_files::source
                            .eq(source)
                            .and(schema::book_files::external_id.eq(&entry.id)),
                    )
                    .select(schema::book_files::book_id)
                    .first(&mut self.diesel_db.get().await?)
                    .await
                    .optional()?;
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
                (book_id.to_string(), Some(file))
            }

            // ---- metadata only (no content) ----------------------------
            // Nothing to attach: a metadata-only addition must describe
            // its own entry (plugin catalog or manual fields).
            (AcquireContent::None, AcquireMetadata::Attach { .. }) => {
                return Err(ApiError::bad_request(
                    "a metadata-only addition has no content to attach — drop `book_id` or \
                     provide a file / plugin book",
                ));
            }

            // A catalog entry's metadata, stored on its own (no virtual
            // file): the content may come later, e.g. from an upload
            // attached to this entry.
            (
                AcquireContent::None,
                AcquireMetadata::Plugin {
                    source,
                    book_id_in_source,
                    overrides,
                },
            ) => {
                let entry = self.plugin_book_entry(source, book_id_in_source).await?;
                // Idempotent: if the book already has content, reuse its
                // metadata entry instead of creating a duplicate.
                let existing: Option<String> = schema::book_files::table
                    .filter(
                        schema::book_files::source
                            .eq(source)
                            .and(schema::book_files::external_id.eq(&entry.id)),
                    )
                    .select(schema::book_files::book_id)
                    .first(&mut self.diesel_db.get().await?)
                    .await
                    .optional()?;
                let book_id = match existing {
                    Some(book_id) => book_id,
                    None => {
                        let ext = entry
                            .ext
                            .as_ref()
                            .map(|v| BookExt::from_value(v.clone()))
                            .transpose()
                            .map_err(ApiError::bad_request)?;
                        self.create_metadata_entry(
                            &entry.title,
                            &entry.authors,
                            entry.description.as_deref(),
                            entry.cover_url.as_deref(),
                            ext.as_ref(),
                            &user.id,
                        )
                        .await?
                    }
                };
                if !overrides.is_empty() {
                    self.update_book(
                        &book_id,
                        overrides.title.as_deref(),
                        overrides.description.as_deref(),
                        overrides.authors.as_ref(),
                        overrides.cover_url.as_deref(),
                        overrides.ext.as_ref(),
                    )
                    .await?;
                }
                (book_id, None)
            }

            // ---- metadata from plugin A, content from plugin B --------
            // The two sources are fully independent: the metadata entry
            // is created from A's `book-entry`, the content (file or
            // chapter materialization) comes from B. Same source + id
            // takes this path too and behaves like the auto mode.
            (
                AcquireContent::Plugin {
                    source: content_source,
                    book_id_in_source: content_book_id,
                },
                AcquireMetadata::Plugin {
                    source: meta_source,
                    book_id_in_source: meta_book_id,
                    overrides,
                },
            ) => {
                let meta_entry = self.plugin_book_entry(meta_source, meta_book_id).await?;
                let content_entry = self
                    .plugin_book_entry(content_source, content_book_id)
                    .await?;
                // A multi-volume source book cannot be squeezed into one
                // metadata entry here: the split path above declined
                // (e.g. collapsed volume slices), so refuse like attach.
                if content_entry.volumes.len() > 1 {
                    return Err(ApiError::bad_request(
                        "this plugin book spans several volumes but its volume slices are empty; \
                         acquire it as a single-volume book instead",
                    ));
                }
                // One plugin book = one library entry: B's content must
                // not already live under a different metadata entry.
                let existing: Option<String> = schema::book_files::table
                    .filter(
                        schema::book_files::source
                            .eq(content_source)
                            .and(schema::book_files::external_id.eq(&content_entry.id)),
                    )
                    .select(schema::book_files::book_id)
                    .first(&mut self.diesel_db.get().await?)
                    .await
                    .optional()?;
                if let Some(other) = existing {
                    return Err(ApiError::Conflict(format!(
                        "plugin book `{content_book_id}` is already in the library under \
                         metadata `{other}`; attach another source instead"
                    )));
                }
                // Fresh metadata entry from A's catalog data, with the
                // user's fields applied on top. (Already-synced copies of
                // A's entry under other books are deliberately not reused:
                // one metadata per acquisition keeps the flow simple.)
                let ext = meta_entry
                    .ext
                    .as_ref()
                    .map(|v| BookExt::from_value(v.clone()))
                    .transpose()
                    .map_err(ApiError::bad_request)?;
                let final_ext: Option<BookExt> = match &overrides.ext {
                    Some(o) => {
                        let mut e = ext.unwrap_or_default();
                        e.apply_override(o);
                        Some(e)
                    }
                    None => ext,
                };
                let book_id = self
                    .create_metadata_entry(
                        overrides
                            .title
                            .as_deref()
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                            .unwrap_or(&meta_entry.title),
                        overrides.authors.as_ref().unwrap_or(&meta_entry.authors),
                        overrides
                            .description
                            .as_deref()
                            .or(meta_entry.description.as_deref()),
                        overrides
                            .cover_url
                            .as_deref()
                            .or(meta_entry.cover_url.as_deref()),
                        final_ext.as_ref(),
                        &user.id,
                    )
                    .await?;
                self.attach_plugin_file(content_source, Some(&user.id), &content_entry, &book_id)
                    .await?;
                let file = self
                    .materialize_plugin_content(content_source, &content_entry, &book_id)
                    .await?;
                (book_id, Some(file))
            }

            // ---- metadata from a named plugin source (file / none) ------
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
                        overrides.ext.as_ref(),
                    )
                    .await?;
                }
                match content {
                    AcquireContent::File { bytes, filename } => {
                        let mut parsed = bookshelf_formats::parse(bytes, filename)?;
                        ingest_parsed_book(&self.diesel_db, &self.files_dir, &mut parsed).await?;
                        let file = self
                            .store_local_file(
                                &parsed, bytes, &book_id, &user.id, filename, visibility, label,
                            )
                            .await?;
                        (book_id, Some(file))
                    }
                    AcquireContent::Plugin { .. } | AcquireContent::None => {
                        unreachable!("plugin content handled above; metadata-only earlier")
                    }
                }
            }

            // ---- new metadata from an uploaded file ---------------------
            // Auto: let plugins identify the file (first match wins),
            // otherwise create metadata parsed from the file. `overrides`
            // correct the produced metadata in both cases.
            (AcquireContent::File { bytes, filename }, AcquireMetadata::New { overrides }) => {
                let mut parsed = bookshelf_formats::parse(bytes, filename)?;
                ingest_parsed_book(&self.diesel_db, &self.files_dir, &mut parsed).await?;
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
                                overrides.ext.as_ref(),
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
                (book_id, Some(file))
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
                        overrides.ext.as_ref(),
                    )
                    .await?;
                }
                let file = self
                    .materialize_plugin_content(source, &entry, &book_id)
                    .await?;
                (book_id, Some(file))
            }

            // ---- a manually described entry with no content -----------
            (AcquireContent::None, AcquireMetadata::New { overrides }) => {
                // At least a title is required (there is nothing to parse
                // it from).
                let title = overrides
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| {
                        ApiError::bad_request("a metadata-only addition needs a title")
                    })?;
                let authors = overrides.authors.clone().unwrap_or_default();
                let book_id = self
                    .create_metadata_entry(
                        title,
                        &authors,
                        overrides.description.as_deref(),
                        overrides.cover_url.as_deref(),
                        overrides.ext.as_ref(),
                        &user.id,
                    )
                    .await?;
                (book_id, None)
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
    /// when the *content* source's `book-entry` declared >1 volumes and
    /// the split was performed; `None` when the book is effectively
    /// single-volume.
    ///
    /// Only plugin *content* splits (an uploaded file is one complete
    /// edition and never does): the guard in [`Self::acquire_book`]
    /// limits this path to `AcquireContent::Plugin` + new/plugin
    /// metadata. The metadata base is the metadata mode's own entry —
    /// which may live on another plugin than the content.
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
        // The metadata base: the named plugin's entry when the metadata
        // comes from a (possibly different) source, otherwise the
        // content entry itself (auto mode).
        let (base, overrides) = match metadata {
            AcquireMetadata::New { overrides } => (entry.clone(), overrides),
            AcquireMetadata::Plugin {
                source: meta_source,
                book_id_in_source: meta_book_id,
                overrides,
            } => (
                self.plugin_book_entry(meta_source, meta_book_id).await?,
                overrides,
            ),
            AcquireMetadata::Attach { .. } => unreachable!("guarded by the caller"),
        };
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

        // One plugin book = one library series: reject materializing the
        // same source book twice (incl. pre-split merged entries).
        let existing: Option<String> = schema::book_files::table
            .filter(
                schema::book_files::source
                    .eq(source)
                    .and(schema::book_files::external_id.eq(&entry.id)),
            )
            .select(schema::book_files::book_id)
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
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
            .unwrap_or_else(|| base.title.clone());
        let authors_json =
            serde_json::to_string(overrides.authors.as_ref().unwrap_or(&base.authors))
                .unwrap_or_else(|_| "[]".into());
        let description = overrides
            .description
            .clone()
            .or_else(|| base.description.clone());
        let cover_url = overrides
            .cover_url
            .clone()
            .or_else(|| base.cover_url.clone());
        let series_ext =
            SeriesExt::from_source(base.ext.as_ref()).map_err(ApiError::bad_request)?;
        // Extended metadata: the metadata entry's object, manual
        // overrides win per-key. The series keeps its own subset of the
        // same object.
        let mut book_ext = match &base.ext {
            Some(v) => BookExt::from_value(v.clone()).map_err(ApiError::bad_request)?,
            None => BookExt::default(),
        };
        if let Some(o) = &overrides.ext {
            book_ext.apply_override(o);
        }
        let book_ext_json = book_ext.to_column();

        let series_id = uuid::Uuid::new_v4().simple().to_string();

        // Series + volume books + files + placeholder rows. Note: these
        // run in autocommit (a mid-loop failure leaves partial volumes
        // behind; deleting the series cleans them up). The diesel-async
        // transaction closure fights higher-ranked inference with loops,
        // so we keep it simple here.
        let mut conn = self.diesel_db.get().await?;
        let mut books = Vec::with_capacity(slices.len());
        diesel::insert_into(schema::series::table)
            .values((
                schema::series::id.eq(&series_id),
                schema::series::title.eq(&title),
                schema::series::authors.eq(&authors_json),
                schema::series::description.eq(&description),
                schema::series::cover_url.eq(&cover_url),
                schema::series::ext_meta.eq(series_ext.to_column()),
                schema::series::created_by.eq(&user.id),
            ))
            .execute(&mut conn)
            .await?;

        for (index, (vol_title, start, count)) in slices.iter().enumerate() {
            let volume_no = (index + 1) as u32;
            let book_id = uuid::Uuid::new_v4().simple().to_string();
            diesel::insert_into(schema::books::table)
                .values((
                    schema::books::id.eq(&book_id),
                    schema::books::title.eq(&title),
                    schema::books::authors.eq(&authors_json),
                    schema::books::description.eq(&description),
                    schema::books::cover_url.eq(&cover_url),
                    schema::books::series_id.eq(&series_id),
                    schema::books::volume_no.eq(volume_no as i64),
                    schema::books::ext_meta.eq(&book_ext_json),
                    schema::books::created_by.eq(&user.id),
                ))
                .execute(&mut conn)
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
            diesel::insert_into(schema::book_files::table)
                .values((
                    schema::book_files::id.eq(&file_id),
                    schema::book_files::book_id.eq(&book_id),
                    schema::book_files::source.eq(source),
                    schema::book_files::external_id.eq(&entry.id),
                    schema::book_files::content_source.eq(&entry.content_source),
                    schema::book_files::content_external_id.eq(&entry.content_id),
                    schema::book_files::format.eq(FileFormat::Plugin.as_ref()),
                    schema::book_files::label.eq(&file_label),
                    schema::book_files::visibility.eq(visibility.as_ref()),
                    schema::book_files::owner_id.eq(&user.id),
                    schema::book_files::chapter_count.eq(*count as i64),
                    schema::book_files::volume_no.eq(volume_no as i64),
                    schema::book_files::volume_offset.eq(*start as i64),
                ))
                .execute(&mut conn)
                .await?;

            // Title-only placeholder rows for this volume's slice.
            let slice = &titles[*start as usize..(*start + *count) as usize];
            for (idx, t) in slice.iter().enumerate() {
                diesel::insert_into(schema::chapters::table)
                    .values((
                        schema::chapters::file_id.eq(&file_id),
                        schema::chapters::idx.eq(idx as i64),
                        schema::chapters::title.eq(t.trim()),
                        schema::chapters::content.eq(""),
                    ))
                    .execute(&mut conn)
                    .await?;
            }

            books.push((
                BookMeta {
                    id: book_id.clone(),
                    title: title.clone(),
                    authors: parse_authors(&authors_json),
                    description: description.clone(),
                    cover_url: cover_url.clone(),
                    ext: book_ext.clone(),
                    created_by: Some(user.id.clone()),
                    created_at: chrono::Utc::now(),
                    series_id: Some(series_id.clone()),
                    volume_no,
                },
                Some(FileMeta {
                    id: file_id,
                    book_id,
                    source: source.to_string(),
                    external_id: entry.id.clone(),
                    content_source: entry.content_source.clone(),
                    content_external_id: entry.content_id.clone(),
                    format: FileFormat::Plugin,
                    label: file_label,
                    visibility,
                    owner_id: Some(user.id.clone()),
                    chapter_count: *count,
                    created_at: chrono::Utc::now(),
                    volume_no,
                    volume_offset: *start,
                    original: None,
                }),
            ));
        }
        drop(conn);

        let series = SeriesMeta {
            id: series_id,
            title: title.clone(),
            authors: parse_authors(&authors_json),
            description: description.clone(),
            cover_url: cover_url.clone(),
            ext: series_ext,
            created_by: Some(user.id.clone()),
            created_at: chrono::Utc::now(),
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
            .map(SourceBook::try_from)
            .transpose()
            .map_err(|e| ApiError::bad_request(format!("invalid plugin book entry: {e}")))?
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
        let existing: Option<String> = schema::book_files::table
            .filter(
                schema::book_files::source
                    .eq(source)
                    .and(schema::book_files::external_id.eq(&book.id)),
            )
            .select(schema::book_files::id)
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        if let Some(file_id) = existing {
            return Ok(file_id);
        }
        let file_id = uuid::Uuid::new_v4().simple().to_string();
        diesel::insert_into(schema::book_files::table)
            .values((
                schema::book_files::id.eq(&file_id),
                schema::book_files::book_id.eq(book_id),
                schema::book_files::source.eq(source),
                schema::book_files::external_id.eq(&book.id),
                schema::book_files::content_source.eq(&book.content_source),
                schema::book_files::content_external_id.eq(&book.content_id),
                schema::book_files::format.eq(FileFormat::Plugin.as_ref()),
                schema::book_files::visibility.eq("public"),
                schema::book_files::owner_id.eq(owner),
                schema::book_files::chapter_count.eq(0),
            ))
            .execute(&mut self.diesel_db.get().await?)
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
                diesel::update(schema::book_files::table.find(&file.id))
                    .set((
                        schema::book_files::chapter_count.eq(titles.len() as i64),
                        schema::book_files::content_source.eq(&entry.content_source),
                        schema::book_files::content_external_id.eq(&entry.content_id),
                    ))
                    .execute(&mut self.diesel_db.get().await?)
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
        // Extended metadata: parsed (epub OPF) first, manual overrides
        // win per-key.
        let mut ext = parsed.ext.clone();
        if let Some(o) = &overrides.ext {
            ext.apply_override(o);
        }
        ext.sanitize().map_err(ApiError::bad_request)?;
        diesel::insert_into(schema::books::table)
            .values((
                schema::books::id.eq(&book_id),
                schema::books::title.eq(overrides
                    .title
                    .clone()
                    .unwrap_or_else(|| parsed.title.clone())),
                schema::books::authors.eq(authors),
                schema::books::description.eq(overrides
                    .description
                    .clone()
                    .or_else(|| parsed.description.clone())),
                schema::books::cover_url.eq(overrides
                    .cover_url
                    .clone()
                    .or_else(|| parsed.cover_url.clone())),
                schema::books::cover.eq(parsed.cover.as_ref().map(|c| c.bytes.clone())),
                schema::books::cover_mime.eq(parsed.cover.as_ref().map(|c| c.mime.clone())),
                schema::books::ext_meta.eq(ext.to_column()),
                schema::books::created_by.eq(owner_id),
            ))
            .execute(&mut self.diesel_db.get().await?)
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
        diesel::update(schema::books::table.find(book_id))
            .filter(schema::books::cover.is_null())
            .set((
                schema::books::cover.eq(cover.bytes.clone()),
                schema::books::cover_mime.eq(cover.mime.clone()),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        Ok(())
    }

    /// The stored cover image of a metadata entry (bytes, mime), if any.
    pub async fn get_cover(&self, book_id: &str) -> Result<Option<(Vec<u8>, String)>, ApiError> {
        let row: Option<(Option<Vec<u8>>, Option<String>)> = schema::books::table
            .find(book_id)
            .select((schema::books::cover, schema::books::cover_mime))
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
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
        let (orig_ext, orig_sha256, orig_size) = self
            .store_original(&file_id, format.as_ref(), bytes)
            .await?;

        let mut tx = self.diesel_db.get().await?;
        diesel::insert_into(schema::book_files::table)
            .values((
                schema::book_files::id.eq(&file_id),
                schema::book_files::book_id.eq(book_id),
                schema::book_files::source.eq("local"),
                schema::book_files::external_id.eq(&file_id),
                schema::book_files::format.eq(format.as_ref()),
                schema::book_files::label.eq(label),
                schema::book_files::visibility.eq(visibility.as_ref()),
                schema::book_files::owner_id.eq(owner_id),
                schema::book_files::chapter_count.eq(parsed.chapters.len() as i64),
                schema::book_files::toc.eq(toc_json),
                schema::book_files::orig_ext.eq(&orig_ext),
                schema::book_files::orig_sha256.eq(&orig_sha256),
                schema::book_files::orig_size.eq(orig_size),
            ))
            .execute(&mut tx)
            .await?;

        for (idx, chapter) in parsed.chapters.iter().enumerate() {
            diesel::insert_into(schema::chapters::table)
                .values((
                    schema::chapters::file_id.eq(&file_id),
                    schema::chapters::idx.eq(idx as i64),
                    schema::chapters::title.eq(&chapter.title),
                    schema::chapters::content.eq(&chapter.content),
                ))
                .execute(&mut tx)
                .await?;
        }

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
        let mut conn = self.diesel_db.get().await?;

        // Books owning at least one visible file, plus metadata-only
        // entries (no files yet) owned by the caller. Optional filters are
        // assembled in Rust (type-checked builder) instead of the old
        // parameterized switches. Series first (in volume order), then
        // standalone books, newest first.
        let mut book_q = schema::books::table
            .left_join(schema::book_files::table)
            .filter(schema::books::title.like(&pattern))
            .select(BookRow::as_select())
            .distinct()
            .into_boxed();
        if let Some(source) = source {
            book_q = book_q.filter(schema::book_files::source.eq(source));
        }
        if !is_admin {
            book_q = book_q.filter(
                schema::book_files::visibility
                    .eq("public")
                    .or(schema::book_files::owner_id.eq(&user.id))
                    .or(schema::book_files::id
                        .is_null()
                        .and(schema::books::created_by.eq(&user.id))),
            );
        }
        let rows: Vec<BookRow> = book_q
            .order((
                schema::books::series_id.is_not_null().desc(),
                schema::books::series_id.asc(),
                schema::books::volume_no.asc(),
                schema::books::created_at.desc(),
            ))
            .load(&mut conn)
            .await?;

        // Batch-load the visible files of all listed books in one query
        // (fixes the per-book N+1 of the sqlx version). Source filtering
        // only decides which books are listed; every file of a listed
        // book is shown, like the old files_of_book loop.
        let ids: Vec<String> = rows.iter().map(|b| b.id.clone()).collect();
        let mut file_q = schema::book_files::table
            .filter(schema::book_files::book_id.eq_any(ids))
            .order(schema::book_files::created_at.desc())
            .select(FileRow::as_select())
            .into_boxed();
        if !is_admin {
            file_q = file_q.filter(
                schema::book_files::visibility
                    .eq("public")
                    .or(schema::book_files::owner_id.eq(&user.id)),
            );
        }
        let file_rows: Vec<FileRow> = file_q.load(&mut conn).await?;
        drop(conn);

        let mut by_book: std::collections::HashMap<String, Vec<FileMeta>> =
            std::collections::HashMap::new();
        for row in file_rows {
            let meta = row.into_model()?;
            by_book.entry(meta.book_id.clone()).or_default().push(meta);
        }

        let mut books = Vec::new();
        for row in rows {
            let book = row.into_model()?;
            let files = by_book.remove(&book.id).unwrap_or_default();
            // Metadata-only entries (no visible files) belong to the
            // caller and were already admitted by the join filter above.
            books.push((book, files));
        }
        Ok(books)
    }

    pub async fn get_book(&self, id: &str) -> Result<Option<BookMeta>, ApiError> {
        let row: Option<BookRow> = schema::books::table
            .find(id)
            .select(BookRow::as_select())
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        row.map(BookRow::into_model).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_book(
        &self,
        id: &str,
        title: Option<&str>,
        description: Option<&str>,
        authors: Option<&Vec<String>>,
        cover_url: Option<&str>,
        ext: Option<&BookExt>,
    ) -> Result<BookMeta, ApiError> {
        let authors_json =
            authors.map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".into()));
        // Extended metadata merges per-key on top of the stored object
        // (create-time overrides never clear what the source produced).
        if let Some(ext) = ext {
            self.merge_book_ext(id, &serde_json::to_value(ext).unwrap_or_default())
                .await?;
        }
        diesel::update(schema::books::table.find(id))
            .set((
                schema::books::title.eq(coalesce_opt_nn(title, schema::books::title)),
                schema::books::description.eq(coalesce_opt(
                    description.map(str::to_string),
                    schema::books::description,
                )),
                schema::books::authors.eq(coalesce_opt_nn(authors_json, schema::books::authors)),
                schema::books::cover_url.eq(coalesce_opt(
                    cover_url.map(str::to_string),
                    schema::books::cover_url,
                )),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        self.get_book(id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))
    }

    /// Merge-patch a book's extended metadata (`PATCH /api/books/{id}`):
    /// absent key = keep, `null` = clear, value = set. The merged object
    /// is re-validated (ISBN normalization etc.) before it lands.
    pub async fn merge_book_ext(
        &self,
        id: &str,
        patch: &serde_json::Value,
    ) -> Result<(), ApiError> {
        let Some(obj) = patch.as_object() else {
            return Err(ApiError::bad_request("`ext` must be a JSON object"));
        };
        let stored: String = schema::books::table
            .find(id)
            .select(schema::books::ext_meta)
            .first(&mut self.diesel_db.get().await?)
            .await?;
        let mut map = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&stored)
            .unwrap_or_default();
        for (k, v) in obj {
            if v.is_null() {
                map.remove(k);
            } else {
                map.insert(k.clone(), v.clone());
            }
        }
        let ext =
            BookExt::from_value(serde_json::Value::Object(map)).map_err(ApiError::bad_request)?;
        diesel::update(schema::books::table.find(id))
            .set(schema::books::ext_meta.eq(ext.to_column()))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        Ok(())
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
        let plugin_files: Vec<(String, String, String, i64, i64, i64)> = schema::book_files::table
            .filter(
                schema::book_files::book_id
                    .eq(book_id)
                    .and(schema::book_files::source.ne("local")),
            )
            .select((
                schema::book_files::id,
                schema::book_files::source,
                schema::book_files::external_id,
                schema::book_files::volume_no,
                schema::book_files::volume_offset,
                schema::book_files::chapter_count,
            ))
            .load(&mut self.diesel_db.get().await?)
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
            let entry = SourceBook::try_from(entry)
                .map_err(|e| ApiError::bad_request(format!("invalid plugin book entry: {e}")))?;

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
            let ext_json = source_ext_column(entry.ext.as_ref())?;
            diesel::update(schema::books::table.find(book_id))
                .set((
                    schema::books::title.eq(&entry.title),
                    schema::books::authors.eq(authors),
                    schema::books::description.eq(&entry.description),
                    schema::books::cover_url.eq(&entry.cover_url),
                    // Refresh replaces the extended metadata wholesale.
                    schema::books::ext_meta.eq(&ext_json),
                ))
                .execute(&mut self.diesel_db.get().await?)
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
            diesel::update(schema::book_files::table.find(file_id))
                .set((
                    schema::book_files::chapter_count.eq(slice.len() as i64),
                    schema::book_files::content_source.eq(&entry.content_source),
                    schema::book_files::content_external_id.eq(&entry.content_id),
                ))
                .execute(&mut self.diesel_db.get().await?)
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
        let files: Vec<String> = schema::book_files::table
            .filter(schema::book_files::book_id.eq(id))
            .select(schema::book_files::id)
            .load(&mut self.diesel_db.get().await?)
            .await?;
        if !files.is_empty() {
            return Err(ApiError::ConflictWithFiles(files));
        }
        let deleted = diesel::delete(schema::books::table.find(id))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        if deleted == 0 {
            return Err(ApiError::not_found("book"));
        }
        Ok(())
    }

    // ---- series -----------------------------------------------------------

    /// Member books of a series in volume order (all members — file
    /// visibility is applied per book by the caller; the series itself
    /// is already visibility-checked).
    pub async fn series_member_books(&self, id: &str) -> Result<Vec<BookMeta>, ApiError> {
        let rows: Vec<BookRow> = schema::books::table
            .filter(schema::books::series_id.eq(id))
            .order(schema::books::volume_no.asc())
            .select(BookRow::as_select())
            .load(&mut self.diesel_db.get().await?)
            .await?;
        rows.into_iter().map(BookRow::into_model).collect()
    }

    /// Is the series visible to the caller (creator, admin, or ≥1 member
    /// book with a visible file)?
    pub async fn series_visible_to(&self, user: &User, series_id: &str) -> Result<bool, ApiError> {
        if user.role == Role::Admin {
            return Ok(true);
        }
        let creator: Option<Option<String>> = schema::series::table
            .find(series_id)
            .select(schema::series::created_by)
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        let creator = creator.ok_or_else(|| ApiError::not_found("series"))?;
        if creator.as_deref() == Some(user.id.as_str()) {
            return Ok(true);
        }
        let count: i64 = schema::books::table
            .inner_join(schema::book_files::table)
            .filter(
                schema::books::series_id.eq(series_id).and(
                    schema::book_files::visibility
                        .eq("public")
                        .or(schema::book_files::owner_id.eq(&user.id)),
                ),
            )
            .count()
            .get_result(&mut self.diesel_db.get().await?)
            .await?;
        Ok(count > 0)
    }

    pub async fn get_series(&self, id: &str) -> Result<Option<SeriesMeta>, ApiError> {
        let row: Option<SeriesRow> = schema::series::table
            .find(id)
            .select(SeriesRow::as_select())
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        Ok(row.map(SeriesRow::into_model))
    }

    /// Number of member books of a series (any visibility).
    pub async fn series_volume_count(&self, id: &str) -> Result<u32, ApiError> {
        let count: i64 = schema::books::table
            .filter(schema::books::series_id.eq(id))
            .count()
            .get_result(&mut self.diesel_db.get().await?)
            .await?;
        Ok(count.max(0) as u32)
    }

    /// All series the caller can see (creator, or ≥1 visible member
    /// file; admins see all), each with its member count.
    pub async fn list_series(&self, user: &User) -> Result<Vec<(SeriesMeta, u32)>, ApiError> {
        let mut rows_q = schema::series::table
            .select(SeriesRow::as_select())
            .order(schema::series::created_at.desc())
            .into_boxed();
        if user.role != Role::Admin {
            rows_q = rows_q.filter(
                schema::series::created_by
                    .eq(&user.id)
                    .or(diesel::dsl::exists(
                        schema::books::table
                            .inner_join(schema::book_files::table)
                            .filter(
                                schema::books::series_id
                                    .eq(schema::series::id.nullable())
                                    .and(
                                        schema::book_files::visibility
                                            .eq("public")
                                            .or(schema::book_files::owner_id.eq(&user.id)),
                                    ),
                            ),
                    )),
            );
        }
        let rows: Vec<SeriesRow> = rows_q.load(&mut self.diesel_db.get().await?).await?;

        // Member counts in one grouped query (fixes the per-series COUNT
        // N+1 of the sqlx version).
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let counts: Vec<(Option<String>, i64)> = schema::books::table
            .filter(schema::books::series_id.eq_any(ids))
            .group_by(schema::books::series_id)
            .select((schema::books::series_id, diesel::dsl::count_star()))
            .load(&mut self.diesel_db.get().await?)
            .await?;
        let mut by_series: std::collections::HashMap<String, i64> = counts
            .into_iter()
            .filter_map(|(sid, n)| sid.map(|sid| (sid, n)))
            .collect();

        Ok(rows
            .into_iter()
            .map(|row| {
                let count = by_series.remove(&row.id).unwrap_or(0);
                (row.into_model(), count.max(0) as u32)
            })
            .collect())
    }

    /// Create a series (any logged-in user; the creator manages it).
    pub async fn create_series(
        &self,
        user: &User,
        title: &str,
        authors: Option<&Vec<String>>,
        description: Option<&str>,
        ext: Option<SeriesExt>,
    ) -> Result<SeriesMeta, ApiError> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let authors_json =
            authors.map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".into()));
        diesel::insert_into(schema::series::table)
            .values((
                schema::series::id.eq(&id),
                schema::series::title.eq(title),
                schema::series::authors.eq(authors_json.unwrap_or_else(|| "[]".into())),
                schema::series::description.eq(description),
                schema::series::cover_url.eq(Option::<String>::None),
                schema::series::ext_meta.eq(ext.unwrap_or_default().to_column()),
                schema::series::created_by.eq(&user.id),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        self.get_series(&id)
            .await?
            .ok_or_else(|| ApiError::not_found("series"))
    }

    /// Edit series metadata (creator/admin). `ext_patch` is a JSON
    /// merge-patch over the extended metadata (absent key = keep, null =
    /// clear, value = set).
    pub async fn update_series(
        &self,
        user: &User,
        id: &str,
        title: Option<&str>,
        authors: Option<&Vec<String>>,
        description: Option<&str>,
        ext_patch: Option<&serde_json::Value>,
    ) -> Result<SeriesMeta, ApiError> {
        let series = self
            .get_series(id)
            .await?
            .ok_or_else(|| ApiError::not_found("series"))?;
        if series.created_by.as_deref() != Some(user.id.as_str()) && user.role != Role::Admin {
            return Err(ApiError::Forbidden);
        }
        if let Some(patch) = ext_patch {
            let Some(obj) = patch.as_object() else {
                return Err(ApiError::bad_request("`ext` must be a JSON object"));
            };
            let mut map = serde_json::to_value(series.ext.clone())
                .ok()
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            for (k, v) in obj {
                if v.is_null() {
                    map.remove(k);
                } else {
                    map.insert(k.clone(), v.clone());
                }
            }
            let ext = SeriesExt::from_value(serde_json::Value::Object(map))
                .map_err(ApiError::bad_request)?;
            diesel::update(schema::series::table.find(id))
                .set(schema::series::ext_meta.eq(ext.to_column()))
                .execute(&mut self.diesel_db.get().await?)
                .await?;
        }
        let authors_json =
            authors.map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".into()));
        diesel::update(schema::series::table.find(id))
            .set((
                schema::series::title.eq(coalesce_opt_nn(title, schema::series::title)),
                schema::series::description.eq(coalesce_opt(
                    description.map(str::to_string),
                    schema::series::description,
                )),
                schema::series::authors.eq(coalesce_opt_nn(authors_json, schema::series::authors)),
            ))
            .execute(&mut self.diesel_db.get().await?)
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
        let mut conn = self.diesel_db.get().await?;
        diesel::update(schema::books::table.filter(schema::books::series_id.eq(id)))
            .set((
                schema::books::series_id.eq(Option::<String>::None),
                schema::books::volume_no.eq(0),
            ))
            .execute(&mut conn)
            .await?;
        diesel::delete(schema::series::table.find(id))
            .execute(&mut conn)
            .await?;
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
        let mut conn = self.diesel_db.get().await?;
        diesel::update(schema::books::table.filter(schema::books::series_id.eq(id)))
            .set((
                schema::books::series_id.eq(Option::<String>::None),
                schema::books::volume_no.eq(0),
            ))
            .execute(&mut conn)
            .await?;
        for (i, bid) in book_ids.iter().enumerate() {
            diesel::update(schema::books::table.find(bid))
                .set((
                    schema::books::series_id.eq(id),
                    schema::books::volume_no.eq(i as i64 + 1),
                ))
                .execute(&mut conn)
                .await?;
        }
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
            diesel::update(schema::books::table.find(book_id))
                .set((
                    schema::books::series_id.eq(Option::<String>::None),
                    schema::books::volume_no.eq(0),
                ))
                .execute(&mut self.diesel_db.get().await?)
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
                let max: Option<i64> = schema::books::table
                    .filter(
                        schema::books::series_id
                            .eq(&sid)
                            .and(schema::books::id.ne(book_id)),
                    )
                    .select(diesel::dsl::max(schema::books::volume_no))
                    .first(&mut self.diesel_db.get().await?)
                    .await?;
                (max.unwrap_or(0).max(0) as u32).saturating_add(1)
            }
        };
        // Volume numbers within a series are unique.
        let taken: Option<String> = schema::books::table
            .filter(
                schema::books::series_id
                    .eq(&sid)
                    .and(schema::books::volume_no.eq(vol as i64))
                    .and(schema::books::id.ne(book_id)),
            )
            .select(schema::books::id)
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        if let Some(other) = taken {
            return Err(ApiError::Conflict(format!(
                "第{vol}卷 already belongs to book `{other}`; pick another volume number"
            )));
        }
        diesel::update(schema::books::table.find(book_id))
            .set((
                schema::books::series_id.eq(&sid),
                schema::books::volume_no.eq(vol as i64),
            ))
            .execute(&mut self.diesel_db.get().await?)
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
        let mut q = schema::book_files::table
            .filter(schema::book_files::book_id.eq(book_id))
            .order(schema::book_files::created_at.desc())
            .select(FileRow::as_select())
            .into_boxed();
        if !is_admin {
            q = q.filter(
                schema::book_files::visibility
                    .eq("public")
                    .or(schema::book_files::owner_id.eq(&user.id)),
            );
        }
        let rows: Vec<FileRow> = q.load(&mut self.diesel_db.get().await?).await?;
        rows.into_iter().map(FileRow::into_model).collect()
    }

    pub async fn get_file(&self, id: &str) -> Result<Option<FileMeta>, ApiError> {
        let row: Option<FileRow> = schema::book_files::table
            .find(id)
            .select(FileRow::as_select())
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        row.map(FileRow::into_model).transpose()
    }

    pub async fn update_file(
        &self,
        id: &str,
        visibility: Option<Visibility>,
        label: Option<&str>,
    ) -> Result<FileMeta, ApiError> {
        diesel::update(schema::book_files::table.find(id))
            .set((
                schema::book_files::visibility.eq(coalesce_opt_nn(
                    visibility.map(|v| v.to_string()),
                    schema::book_files::visibility,
                )),
                schema::book_files::label.eq(coalesce_opt_nn(
                    label.map(str::to_string),
                    schema::book_files::label,
                )),
            ))
            .execute(&mut self.diesel_db.get().await?)
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
        diesel::update(schema::book_files::table.find(file_id))
            .set((
                schema::book_files::content_source.eq(content_source),
                schema::book_files::content_external_id.eq(content_external_id),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        // Bodies from the previous source must not leak through; keep the
        // title rows as placeholders for the new source.
        diesel::update(schema::chapters::table.filter(schema::chapters::file_id.eq(file_id)))
            .set(schema::chapters::content.eq(""))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        let file = self
            .get_file(file_id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))?;
        self.ensure_titles(&file).await?;
        Ok(file)
    }

    /// Re-materialize a plugin file's chapter bodies: clear the stored
    /// content so every chapter is lazily re-pulled from its content
    /// source on the next read. This is the retry path for degraded
    /// materializations — e.g. images whose download failed and fell back
    /// to remote URLs; the re-pull stores them via `store-image` this
    /// time. Local uploads are not touched (their content comes from the
    /// retained original; use the `--reparse-originals` CLI instead).
    pub async fn rematerialize_file(&self, id: &str) -> Result<FileMeta, ApiError> {
        let file = self
            .get_file(id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))?;
        if file.source == "local" {
            return Err(ApiError::bad_request(
                "local files have no plugin content to re-materialize",
            ));
        }
        // Bodies from the previous materialization must not leak through;
        // keep the title rows as placeholders.
        diesel::update(schema::chapters::table.filter(schema::chapters::file_id.eq(id)))
            .set(schema::chapters::content.eq(""))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        self.ensure_titles(&file).await?;
        info!(file = %id, source = %file.source, "file chapters cleared for re-materialization");
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
        let deleted = diesel::delete(schema::book_files::table.find(id))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        if deleted == 0 {
            return Err(ApiError::not_found("file"));
        }
        Ok(())
    }

    // ---- chapters --------------------------------------------------------

    pub async fn chapter_titles(&self, file_id: &str) -> Result<Vec<ChapterMeta>, ApiError> {
        let rows: Vec<ChapterTitleRow> = schema::chapters::table
            .filter(schema::chapters::file_id.eq(file_id))
            .order(schema::chapters::idx.asc())
            .select((schema::chapters::idx, schema::chapters::title))
            .load(&mut self.diesel_db.get().await?)
            .await?;
        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    /// The file's hierarchical TOC. Files stored without one (plugin
    /// books, legacy uploads) get a flat tree synthesized from the chapter
    /// title rows.
    pub async fn file_toc(&self, file_id: &str) -> Result<Vec<TocNode>, ApiError> {
        let raw: String = schema::book_files::table
            .find(file_id)
            .select(schema::book_files::toc)
            .first(&mut self.diesel_db.get().await?)
            .await?;
        if !raw.trim().is_empty()
            && let Ok(toc) = serde_json::from_str::<Vec<TocNode>>(&raw)
        {
            return Ok(toc);
        }
        let rows: Vec<ChapterTitleRow> = schema::chapters::table
            .filter(schema::chapters::file_id.eq(file_id))
            .order(schema::chapters::idx.asc())
            .select((schema::chapters::idx, schema::chapters::title))
            .load(&mut self.diesel_db.get().await?)
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
        let row: Option<ChapterRow> = schema::chapters::table
            .find((&file.id, idx as i64))
            .select((
                schema::chapters::idx,
                schema::chapters::title,
                schema::chapters::content,
            ))
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;

        if let Some(row) = row {
            // Title-only placeholder rows are materialized by
            // `ensure_titles`; fetch the real content lazily.
            if !row.content.is_empty() {
                let mut model = row.into_model();
                // Best-effort width/height on stored img tags (chapters
                // materialized before the dimensions existed lack them;
                // without them every image load re-anchors the view).
                model.content = annotate_image_dims(&self.diesel_db, &model.content).await;
                return Ok(Some(model));
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
        if file.format == FileFormat::Plugin && file.content_source.is_none() && file.volume_no == 0
        {
            let entry = self
                .plugins
                .get_book(source_id, external_id)
                .await
                .map_err(|e| warn!(source = %source_id, book = %external_id, "file-mode metadata unavailable: {e}"))
                .ok()
                .flatten()
                .and_then(|entry| match SourceBook::try_from(entry) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        warn!(source = %source_id, book = %external_id, "invalid plugin book entry: {e}");
                        None
                    }
                });
            if self.plugins.declares(source_id, "book-file").await?
                && let Some(book_file) = self.plugins.get_book_file(source_id, external_id).await?
            {
                let parsed = bookshelf_formats::parse(&book_file.bytes, &book_file.filename)
                    .inspect_err(|e| warn!(error = %e, "file-mode reparse failed"))
                    .ok();
                if let (Some(entry), Some(mut parsed)) = (entry.as_ref(), parsed) {
                    ingest_parsed_book(&self.diesel_db, &self.files_dir, &mut parsed).await?;
                    // Rewrite this row like `materialize_file_mode`
                    // (metadata stays; content becomes self-contained).
                    let toc_json =
                        serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());
                    let format = detect_format(&book_file.filename);
                    let label = format!("{source_id} 下载");
                    let mut conn = self.diesel_db.get().await?;
                    diesel::update(schema::book_files::table.find(&file.id))
                        .set((
                            schema::book_files::format.eq(format.as_ref()),
                            schema::book_files::label.eq(&label),
                            schema::book_files::toc.eq(toc_json),
                            schema::book_files::content_source.eq(Option::<String>::None),
                            schema::book_files::content_external_id.eq(Option::<String>::None),
                            schema::book_files::chapter_count.eq(parsed.chapters.len() as i64),
                        ))
                        .execute(&mut conn)
                        .await?;
                    diesel::delete(
                        schema::chapters::table.filter(schema::chapters::file_id.eq(&file.id)),
                    )
                    .execute(&mut conn)
                    .await?;
                    for (i, chapter) in parsed.chapters.iter().enumerate() {
                        diesel::insert_into(schema::chapters::table)
                            .values((
                                schema::chapters::file_id.eq(&file.id),
                                schema::chapters::idx.eq(i as i64),
                                schema::chapters::title.eq(&chapter.title),
                                schema::chapters::content.eq(&chapter.content),
                            ))
                            .execute(&mut conn)
                            .await?;
                    }
                    // Keep the metadata fresh from the source entry.
                    let authors =
                        serde_json::to_string(&entry.authors).unwrap_or_else(|_| "[]".into());
                    let ext_json = source_ext_column(entry.ext.as_ref())?;
                    diesel::update(schema::books::table.find(&file.book_id))
                        .set((
                            schema::books::title.eq(&entry.title),
                            schema::books::authors.eq(authors),
                            schema::books::description.eq(&entry.description),
                            schema::books::cover_url.eq(&entry.cover_url),
                            schema::books::ext_meta.eq(&ext_json),
                        ))
                        .execute(&mut conn)
                        .await?;
                    info!(
                        source = %source_id,
                        book = %file.book_id,
                        file = %file.id,
                        format = %format,
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
        if let Some(row) = schema::chapters::table
            .find((&file.id, idx as i64))
            .select((
                schema::chapters::idx,
                schema::chapters::title,
                schema::chapters::content,
            ))
            .first::<ChapterRow>(&mut self.diesel_db.get().await?)
            .await
            .optional()?
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
        let content = annotate_image_dims(
            &self.diesel_db,
            &bookshelf_formats::htmlize::plugin_text_to_html(&chapter.content),
        )
        .await;
        diesel::insert_into(schema::chapters::table)
            .values((
                schema::chapters::file_id.eq(&file.id),
                schema::chapters::idx.eq(idx as i64),
                schema::chapters::title.eq(&chapter.title),
                schema::chapters::content.eq(&content),
            ))
            .on_conflict((schema::chapters::file_id, schema::chapters::idx))
            .do_update()
            .set((
                schema::chapters::title.eq(diesel::upsert::excluded(schema::chapters::title)),
                schema::chapters::content.eq(diesel::upsert::excluded(schema::chapters::content)),
            ))
            .execute(&mut self.diesel_db.get().await?)
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
        let mut conn = self.diesel_db.get().await?;
        // Materialized rows are left untouched (the legacy statement had
        // `DO UPDATE ... WHERE content = ''`; the guard moved here because
        // Diesel's typed WHERE needs a Bool-typed expression).
        let has_body: std::collections::HashSet<i64> = schema::chapters::table
            .filter(schema::chapters::file_id.eq(file_id))
            .select((schema::chapters::idx, schema::chapters::content))
            .load::<(i64, String)>(&mut conn)
            .await?
            .into_iter()
            .filter(|(_, c)| !c.is_empty())
            .map(|(idx, _)| idx)
            .collect();
        for (idx, title) in titles.iter().enumerate() {
            let idx = idx as i64;
            if has_body.contains(&idx) {
                continue;
            }
            diesel::insert_into(schema::chapters::table)
                .values((
                    schema::chapters::file_id.eq(file_id),
                    schema::chapters::idx.eq(idx),
                    schema::chapters::title.eq(title.trim()),
                    schema::chapters::content.eq(""),
                ))
                .on_conflict((schema::chapters::file_id, schema::chapters::idx))
                .do_update()
                .set(schema::chapters::title.eq(diesel::upsert::excluded(schema::chapters::title)))
                .execute(&mut conn)
                .await?;
        }
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
        diesel::delete(
            schema::chapters::table.filter(
                schema::chapters::file_id
                    .eq(&file.id)
                    .and(schema::chapters::content.eq(""))
                    .and(schema::chapters::idx.ge(slice.len() as i64)),
            ),
        )
        .execute(&mut self.diesel_db.get().await?)
        .await?;
        if slice.len() != file.chapter_count as usize {
            diesel::update(schema::book_files::table.find(&file.id))
                .set(schema::book_files::chapter_count.eq(slice.len() as i64))
                .execute(&mut self.diesel_db.get().await?)
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

fn detect_format(filename: &str) -> FileFormat {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".epub") {
        FileFormat::Epub
    } else {
        FileFormat::Txt
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

impl Library {
    /// The stored mime type of a stored image.
    pub async fn image_mime(&self, id: &ImageId) -> Result<Option<String>, ApiError> {
        let mime: Option<String> = schema::images::table
            .find(id.as_str())
            .select(schema::images::mime)
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        Ok(mime)
    }

    /// Read a localized image's bytes from disk.
    pub async fn image_bytes(&self, id: &ImageId) -> Result<Option<Vec<u8>>, ApiError> {
        match tokio::fs::read(self.files_dir.join("images").join(id.as_str())).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(anyhow::Error::new(e).context("read image").into()),
        }
    }
}

// ---- chapter image localization ------------------------------------------

/// Store image bytes in the content-addressed image store: id = sha256 hex,
/// bytes at `{files_dir}/images/{id}`, row in the `images` table. Duplicate
/// bytes are a no-op (same id). Returns the id for referencing the image as
/// `/api/images/{id}`.
pub async fn store_image(
    db: &DieselDb,
    files_dir: &Path,
    bytes: &[u8],
    mime: &str,
) -> anyhow::Result<ImageId> {
    let id = sha256_hex(bytes);
    let dims = bookshelf_formats::imgdim::image_dimensions(bytes);
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
    use diesel_async::RunQueryDsl as _;
    diesel::insert_into(schema::images::table)
        .values((
            schema::images::id.eq(&id),
            schema::images::mime.eq(mime),
            schema::images::size.eq(bytes.len() as i64),
            schema::images::width.eq(dims.map(|(w, _)| w as i64)),
            schema::images::height.eq(dims.map(|(_, h)| h as i64)),
        ))
        .on_conflict_do_nothing()
        .execute(&mut db.get().await?)
        .await?;
    Ok(ImageId::from_sha256_hex(id).expect("sha256 hex is always a valid image id"))
}

/// Ingest boundary for parsed book files: stores every extracted image in
/// the image store and rewrites the parser's ingest placeholders
/// (`src="image:{n}"`) to canonical `/api/images/{id}` references, so all
/// stored chapter HTML — uploads and plugin materializations alike — uses
/// one image reference format.
pub async fn ingest_parsed_book(
    db: &DieselDb,
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
            // width/height attributes reserve layout space before the
            // bytes load (otherwise every image load grows the page and
            // the reader view keeps re-anchoring while scrolling).
            let reference =
                match bookshelf_formats::imgdim::image_dimensions(&parsed.images[n].bytes) {
                    Some((w, h)) => {
                        format!("src=\"/api/images/{id}\" width=\"{w}\" height=\"{h}\"")
                    }
                    None => format!("src=\"/api/images/{id}\""),
                };
            if chapter.content.contains(&placeholder) {
                chapter.content = chapter.content.replace(&placeholder, &reference);
            }
        }
    }
    Ok(())
}

/// The `/api/images/{id}` references contained in `html`, in order of
/// first appearance (duplicates removed).
fn image_ids_in_html(html: &str) -> Vec<String> {
    const NEEDLE: &str = "src=\"/api/images/";
    let mut ids = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find(NEEDLE) {
        let after_id = &rest[pos + NEEDLE.len()..];
        let Some(id_len) = after_id.find('"') else {
            break;
        };
        let id = &after_id[..id_len];
        if !ids.iter().any(|seen: &String| seen == id) {
            ids.push(id.to_string());
        }
        rest = &after_id[id_len..];
    }
    ids
}

/// Add `width`/`height` attributes to `/api/images/{id}` `<img>` tags that
/// lack them (plugin-generated chapter HTML; the dimensions live in the
/// image store from when the plugin stored the bytes). Best-effort: on a
/// lookup error the HTML is returned unchanged.
pub fn annotate_image_dimensions(html: &str, dims_by_id: &HashMap<String, (i64, i64)>) -> String {
    const NEEDLE: &str = "src=\"/api/images/";
    if dims_by_id.is_empty() || !html.contains(NEEDLE) {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(pos) = rest.find(NEEDLE) {
        let after_id = &rest[pos + NEEDLE.len()..];
        let Some(id_len) = after_id.find('"') else {
            break;
        };
        let id = &after_id[..id_len];
        let Some(tag_end_rel) = after_id[id_len..].find('>') else {
            break;
        };
        let tag_end = pos + NEEDLE.len() + id_len + tag_end_rel;
        // Insert before a self-closing tag's `/` (`<img … />`), otherwise
        // directly before the `>`.
        let insert_at = if tag_end > 0 && rest.as_bytes()[tag_end - 1] == b'/' {
            tag_end - 1
        } else {
            tag_end
        };
        out.push_str(&rest[..insert_at]);
        let already = rest[pos..insert_at].contains("width=");
        if let (false, Some((w, h))) = (already, dims_by_id.get(id)) {
            out.push_str(&format!(" width=\"{w}\" height=\"{h}\""));
        }
        out.push_str(&rest[insert_at..tag_end]);
        rest = &rest[tag_end..];
    }
    out.push_str(rest);
    out
}

/// Look up stored dimensions for the `/api/images/{id}` references in
/// `html` and add width/height attributes to the img tags (best-effort:
/// any lookup failure returns the HTML unchanged).
async fn annotate_image_dims(db: &DieselDb, html: &str) -> String {
    const NEEDLE: &str = "src=\"/api/images/";
    if !html.contains(NEEDLE) {
        return html.to_string();
    }
    let ids = image_ids_in_html(html);
    if ids.is_empty() {
        return html.to_string();
    }
    let rows: Vec<(String, Option<i64>, Option<i64>)> = match db.get().await {
        Ok(mut conn) => schema::images::table
            .filter(schema::images::id.eq_any(ids))
            .select((
                schema::images::id,
                schema::images::width,
                schema::images::height,
            ))
            .load(&mut conn)
            .await
            .unwrap_or_default(),
        Err(_) => return html.to_string(),
    };
    let dims_by_id: HashMap<String, (i64, i64)> = rows
        .into_iter()
        .filter_map(|(id, w, h)| w.zip(h).map(|(w, h)| (id, (w, h))))
        .collect();
    annotate_image_dimensions(html, &dims_by_id)
}

/// Startup backfill: sniff intrinsic dimensions for images stored before
/// they were recorded (migration 0011 added the columns; older rows are
/// NULL). Returns the number of rows updated.
pub async fn backfill_image_dimensions(db: &DieselDb, files_dir: &Path) -> anyhow::Result<usize> {
    let rows: Vec<String> = schema::images::table
        .filter(schema::images::width.is_null())
        .select(schema::images::id)
        .load(&mut db.get().await?)
        .await?;
    let mut updated = 0;
    for id in rows {
        let path = files_dir.join("images").join(&id);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(anyhow::Error::new(e).context("read image for backfill")),
        };
        if let Some((w, h)) = bookshelf_formats::imgdim::image_dimensions(&bytes) {
            diesel::update(schema::images::table.find(&id))
                .set((
                    schema::images::width.eq(w as i64),
                    schema::images::height.eq(h as i64),
                ))
                .execute(&mut db.get().await?)
                .await?;
            updated += 1;
        }
    }
    Ok(updated)
}

/// One-shot upgrade migration (`--reparse-originals`, owner decision:
/// manual, no HTTP endpoint): re-run the current parser over every
/// retained original and replace the stored chapters + toc in one
/// transaction per file. Sessions are kept; `chapter_idx` is clamped to
/// the new chapter count. Returns (reparsed, failed).
pub async fn reparse_originals(db: &DieselDb, files_dir: &Path) -> anyhow::Result<(usize, usize)> {
    let rows: Vec<(String, String)> = schema::book_files::table
        .filter(schema::book_files::orig_ext.is_not_null())
        .order(schema::book_files::created_at.asc())
        .select((schema::book_files::id, schema::book_files::orig_ext))
        .load::<(String, Option<String>)>(&mut db.get().await?)
        .await?
        .into_iter()
        .map(|(id, ext)| (id, ext.unwrap_or_default()))
        .collect::<Vec<_>>();
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
        let mut conn = db.get().await?;
        diesel::delete(schema::chapters::table.filter(schema::chapters::file_id.eq(&file_id)))
            .execute(&mut conn)
            .await?;
        for (idx, chapter) in parsed.chapters.iter().enumerate() {
            diesel::insert_into(schema::chapters::table)
                .values((
                    schema::chapters::file_id.eq(&file_id),
                    schema::chapters::idx.eq(idx as i64),
                    schema::chapters::title.eq(&chapter.title),
                    schema::chapters::content.eq(&chapter.content),
                ))
                .execute(&mut conn)
                .await?;
        }
        diesel::update(schema::book_files::table.find(&file_id))
            .set((
                schema::book_files::chapter_count.eq(count),
                schema::book_files::toc.eq(toc_json),
            ))
            .execute(&mut conn)
            .await?;
        // Keep every session; clamp positions into the new chapter range.
        diesel::update(schema::sessions::table.filter(schema::sessions::file_id.eq(&file_id)))
            .set((
                schema::sessions::chapter_idx.eq(least(schema::sessions::chapter_idx, count - 1)),
                schema::sessions::fraction.eq(least_double(schema::sessions::fraction, 1.0)),
            ))
            .execute(&mut conn)
            .await?;
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
            format: FileFormat::Plugin,
            label: String::new(),
            visibility: Visibility::Public,
            owner_id: None,
            chapter_count: 0,
            created_at: chrono::Utc::now(),
            volume_no: 0,
            volume_offset: 0,
            original: None,
        }
    }

    #[tokio::test]
    async fn ingest_stores_images_and_rewrites_placeholders() {
        let dir = std::env::temp_dir().join(format!(
            "bookshelf-ingest-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::connect_diesel(&dir.join("t.db")).unwrap();
        {
            use diesel_async::SimpleAsyncConnection as _;
            let mut conn = db.get().await.unwrap();
            conn.batch_execute(
                "CREATE TABLE images (id TEXT PRIMARY KEY NOT NULL, mime TEXT NOT NULL, \
                 size INTEGER NOT NULL, width INTEGER, height INTEGER, \
                 created_at TEXT NOT NULL DEFAULT '');",
            )
            .await
            .unwrap();
        }

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
            ext: Default::default(),
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
        use diesel_async::RunQueryDsl as _;
        let rows: Vec<(String, String, i64)> = schema::images::table
            .order(schema::images::id.asc())
            .select((
                schema::images::id,
                schema::images::mime,
                schema::images::size,
            ))
            .load(&mut db.get().await.unwrap())
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

    #[test]
    fn annotates_image_dimensions_on_img_tags() {
        let dims = HashMap::from([
            ("aaa".to_string(), (800, 600)),
            ("bbb".to_string(), (320, 200)),
        ]);
        let html = concat!(
            "<p>前文</p>",
            "<figure><img src=\"/api/images/aaa\"/></figure>",
            "<figure><img src=\"https://remote.example/x.jpg\"/></figure>",
            "<img src=\"/api/images/bbb\" alt=\"插图\">",
            // already annotated: untouched; unknown id: left alone
            "<img src=\"/api/images/aaa\" width=\"1\" height=\"1\">",
            "<img src=\"/api/images/ccc\">"
        );
        let out = annotate_image_dimensions(html, &dims);
        assert!(out.contains("<img src=\"/api/images/aaa\" width=\"800\" height=\"600\"/>"));
        assert!(
            out.contains("<img src=\"/api/images/bbb\" alt=\"插图\" width=\"320\" height=\"200\">")
        );
        assert!(out.contains("width=\"1\" height=\"1\""));
        assert!(!out.contains("ccc\" width"));
        assert!(!out.contains("remote.example/x.jpg\" width"));
        // empty lookup map → unchanged
        assert_eq!(annotate_image_dimensions(html, &HashMap::new()), html);
    }
}
