//! Database row structs mapped into core models.

use mandara_core::model::{
    BookMeta, Chapter, ChapterMeta, ContentMode, FileFormat, FileMeta, InChapter, OriginalInfo,
    Position, ReadingProgress, ReadingSession, Role, SeriesMeta, Share, ShareKind, User,
    Visibility,
};
use mandara_core::{BookExt, SeriesExt};

#[cfg(test)]
mod tests;
mod value;

use chrono::{DateTime, Utc};
use value::*;

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::users, check_for_backend(diesel::sqlite::Sqlite))]
pub struct UserRow {
    pub id: String,
    pub username: String,
    #[diesel(deserialize_as = RoleValue)]
    pub role: Role,
    #[diesel(deserialize_as = Timestamp)]
    pub created_at: DateTime<Utc>,
}

impl UserRow {
    pub fn into_model(self) -> User {
        User {
            id: self.id,
            username: self.username,
            role: self.role,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::series, check_for_backend(diesel::sqlite::Sqlite))]
pub struct SeriesRow {
    pub id: String,
    pub title: String,
    #[diesel(deserialize_as = Authors)]
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub created_by: Option<String>,
    #[diesel(deserialize_as = Timestamp)]
    pub created_at: DateTime<Utc>,
    #[diesel(deserialize_as = SeriesExtValue)]
    pub ext_meta: SeriesExt,
}

impl SeriesRow {
    pub fn into_model(self) -> SeriesMeta {
        SeriesMeta {
            id: self.id,
            title: self.title,
            authors: self.authors,
            description: self.description,
            cover_url: self.cover_url,
            ext: self.ext_meta,
            created_by: self.created_by,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::books, check_for_backend(diesel::sqlite::Sqlite))]
pub struct BookRow {
    pub id: String,
    pub title: String,
    #[diesel(deserialize_as = Authors)]
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub created_by: Option<String>,
    #[diesel(deserialize_as = Timestamp)]
    pub created_at: DateTime<Utc>,
    pub series_id: Option<String>,
    #[diesel(deserialize_as = Unsigned)]
    pub volume_no: u32,
    #[diesel(deserialize_as = BookExtValue)]
    pub ext_meta: BookExt,
    /// Plugin source that provided this entry's metadata (migration
    /// 0013); independent of the files' content sources. `None` = no
    /// plugin metadata source.
    pub meta_source: Option<String>,
    pub meta_external_id: Option<String>,
}

impl BookRow {
    pub fn into_model(self) -> BookMeta {
        BookMeta {
            id: self.id,
            title: self.title,
            authors: self.authors,
            description: self.description,
            cover_url: self.cover_url,
            ext: self.ext_meta,
            created_by: self.created_by,
            created_at: self.created_at,
            series_id: self.series_id,
            volume_no: self.volume_no,
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::book_files, check_for_backend(diesel::sqlite::Sqlite))]
pub struct FileRow {
    pub id: String,
    pub book_id: String,
    pub source: String,
    pub external_id: String,
    #[diesel(deserialize_as = FormatValue)]
    pub format: FileFormat,
    pub label: String,
    #[diesel(deserialize_as = VisibilityValue)]
    pub visibility: Visibility,
    pub owner_id: Option<String>,
    #[diesel(deserialize_as = Unsigned)]
    pub chapter_count: u32,
    #[diesel(deserialize_as = Timestamp)]
    pub created_at: DateTime<Utc>,
    #[diesel(deserialize_as = ContentModeValue)]
    pub content_mode: ContentMode,
    pub orig_ext: Option<String>,
    pub orig_sha256: Option<String>,
    pub orig_size: Option<i64>,
}

impl FileRow {
    pub fn into_model(self) -> FileMeta {
        FileMeta {
            id: self.id,
            book_id: self.book_id,
            source: self.source,
            external_id: self.external_id,
            format: self.format,
            label: self.label,
            visibility: self.visibility,
            owner_id: self.owner_id,
            chapter_count: self.chapter_count,
            created_at: self.created_at,
            content_mode: self.content_mode,
            original: match (self.orig_ext, self.orig_sha256, self.orig_size) {
                (Some(ext), Some(sha), Some(size)) => Some(OriginalInfo {
                    ext,
                    size: size.max(0) as u64,
                    sha256: sha,
                }),
                _ => None,
            },
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::chapters, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ChapterRow {
    #[diesel(deserialize_as = Unsigned)]
    pub idx: u32,
    pub title: String,
    pub content: Option<String>,
}

impl ChapterRow {
    pub fn into_model(self) -> Option<Chapter> {
        Some(Chapter {
            idx: self.idx,
            title: self.title,
            content: self.content?,
        })
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::chapters, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ChapterTitleRow {
    #[diesel(deserialize_as = Unsigned)]
    pub idx: u32,
    pub title: String,
    #[diesel(deserialize_as = Boolean)]
    pub linear: bool,
}

impl ChapterTitleRow {
    pub fn into_model(self) -> ChapterMeta {
        ChapterMeta {
            idx: self.idx,
            title: self.title,
            linear: self.linear,
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::reading_sessions, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ReadingSessionRow {
    pub id: String,
    pub user_id: String,
    pub label: String,
    #[diesel(deserialize_as = Timestamp)]
    pub created_at: DateTime<Utc>,
}

impl ReadingSessionRow {
    pub fn into_model(self) -> ReadingSession {
        ReadingSession {
            id: self.id,
            user_id: self.user_id,
            label: self.label,
            created_at: self.created_at,
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::sessions, check_for_backend(diesel::sqlite::Sqlite))]
pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub label: String,
    #[diesel(deserialize_as = Unsigned)]
    pub chapter_idx: u32,
    pub offset: i64,
    pub fraction: f64,
    #[diesel(deserialize_as = Timestamp)]
    pub updated_at: DateTime<Utc>,
    pub reading_session_id: Option<String>,
    pub position_kind: String,
}

impl SessionRow {
    pub fn into_model(self) -> ReadingProgress {
        ReadingProgress {
            session_id: self.reading_session_id.expect("progress has a context"),
            id: self.id,
            user_id: self.user_id,
            file_id: self.file_id,
            label: self.label,
            position: Position {
                chapter_idx: self.chapter_idx,
                in_chapter: if self.position_kind == "offset" {
                    InChapter::CharOffset {
                        offset: self.offset as u32,
                    }
                } else {
                    InChapter::Fraction {
                        fraction: self.fraction.clamp(0.0, 1.0),
                    }
                },
            },
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::shares, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ShareRow {
    pub token: String,
    #[diesel(deserialize_as = ShareKindValue)]
    pub kind: ShareKind,
    pub file_id: String,
    pub session_id: Option<String>,
    pub created_by: Option<String>,
    #[diesel(deserialize_as = OptionalTimestamp)]
    pub expires_at: Option<DateTime<Utc>>,
    #[diesel(deserialize_as = Timestamp)]
    pub created_at: DateTime<Utc>,
}

impl ShareRow {
    pub fn into_model(self) -> Share {
        Share {
            token: self.token,
            kind: self.kind,
            file_id: self.file_id,
            session_id: self.session_id,
            created_by: self.created_by,
            expires_at: self.expires_at,
            created_at: self.created_at,
        }
    }
}
