//! Field adapters for Diesel's `deserialize_as` attribute.
//!
//! Keep SQL representations here and domain types in row declarations, without
//! introducing a Diesel dependency into mandara-core. Legacy JSON, timestamp,
//! and integer fallbacks intentionally match the previous row conversions.

use chrono::{DateTime, Utc};
use diesel::deserialize::{self, Queryable};
use diesel::sql_types::{BigInt, Nullable, Text};
use diesel::sqlite::Sqlite;
use mandara_core::model::{ContentMode, FileFormat, Role, ShareKind, Visibility};
use mandara_core::{BookExt, SeriesExt};

use crate::time::DbTs;

macro_rules! field_adapter {
    ($name:ident, $target:ty, $sql:ty, $stored:ty, $decode:expr) => {
        pub struct $name($target);

        impl Queryable<$sql, Sqlite> for $name {
            type Row = $stored;

            fn build(value: Self::Row) -> deserialize::Result<Self> {
                let decode: fn($stored) -> deserialize::Result<$target> = $decode;
                decode(value).map(Self)
            }
        }

        impl From<$name> for $target {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

field_adapter!(RoleValue, Role, Text, String, |v| Ok(v.parse()?));
field_adapter!(FormatValue, FileFormat, Text, String, |v| Ok(v.parse()?));
field_adapter!(
    VisibilityValue,
    Visibility,
    Text,
    String,
    |v| Ok(v.parse()?)
);
field_adapter!(ContentModeValue, ContentMode, Text, String, |v| Ok(
    v.parse()?
));
field_adapter!(ShareKindValue, ShareKind, Text, String, |v| Ok(v.parse()?));
field_adapter!(Timestamp, DateTime<Utc>, Text, String, |v| Ok(
    DbTs::from_str(v).parse()
));
field_adapter!(
    OptionalTimestamp,
    Option<DateTime<Utc>>,
    Nullable<Text>,
    Option<String>,
    |v| { Ok(v.map(|v| DbTs::from_str(v).parse())) }
);
field_adapter!(Authors, Vec<String>, Text, String, |v| Ok(
    serde_json::from_str(&v).unwrap_or_default()
));
field_adapter!(BookExtValue, BookExt, Text, String, |v| Ok(
    BookExt::from_column(&v)
));
field_adapter!(SeriesExtValue, SeriesExt, Text, String, |v| Ok(
    SeriesExt::from_column(&v)
));
field_adapter!(Unsigned, u32, BigInt, i64, |v| Ok(v.max(0) as u32));
field_adapter!(Boolean, bool, BigInt, i64, |v| Ok(v != 0));
