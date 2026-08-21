//! The library service: central storage for books, metadata and chapters.
//!
//! Books from every source (local uploads and wasm plugins) are catalogued
//! here; plugin chapters are materialized into the same tables on first
//! access, so readers see one unified store.

use std::sync::Arc;

use sqlx::SqlitePool;
use tracing::info;

use bookshelf_core::error::Result as CoreResult;
use bookshelf_core::model::{BookMeta, Chapter, ChapterMeta, User, Visibility};
use bookshelf_core::source::BookSource;

use crate::error::ApiError;
use crate::rows::{BookRow, ChapterRow, ChapterTitleRow};

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

    /// Re-catalogue all plugin books into the library.
    /// Returns the number of books upserted.
    pub async fn sync_plugins(&self) -> Result<usize, ApiError> {
        let mut total = 0;
        for source in &self.sources {
            let books = source.list_books().await?;
            let count = books.len();
            for book in &books {
                let titles = source.chapter_titles(&book.id).await?;
                let authors = serde_json::to_string(&book.authors).unwrap_or_else(|_| "[]".into());
                let result = sqlx::query(
                    "INSERT INTO books (id, source, external_id, title, authors, description, cover_url, visibility, chapter_count, owner_id) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, 'public', ?, NULL) \
                     ON CONFLICT (source, external_id) DO UPDATE SET \
                         title = excluded.title, authors = excluded.authors, \
                         description = excluded.description, cover_url = excluded.cover_url, \
                         chapter_count = excluded.chapter_count, visibility = excluded.visibility",
                )
                .bind(uuid::Uuid::new_v4().simple().to_string())
                .bind(source.id())
                .bind(&book.id)
                .bind(&book.title)
                .bind(authors)
                .bind(&book.description)
                .bind(&book.cover_url)
                .bind(titles.len() as i64)
                .execute(&self.db)
                .await?;
                total += result.rows_affected() as usize;
            }
            info!(source = source.id(), books = count, "plugin catalog synced");
        }
        Ok(total)
    }

    pub fn source(&self, id: &str) -> Option<Arc<dyn BookSource>> {
        self.sources.iter().find(|s| s.id() == id).cloned()
    }

    // ---- books -----------------------------------------------------------

    /// Parse and store an uploaded epub/txt file.
    pub async fn ingest_upload(
        &self,
        owner_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<BookMeta, ApiError> {
        let parsed = bookshelf_formats::parse(bytes, filename)?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let authors = serde_json::to_string(&parsed.authors).unwrap_or_else(|_| "[]".into());
        let chapter_count = parsed.chapters.len() as i64;

        let mut tx = self.db.begin().await?;
        sqlx::query(
            "INSERT INTO books (id, source, external_id, title, authors, description, cover_url, visibility, owner_id, chapter_count) \
             VALUES (?, 'local', ?, ?, ?, ?, ?, 'private', ?, ?)",
        )
        .bind(&id)
        .bind(&id)
        .bind(&parsed.title)
        .bind(authors)
        .bind(&parsed.description)
        .bind(&parsed.cover_url)
        .bind(owner_id)
        .bind(chapter_count)
        .execute(&mut *tx)
        .await?;

        for (idx, chapter) in parsed.chapters.iter().enumerate() {
            sqlx::query("INSERT INTO chapters (book_id, idx, title, content) VALUES (?, ?, ?, ?)")
                .bind(&id)
                .bind(idx as i64)
                .bind(&chapter.title)
                .bind(&chapter.content)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        self.get_book(&id).await?.ok_or_else(|| ApiError::not_found("book"))
    }

    /// List books the user is allowed to see, optionally filtered by title
    /// substring and source.
    pub async fn list_books(
        &self,
        query: Option<&str>,
        source: Option<&str>,
        user: &User,
    ) -> Result<Vec<BookMeta>, ApiError> {
        let pattern = format!("%{}%", query.unwrap_or("").trim());
        let is_admin = user.role == bookshelf_core::model::Role::Admin;

        let mut q = sqlx::QueryBuilder::new(
            "SELECT id, source, external_id, title, authors, description, cover_url, visibility, owner_id, chapter_count, created_at FROM books WHERE title LIKE ",
        );
        q.push_bind(pattern);
        if let Some(src) = source {
            q.push(" AND source = ").push_bind(src);
        }
        if is_admin {
            q.push(" ORDER BY created_at DESC");
        } else {
            q.push(" AND (visibility = 'public' OR owner_id = ")
                .push_bind(&user.id)
                .push(") ORDER BY created_at DESC");
        }
        let rows: Vec<BookRow> = q.build_query_as().fetch_all(&self.db).await?;
        rows.into_iter().map(BookRow::into_model).collect()
    }

    pub async fn get_book(&self, id: &str) -> Result<Option<BookMeta>, ApiError> {
        let row: Option<BookRow> =
            sqlx::query_as("SELECT id, source, external_id, title, authors, description, cover_url, visibility, owner_id, chapter_count, created_at FROM books WHERE id = ?")
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
        visibility: Option<Visibility>,
    ) -> Result<BookMeta, ApiError> {
        sqlx::query(
            "UPDATE books SET title = COALESCE(?, title), description = COALESCE(?, description), visibility = COALESCE(?, visibility) WHERE id = ?",
        )
        .bind(title)
        .bind(description)
        .bind(visibility.map(|v| v.as_str()))
        .bind(id)
        .execute(&self.db)
        .await?;
        self.get_book(id).await?.ok_or_else(|| ApiError::not_found("book"))
    }

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

    // ---- chapters --------------------------------------------------------

    pub async fn chapter_titles(&self, book_id: &str) -> Result<Vec<ChapterMeta>, ApiError> {
        let rows: Vec<ChapterTitleRow> = sqlx::query_as(
            "SELECT idx, title FROM chapters WHERE book_id = ? ORDER BY idx",
        )
        .bind(book_id)
        .fetch_all(&self.db)
        .await?;
        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    /// Fetch a chapter, materializing it from its plugin source on first
    /// access.
    pub async fn get_chapter(
        &self,
        book: &BookMeta,
        idx: u32,
    ) -> Result<Option<Chapter>, ApiError> {
        if idx as i64 >= book.chapter_count as i64 {
            return Ok(None);
        }
        let row: Option<ChapterRow> =
            sqlx::query_as("SELECT idx, title, content FROM chapters WHERE book_id = ? AND idx = ?")
                .bind(&book.id)
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

        // Not stored yet: lazily materialize from the plugin source.
        let Some(source) = self.source(&book.source) else {
            return Ok(None);
        };
        let external = &book.external_id;
        let fetched = self.materialize_from_source(source, external, &book.id, idx).await?;
        if let Some(chapter) = fetched {
            Ok(Some(chapter))
        } else {
            Ok(None)
        }
    }

    async fn materialize_from_source(
        &self,
        source: Arc<dyn BookSource>,
        external_id: &str,
        book_id: &str,
        idx: u32,
    ) -> Result<Option<Chapter>, ApiError> {
        let Some(chapter) = source.get_chapter(external_id, idx).await? else {
            return Ok(None);
        };
        sqlx::query(
            "INSERT INTO chapters (book_id, idx, title, content) VALUES (?, ?, ?, ?) \
             ON CONFLICT (book_id, idx) DO UPDATE SET title = excluded.title, content = excluded.content",
        )
        .bind(book_id)
        .bind(idx as i64)
        .bind(&chapter.title)
        .bind(&chapter.content)
        .execute(&self.db)
        .await?;
        Ok(Some(Chapter {
            idx,
            title: chapter.title,
            content: chapter.content,
        }))
    }

    /// Ensure the chapter title rows of a plugin book exist (so that
    /// `chapter_titles` works for books never read yet).
    pub async fn ensure_titles(&self, book: &BookMeta) -> Result<(), ApiError> {
        if book.source == "local" || book.chapter_count == 0 {
            return Ok(());
        }
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chapters WHERE book_id = ?")
            .bind(&book.id)
            .fetch_one(&self.db)
            .await?;
        if stored > 0 {
            return Ok(());
        }
        let source = self.source(&book.source);
        if let Some(source) = source {
            let titles: CoreResult<Vec<String>> = source.chapter_titles(&book.external_id).await;
            if let Ok(titles) = titles {
                let mut tx = self.db.begin().await?;
                for (idx, title) in titles.into_iter().enumerate() {
                    sqlx::query(
                        "INSERT OR IGNORE INTO chapters (book_id, idx, title, content) VALUES (?, ?, ?, '')",
                    )
                    .bind(&book.id)
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