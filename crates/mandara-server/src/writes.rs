//! Typed write projections; Diesel widens unsigned values at the ORM boundary.

use diesel::ExpressionMethods as _;

#[derive(Clone, Copy, diesel::Insertable, diesel::AsChangeset)]
#[diesel(table_name = crate::schema::books)]
pub struct BookVolume {
    #[diesel(serialize_as = i64)]
    pub volume_no: u32,
}

#[derive(Clone, Copy, diesel::Insertable, diesel::AsChangeset)]
#[diesel(table_name = crate::schema::book_files)]
pub struct FileChapterCount {
    #[diesel(serialize_as = i64)]
    pub chapter_count: u32,
}

#[derive(Clone, Copy, diesel::Insertable, diesel::AsChangeset)]
#[diesel(table_name = crate::schema::chapters)]
pub struct ChapterIndex {
    #[diesel(serialize_as = i64)]
    pub idx: u32,
}

#[derive(Clone, Copy, diesel::Insertable, diesel::AsChangeset)]
#[diesel(table_name = crate::schema::sessions)]
pub struct SessionCoordinates {
    #[diesel(serialize_as = i64)]
    pub chapter_idx: u32,
    #[diesel(serialize_as = i64)]
    pub offset: u32,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = crate::schema::images)]
pub struct ImageDimensions {
    #[diesel(serialize_as = i64)]
    pub width: u32,
    #[diesel(serialize_as = i64)]
    pub height: u32,
}
