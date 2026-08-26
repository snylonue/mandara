//! Database row structs mapped into core models.

use anyhow::anyhow;

use bookshelf_core::model::{
    BookMeta, Chapter, ChapterMeta, FileFormat, FileMeta, OriginalInfo, Position, ReadingSession,
    Role, SeriesMeta, Share, ShareKind, User, Visibility,
};
use bookshelf_core::{BookExt, SeriesExt};

use crate::error::ApiError;
use crate::time::parse_ts;

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::users, check_for_backend(diesel::sqlite::Sqlite))]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub role: String,
    pub created_at: String,
}

impl UserRow {
    pub fn into_model(self) -> Result<User, ApiError> {
        Ok(User {
            id: self.id,
            username: self.username,
            role: self
                .role
                .parse::<Role>()
                .map_err(|_| anyhow!("bad role in db: {}", self.role))?,
            created_at: parse_ts(&self.created_at),
        })
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::series, check_for_backend(diesel::sqlite::Sqlite))]
pub struct SeriesRow {
    pub id: String,
    pub title: String,
    pub authors: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub ext_meta: String,
}

impl SeriesRow {
    pub fn into_model(self) -> SeriesMeta {
        SeriesMeta {
            id: self.id,
            title: self.title,
            authors: serde_json::from_str(&self.authors).unwrap_or_default(),
            description: self.description,
            cover_url: self.cover_url,
            ext: SeriesExt::from_column(&self.ext_meta),
            created_by: self.created_by,
            created_at: parse_ts(&self.created_at),
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::books, check_for_backend(diesel::sqlite::Sqlite))]
pub struct BookRow {
    pub id: String,
    pub title: String,
    pub authors: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub series_id: Option<String>,
    pub volume_no: i64,
    pub ext_meta: String,
}

impl BookRow {
    pub fn into_model(self) -> Result<BookMeta, ApiError> {
        Ok(BookMeta {
            id: self.id,
            title: self.title,
            authors: serde_json::from_str(&self.authors).unwrap_or_default(),
            description: self.description,
            cover_url: self.cover_url,
            ext: BookExt::from_column(&self.ext_meta),
            created_by: self.created_by,
            created_at: parse_ts(&self.created_at),
            series_id: self.series_id,
            volume_no: self.volume_no.max(0) as u32,
        })
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::book_files, check_for_backend(diesel::sqlite::Sqlite))]
pub struct FileRow {
    pub id: String,
    pub book_id: String,
    pub source: String,
    pub external_id: String,
    pub content_source: Option<String>,
    pub content_external_id: Option<String>,
    pub format: String,
    pub label: String,
    pub visibility: String,
    pub owner_id: Option<String>,
    pub chapter_count: i64,
    pub created_at: String,
    pub volume_no: i64,
    pub volume_offset: i64,
    pub orig_ext: Option<String>,
    pub orig_sha256: Option<String>,
    pub orig_size: Option<i64>,
}

impl FileRow {
    pub fn into_model(self) -> Result<FileMeta, ApiError> {
        Ok(FileMeta {
            id: self.id,
            book_id: self.book_id,
            source: self.source,
            external_id: self.external_id,
            content_source: self.content_source,
            content_external_id: self.content_external_id,
            format: self
                .format
                .parse::<FileFormat>()
                .map_err(|_| anyhow!("bad file format in db: {}", self.format))?,
            label: self.label,
            visibility: self
                .visibility
                .parse::<Visibility>()
                .unwrap_or(Visibility::Private),
            owner_id: self.owner_id,
            chapter_count: self.chapter_count.max(0) as u32,
            created_at: parse_ts(&self.created_at),
            volume_no: self.volume_no.max(0) as u32,
            volume_offset: self.volume_offset.max(0) as u32,
            original: match (self.orig_ext, self.orig_sha256, self.orig_size) {
                (Some(ext), Some(sha), Some(size)) => Some(OriginalInfo {
                    ext,
                    size: size.max(0) as u64,
                    sha256: sha,
                }),
                _ => None,
            },
        })
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::chapters, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ChapterRow {
    pub idx: i64,
    pub title: String,
    pub content: String,
}

impl ChapterRow {
    pub fn into_model(self) -> Chapter {
        Chapter {
            idx: self.idx.max(0) as u32,
            title: self.title,
            content: self.content,
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::chapters, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ChapterTitleRow {
    pub idx: i64,
    pub title: String,
}

impl ChapterTitleRow {
    pub fn into_model(self) -> ChapterMeta {
        ChapterMeta {
            idx: self.idx.max(0) as u32,
            title: self.title,
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
    pub chapter_idx: i64,
    pub offset: i64,
    pub fraction: f64,
    pub updated_at: String,
}

impl SessionRow {
    pub fn into_model(self) -> ReadingSession {
        ReadingSession {
            id: self.id,
            user_id: self.user_id,
            file_id: self.file_id,
            label: self.label,
            position: Position {
                chapter_idx: self.chapter_idx.max(0) as u32,
                offset: self.offset.max(0) as u32,
                fraction: self.fraction.clamp(0.0, 1.0),
            },
            updated_at: parse_ts(&self.updated_at),
        }
    }
}

#[derive(Debug, diesel::prelude::Queryable, diesel::prelude::Selectable)]
#[diesel(table_name = crate::schema::shares, check_for_backend(diesel::sqlite::Sqlite))]
pub struct ShareRow {
    pub token: String,
    pub kind: String,
    pub mode: String,
    pub file_id: String,
    pub session_id: Option<String>,
    pub created_by: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

impl ShareRow {
    pub fn into_model(self) -> Result<Share, ApiError> {
        Ok(Share {
            token: self.token,
            kind: self
                .kind
                .parse::<ShareKind>()
                .map_err(|_| anyhow!("bad share kind in db: {}", self.kind))?,
            mode: self.mode,
            file_id: self.file_id,
            session_id: self.session_id,
            created_by: self.created_by,
            expires_at: self.expires_at.map(|e| parse_ts(&e)),
            created_at: parse_ts(&self.created_at),
        })
    }
}
