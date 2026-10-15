//! Query projections and conversions for rows whose shape differs from domain models.

use crate::schema::{book_files, books, chapters, reading_sessions, series, shares, users};
use anyhow::{Context as _, ensure};
use chrono::{DateTime, Utc};
use mandara_core::model::{Chapter, InChapter, Position, PositionKind, ReadingProgress};

#[cfg(test)]
mod tests;

// Tuple order matches each core Queryable model.
pub const BOOK_COLUMNS: (
    books::id,
    books::title,
    books::authors,
    books::description,
    books::cover_url,
    books::ext,
    books::created_by,
    books::created_at,
    books::series_id,
    books::volume_no,
) = (
    books::id,
    books::title,
    books::authors,
    books::description,
    books::cover_url,
    books::ext,
    books::created_by,
    books::created_at,
    books::series_id,
    books::volume_no,
);
type OriginalFileColumns = (
    book_files::orig_ext,
    book_files::orig_size,
    book_files::orig_sha256,
);
pub const FILE_COLUMNS: (
    book_files::id,
    book_files::book_id,
    book_files::source,
    book_files::external_id,
    book_files::format,
    book_files::label,
    book_files::visibility,
    book_files::owner_id,
    book_files::chapter_count,
    book_files::created_at,
    book_files::content_mode,
    OriginalFileColumns,
) = (
    book_files::id,
    book_files::book_id,
    book_files::source,
    book_files::external_id,
    book_files::format,
    book_files::label,
    book_files::visibility,
    book_files::owner_id,
    book_files::chapter_count,
    book_files::created_at,
    book_files::content_mode,
    (
        book_files::orig_ext,
        book_files::orig_size,
        book_files::orig_sha256,
    ),
);
pub const CHAPTER_META_COLUMNS: (chapters::idx, chapters::title, chapters::linear) =
    (chapters::idx, chapters::title, chapters::linear);
pub const USER_COLUMNS: (users::id, users::username, users::role, users::created_at) =
    (users::id, users::username, users::role, users::created_at);
pub const SERIES_COLUMNS: (
    series::id,
    series::title,
    series::authors,
    series::description,
    series::cover_url,
    series::ext,
    series::created_by,
    series::created_at,
) = (
    series::id,
    series::title,
    series::authors,
    series::description,
    series::cover_url,
    series::ext,
    series::created_by,
    series::created_at,
);
pub const READING_SESSION_COLUMNS: (
    reading_sessions::id,
    reading_sessions::user_id,
    reading_sessions::label,
    reading_sessions::created_at,
) = (
    reading_sessions::id,
    reading_sessions::user_id,
    reading_sessions::label,
    reading_sessions::created_at,
);
pub const SHARE_COLUMNS: (
    shares::token,
    shares::kind,
    shares::file_id,
    shares::session_id,
    shares::created_by,
    shares::expires_at,
    shares::created_at,
) = (
    shares::token,
    shares::kind,
    shares::file_id,
    shares::session_id,
    shares::created_by,
    shares::expires_at,
    shares::created_at,
);

#[derive(Debug, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::schema::chapters, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ChapterRow {
    #[diesel(deserialize_as = i64)]
    pub idx: u32,
    pub title: String,
    pub content: Option<String>,
}

impl From<ChapterRow> for Option<Chapter> {
    fn from(row: ChapterRow) -> Self {
        row.content.map(|content| Chapter {
            idx: row.idx,
            title: row.title,
            content,
        })
    }
}

#[derive(Debug, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::schema::sessions, check_for_backend(diesel::sqlite::Sqlite))]
pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub label: String,
    #[diesel(deserialize_as = i64)]
    pub chapter_idx: u32,
    #[diesel(deserialize_as = i64)]
    pub offset: u32,
    pub fraction: f64,
    pub updated_at: DateTime<Utc>,
    pub reading_session_id: Option<String>,
    pub position_kind: PositionKind,
}

impl TryFrom<SessionRow> for ReadingProgress {
    type Error = anyhow::Error;

    fn try_from(row: SessionRow) -> Result<Self, Self::Error> {
        let in_chapter = match row.position_kind {
            PositionKind::Offset => InChapter::CharOffset { offset: row.offset },
            PositionKind::Fraction => {
                ensure!(
                    row.fraction.is_finite() && (0.0..=1.0).contains(&row.fraction),
                    "invalid stored reading fraction: {}",
                    row.fraction
                );
                InChapter::Fraction {
                    fraction: row.fraction,
                }
            }
        };
        Ok(Self {
            session_id: row
                .reading_session_id
                .context("stored progress has no reading context")?,
            id: row.id,
            user_id: row.user_id,
            file_id: row.file_id,
            label: row.label,
            position: Position {
                chapter_idx: row.chapter_idx,
                in_chapter,
            },
            updated_at: row.updated_at,
        })
    }
}

/// A one-column unsigned aggregate or scalar, checked by Diesel on load.
#[derive(diesel::Queryable)]
pub struct U32Value(#[diesel(deserialize_as = i64)] pub u32);

#[derive(diesel::Queryable)]
pub struct SeriesCount {
    pub series_id: Option<String>,
    #[diesel(deserialize_as = i64)]
    pub count: u32,
}
