//! Timestamp handling for the TEXT timestamp columns.
//!
//! Every timestamp column stores RFC 3339 with millisecond precision and a
//! UTC `Z` suffix — the format SQLite's column defaults produce
//! (`strftime('%Y-%m-%dT%H:%M:%fZ', 'now')`). All writes go through
//! [`rfc3339_millis`], all reads through [`parse_ts`], so domain models can
//! carry typed [`DateTime<Utc>`] instead of strings.

use chrono::{DateTime, SecondsFormat, Utc};

/// Format a timestamp for storage in a TEXT timestamp column.
pub fn rfc3339_millis(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Current UTC timestamp in the stored format.
pub fn now() -> String {
    rfc3339_millis(Utc::now())
}

/// Parse a stored timestamp. Legacy/empty values fall back to the Unix
/// epoch so callers always get a comparable `DateTime`.
pub fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|_| {
            tracing::warn!("unparseable timestamp in db: {s:?}");
            DateTime::<Utc>::default()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_matches_sqlite_default_format() {
        // strftime('%Y-%m-%dT%H:%M:%fZ', 'now') output shape.
        let stored = "2026-08-26T12:34:56.789Z";
        let ts = parse_ts(stored);
        assert_eq!(rfc3339_millis(ts), stored);
        assert_eq!(now().len(), 24);
    }

    #[test]
    fn parse_ts_falls_back_to_epoch() {
        assert_eq!(parse_ts(""), DateTime::<Utc>::default());
    }
}
