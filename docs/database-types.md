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
- User, BookMeta, SeriesMeta, FileMeta, ChapterMeta, ReadingSession, and Share
  derive Queryable and load directly through server-side column projections.
  Integer fields use `#[diesel(deserialize_as = i64)]` to invoke the standard
  checked conversion into u32/u64 during ORM deserialization. Negative or
  out-of-range integers fail the query; they are not narrowed in service code.
- OriginalInfo uses Diesel's native nullable-composite mapping. The database
  guarantees its three columns are either all NULL or all non-NULL, so the ORM
  loads None or a complete OriginalInfo directly. Negative sizes remain ORM
  deserialization errors. Remaining rows represent genuinely different
  shapes: optional chapter content and the position discriminator. Standard
  From/TryFrom handle these structures, not integer range checks. NULL chapter
  content means uncached, while an empty string is valid cached content.

## Numeric write boundaries

Write interfaces use unsigned domain types for volumes, chapter counts and
indices, character offsets, file sizes, and image dimensions. Widen u32 with
`i64::from`; convert u64 sizes with `i64::try_from` because SQLite integers
are signed. Check collection lengths as u32 before writing any chapter rows,
then enumerate using the checked unsigned range. Computed volume ordinals use
checked addition, not saturation or truncating casts.

Numeric range constraints remain in the ORM and application. No STRICT-table
migration is introduced.

## Optional original metadata

Migration 0025 requires original extension, hash, and size to be present or
absent together. Insert and update triggers enforce this invariant without
rebuilding book_files, a parent table with foreign-key dependents. SQLite
cannot add a table-level CHECK directly to an existing table.

The migration checks existing records first and fails if any are partial; it
does not discard metadata, invent missing values, or alter valid records.
Empty strings and a zero size are values, not NULL. OriginalInfo no longer
needs a custom partial-state decoder.

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
