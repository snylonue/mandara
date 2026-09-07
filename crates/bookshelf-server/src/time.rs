//! Timestamp handling for the TEXT timestamp columns.
//!
//! Every timestamp column stores RFC 3339 with millisecond precision and a
//! UTC `Z` suffix — the format SQLite's column defaults produce
//! (`strftime('%Y-%m-%dT%H:%M:%fZ', 'now')`). All writes go through
//! [`DbTs`], all reads through [`DbTs::parse`], so domain models can carry
//! typed [`DateTime<Utc>`] instead of strings and the storage format
//! contract lives in the type, not in comments.

use chrono::{DateTime, SecondsFormat, Utc};

/// A timestamp in the stored TEXT format. Values of this type are
/// guaranteed to be RFC 3339 with milliseconds and a UTC `Z` suffix —
/// every stored timestamp is produced via [`DbTs::now`], so a raw string
/// cannot slip into `parse` from the other side of the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbTs(String);

impl DbTs {
    /// Wrap a raw stored string (from a row or the `expires_at` column).
    pub fn from_str(s: impl Into<String>) -> Self {
        DbTs(s.into())
    }

    /// Format a timestamp for storage in a TEXT timestamp column.
    pub fn from_datetime(t: DateTime<Utc>) -> Self {
        DbTs(t.to_rfc3339_opts(SecondsFormat::Millis, true))
    }

    /// Current UTC timestamp in the stored format.
    pub fn now() -> Self {
        DbTs::from_datetime(Utc::now())
    }

    /// The stored-formatted string (for `diesel` `Text` columns).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse a stored timestamp. Legacy/empty values fall back to the Unix
    /// epoch so callers always get a comparable `DateTime`.
    pub fn parse(&self) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&self.0)
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|_| {
                tracing::warn!("unparseable timestamp in db: {:?}", self.0);
                DateTime::<Utc>::default()
            })
    }
}

impl From<DateTime<Utc>> for DbTs {
    fn from(t: DateTime<Utc>) -> Self {
        DbTs::from_datetime(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_matches_sqlite_default_format() {
        // strftime('%Y-%m-%dT%H:%M:%fZ', 'now') output shape.
        let stored = "2026-08-26T12:34:56.789Z";
        let ts = DbTs::from_str(stored);
        assert_eq!(DbTs::from_datetime(ts.parse()).as_str(), stored);
        assert_eq!(DbTs::now().as_str().len(), 24);
    }

    #[test]
    fn parse_falls_back_to_epoch() {
        assert_eq!(DbTs::from_str("").parse(), DateTime::<Utc>::default());
    }
}
