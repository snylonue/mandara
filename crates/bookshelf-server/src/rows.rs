//! Database row structs mapped into core models.

use anyhow::anyhow;
use sqlx::FromRow;

use bookshelf_core::model::{
    BookMeta, Chapter, ChapterFormat, ChapterMeta, FileMeta, Position, ReadingSession, Role,
    Share, ShareKind, User, Visibility,
};

use crate::error::ApiError;

#[derive(Debug, FromRow)]
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
            role: Role::parse(&self.role)
                .ok_or_else(|| ApiError::Internal(anyhow!("bad role in db: {}", self.role)))?,
            created_at: self.created_at,
        })
    }
}

#[derive(Debug, FromRow)]
pub struct BookRow {
    pub id: String,
    pub title: String,
    pub authors: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
}

impl BookRow {
    pub fn into_model(self) -> Result<BookMeta, ApiError> {
        Ok(BookMeta {
            id: self.id,
            title: self.title,
            authors: serde_json::from_str(&self.authors).unwrap_or_default(),
            description: self.description,
            cover_url: self.cover_url,
            created_by: self.created_by,
            created_at: self.created_at,
        })
    }
}

#[derive(Debug, FromRow)]
pub struct FileRow {
    pub id: String,
    pub book_id: String,
    pub source: String,
    pub external_id: String,
    pub format: String,
    pub label: String,
    pub visibility: String,
    pub owner_id: Option<String>,
    pub chapter_count: i64,
    pub created_at: String,
}

impl FileRow {
    pub fn into_model(self) -> Result<FileMeta, ApiError> {
        Ok(FileMeta {
            id: self.id,
            book_id: self.book_id,
            source: self.source,
            external_id: self.external_id,
            format: self.format,
            label: self.label,
            visibility: Visibility::parse(&self.visibility).unwrap_or(Visibility::Private),
            owner_id: self.owner_id,
            chapter_count: self.chapter_count.max(0) as u32,
            created_at: self.created_at,
        })
    }
}

#[derive(Debug, FromRow)]
pub struct ChapterRow {
    pub idx: i64,
    pub title: String,
    pub format: String,
    pub content: String,
}

impl ChapterRow {
    pub fn into_model(self) -> Chapter {
        Chapter {
            idx: self.idx.max(0) as u32,
            title: self.title,
            format: ChapterFormat::parse(&self.format),
            content: self.content,
        }
    }
}

#[derive(Debug, FromRow)]
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

#[derive(Debug, FromRow)]
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
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug, FromRow)]
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
            kind: match self.kind.as_str() {
                "book" => ShareKind::Book,
                "session" => ShareKind::Session,
                other => {
                    return Err(ApiError::Internal(anyhow!("bad share kind in db: {other}")));
                }
            },
            mode: self.mode,
            file_id: self.file_id,
            session_id: self.session_id,
            created_by: self.created_by,
            expires_at: self.expires_at,
            created_at: self.created_at,
        })
    }
}