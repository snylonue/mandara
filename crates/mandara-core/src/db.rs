//! SQLite codecs and composite value mappings shared by domain models.

use diesel::deserialize::{self, FromSql};
use diesel::serialize::{self, IsNull, Output, ToSql};
use diesel::sql_types::Text;
use diesel::sqlite::{Sqlite, SqliteValue};
use serde::{Serialize, de::DeserializeOwned};

use crate::{Authors, BookExt, SeriesExt};

fn read_json<T: DeserializeOwned>(value: SqliteValue<'_, '_, '_>) -> deserialize::Result<T> {
    let text = <String as FromSql<Text, Sqlite>>::from_sql(value)?;
    Ok(serde_json::from_str(&text)?)
}

fn write_json<T: Serialize>(value: &T, out: &mut Output<'_, '_, Sqlite>) -> serialize::Result {
    out.set_value(serde_json::to_string(value)?);
    Ok(IsNull::No)
}

impl FromSql<Text, Sqlite> for Authors {
    fn from_sql(value: SqliteValue<'_, '_, '_>) -> deserialize::Result<Self> {
        read_json(value)
    }
}

impl ToSql<Text, Sqlite> for Authors {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Sqlite>) -> serialize::Result {
        write_json(self, out)
    }
}

impl FromSql<Text, Sqlite> for BookExt {
    fn from_sql(value: SqliteValue<'_, '_, '_>) -> deserialize::Result<Self> {
        read_json(value)
    }
}

impl ToSql<Text, Sqlite> for BookExt {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Sqlite>) -> serialize::Result {
        write_json(self, out)
    }
}

impl FromSql<Text, Sqlite> for SeriesExt {
    fn from_sql(value: SqliteValue<'_, '_, '_>) -> deserialize::Result<Self> {
        read_json(value)
    }
}

impl ToSql<Text, Sqlite> for SeriesExt {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Sqlite>) -> serialize::Result {
        write_json(self, out)
    }
}

// A nullable composite is absent only when all three columns are NULL.
#[derive(diesel::Queryable)]
pub struct OriginalColumns {
    ext: Option<String>,
    size: Option<i64>,
    sha256: Option<String>,
}

impl TryFrom<OriginalColumns> for Option<crate::OriginalInfo> {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(columns: OriginalColumns) -> Result<Self, Self::Error> {
        match (columns.ext, columns.size, columns.sha256) {
            (None, None, None) => Ok(None),
            (Some(ext), Some(size), Some(sha256)) => {
                use diesel::{Queryable, sql_types::BigInt};
                let original =
                    <crate::OriginalInfo as Queryable<(Text, BigInt, Text), Sqlite>>::build((
                        ext, size, sha256,
                    ))?;
                Ok(Some(original))
            }
            _ => Err("incomplete stored original file metadata".into()),
        }
    }
}
