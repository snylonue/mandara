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
use bookshelf_core::model::{BookMeta, Chapter, ChapterFormat, ChapterMeta, FileMeta, TocNode, User, Visibility};
use bookshelf_core::source::BookSource;

use crate::error::ApiError;
use crate::rows::{BookRow, ChapterRow, ChapterTitleRow, FileRow};

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
                self.upsert_plugin_book(source.id(), book, &titles).await?;
            }
            info!(source = source.id(), books = count, "plugin catalog synced");
            total += count;
        }
        Ok(total)
    }

    async fn upsert_plugin_book(
        &self,
        source_id: &str,
        book: &bookshelf_core::source::SourceBook,
        titles: &[String],
    ) -> Result<(), ApiError> {
        let authors = serde_json::to_string(&book.authors).unwrap_or_else(|_| "[]".into());

        // Reuse the metadata entry when this plugin file was synced before,
        // otherwise create both the book metadata and its virtual file.
        let existing: Option<(String,)> = sqlx::query_as(
            "SELECT book_id FROM book_files WHERE source = ? AND external_id = ?",
        )
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
                     VALUES (?, ?, ?, ?, ?, NULL)",
                )
                .bind(&book_id)
                .bind(&book.title)
                .bind(&authors)
                .bind(&book.description)
                .bind(&book.cover_url)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO book_files (id, book_id, source, external_id, format, label, visibility, owner_id, chapter_count) \
                     VALUES (?, ?, ?, ?, 'plugin', '', 'public', NULL, ?)",
                )
                .bind(uuid::Uuid::new_v4().simple().to_string())
                .bind(&book_id)
                .bind(source_id)
                .bind(&book.id)
                .bind(titles.len() as i64)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                book_id
            }
        };

        // Keep title/metadata fresh on every sync. Plugin files stay public
        // (shared with every logged-in user).
        sqlx::query(
            "UPDATE books SET title = ?, authors = ?, description = ?, cover_url = ? WHERE id = ?",
        )
        .bind(&book.title)
        .bind(&authors)
        .bind(&book.description)
        .bind(&book.cover_url)
        .bind(&book_id)
        .execute(&self.db)
        .await?;
        sqlx::query(
            "UPDATE book_files SET chapter_count = ? WHERE source = ? AND external_id = ?",
        )
        .bind(titles.len() as i64)
        .bind(source_id)
        .bind(&book.id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    pub fn source(&self, id: &str) -> Option<Arc<dyn BookSource>> {
        self.sources.iter().find(|s| s.id() == id).cloned()
    }

    // ---- uploads ---------------------------------------------------------

    /// Parse an uploaded epub/txt file into a *new* metadata entry plus its
    /// first file.
    pub async fn ingest_upload(
        &self,
        owner_id: &str,
        filename: &str,
        bytes: &[u8],
        visibility: Visibility,
        label: &str,
    ) -> Result<(BookMeta, FileMeta), ApiError> {
        let parsed = bookshelf_formats::parse(bytes, filename)?;
        let book_id = uuid::Uuid::new_v4().simple().to_string();
        let file_id = uuid::Uuid::new_v4().simple().to_string();
        let format = detect_format(filename);
        let authors = serde_json::to_string(&parsed.authors).unwrap_or_else(|_| "[]".into());

        let mut tx = self.db.begin().await?;
        sqlx::query(
            "INSERT INTO books (id, title, authors, description, cover_url, created_by) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&book_id)
        .bind(&parsed.title)
        .bind(authors)
        .bind(&parsed.description)
        .bind(&parsed.cover_url)
        .bind(owner_id)
        .execute(&mut *tx)
        .await?;

        let toc_json = serde_json::to_string(&parsed.toc).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "INSERT INTO book_files (id, book_id, source, external_id, format, label, visibility, owner_id, chapter_count, toc) \
             VALUES (?, ?, 'local', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(&book_id)
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

        let book = self
            .get_book(&book_id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))?;
        let file = self
            .get_file(&file_id)
            .await?
            .ok_or_else(|| ApiError::not_found("file"))?;
        Ok((book, file))
    }

    /// Attach another uploaded file (a different format/edition) to an
    /// existing metadata entry.
    pub async fn attach_upload(
        &self,
        book_id: &str,
        owner_id: &str,
        filename: &str,
        bytes: &[u8],
        visibility: Visibility,
        label: &str,
    ) -> Result<FileMeta, ApiError> {
        let parsed = bookshelf_formats::parse(bytes, filename)?;
        // For editions of the same work, reuse this book's title when the
        // parsed one is missing.
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
    ) -> Result<BookMeta, ApiError> {
        sqlx::query(
            "UPDATE books SET title = COALESCE(?, title), description = COALESCE(?, description) WHERE id = ?",
        )
        .bind(title)
        .bind(description)
        .bind(id)
        .execute(&self.db)
        .await?;
        self.get_book(id)
            .await?
            .ok_or_else(|| ApiError::not_found("book"))
    }

    /// Delete a metadata entry; its files (and their chapters/sessions/
    /// shares) cascade away.
    pub async fn delete_book(&self, id: &str) -> Result<(), ApiError> {
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
        let rows: Vec<ChapterTitleRow> = sqlx::query_as(
            "SELECT idx, title FROM chapters WHERE file_id = ? ORDER BY idx",
        )
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
        let rows: Vec<ChapterTitleRow> = sqlx::query_as(
            "SELECT idx, title FROM chapters WHERE file_id = ? ORDER BY idx",
        )
        .bind(file_id)
        .fetch_all(&self.db)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| TocNode {
                title: r.title,
                idx: Some(r.idx.max(0) as u32),
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
            let titles: CoreResult<Vec<String>> =
                source.chapter_titles(&file.external_id).await;
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