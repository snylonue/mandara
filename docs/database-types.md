# Database value mappings

Domain values implement their SQLite mappings in mandara-core. Queries bind
and load the same values used by the application, rather than parsing strings
in route handlers or row-to-model conversions.

- Role, Visibility, FileFormat, ContentMode, ShareKind, and PositionKind derive
  DbEnum from diesel-derive-enum. Their generated SQL mapping types keep enum columns
  distinct even though SQLite stores their values as text.
- Authors is a serde-transparent newtype around Vec<String>. Authors, BookExt,
  and SeriesExt derive Diesel's AsExpression and FromSqlRow and implement
  FromSql/ToSql for Text using shared JSON codecs. Their HTTP representation
  remains an array or object. Malformed stored JSON is a deserialization error,
  not a silently empty value.
- DateTime<Utc> uses Diesel's chrono feature and TimestamptzSqlite mapping.
  Application writes bind UTC timestamps directly, using Diesel's native
  format, such as `2026-08-26 12:34:56.123+00:00`. Fractional seconds retain
  their precision. The initial reading context copies its account's timestamp.
  API timestamps are still serialized by chrono/serde as RFC 3339. There is
  no application-level legacy timestamp parser or fallback to the Unix epoch.
- Metadata JSON columns and model fields are both named ext. Chapter linear
  flags use Diesel's native Bool mapping.
- User, SeriesMeta, ReadingSession, and Share derive Queryable and are loaded
  directly through server-side column projections; there are no duplicate rows.
- Rows remain only for signed-to-unsigned integer conversions and shapes that
  differ from the domain model. Standard TryFrom implementations reject invalid
  stored numbers, missing reading contexts, and incomplete original-file data
  instead of clamping, truncating, or silently substituting another value.
  OriginalInfo and Position are assembled from multiple columns. NULL chapter
  content means uncached, while an empty string is valid cached content.

## Schema generation

Run `just schema` after every migration. The recipe applies all SQL migrations
on a scratch database and runs diesel print-schema. The server's diesel.toml
uses the official print_schema.patch_file setting to apply schema.patch. The
result includes enum and timestamp mappings, widened numeric types, the
escaped offset column, and the generated-file header; do not reapply these by
hand. A generation failure leaves the committed schema untouched.

When a migration changes the raw schema enough that the patch no longer
applies, regenerate it:

1. Apply all migrations to a scratch SQLite database.
2. From crates/mandara-server, generate the unpatched schema using
   `DATABASE_URL=<scratch-db> diesel --config-file /dev/null print-schema`.
3. Update the desired schema with the new columns and their domain mappings.
4. Use a unified diff from the raw schema to the desired schema as schema.patch.
5. Run `just schema` again and verify that its output is byte-identical to the
   desired schema. Run formatting, checks, and tests before committing.
