//! The library service: central storage for book metadata, files and
//! chapters.
//!
//! Two-level model: `books` are pure metadata entries, `book_files` are
//! the actual books (uploads or virtual plugin books). Files from every
//! source are catalogued here; plugin chapters are materialized into the
//! same tables by background downloads, so readers see one unified store.
//!
//! Plugin system v2: sources are *instances* registered in
//! `plugin_instances` (see [`PluginService`]). Catalog access is lazy —
//! `declare`-capable instances sync their static catalog at startup,
//! `search`-capable instances are browsed on demand and single books are
//! materialized via `get-book`. A book can hold multiple content items from
//! different instances; each item has its own immutable source identity.
//!
//! Visibility is per file: `private` = owner + admins only, `public` =
//! visible to every logged-in user.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use tracing::{info, warn};

use mandara_core::model::{
    BookMeta, Chapter, ChapterMeta, ContentMode, FileFormat, FileMeta, ImageId, Role, SeriesMeta,
    TocNode, User, Visibility,
};
use mandara_core::source::{
    SourceBook, SourceBookFile, SourceDeclaredChapter, SourceDeclaredContent, SourceSeries,
};
use mandara_core::{BookExt, SeriesExt};

use mandara_formats::ParsedBook;

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

/// Validate + normalize a source's extended metadata object and render it
/// as the stored column value. Invalid payloads fail the acquisition —
/// bad data never lands (docs/metadata-ext-design.md).
fn source_ext_column(ext: Option<&serde_json::Value>) -> Result<String, ApiError> {
    match ext {
        None => Ok("{}".into()),
        Some(v) => Ok(BookExt::from_value(v.clone())?.to_column()),
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
    /// Independently identified plugin content; its own declaration selects
    /// file or chapter loading without consulting book metadata.
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

/// One acquired book and its file.
type AcquiredBooks = (
    BookMeta,
    Option<FileMeta>,
    Vec<(BookMeta, Option<FileMeta>)>,
);

/// Result of acquiring one independently addressable book and its parent series.
pub struct AcquireOutcome {
    pub series: Option<SeriesMeta>,
    /// Exactly one independently selected book.
    pub books: Vec<(BookMeta, Option<FileMeta>)>,
}

impl AcquireOutcome {
    /// Split the outcome into its first book and the rest. The first book
    /// always exists — [`Self::books`] is documented non-empty and every
    /// producer upholds it — so this is the single place that turns the
    /// contract into a value (a programmer error surfaces as a panic here,
    /// not as a 500 "internal server error" to clients).
    pub fn into_parts(self) -> AcquiredBooks {
        let mut it = self.books.into_iter();
        let first = it
            .next()
            .expect("acquire_book always produces at least one book");
        (first.0, first.1, it.collect())
    }
}

/// One book produced by the metadata stage, ready to have its content
/// attached by the content stage. The two stages are orthogonal: stage 1
/// resolves one metadata entry; stage 2 attaches independently chosen content.
struct PlannedBook {
    book_id: String,
    /// How to fill this book's content (agreed between the stages).
    fill: PlannedFill,
}

/// The content-fill recipe for one planned book.
enum PlannedFill {
    /// An uploaded edition. `fill_cover` stores a missing cover from the upload —
    /// only when attaching to a pre-existing metadata entry.
    Upload { fill_cover: bool },
    /// Content from one independently addressable plugin book.
    Plugin,
    /// No content (metadata only).
    None,
}

/// The result of the metadata stage.
struct MetadataPlan {
    series: Option<SeriesMeta>,
    books: Vec<PlannedBook>,
}

#[derive(Clone)]
pub struct Library {
    diesel_db: crate::db::DieselDb,
    plugins: Arc<PluginService>,
    /// Directory for retained original file bytes
    /// (`data/files/{file_id}.{ext}`; see docs/storage-unification-design.md).
    files_dir: PathBuf,
    downloads: Arc<super::downloads::Downloads>,
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
            downloads: Arc::new(super::downloads::Downloads::default()),
        }
    }

    /// The plugin instance service (instance management, config, calls).
    pub fn plugins(&self) -> &PluginService {
        &self.plugins
    }

    /// Download missing bodies during acquisition or explicit refresh only.
    fn queue_download(&self, file: &FileMeta) {
        let library = self.clone();
        let file = file.clone();
        tokio::spawn(async move {
            if let Err(error) = library.download_chapters(&file).await {
                warn!(file = %file.id, %error, "background chapter download failed");
            }
        });
    }

    async fn download_chapters(&self, file: &FileMeta) -> Result<(), ApiError> {
        for idx in 0..file.chapter_count.max(1) {
            let _permit = self
                .downloads
                .permits
                .acquire()
                .await
                .expect("download limiter stays open");
            if let Err(error) = self.download_chapter(file, idx).await {
                warn!(file = %file.id, idx, %error, "chapter download failed");
            }
        }
        Ok(())
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
        for entry in &catalog.books {
            let book_id = self
                .resolve_plugin_metadata(&catalog.instance, None, &entry.book)
                .await?;
            if let Some(content) = &entry.content {
                let mut file = self
                    .attach_plugin_file(
                        &catalog.instance,
                        None,
                        &content.id,
                        content.mode,
                        &book_id,
                    )
                    .await?;
                self.upsert_chapters(&file.id, &content.chapters).await?;
                file.chapter_count = content.chapters.len() as u32;
                diesel::update(schema::book_files::table.find(&file.id))
                    .set(schema::book_files::chapter_count.eq(i64::from(file.chapter_count)))
                    .execute(&mut self.diesel_db.get().await?)
                    .await?;
                self.queue_download(&file);
            }
            info!(source = %catalog.instance, book = %book_id, "declared book synced");
        }
        Ok(catalog.books.len())
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
    ) -> Result<(u64, Vec<(mandara_plugin::CatalogEntry, Option<String>)>), ApiError> {
        let result = self
            .plugins
            .search_books(instance, query, offset, limit)
            .await?;
        let mut items = Vec::with_capacity(result.items.len());
        {
            let mut conn = self.diesel_db.get().await?;
            for entry in result.items {
                let book = match &entry {
                    mandara_plugin::CatalogEntry::Single(book) => {
                        SourceBook::try_from(book.clone())?
                    }
                    mandara_plugin::CatalogEntry::Series(series) => {
                        SourceSeries::try_from(series.clone())?;
                        items.push((entry, None));
                        continue;
                    }
                };
                let book_id: Option<String> = schema::books::table
                    .filter(
                        schema::books::meta_source
                            .eq(instance)
                            .and(schema::books::meta_external_id.eq(&book.id)),
                    )
                    .select(schema::books::id)
                    .first(&mut conn)
                    .await
                    .optional()?;
                items.push((entry, book_id));
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
        let (book, file, _rest) = outcome.into_parts();
        let file = file
            .ok_or_else(|| ApiError::bad_request("metadata-only acquisition produced no file"))?;
        Ok((book, file))
    }

    /// Store parsed plugin content using the same original/cache path for
    /// acquisition and reads. Metadata provenance is never changed here.
    async fn materialize_file_mode(
        &self,
        file: &FileMeta,
        book_file: &SourceBookFile,
        mut parsed: mandara_formats::ParsedBook,
    ) -> Result<(FileMeta, Vec<Chapter>), ApiError> {
        ingest_parsed_book(&self.diesel_db, &self.files_dir, &mut parsed).await?;
        let file_id = &file.id;
        let source = &file.source;
        let book_id = &file.book_id;
        let format = detect_format(&book_file.filename);
        let toc_json = serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());
        let label = format!("{source} 下载");
        // Cache the fetched bytes as the file's original: the source may
        // disappear later; download/reparse should not depend on it.
        let (orig_ext, orig_sha256, orig_size) = self
            .store_original(file_id, format.as_ref(), &book_file.bytes)
            .await?;

        // Rewrite the virtual row into a stored file: self-contained
        // (no content indirection), real format/toc/chapter count.
        diesel::update(schema::book_files::table.find(file_id))
            .set((
                schema::book_files::format.eq(format.as_ref()),
                schema::book_files::label.eq(&label),
                schema::book_files::toc.eq(toc_json),
                schema::book_files::chapter_count.eq(parsed.chapters.len() as i64),
                schema::book_files::orig_ext.eq(&orig_ext),
                schema::book_files::orig_sha256.eq(&orig_sha256),
                schema::book_files::orig_size.eq(orig_size),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        diesel::delete(schema::chapters::table.filter(schema::chapters::file_id.eq(file_id)))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        {
            let mut conn = self.diesel_db.get().await?;
            for (idx, chapter) in parsed.chapters.iter().enumerate() {
                diesel::insert_into(schema::chapters::table)
                    .values((
                        schema::chapters::file_id.eq(file_id),
                        schema::chapters::idx.eq(idx as i64),
                        schema::chapters::title.eq(&chapter.title),
                        schema::chapters::content.eq(&chapter.content),
                        schema::chapters::linear.eq(i64::from(chapter.linear)),
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

        let mut file = file.clone();
        file.format = format;
        file.label = label;
        file.chapter_count = parsed.chapters.len() as u32;
        file.original = Some(mandara_core::model::OriginalInfo {
            ext: orig_ext,
            sha256: orig_sha256,
            size: orig_size as u64,
        });
        let chapters = parsed
            .chapters
            .into_iter()
            .enumerate()
            .map(|(idx, chapter)| Chapter {
                idx: idx as u32,
                title: chapter.title,
                content: chapter.content,
            })
            .collect();
        Ok((file, chapters))
    }

    /// Import explicit series metadata and only the selected member books, without content.
    pub async fn import_plugin_series(
        &self,
        user: &User,
        source: &str,
        series_id: &str,
        book_ids: &[String],
    ) -> Result<SeriesMeta, ApiError> {
        if series_id.trim().is_empty() {
            return Err(ApiError::bad_request("series ID must not be empty"));
        }
        let result = self
            .plugins
            .get_series(source, series_id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin series"))?;
        let series = SourceSeries::try_from(result.series)?;
        let members: std::collections::HashSet<_> =
            result.books.iter().map(|b| b.id.as_str()).collect();
        let mut selected = std::collections::HashSet::new();
        let mut books = Vec::with_capacity(book_ids.len());
        // Validate membership and resolve full book metadata before writing anything.
        for id in book_ids {
            if !members.contains(id.as_str()) || !selected.insert(id) {
                return Err(ApiError::bad_request(
                    "book IDs must be distinct members of the selected series",
                ));
            }
            let book = self
                .plugins
                .get_book(source, id)
                .await?
                .ok_or_else(|| ApiError::not_found("plugin member book"))?;
            let book = SourceBook::try_from(book)?;
            if book.id != *id || book.series.as_ref().map(|s| &s.id) != Some(&series.id) {
                return Err(ApiError::bad_request(
                    "member book does not belong to the selected series",
                ));
            }
            books.push(book);
        }
        for book in books {
            self.resolve_plugin_metadata(source, Some(&user.id), &book)
                .await?;
        }
        let id = self
            .resolve_plugin_series(source, Some(&user.id), &series)
            .await?;
        // Claim previously unowned metadata, without replacing another creator.
        diesel::update(schema::series::table.find(&id))
            .set(
                schema::series::created_by
                    .eq(coalesce(schema::series::created_by, user.id.as_str())),
            )
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        self.get_series(&id)
            .await?
            .ok_or_else(|| ApiError::not_found("series"))
    }

    /// Resolve series metadata by canonical plugin-instance provenance.
    async fn resolve_plugin_series(
        &self,
        source: &str,
        owner: Option<&str>,
        series: &SourceSeries,
    ) -> Result<String, ApiError> {
        let ext =
            SeriesExt::from_value(series.ext.clone().unwrap_or_else(|| serde_json::json!({})))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let authors = serde_json::to_string(&series.authors).unwrap_or_else(|_| "[]".into());
        diesel::insert_into(schema::series::table)
            .values((
                schema::series::id.eq(&id),
                schema::series::title.eq(&series.title),
                schema::series::authors.eq(&authors),
                schema::series::description.eq(&series.description),
                schema::series::cover_url.eq(&series.cover_url),
                schema::series::ext_meta.eq(ext.to_column()),
                schema::series::created_by.eq(owner),
                schema::series::meta_source.eq(source),
                schema::series::meta_external_id.eq(&series.id),
            ))
            .on_conflict((
                schema::series::meta_source,
                schema::series::meta_external_id,
            ))
            .do_update()
            .set((
                schema::series::title.eq(&series.title),
                schema::series::authors.eq(&authors),
                schema::series::description.eq(&series.description),
                schema::series::cover_url.eq(&series.cover_url),
                schema::series::ext_meta.eq(ext.to_column()),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        Ok(schema::series::table
            .filter(
                schema::series::meta_source
                    .eq(source)
                    .and(schema::series::meta_external_id.eq(&series.id)),
            )
            .select(schema::series::id)
            .first(&mut self.diesel_db.get().await?)
            .await?)
    }

    /// Resolve book metadata and its explicit parent series without creating content files.
    async fn resolve_plugin_metadata(
        &self,
        source: &str,
        owner: Option<&str>,
        book: &SourceBook,
    ) -> Result<String, ApiError> {
        let authors = serde_json::to_string(&book.authors).unwrap_or_else(|_| "[]".into());
        let ext_json = source_ext_column(book.ext.as_ref())?;
        let series_id = match &book.series {
            Some(series) => Some(self.resolve_plugin_series(source, owner, series).await?),
            None => None,
        };

        let existing: Option<String> = schema::books::table
            .filter(
                schema::books::meta_source
                    .eq(source)
                    .and(schema::books::meta_external_id.eq(&book.id)),
            )
            .select(schema::books::id)
            .order(schema::books::created_at.asc())
            .first::<String>(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        let book_id = match existing {
            Some(book_id) => book_id,
            None => {
                let book_id = uuid::Uuid::new_v4().simple().to_string();
                diesel::insert_into(schema::books::table)
                    .values((
                        schema::books::id.eq(&book_id),
                        schema::books::title.eq(&book.title),
                        schema::books::authors.eq(&authors),
                        schema::books::description.eq(&book.description),
                        schema::books::cover_url.eq(&book.cover_url),
                        schema::books::ext_meta.eq(&ext_json),
                        // The metadata source is this plugin itself.
                        schema::books::meta_source.eq(source),
                        schema::books::meta_external_id.eq(&book.id),
                        schema::books::created_by.eq(owner),
                    ))
                    .execute(&mut self.diesel_db.get().await?)
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
                schema::books::series_id.eq(&series_id),
                schema::books::volume_no.eq(book.volume_no as i64),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        if let Some(owner) = owner {
            diesel::update(schema::books::table.find(&book_id))
                .set(schema::books::created_by.eq(coalesce(schema::books::created_by, owner)))
                .execute(&mut self.diesel_db.get().await?)
                .await?;
        }
        Ok(book_id)
    }

    /// Apply user-supplied metadata overrides on top of a produced entry
    /// (title/authors/description/cover/ext). No-op when empty.
    async fn apply_overrides(
        &self,
        book_id: &str,
        overrides: &MetadataOverrides,
    ) -> Result<(), ApiError> {
        if overrides.is_empty() {
            return Ok(());
        }
        self.update_book(
            book_id,
            overrides.title.as_deref(),
            overrides.description.as_deref(),
            overrides.authors.as_ref(),
            overrides.cover_url.as_deref(),
            overrides.ext.as_ref(),
        )
        .await?;
        Ok(())
    }

    /// Store declared chapter rows; missing bodies remain lazy placeholders.
    async fn upsert_chapters(
        &self,
        file_id: &str,
        chapters: &[SourceDeclaredChapter],
    ) -> Result<(), ApiError> {
        let mut conn = self.diesel_db.get().await?;
        for (idx, chapter) in chapters.iter().enumerate() {
            let content = if let Some(body) = &chapter.content {
                Some(
                    with_stored_dims(
                        &self.diesel_db,
                        &mandara_formats::htmlize::plugin_to_html(&chapter.format, body),
                    )
                    .await,
                )
            } else {
                None
            };
            // Missing bodies never overwrite materialized content: keep the
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
                    schema::chapters::linear.eq(diesel::upsert::excluded(schema::chapters::linear)),
                    schema::chapters::content.eq(coalesce_opt(
                        diesel::upsert::excluded(schema::chapters::content),
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
        book_id: &str,
    ) -> Result<Option<FileMeta>, ApiError> {
        let mut conn = self.diesel_db.get().await?;
        let row: Option<FileRow> = schema::book_files::table
            .filter(
                schema::book_files::source
                    .eq(source)
                    .and(schema::book_files::external_id.eq(external_id))
                    .and(schema::book_files::book_id.eq(book_id)),
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
    /// `meta_source`/`meta_external_id` record the plugin entry the
    /// metadata came from (refresh pulls metadata from there only).
    #[allow(clippy::too_many_arguments)]
    async fn create_metadata_entry(
        &self,
        title: &str,
        authors: &[String],
        description: Option<&str>,
        cover_url: Option<&str>,
        ext: Option<&BookExt>,
        meta_source: Option<&str>,
        meta_external_id: Option<&str>,
        owner_id: &str,
    ) -> Result<String, ApiError> {
        let book_id = uuid::Uuid::new_v4().simple().to_string();
        let authors_json = serde_json::to_string(authors).unwrap_or_else(|_| "[]".into());
        let mut ext = ext.cloned().unwrap_or_default();
        ext.sanitize()?;
        diesel::insert_into(schema::books::table)
            .values((
                schema::books::id.eq(&book_id),
                schema::books::title.eq(title),
                schema::books::authors.eq(authors_json),
                schema::books::description.eq(description.map(str::to_string)),
                schema::books::cover_url.eq(cover_url.map(str::to_string)),
                schema::books::ext_meta.eq(ext.to_column()),
                schema::books::meta_source.eq(meta_source),
                schema::books::meta_external_id.eq(meta_external_id),
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
    /// Two sequential stages (see [`Self::resolve_metadata`] then
    /// [`Self::attach_content`]): the metadata is resolved first into a
    /// plan for one book, then the content is attached. The two dimensions
    /// stay orthogonal.
    pub async fn acquire_book(
        &self,
        user: &User,
        content: AcquireContent<'_>,
        metadata: AcquireMetadata<'_>,
        visibility: Visibility,
        label: &str,
    ) -> Result<AcquireOutcome, ApiError> {
        // Two sequential stages, each a 1-D match: stage 1 resolves the
        // metadata into a plan; stage 2 attaches the content to the planned
        // book. The two dimensions stay orthogonal — no 2-D cartesian
        // dispatch; adding a content or metadata type only adds one arm to
        // its own stage.
        let plan = self.resolve_metadata(user, &content, &metadata).await?;
        let mut books = Vec::with_capacity(plan.books.len());
        for planned in plan.books {
            let file = self
                .attach_content(user, &content, &planned, visibility, label)
                .await?;
            let book = self
                .get_book(&planned.book_id)
                .await?
                .ok_or_else(|| ApiError::not_found("book"))?;
            books.push((book, file));
        }
        let series = match books
            .as_slice()
            .first()
            .and_then(|(book, _)| book.series_id.as_deref())
        {
            Some(id) => self.get_series(id).await?,
            None => plan.series,
        };
        for (_, file) in &books {
            if let Some(file) = file {
                self.queue_download(file);
            }
        }
        Ok(AcquireOutcome { series, books })
    }

    /// Stage 1: resolve the metadata source into a [`MetadataPlan`] — the
    /// selected book to fill. A
    /// 1-D match on [`AcquireMetadata`]; the `New` branch also matches on
    /// the content type, since auto-metadata genuinely depends on whether
    /// there is an upload to parse/identify or a plugin entry to use.
    async fn resolve_metadata(
        &self,
        user: &User,
        content: &AcquireContent<'_>,
        metadata: &AcquireMetadata<'_>,
    ) -> Result<MetadataPlan, ApiError> {
        match metadata {
            // ---- attach to an existing metadata entry -----------------
            AcquireMetadata::Attach { book_id } => {
                if matches!(content, AcquireContent::None) {
                    return Err(ApiError::bad_request(
                        "a metadata-only addition has no content to attach — drop `book_id` or \
                         provide a file / plugin book",
                    ));
                }
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
                let fill = match content {
                    AcquireContent::File { .. } => PlannedFill::Upload { fill_cover: true },
                    AcquireContent::Plugin { .. } => PlannedFill::Plugin,
                    AcquireContent::None => unreachable!("checked above"),
                };
                Ok(MetadataPlan {
                    series: None,
                    books: vec![PlannedBook {
                        book_id: book_id.to_string(),
                        fill,
                    }],
                })
            }

            // ---- metadata from a named plugin source -----------------
            AcquireMetadata::Plugin {
                source,
                book_id_in_source,
                overrides,
            } => {
                let meta_entry = self.plugin_book_entry(source, book_id_in_source).await?;
                let book_id = self
                    .resolve_plugin_metadata(source, Some(&user.id), &meta_entry)
                    .await?;
                self.apply_overrides(&book_id, overrides).await?;
                Ok(MetadataPlan {
                    series: None,
                    books: vec![PlannedBook {
                        book_id,
                        fill: self.content_fill(content),
                    }],
                })
            }

            // ---- new metadata (auto / identify / manual) -------------
            AcquireMetadata::New { overrides } => match content {
                // Upload: auto metadata — plugins may identify the file
                // (first match), otherwise parse it into a fresh entry.
                AcquireContent::File { bytes, filename } => {
                    let mut parsed = mandara_formats::parse(bytes, filename)?;
                    ingest_parsed_book(&self.diesel_db, &self.files_dir, &mut parsed).await?;
                    let hash = sha256_hex(bytes);
                    let book_id = match self.plugins.identify_upload(filename, &hash).await? {
                        Some((source, entry)) => {
                            let book_id = self
                                .resolve_plugin_metadata(&source, Some(&user.id), &entry)
                                .await?;
                            self.apply_overrides(&book_id, overrides).await?;
                            book_id
                        }
                        None => self.create_book(&parsed, &user.id, overrides).await?,
                    };
                    // An upload is one complete edition — never a split.
                    Ok(MetadataPlan {
                        series: None,
                        books: vec![PlannedBook {
                            book_id,
                            fill: PlannedFill::Upload { fill_cover: false },
                        }],
                    })
                }

                // Plugin: self-describing form — the plugin is both the
                // metadata and the content source.
                AcquireContent::Plugin {
                    source,
                    book_id_in_source,
                } => {
                    let entry = self.plugin_book_entry(source, book_id_in_source).await?;
                    let book_id = self
                        .resolve_plugin_metadata(source, Some(&user.id), &entry)
                        .await?;
                    self.apply_overrides(&book_id, overrides).await?;
                    let fill = PlannedFill::Plugin;
                    Ok(MetadataPlan {
                        series: None,
                        books: vec![PlannedBook { book_id, fill }],
                    })
                }

                // None: a manually described entry (title required).
                AcquireContent::None => {
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
                            None,
                            None,
                            &user.id,
                        )
                        .await?;
                    Ok(MetadataPlan {
                        series: None,
                        books: vec![PlannedBook {
                            book_id,
                            fill: PlannedFill::None,
                        }],
                    })
                }
            },
        }
    }

    /// Plan content attachment independently of book metadata.
    fn content_fill(&self, content: &AcquireContent<'_>) -> PlannedFill {
        match content {
            AcquireContent::File { .. } => PlannedFill::Upload { fill_cover: false },
            AcquireContent::None => PlannedFill::None,
            AcquireContent::Plugin { .. } => PlannedFill::Plugin,
        }
    }

    /// Stage 2: attach the content to one planned book — a 1-D match on
    /// [`AcquireContent`], orthogonal to the metadata stage. Returns the
    /// stored file (or `None` for a metadata-only book).
    async fn attach_content(
        &self,
        user: &User,
        content: &AcquireContent<'_>,
        planned: &PlannedBook,
        visibility: Visibility,
        label: &str,
    ) -> Result<Option<FileMeta>, ApiError> {
        match content {
            AcquireContent::File { bytes, filename } => {
                let PlannedFill::Upload { fill_cover } = &planned.fill else {
                    unreachable!("stage 1 planned an upload for an upload")
                };
                let mut parsed = mandara_formats::parse(bytes, filename)?;
                ingest_parsed_book(&self.diesel_db, &self.files_dir, &mut parsed).await?;
                // The attached edition may be the first one carrying a
                // real cover — fill it in when the entry has none yet (only
                // when attaching to a pre-existing entry).
                if *fill_cover && let Some(cover) = &parsed.cover {
                    self.store_cover_if_missing(&planned.book_id, cover).await?;
                }
                let file = self
                    .store_local_file(
                        &parsed,
                        bytes,
                        &planned.book_id,
                        &user.id,
                        filename,
                        visibility,
                        label,
                    )
                    .await?;
                Ok(Some(file))
            }
            AcquireContent::Plugin {
                source: content_source,
                book_id_in_source: content_book_id,
            } => {
                debug_assert!(matches!(planned.fill, PlannedFill::Plugin));
                let content = self
                    .plugins
                    .get_content(content_source, content_book_id, true)
                    .await?
                    .ok_or_else(|| ApiError::not_found("content"))?;
                let file = self
                    .attach_plugin_file(
                        content_source,
                        Some(&user.id),
                        &content.id,
                        content.mode,
                        &planned.book_id,
                    )
                    .await?;
                let file = self.materialize_plugin_content(&file, &content).await?;
                Ok(Some(file))
            }
            AcquireContent::None => {
                debug_assert!(matches!(planned.fill, PlannedFill::None));
                Ok(None)
            }
        }
    }

    /// Fetch one entry from a plugin source by id (`lookup` capability).
    async fn plugin_book_entry(&self, source: &str, book_id: &str) -> Result<SourceBook, ApiError> {
        let entry = self.plugins.get_book(source, book_id).await?;
        entry
            .map(SourceBook::try_from)
            .transpose()?
            .ok_or_else(|| ApiError::bad_request(format!("plugin does not offer book `{book_id}`")))
    }

    /// Attach a content identity to the selected metadata entry; no metadata lookup.
    async fn attach_plugin_file(
        &self,
        source: &str,
        owner: Option<&str>,
        content_id: &str,
        mode: ContentMode,
        book_id: &str,
    ) -> Result<FileMeta, ApiError> {
        if let Some(file) = self.plugin_file(source, content_id, book_id).await? {
            return Ok(file);
        }
        let file_id = uuid::Uuid::new_v4().simple().to_string();
        diesel::insert_into(schema::book_files::table)
            .values((
                schema::book_files::id.eq(&file_id),
                schema::book_files::book_id.eq(book_id),
                schema::book_files::source.eq(source),
                schema::book_files::external_id.eq(content_id),
                schema::book_files::content_mode.eq(mode.as_ref()),
                schema::book_files::format.eq(FileFormat::Plugin.as_ref()),
                schema::book_files::visibility.eq("public"),
                schema::book_files::owner_id.eq(owner),
                schema::book_files::chapter_count.eq(0_i64),
                schema::book_files::label.eq(format!("{source} 虚拟")),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        self.get_file(&file_id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))
    }

    /// Execute the content form declared by the plugin at acquisition.
    async fn materialize_plugin_content(
        &self,
        file: &FileMeta,
        content: &SourceDeclaredContent,
    ) -> Result<FileMeta, ApiError> {
        let mut file = file.clone();
        if file.content_mode.has_file() {
            let book_file = self.source_book_file(&file).await?;
            let parsed = mandara_formats::parse(&book_file.bytes, &book_file.filename)?;
            return self
                .materialize_file_mode(&file, &book_file, parsed)
                .await
                .map(|(file, _)| file);
        }
        self.upsert_chapters(&file.id, &content.chapters).await?;
        file.chapter_count = content.chapters.len() as u32;
        diesel::update(schema::book_files::table.find(&file.id))
            .set(schema::book_files::chapter_count.eq(i64::from(file.chapter_count)))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        Ok(file)
    }

    /// Create a fresh metadata entry from the parsed file, applying user
    /// overrides on top.
    async fn create_book(
        &self,
        parsed: &mandara_formats::ParsedBook,
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
        ext.sanitize()?;
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
        cover: &mandara_formats::CoverImage,
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
        parsed: &mandara_formats::ParsedBook,
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
                schema::book_files::content_mode.eq(ContentMode::File.as_ref()),
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
                    schema::chapters::linear.eq(i64::from(chapter.linear)),
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
        let is_admin = user.role == mandara_core::model::Role::Admin;
        let mut conn = self.diesel_db.get().await?;

        // Books owning at least one visible file, plus metadata-only
        // entries (no files yet) owned by the caller. Optional filters are
        // assembled in Rust (type-checked builder) instead of the old
        // parameterized switches. Series first (in volume order), then
        // standalone books, newest first.
        //
        // Visibility + source are EXISTS subqueries (same shape as
        // `list_series`), never LEFT JOIN cross-table predicates: a book
        // is listed iff it owns a visible file (optionally from
        // `source`), or it is a metadata-only entry the caller created.
        // The source filter decides which books are listed, and the file
        // batch below still returns every visible file of a listed book.
        let mut book_q = schema::books::table
            .filter(schema::books::title.like(&pattern))
            .select(BookRow::as_select())
            .into_boxed::<diesel::sqlite::Sqlite>();
        if let Some(source) = source {
            // A book with at least one file from the source the caller
            // can see (admins: any file from the source). Metadata-only
            // entries have no source and are excluded, as before.
            let file_q = schema::book_files::table.filter(
                schema::book_files::book_id
                    .eq(schema::books::id)
                    .and(schema::book_files::source.eq(source)),
            );
            if !is_admin {
                book_q = book_q.filter(diesel::dsl::exists(
                    file_q.filter(
                        schema::book_files::visibility
                            .eq("public")
                            .or(schema::book_files::owner_id.eq(&user.id)),
                    ),
                ));
            } else {
                book_q = book_q.filter(diesel::dsl::exists(file_q));
            }
        } else if !is_admin {
            // At least one visible file, or a metadata-only entry (no
            // files at all) the caller created — never a book whose
            // files are all invisible to the caller.
            book_q = book_q.filter(
                diesel::dsl::exists(
                    schema::book_files::table.filter(
                        schema::book_files::book_id.eq(schema::books::id).and(
                            schema::book_files::visibility
                                .eq("public")
                                .or(schema::book_files::owner_id.eq(&user.id)),
                        ),
                    ),
                )
                .or(diesel::dsl::not(diesel::dsl::exists(
                    schema::book_files::table
                        .filter(schema::book_files::book_id.eq(schema::books::id)),
                ))
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
        let mut conn = self.diesel_db.get().await?;
        // Existence check first: a missing book is a 404, never a DB
        // error (the old path surfaced `diesel::result::Error::NotFound`
        // as a 500 from the ext read).
        let stored_ext: Option<String> = schema::books::table
            .find(id)
            .select(schema::books::ext_meta)
            .first(&mut conn)
            .await
            .optional()?;
        stored_ext
            .as_deref()
            .ok_or_else(|| ApiError::not_found("book"))?;
        // Extended metadata merges per-key on top of the stored object
        // (create-time overrides never clear what the source produced).
        // The merge result and the field update land in ONE UPDATE, so a
        // failure cannot leave a half-updated book (ext changed but the
        // title/description not, or vice versa).
        let ext_json = match ext {
            None => None,
            Some(ext) => {
                let stored = stored_ext.as_deref().unwrap_or("{}");
                let mut map =
                    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(stored)
                        .unwrap_or_default();
                let patch = serde_json::to_value(ext).unwrap_or_default();
                if let Some(obj) = patch.as_object() {
                    for (k, v) in obj {
                        if v.is_null() {
                            map.remove(k);
                        } else {
                            map.insert(k.clone(), v.clone());
                        }
                    }
                }
                let merged = BookExt::from_value(serde_json::Value::Object(map))?;
                Some(merged.to_column())
            }
        };
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
                schema::books::ext_meta.eq(coalesce_opt_nn(ext_json, schema::books::ext_meta)),
            ))
            .execute(&mut conn)
            .await?;
        drop(conn);
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
        let stored: Option<String> = schema::books::table
            .find(id)
            .select(schema::books::ext_meta)
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        let stored = stored.ok_or_else(|| ApiError::not_found("book"))?;
        let mut map = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&stored)
            .unwrap_or_default();
        for (k, v) in obj {
            if v.is_null() {
                map.remove(k);
            } else {
                map.insert(k.clone(), v.clone());
            }
        }
        let ext = BookExt::from_value(serde_json::Value::Object(map))?;
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
        if !owned && user.role != mandara_core::model::Role::Admin {
            return Err(ApiError::Forbidden);
        }

        // Files of this book backed by a plugin source.
        let plugin_files: Vec<FileRow> = schema::book_files::table
            .filter(
                schema::book_files::book_id
                    .eq(book_id)
                    .and(schema::book_files::source.ne("local")),
            )
            .select(FileRow::as_select())
            .load(&mut self.diesel_db.get().await?)
            .await?;

        // Where the metadata comes from: the recorded metadata source
        // (migration 0013), independent of the files' content sources —
        // one metadata entry can hold files from many plugins, and
        // refreshing must never overwrite A's metadata with B's.
        let row: BookRow = schema::books::table
            .find(book_id)
            .select(BookRow::as_select())
            .first(&mut self.diesel_db.get().await?)
            .await?;
        let meta_keys = row.meta_source.zip(row.meta_external_id);
        if plugin_files.is_empty() && meta_keys.is_none() {
            return Err(ApiError::bad_request(
                "book has no plugin source to refresh from",
            ));
        }

        // 1) Metadata: pulled only from the recorded metadata source.
        if let Some((meta_src, meta_ext_id)) = &meta_keys {
            let entry = self.plugins.get_book(meta_src, meta_ext_id).await?;
            let entry = entry.ok_or_else(|| {
                ApiError::NotFound(format!(
                    "source `{meta_src}` no longer offers book `{meta_ext_id}`"
                ))
            })?;
            let entry = SourceBook::try_from(entry)?;
            let authors = serde_json::to_string(&entry.authors).unwrap_or_else(|_| "[]".into());
            let ext_json = source_ext_column(entry.ext.as_ref())?;
            let series_id = match &entry.series {
                Some(series) => Some(
                    self.resolve_plugin_series(meta_src, Some(&user.id), series)
                        .await?,
                ),
                None => None,
            };
            diesel::update(schema::books::table.find(book_id))
                .set((
                    schema::books::title.eq(&entry.title),
                    schema::books::authors.eq(authors),
                    schema::books::description.eq(&entry.description),
                    schema::books::cover_url.eq(&entry.cover_url),
                    // Refresh replaces the extended metadata wholesale.
                    schema::books::ext_meta.eq(ext_json),
                    schema::books::series_id.eq(&series_id),
                    schema::books::volume_no.eq(entry.volume_no as i64),
                ))
                .execute(&mut self.diesel_db.get().await?)
                .await?;
            info!(book = %book_id, source = %meta_src, "book metadata refreshed from its metadata source");
        }

        // 2) Chapter-title placeholders / content pointers per file, from
        //    each file's own (content) source. Book metadata is untouched
        //    here — a file's plugin never overwrites it.
        for row in plugin_files {
            let mut file = row.into_model()?;
            if !file.content_mode.has_chapters() {
                continue;
            }
            file.chapter_count = self.refresh_titles(&file).await?;
            self.queue_download(&file);
            info!(book = %book_id, source = %file.source, "file chapter titles refreshed from content source");
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
            let ext = SeriesExt::from_value(serde_json::Value::Object(map))?;
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
        let is_admin = user.role == mandara_core::model::Role::Admin;
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

    /// Re-materialize a plugin file's chapter bodies: clear the stored
    /// content and queue every chapter for background download from its
    /// content source. This is the retry path for degraded
    /// materializations — e.g. images whose download failed and fell back
    /// to remote URLs; the re-pull stores them via `store-image` this
    /// time. Uploaded files use their retained original through the same loader.
    pub async fn rematerialize_file(&self, file: &FileMeta) -> Result<FileMeta, ApiError> {
        let id = &file.id;

        // Bodies from the previous materialization must not leak through;
        // keep the title rows as placeholders.
        diesel::update(schema::chapters::table.filter(schema::chapters::file_id.eq(id)))
            .set(schema::chapters::content.eq(None::<String>))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        let mut updated = file.clone();
        updated.chapter_count = self.refresh_titles(file).await?;
        self.queue_download(&updated);
        info!(file = %id, source = %file.source, "file chapters queued for re-materialization");
        Ok(updated)
    }

    pub async fn delete_file(&self, id: &str) -> Result<(), ApiError> {
        // Delete the row first: if that fails (e.g. 404), the retained
        // original is untouched. Only after the row is gone is the disk
        // file removed — a leftover disk file without a row is harmless
        // garbage, whereas a row without its bytes breaks the file.
        // Read the retained-original extension before deleting the row: the
        // disk cleanup needs it ringside, and the row is gone afterwards.
        let orig_ext: Option<Option<String>> = schema::book_files::table
            .find(id)
            .select(schema::book_files::orig_ext)
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;
        let deleted = diesel::delete(schema::book_files::table.find(id))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        if deleted == 0 {
            return Err(ApiError::not_found("file"));
        }
        if let Some(Some(ext)) = orig_ext
            && let Err(e) = tokio::fs::remove_file(self.original_path(id, &ext)).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            warn!(file = %id, ext = %ext, error = %e, "failed to remove original");
        }
        Ok(())
    }

    // ---- chapters --------------------------------------------------------

    pub async fn chapter_titles(&self, file_id: &str) -> Result<Vec<ChapterMeta>, ApiError> {
        let rows: Vec<ChapterTitleRow> = schema::chapters::table
            .filter(schema::chapters::file_id.eq(file_id))
            .order(schema::chapters::idx.asc())
            .select((
                schema::chapters::idx,
                schema::chapters::title,
                schema::chapters::linear,
            ))
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
            .select((
                schema::chapters::idx,
                schema::chapters::title,
                schema::chapters::linear,
            ))
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

    /// Read only persisted content; never contact the source or wait for a download.
    pub async fn get_chapter(&self, file: &FileMeta, idx: u32) -> Result<Chapter, ApiError> {
        self.cached_chapter(&file.id, idx)
            .await?
            .ok_or_else(|| ApiError::not_found("stored chapter"))
    }

    /// Populate content for acquisition and explicit refresh jobs.
    async fn download_chapter(&self, file: &FileMeta, idx: u32) -> Result<Chapter, ApiError> {
        if let Some(chapter) = self.cached_chapter(&file.id, idx).await? {
            return Ok(chapter);
        }
        let content = Self::wait_download(self.download_missing(file.clone(), idx))
            .await
            .map_err(Self::download_error)?;
        content.chapter(idx).map_err(Self::download_error)
    }

    fn download_error(error: super::downloads::DownloadError) -> ApiError {
        match error {
            super::downloads::DownloadError::NotFound => ApiError::not_found("chapter"),
            super::downloads::DownloadError::Failed => ApiError::internal(error),
        }
    }

    async fn wait_download(
        completed: tokio::sync::oneshot::Receiver<super::downloads::DownloadResult>,
    ) -> super::downloads::DownloadResult {
        completed
            .await
            .unwrap_or(Err(super::downloads::DownloadError::Failed))
    }

    fn download_missing(
        &self,
        file: FileMeta,
        idx: u32,
    ) -> tokio::sync::oneshot::Receiver<super::downloads::DownloadResult> {
        use super::downloads::{CacheKey, DownloadError};
        let book_file = file.content_mode.has_file();
        let key = if book_file {
            CacheKey::BookFile(file.id.clone())
        } else {
            CacheKey::Chapter(file.id.clone(), idx)
        };
        let (completed, start) = self.downloads.join(key.clone());
        if start {
            let library = self.clone();
            let downloads = self.downloads.clone();
            tokio::spawn(async move {
                // Joining also releases waiters when the writer panics.
                let result = tokio::spawn(async move {
                    if book_file {
                        library.load_book_file(&file).await
                    } else {
                        library.load_chapter(&file, idx).await
                    }
                })
                .await
                .unwrap_or_else(|error| Err(error.into()));
                let result = result.map_err(|error| {
                    if matches!(error, ApiError::NotFound(_)) {
                        DownloadError::NotFound
                    } else {
                        tracing::error!(cache_key = ?key, error = %error, "content download failed");
                        DownloadError::Failed
                    }
                });
                // Content is sent only after the cache write has completed.
                downloads.complete(&key, result);
            });
        }
        completed
    }

    /// Only provider I/O differs; parsing, caching and delivery are shared.
    async fn source_book_file(&self, file: &FileMeta) -> Result<SourceBookFile, ApiError> {
        if file.source == "local" {
            let (bytes, ext) = self
                .get_original(file)
                .await?
                .ok_or_else(|| ApiError::not_found("book file"))?;
            return Ok(SourceBookFile {
                filename: format!("{}.{}", file.id, ext),
                mime: String::new(),
                bytes,
            });
        }
        self.plugins
            .get_book_file(&file.source, &file.external_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book file"))
    }

    async fn load_book_file(
        &self,
        file: &FileMeta,
    ) -> Result<super::downloads::DownloadedContent, ApiError> {
        use super::downloads::DownloadedContent;
        let id = &file.id;
        // A writer may have completed between the caller's cache miss and registration.
        let cached = schema::chapters::table
            .filter(schema::chapters::file_id.eq(id))
            .order(schema::chapters::idx.asc())
            .select((
                schema::chapters::idx,
                schema::chapters::title,
                schema::chapters::content,
            ))
            .load::<ChapterRow>(&mut self.diesel_db.get().await?)
            .await?
            .into_iter()
            .filter_map(ChapterRow::into_model)
            .collect::<Vec<_>>();
        if file.chapter_count != 0 && cached.len() == file.chapter_count as usize {
            let mut chapters = Vec::with_capacity(cached.len());
            for mut chapter in cached {
                chapter.content = with_stored_dims(&self.diesel_db, &chapter.content).await;
                chapters.push(chapter);
            }
            return Ok(DownloadedContent::Book(chapters.into()));
        }
        let book_file = self.source_book_file(file).await?;
        let parsed = match mandara_formats::parse(&book_file.bytes, &book_file.filename) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Err(ApiError::internal(error));
            }
        };
        let (_, chapters) = self.materialize_file_mode(file, &book_file, parsed).await?;
        Ok(DownloadedContent::Book(chapters.into()))
    }

    async fn cached_chapter(&self, file_id: &str, idx: u32) -> Result<Option<Chapter>, ApiError> {
        let row: Option<ChapterRow> = schema::chapters::table
            .find((file_id, idx as i64))
            .select((
                schema::chapters::idx,
                schema::chapters::title,
                schema::chapters::content,
            ))
            .first(&mut self.diesel_db.get().await?)
            .await
            .optional()?;

        if let Some(mut model) = row.and_then(ChapterRow::into_model) {
            // Best-effort width/height on stored img tags (chapters
            // materialized before the dimensions existed lack them;
            // without them every image load re-anchors the view).
            model.content = with_stored_dims(&self.diesel_db, &model.content).await;
            return Ok(Some(model));
        }

        Ok(None)
    }

    async fn load_chapter(
        &self,
        file: &FileMeta,
        idx: u32,
    ) -> Result<super::downloads::DownloadedContent, ApiError> {
        if let Some(chapter) = self.cached_chapter(&file.id, idx).await? {
            return Ok(super::downloads::DownloadedContent::Chapter(chapter));
        }
        let (source_id, external_id) = (file.source.as_str(), file.external_id.as_str());

        // Chapter mode (acquisition or explicit refresh pulls from the content source).
        // Plugin indices are book-local. Plugin text is
        // normalized to canonical HTML at the ingest boundary (including
        // the illustration conventions; see mandara_formats::htmlize).
        let Some(chapter) = self
            .plugins
            .get_chapter(source_id, external_id, idx)
            .await?
        else {
            return Err(ApiError::not_found("chapter"));
        };
        let content = with_stored_dims(
            &self.diesel_db,
            &mandara_formats::htmlize::plugin_to_html(&chapter.format, &chapter.content),
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
                schema::chapters::linear.eq(diesel::upsert::excluded(schema::chapters::linear)),
                schema::chapters::content.eq(diesel::upsert::excluded(schema::chapters::content)),
            ))
            .execute(&mut self.diesel_db.get().await?)
            .await?;
        Ok(super::downloads::DownloadedContent::Chapter(Chapter {
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
        // Preserve all cached bodies, including valid empty chapters.
        let has_body: std::collections::HashSet<i64> = schema::chapters::table
            .filter(
                schema::chapters::file_id
                    .eq(file_id)
                    .and(schema::chapters::content.is_not_null()),
            )
            .select(schema::chapters::idx)
            .load::<i64>(&mut conn)
            .await?
            .into_iter()
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
                    schema::chapters::content.eq(None::<String>),
                ))
                .on_conflict((schema::chapters::file_id, schema::chapters::idx))
                .do_update()
                .set(schema::chapters::title.eq(diesel::upsert::excluded(schema::chapters::title)))
                .execute(&mut conn)
                .await?;
        }
        Ok(())
    }

    /// Refresh stored titles from the content source after an explicit user action.
    async fn refresh_titles(&self, file: &FileMeta) -> Result<u32, ApiError> {
        let (source_id, external_id) = (file.source.as_str(), file.external_id.as_str());
        if !file.content_mode.has_chapters() {
            return Ok(file.chapter_count);
        }
        let content = self
            .plugins
            .get_content(source_id, external_id, false)
            .await?
            .ok_or_else(|| ApiError::not_found("content"))?;
        let titles = content
            .chapters
            .into_iter()
            .map(|chapter| chapter.title)
            .collect::<Vec<_>>();
        if titles.is_empty() {
            return Ok(file.chapter_count);
        }
        // The plugin returns only this content item's chapter titles.
        self.upsert_placeholder_titles(&file.id, &titles).await?;
        // Placeholder rows beyond the book's range are stale (previous
        // chapter count that shrank); drop the
        // uncached ones. Materialized bodies stay untouched.
        diesel::delete(
            schema::chapters::table.filter(
                schema::chapters::file_id
                    .eq(&file.id)
                    .and(schema::chapters::content.is_null())
                    .and(schema::chapters::idx.ge(titles.len() as i64)),
            ),
        )
        .execute(&mut self.diesel_db.get().await?)
        .await?;
        if titles.len() != file.chapter_count as usize {
            diesel::update(schema::book_files::table.find(&file.id))
                .set(schema::book_files::chapter_count.eq(titles.len() as i64))
                .execute(&mut self.diesel_db.get().await?)
                .await?;
        }
        Ok(titles.len() as u32)
    }
}

fn detect_format(filename: &str) -> FileFormat {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".epub") {
        FileFormat::Epub
    } else {
        FileFormat::Txt
    }
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
    let dims = mandara_formats::imgdim::image_dimensions(bytes);
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
            let reference = match mandara_formats::imgdim::image_dimensions(&parsed.images[n].bytes)
            {
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
/// any lookup failure returns the HTML unchanged). This is the DB-backed
/// wrapper around the pure [`annotate_image_dimensions`] (covered by the
/// tests directly).
async fn with_stored_dims(db: &DieselDb, html: &str) -> String {
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
        let mut parsed = match mandara_formats::parse(&bytes, &format!("original.{ext}")) {
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
                    schema::chapters::linear.eq(i64::from(chapter.linear)),
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

/// Lowercase hex sha-256 digest, passed to plugins for upload
/// identification.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_formats::ParsedImage;

    async fn download_fixture() -> (Library, FileMeta, PathBuf) {
        let dir = std::env::temp_dir().join(format!("mandara-download-{}", uuid::Uuid::new_v4()));
        let path = dir.join("library.db");
        crate::db::run_migrations(&path).await.unwrap();
        let db = crate::db::connect_diesel(&path).unwrap();
        let images = crate::service::images::DbImageStore::new(db.clone(), dir.join("files"));
        let wasm = Arc::new(
            mandara_plugin::WasmPlugin::load(
                "fixture.wasm",
                include_bytes!("../../../mandara-plugin/tests/fixtures/hello.wasm").to_vec(),
                Arc::new(mandara_plugin::FetchPolicy::default()),
                images,
            )
            .unwrap(),
        );
        let plugins = Arc::new(PluginService::new(db.clone(), vec![wasm]));
        plugins.register("fixture", "fixture.wasm").await.unwrap();
        plugins
            .set_config("fixture", serde_json::json!({"site-name": "示例"}))
            .await
            .unwrap();
        let library = Library::new(db, plugins, dir.join("files"));
        let book = SourceBook {
            id: "hello-1".into(),
            title: "示例".into(),
            authors: vec![],
            description: None,
            cover_url: None,
            ext: None,
            series: None,
            volume_no: 0,
        };
        let book_id = library
            .resolve_plugin_metadata("fixture", None, &book)
            .await
            .unwrap();
        let file = library
            .attach_plugin_file(
                "fixture",
                None,
                "hello-content-1",
                ContentMode::Chapters,
                &book_id,
            )
            .await
            .unwrap();
        let content = library
            .plugins
            .get_content("fixture", "hello-content-1", true)
            .await
            .unwrap()
            .unwrap();
        let file = library
            .materialize_plugin_content(&file, &content)
            .await
            .unwrap();
        (library, file, dir)
    }

    async fn await_download(library: &Library, id: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let file = library.get_file(id).await.unwrap().unwrap();
                if cached_count(library, &file).await >= i64::from(file.chapter_count) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("background downloads populate the persistent cache");
    }

    async fn cached_count(library: &Library, file: &FileMeta) -> i64 {
        schema::chapters::table
            .filter(
                schema::chapters::file_id
                    .eq(&file.id)
                    .and(schema::chapters::content.is_not_null()),
            )
            .count()
            .get_result(&mut library.diesel_db.get().await.unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn content_attachment_does_not_require_book_metadata_from_its_provider() {
        let (library, fixture, dir) = download_fixture().await;
        diesel::insert_into(schema::users::table)
            .values((
                schema::users::id.eq("u"),
                schema::users::username.eq("示例用户"),
                schema::users::password_hash.eq("unused"),
                schema::users::role.eq("admin"),
            ))
            .execute(&mut library.diesel_db.get().await.unwrap())
            .await
            .unwrap();
        let user = User {
            id: "u".into(),
            username: "示例用户".into(),
            role: Role::Admin,
            created_at: chrono::Utc::now(),
        };
        let original = library.get_book(&fixture.book_id).await.unwrap().unwrap();
        let result = library
            .acquire_book(
                &user,
                AcquireContent::Plugin {
                    source: "fixture",
                    book_id_in_source: "hello-content-2",
                },
                AcquireMetadata::Attach {
                    book_id: &fixture.book_id,
                },
                Visibility::Public,
                "正文",
            )
            .await
            .unwrap();
        let (book, content, _) = result.into_parts();
        let content = content.unwrap();
        assert_eq!(book.id, original.id);
        assert_eq!(book.title, original.title);
        assert_eq!(content.external_id, "hello-content-2");
        assert_eq!(content.content_mode, ContentMode::Chapters);
        // Acquisition persists the directory before returning, independently of body downloads.
        assert_eq!(library.chapter_titles(&content.id).await.unwrap().len(), 3);
        await_download(&library, &content.id).await;
        assert_eq!(library.get_chapter(&content, 2).await.unwrap().content, "");
        library.delete_file(&content.id).await.unwrap();
        library.delete_file(&fixture.id).await.unwrap();
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn downloads_use_the_supplied_content_form_without_reloading_metadata() {
        let (library, mut file, dir) = download_fixture().await;
        assert_eq!(file.content_mode, ContentMode::Chapters);
        file.content_mode = ContentMode::File;
        // The fixture offers chapters but no book file: no host fallback or requery.
        assert!(matches!(
            library.download_chapter(&file, 0).await,
            Err(ApiError::NotFound(_))
        ));
        file.content_mode = ContentMode::Chapters;
        file.chapter_count = 0;
        assert!(
            library
                .download_chapter(&file, 1)
                .await
                .unwrap()
                .content
                .contains("插件")
        );
        library.delete_file(&file.id).await.unwrap();
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn uploaded_files_use_the_same_materialization_path() {
        let (library, fixture, dir) = download_fixture().await;
        diesel::insert_into(schema::users::table)
            .values((
                schema::users::id.eq("u"),
                schema::users::username.eq("示例用户"),
                schema::users::password_hash.eq("unused"),
                schema::users::role.eq("admin"),
            ))
            .execute(&mut library.diesel_db.get().await.unwrap())
            .await
            .unwrap();
        let bytes = "第一章 空章节\n\n第二章 正文\n来自保留的原文件。".as_bytes();
        let parsed = mandara_formats::parse(bytes, "示例.txt").unwrap();
        let file = library
            .store_local_file(
                &parsed,
                bytes,
                &fixture.book_id,
                "u",
                "示例.txt",
                Visibility::Public,
                "原文件",
            )
            .await
            .unwrap();
        assert_eq!(file.content_mode, ContentMode::File);
        assert_eq!(library.chapter_titles(&file.id).await.unwrap().len(), 2);
        assert_eq!(library.get_chapter(&file, 0).await.unwrap().content, "");
        diesel::update(schema::chapters::table.filter(schema::chapters::file_id.eq(&file.id)))
            .set(schema::chapters::content.eq(None::<String>))
            .execute(&mut library.diesel_db.get().await.unwrap())
            .await
            .unwrap();
        assert!(matches!(
            library.get_chapter(&file, 0).await,
            Err(ApiError::NotFound(_))
        ));
        // Even retained local originals are reparsed only by an explicit refresh.
        library.rematerialize_file(&file).await.unwrap();
        await_download(&library, &file.id).await;
        assert_eq!(library.get_chapter(&file, 0).await.unwrap().content, "");
        assert!(
            library
                .get_chapter(&file, 1)
                .await
                .unwrap()
                .content
                .contains("原文件")
        );
        assert_eq!(
            library
                .get_file(&file.id)
                .await
                .unwrap()
                .unwrap()
                .content_mode,
            ContentMode::File
        );
        library.delete_file(&file.id).await.unwrap();
        library.delete_file(&fixture.id).await.unwrap();
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn declared_bodies_preserve_presence_through_ingestion() {
        let (library, file, dir) = download_fixture().await;
        let catalog = library
            .plugins
            .declared_catalog("fixture")
            .await
            .unwrap()
            .unwrap();
        let content = catalog
            .books
            .into_iter()
            .filter_map(|book| book.content)
            .find(|content| {
                content
                    .chapters
                    .iter()
                    .any(|chapter| chapter.content.is_none())
            })
            .expect("fixture declares lazy content");
        assert_eq!(content.chapters[1].content, None);
        assert_eq!(content.chapters[2].content.as_deref(), Some(""));
        library
            .upsert_chapters(&file.id, &content.chapters)
            .await
            .unwrap();
        let original = library
            .cached_chapter(&file.id, 0)
            .await
            .unwrap()
            .unwrap()
            .content;
        assert!(original.contains("声明式"));
        assert!(library.cached_chapter(&file.id, 1).await.unwrap().is_none());
        assert_eq!(
            library
                .cached_chapter(&file.id, 2)
                .await
                .unwrap()
                .unwrap()
                .content,
            ""
        );
        assert_eq!(cached_count(&library, &file).await, 2);

        // A missing declaration keeps an existing body, even an empty one.
        let missing = content
            .chapters
            .iter()
            .map(|chapter| SourceDeclaredChapter {
                title: chapter.title.clone(),
                format: chapter.format.clone(),
                content: None,
            })
            .collect::<Vec<_>>();
        library.upsert_chapters(&file.id, &missing).await.unwrap();
        assert_eq!(
            library
                .cached_chapter(&file.id, 0)
                .await
                .unwrap()
                .unwrap()
                .content,
            original
        );
        assert_eq!(
            library
                .cached_chapter(&file.id, 2)
                .await
                .unwrap()
                .unwrap()
                .content,
            ""
        );

        // Supplied empty, blank and sanitized-away bodies are all materialized.
        let supplied = vec![
            SourceDeclaredChapter {
                title: "空章".into(),
                format: "text".into(),
                content: Some("".into()),
            },
            SourceDeclaredChapter {
                title: "空白章".into(),
                format: "text".into(),
                content: Some(" \n\t".into()),
            },
            SourceDeclaredChapter {
                title: "过滤后空章".into(),
                format: "html".into(),
                content: Some("<script>removed</script>".into()),
            },
        ];
        library.upsert_chapters(&file.id, &supplied).await.unwrap();
        assert_eq!(cached_count(&library, &file).await, 3);
        for idx in 0..3 {
            assert_eq!(
                library
                    .cached_chapter(&file.id, idx)
                    .await
                    .unwrap()
                    .unwrap()
                    .content,
                ""
            );
        }
        assert_eq!(library.get_chapter(&file, 0).await.unwrap().content, "");
        library.delete_file(&file.id).await.unwrap();
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn empty_cached_bodies_are_distinct_from_null_placeholders() {
        use super::super::downloads::{CacheKey, DownloadError};
        let (library, file, dir) = download_fixture().await;
        assert!(library.cached_chapter(&file.id, 0).await.unwrap().is_none());
        assert_eq!(cached_count(&library, &file).await, 0);
        for (idx, body) in [(0_i64, ""), (1_i64, " \n\t")] {
            diesel::update(schema::chapters::table.find((&file.id, idx)))
                .set(schema::chapters::content.eq(body))
                .execute(&mut library.diesel_db.get().await.unwrap())
                .await
                .unwrap();
        }
        assert_eq!(cached_count(&library, &file).await, 2);
        // Cache hits return empty content without contacting the source.
        assert_eq!(library.get_chapter(&file, 0).await.unwrap().content, "");
        assert_eq!(
            library.get_chapter(&file, 1).await.unwrap().content,
            " \n\t"
        );
        let book = library.load_book_file(&file).await.unwrap();
        assert_eq!(book.clone().chapter(0).unwrap().content, "");
        assert_eq!(book.chapter(1).unwrap().content, " \n\t");
        library
            .upsert_placeholder_titles(
                &file.id,
                &["新标题一".into(), "新标题二".into(), "未下载".into()],
            )
            .await
            .unwrap();
        assert_eq!(
            library.get_chapter(&file, 0).await.unwrap().title,
            "第一章 你好，书架"
        );
        assert!(library.cached_chapter(&file.id, 2).await.unwrap().is_none());
        // Lazy declarations cannot replace an already cached empty body with NULL.
        library
            .upsert_chapters(
                &file.id,
                &[SourceDeclaredChapter {
                    title: "空章".into(),
                    content: None,
                    format: "text".into(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(library.get_chapter(&file, 0).await.unwrap().content, "");
        assert_eq!(cached_count(&library, &file).await, 2);
        diesel::insert_into(schema::chapters::table)
            .values((
                schema::chapters::file_id.eq(&file.id),
                schema::chapters::idx.eq(3_i64),
                schema::chapters::title.eq("空附录"),
                schema::chapters::content.eq(""),
            ))
            .execute(&mut library.diesel_db.get().await.unwrap())
            .await
            .unwrap();
        library.refresh_titles(&file).await.unwrap();
        assert!(library.cached_chapter(&file.id, 2).await.unwrap().is_none());
        assert_eq!(
            library
                .cached_chapter(&file.id, 3)
                .await
                .unwrap()
                .unwrap()
                .content,
            ""
        );
        // Keep background writers pending so invalidation can be inspected directly.
        let first = CacheKey::Chapter(file.id.clone(), 0);
        let second = CacheKey::Chapter(file.id.clone(), 1);
        let (_, start) = library.downloads.join(first.clone());
        assert!(start);
        let (_, start) = library.downloads.join(second.clone());
        assert!(start);
        library.rematerialize_file(&file).await.unwrap();
        assert_eq!(cached_count(&library, &file).await, 0);
        assert!(library.cached_chapter(&file.id, 0).await.unwrap().is_none());
        library
            .downloads
            .complete(&first, Err(DownloadError::NotFound));
        library
            .downloads
            .complete(&second, Err(DownloadError::NotFound));
        library.delete_file(&file.id).await.unwrap();
        drop(library);
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }

    #[tokio::test]
    async fn cached_reads_bypass_inflight_fetches_and_other_chapters_can_load() {
        let (library, file, dir) = download_fixture().await;
        let key = super::super::downloads::CacheKey::Chapter(file.id.clone(), 0);
        let (pending, start) = library.downloads.join(key.clone());
        assert!(start);
        let (waiting, start) = library.downloads.join(key.clone());
        assert!(!start);
        assert!(matches!(
            library.get_chapter(&file, 0).await,
            Err(ApiError::NotFound(_))
        ));
        let mut reader = {
            let library = library.clone();
            let file = file.clone();
            tokio::spawn(async move { library.download_chapter(&file, 0).await })
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut reader)
                .await
                .is_err()
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            // A different missing chapter must not wait for chapter zero.
            assert_eq!(library.download_chapter(&file, 1).await.unwrap().idx, 1);
            diesel::update(schema::chapters::table.find((&file.id, 0_i64)))
                .set(schema::chapters::content.eq("<p>已缓存</p>"))
                .execute(&mut library.diesel_db.get().await.unwrap())
                .await
                .unwrap();
            assert_eq!(
                library.get_chapter(&file, 0).await.unwrap().content,
                "<p>已缓存</p>"
            );
        })
        .await
        .expect("cached reads and different chapters bypass the slow fetch");
        // The waiter must return the delivered content, even if the cache changes.
        let delivered = library.cached_chapter(&file.id, 0).await.unwrap().unwrap();
        diesel::update(schema::chapters::table.find((&file.id, 0_i64)))
            .set(schema::chapters::content.eq(None::<String>))
            .execute(&mut library.diesel_db.get().await.unwrap())
            .await
            .unwrap();
        library.downloads.complete(
            &key,
            Ok(super::super::downloads::DownloadedContent::Chapter(
                delivered,
            )),
        );
        pending.await.unwrap().unwrap();
        waiting.await.unwrap().unwrap();
        assert_eq!(reader.await.unwrap().unwrap().content, "<p>已缓存</p>");
        library.delete_file(&file.id).await.unwrap();
        assert!(library.get_file(&file.id).await.unwrap().is_none());
        drop(library);
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }

    #[tokio::test]
    async fn background_downloads_reuse_cache_and_keep_content_items_independent() {
        let (library, first, dir) = download_fixture().await;
        assert_eq!(first.content_mode, ContentMode::Chapters);
        diesel::update(schema::chapters::table.find((&first.id, 0_i64)))
            .set(schema::chapters::content.eq("<p>保留缓存</p>"))
            .execute(&mut library.diesel_db.get().await.unwrap())
            .await
            .unwrap();
        library
            .plugins
            .register("second", "fixture.wasm")
            .await
            .unwrap();
        library
            .plugins
            .set_config("second", serde_json::json!({"site-name":"示例", "mode":1}))
            .await
            .unwrap();
        let second = library
            .attach_plugin_file(
                "second",
                None,
                "hello-content-1",
                ContentMode::Chapters,
                &first.book_id,
            )
            .await
            .unwrap();
        let content = library
            .plugins
            .get_content("second", "hello-content-1", true)
            .await
            .unwrap()
            .unwrap();
        let second = library
            .materialize_plugin_content(&second, &content)
            .await
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(first.book_id, second.book_id);
        library.queue_download(&first);
        library.queue_download(&first);
        library.queue_download(&second);
        await_download(&library, &first.id).await;
        await_download(&library, &second.id).await;
        assert_eq!(cached_count(&library, &first).await, 2);
        assert_eq!(cached_count(&library, &second).await, 2);
        assert_eq!(
            library.get_chapter(&first, 0).await.unwrap().content,
            "<p>保留缓存</p>"
        );
        let alternate = library.get_chapter(&second, 0).await.unwrap().content;
        assert!(alternate.contains("alt"));
        library.rematerialize_file(&second).await.unwrap();
        await_download(&library, &second.id).await;
        assert_eq!(cached_count(&library, &second).await, 2);
        library.delete_file(&first.id).await.unwrap();
        assert!(library.get_file(&first.id).await.unwrap().is_none());
        assert_eq!(
            library.get_chapter(&second, 0).await.unwrap().content,
            alternate
        );
        drop(library);
        assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }

    #[tokio::test]
    async fn explicit_series_groups_books_by_instance_and_opaque_series_id() {
        let dir = std::env::temp_dir().join(format!("mandara-series-{}", uuid::Uuid::new_v4()));
        let path = dir.join("library.db");
        crate::db::run_migrations(&path).await.unwrap();
        let db = crate::db::connect_diesel(&path).unwrap();
        let library = Library::new(
            db.clone(),
            Arc::new(PluginService::new(db.clone(), vec![])),
            dir.join("files"),
        );
        let mut book = SourceBook {
            id: "opaque-volume-alpha".into(),
            title: "第一卷".into(),
            authors: vec![],
            description: None,
            cover_url: None,
            ext: None,
            series: Some(SourceSeries {
                id: "opaque-series".into(),
                title: "系列".into(),
                authors: vec![],
                description: None,
                cover_url: None,
                ext: None,
            }),
            volume_no: 1,
        };
        let first = library
            .resolve_plugin_metadata("source-a", None, &book)
            .await
            .unwrap();
        book.id = "opaque-volume-beta".into();
        book.title = "第二卷".into();
        book.volume_no = 2;
        let second = library
            .resolve_plugin_metadata("source-a", None, &book)
            .await
            .unwrap();
        let other = library
            .resolve_plugin_metadata("source-b", None, &book)
            .await
            .unwrap();
        let first = library.get_book(&first).await.unwrap().unwrap();
        let second = library.get_book(&second).await.unwrap().unwrap();
        let other = library.get_book(&other).await.unwrap().unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(first.series_id, second.series_id);
        assert_ne!(first.series_id, other.series_id);
        assert_ne!(first.id, "opaque-volume-alpha");
        assert_eq!(second.volume_no, 2);
        let count: i64 = schema::book_files::table
            .filter(schema::book_files::book_id.eq(&second.id))
            .count()
            .get_result(&mut db.get().await.unwrap())
            .await
            .unwrap();
        assert_eq!(count, 0);
        let count = library
            .sync_catalog(&DeclaredCatalog {
                instance: "metadata-only".into(),
                books: vec![crate::service::plugins::DeclaredCatalogBook {
                    book,
                    content: None,
                }],
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
        let count: i64 = schema::book_files::table
            .count()
            .get_result(&mut db.get().await.unwrap())
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "declared metadata books do not invent a content file"
        );
    }

    #[tokio::test]
    async fn chapter_linear_migration_preserves_legacy_rows_and_auxiliary_flags() {
        use diesel_async::SimpleAsyncConnection as _;
        let dir =
            std::env::temp_dir().join(format!("mandara-linear-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::connect_diesel(&dir.join("test.db")).unwrap();
        let mut conn = db.get().await.unwrap();
        conn.batch_execute("CREATE TABLE chapters (file_id TEXT NOT NULL, idx INTEGER NOT NULL, title TEXT NOT NULL, content TEXT NOT NULL, PRIMARY KEY(file_id,idx)); INSERT INTO chapters VALUES ('f',0,'正文','<p>正文</p>');").await.unwrap();
        conn.batch_execute(include_str!("../../migrations/0015_chapter_linear.sql"))
            .await
            .unwrap();
        diesel::insert_into(schema::chapters::table)
            .values((
                schema::chapters::file_id.eq("f"),
                schema::chapters::idx.eq(1_i64),
                schema::chapters::title.eq("注释"),
                schema::chapters::content.eq("<p>注释</p>"),
                schema::chapters::linear.eq(0_i64),
            ))
            .execute(&mut conn)
            .await
            .unwrap();
        let rows: Vec<ChapterTitleRow> = schema::chapters::table
            .order(schema::chapters::idx.asc())
            .select(ChapterTitleRow::as_select())
            .load(&mut conn)
            .await
            .unwrap();
        let metas: Vec<_> = rows.into_iter().map(ChapterTitleRow::into_model).collect();
        assert!(metas[0].linear);
        assert!(!metas[1].linear);
        assert_eq!(metas[0].title, "正文");
        assert!(
            conn.batch_execute("UPDATE chapters SET linear=2 WHERE idx=1")
                .await
                .is_err()
        );
        drop(conn);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn ingest_stores_images_and_rewrites_placeholders() {
        let dir = std::env::temp_dir().join(format!(
            "mandara-ingest-test-{}-{}",
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
            chapters: vec![mandara_formats::ParsedChapter {
                linear: true,
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
