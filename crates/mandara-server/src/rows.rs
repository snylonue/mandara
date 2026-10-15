//! Query projections and conversions for rows whose shape differs from domain models.

use crate::schema::{reading_sessions, series, shares, users};
use anyhow::{Context as _, ensure};
use chrono::{DateTime, Utc};
use mandara_core::BookExt;
use mandara_core::model::{
    Authors, BookMeta, Chapter, ChapterMeta, ContentMode, FileFormat, FileMeta, InChapter,
    OriginalInfo, Position, PositionKind, ReadingProgress, Visibility,
};

#[cfg(test)]
mod tests;

// These projections load core Queryable models directly, without duplicate row structs.
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
#[diesel(table_name = crate::schema::books, check_for_backend(diesel::sqlite::Sqlite))]
pub struct BookRow {
    pub id: String,
    pub title: String,
    pub authors: Authors,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub series_id: Option<String>,
    pub volume_no: i64,
    pub ext: BookExt,
}

impl TryFrom<BookRow> for BookMeta {
    type Error = anyhow::Error;

    fn try_from(row: BookRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            title: row.title,
            authors: row.authors,
            description: row.description,
            cover_url: row.cover_url,
            ext: row.ext,
            created_by: row.created_by,
            created_at: row.created_at,
            series_id: row.series_id,
            volume_no: u32::try_from(row.volume_no).context("invalid stored volume number")?,
        })
    }
}

#[derive(Debug, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::schema::book_files, check_for_backend(diesel::sqlite::Sqlite))]
pub struct FileRow {
    pub id: String,
    pub book_id: String,
    pub source: String,
    pub external_id: String,
    pub format: FileFormat,
    pub label: String,
    pub visibility: Visibility,
    pub owner_id: Option<String>,
    pub chapter_count: i64,
    pub created_at: DateTime<Utc>,
    pub content_mode: ContentMode,
    pub orig_ext: Option<String>,
    pub orig_sha256: Option<String>,
    pub orig_size: Option<i64>,
}

impl TryFrom<FileRow> for FileMeta {
    type Error = anyhow::Error;

    fn try_from(row: FileRow) -> Result<Self, Self::Error> {
        let original = match (row.orig_ext, row.orig_sha256, row.orig_size) {
            (None, None, None) => None,
            (Some(ext), Some(sha256), Some(size)) => Some(OriginalInfo {
                ext,
                sha256,
                size: u64::try_from(size).context("invalid stored file size")?,
            }),
            _ => anyhow::bail!("incomplete stored original file metadata"),
        };
        Ok(Self {
            id: row.id,
            book_id: row.book_id,
            source: row.source,
            external_id: row.external_id,
            format: row.format,
            label: row.label,
            visibility: row.visibility,
            owner_id: row.owner_id,
            chapter_count: u32::try_from(row.chapter_count)
                .context("invalid stored chapter count")?,
            created_at: row.created_at,
            content_mode: row.content_mode,
            original,
        })
    }
}

#[derive(Debug, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::schema::chapters, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ChapterRow {
    pub idx: i64,
    pub title: String,
    pub content: Option<String>,
}

impl TryFrom<ChapterRow> for Option<Chapter> {
    type Error = anyhow::Error;

    fn try_from(row: ChapterRow) -> Result<Self, Self::Error> {
        // NULL is an uncached placeholder; an empty string is cached content.
        row.content
            .map(|content| {
                Ok(Chapter {
                    idx: u32::try_from(row.idx).context("invalid stored chapter index")?,
                    title: row.title,
                    content,
                })
            })
            .transpose()
    }
}

#[derive(Debug, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::schema::chapters, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ChapterTitleRow {
    pub idx: i64,
    pub title: String,
    pub linear: bool,
}

impl TryFrom<ChapterTitleRow> for ChapterMeta {
    type Error = anyhow::Error;

    fn try_from(row: ChapterTitleRow) -> Result<Self, Self::Error> {
        Ok(Self {
            idx: u32::try_from(row.idx).context("invalid stored chapter index")?,
            title: row.title,
            linear: row.linear,
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
    pub chapter_idx: i64,
    pub offset: i64,
    pub fraction: f64,
    pub updated_at: DateTime<Utc>,
    pub reading_session_id: Option<String>,
    pub position_kind: PositionKind,
}

impl TryFrom<SessionRow> for ReadingProgress {
    type Error = anyhow::Error;

    fn try_from(row: SessionRow) -> Result<Self, Self::Error> {
        let in_chapter = match row.position_kind {
            PositionKind::Offset => InChapter::CharOffset {
                offset: u32::try_from(row.offset).context("invalid stored character offset")?,
            },
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
                chapter_idx: u32::try_from(row.chapter_idx)
                    .context("invalid stored chapter index")?,
                in_chapter,
            },
            updated_at: row.updated_at,
        })
    }
}
