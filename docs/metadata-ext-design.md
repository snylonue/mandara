# Extensible Book Metadata (ISBN / Publisher / …)

Status: **approved** (owner decisions: JSON column approach; field set
confirmed; ISBN is parsed/normalized before storage; dates are stored
verbatim — formatting tolerance is the plugin's/user's job, not the
host's; epub extraction included; series get their own ext field set,
see below.

## Problem

`BookMeta` today carries only `title / authors / description / cover_url`
(+ series fields). Real-world catalog platforms track much more:

- **Douban** books: 副标题, 原作名, ISBN, 出版社, 出版年, 页数, 定价, 装帧,
  丛书, 译者.
- **Bangumi** book subjects: ISBN, 发售日, 价格, 页数, 作者, 插图 (illustrator),
  出版社.

We want an extended metadata field set where **every field is nullable**,
and — explicitly requested — adding *future* fields must be cheap.

## Design decision

### One JSON column + one typed Rust struct (JSON-first)

Add a single column instead of one column per field:

```sql
-- migration 0012_metadata_ext.sql
ALTER TABLE books ADD COLUMN ext_meta TEXT NOT NULL DEFAULT '{}';
```

with one typed struct in `mandara-core` that owns the well-known keys
and passes everything else through verbatim:

```rust
/// Extended book metadata (bangumi/douban-style). All fields optional;
/// unknown keys are preserved round-trip so older/newer servers and
/// plugin sources never lose data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BookExt {
    pub subtitle: Option<String>,        // 副标题
    pub original_title: Option<String>,  // 原作名
    pub isbn: Option<String>,            // raw string, hyphens preserved
    pub publisher: Option<String>,       // 出版社
    pub pub_date: Option<String>,        // partial dates ok: "2012-5"
    pub translators: Option<Vec<String>>,// 译者
    pub illustrators: Option<Vec<String>>,// 插图/插画师 (bangumi 插図)
    pub pages: Option<u32>,              // 页数
    pub price: Option<String>,           // 定价, currency as printed
    pub binding: Option<String>,         // 装帧 (文库/单行本/…)
    pub language: Option<String>,        // BCP-47 hint: "zh-CN", "ja"

    /// Anything we did not model yet. Survives every read/write cycle.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
```

`BookMeta` gains `pub ext: BookExt`, stored serialized in
`books.ext_meta` (`'{}'` when empty).

### ISBN normalization

`isbn` is **parsed before storage**, never kept raw:

1. strip spaces/hyphens, uppercase the check character;
2. accept valid ISBN-10 or ISBN-13 (checksum verified);
3. store canonically as **hyphenless ISBN-13** (valid ISBN-10 inputs are
   converted via the standard 978 prefix + recomputed check digit);
4. anything else is a hard error: API/manual writes fail with 400,
   plugin payloads fail the acquisition call — bad data never lands.

Other string fields are only trimmed (empty → `None`); there is no
host-side date parsing or fuzzy tolerance — `pub_date` is stored exactly
as supplied.

### Series extension fields

Series carry their own ext object (same mechanism: `series.ext_meta`
JSON column, `SeriesExt` struct with flatten passthrough) because a
series has edition-independent facts that neither belong on one volume
nor duplicate well onto every 卷 row:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeriesExt {
    pub original_title: Option<String>, // 原作名
    pub publisher: Option<String>,      // 出版社 (usually constant per series)
    pub pub_date: Option<String>,       // 首卷发售时间, verbatim
    pub language: Option<String>,       // BCP-47 hint
    /// ongoing | completed | hiatus
    pub status: Option<SeriesStatus>,
    pub total_volumes: Option<u32>,     // planned volume count
    pub tags: Option<Vec<String>>,      // 类型标签 (bangumi-style)
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
```

`SeriesStatus` is an enum (`ongoing`/`completed`/`hiatus`) following the
existing `FromStr`/`AsRef<str>`/`Display` pattern used by
`Role`/`Visibility`. Series PATCH merges keys exactly like book PATCH;
acquire-time series creation takes `ext` from the plugin entry.

### Why this over one-column-per-field

| | A: typed columns | B: JSON column + typed struct (**chosen**) |
|---|---|---|
| future field cost | migration + `just schema` + touch points everywhere | Rust struct field (+ optional openapi/doc note) — **zero migrations** |
| queryable/indexable (search by ISBN) | native | needs `json_extract()` (+ expression index) when first needed |
| DB-level type guarantees | yes | validated at the Rust boundary only |
| precedent | — | `authors` is already JSON-in-TEXT |

Metadata here is display-shaped, not query-shaped: no current endpoint
filters by publisher or ISBN. When that changes, a single field can be
*promoted* to a real column by one migration copying
`json_extract(ext_meta, '$.isbn')` — the upgrade path stays open, which
is exactly what "consider future additions" requires.

Normalization rules stay simple and live in one place (`BookExt::sanitize`,
applied on every write boundary): trim strings, empty → `None`, drop
`null` values from the passthrough map. No checksum validation for ISBN
in v1 (store what the source says).

## Where metadata flows — all six write paths

1. **Upload parse**: `ParsedBook` gains `ext: BookExt`
   (see *epub extraction* below; txt starts `{}`).
2. **Manual acquire overrides**: `POST /api/books` multipart gains an
   optional `ext` form field (JSON string). Manual values win over the
   parsed/plugin value per-key (shallow merge onto the source's ext).
3. **Attach**: existing metadata wins; ext untouched (same rule as other
   fields).
4. **Plugin entries**: see WIT below.
5. **Refresh** (`POST /api/books/{id}/refresh`): plugin ext replaces the
   stored ext wholesale, mirroring how refresh already replaces
   title/description/cover.
6. **PATCH `/api/books/{id}`**: `PatchBook.ext` is merged shallowly into
   the stored object — absent key = keep, `null` = clear, value = set.
   This reuses the three-state pattern already established by
   `series_id` but per-key inside one object, avoiding an explosion of
   `Option<Option<T>>` request fields.

`series` is intentionally left alone: ISBN/publisher are edition-level
facts and do not aggregate over a series.

## API contract (OpenAPI)

```yaml
BookExt:
  type: object
  description: |
    Extended metadata (bangumi/douban-style). Every known key is
    optional; `null` = unset. Unknown keys are preserved verbatim.
  properties:
    subtitle:        { type: [string, "null"] }
    original_title:  { type: [string, "null"] }
    isbn:            { type: [string, "null"] }
    publisher:       { type: [string, "null"] }
    pub_date:        { type: [string, "null"] }
    translators:     { type: [array, "null"], items: { type: string } }
    illustrators:    { type: [array, "null"], items: { type: string } }
    pages:           { type: [integer, "null"], minimum: 0 }
    price:           { type: [string, "null"] }
    binding:         { type: [string, "null"] }
    language:        { type: [string, "null"] }
  additionalProperties: true     # future keys flow through

BookMeta:
  # ... existing properties ...
  ext: { $ref: "#/components/schemas/BookExt" }   # required, may be {}
```

`additionalProperties: true` keeps generated TS types useful for the
known keys while remaining forward-compatible. The change is additive:
old clients ignore `BookMeta.ext`.

## Plugin surface (WIT 0.7.0)

One new field on `book-entry` — deliberately a JSON string, not ten
typed options:

```wit
record book-entry {
    // ...existing fields...
    /// Optional extended metadata: a JSON object (BookExt shape:
    /// subtitle/isbn/publisher/pub_date/translators/illustrators/
    /// pages/price/binding/language + free-form extras).
    extra: option<string>,
}
```

- Host parses & validates it once at the conversion boundary
  (`source.rs`); a malformed payload fails the call like any other bad
  plugin output.
- **Future metadata fields will never require a WIT bump or guest
  rebuilds** — plugins just add keys to the JSON object.
- Demo plugins rebuilt; wenku8 can start filling `publisher` etc. later
  without host changes. Documented in `docs/plugins.md`.

## Optional follow-up: epub extraction

EPUB OPF metadata often carries `dc:identifier` (urn:isbn),
`dc:publisher`, `dc:date`, `dc:creator` roles (translator via
`role="trl"`) and `dc:subject`/mediaType illustrator hints. A small
extension to the OPF parsing fills `ParsedBook.ext` so plain uploads get
ISBN/publisher for free. Kept as its own commit because it touches
parser tests only.

## Implementation plan (one feature = one commit)

1. `feat(core/db)` — migration 0012 (`books.ext_meta`), `BookExt` +
   `BookMeta.ext`, sanitize helper, storage read/write on all six paths'
   shared helpers, `just schema` regen. Unit tests: serde round-trip incl.
   unknown-key preservation, sanitize rules.
2. `feat(api)` — `BookMeta.ext` / `PatchBook.ext` merge semantics /
   multipart `ext` override / refresh replacement; OpenAPI + regenerated
   frontend types. Unit tests: patch merge (set/clear/keep), override
   precedence.
3. `feat(plugin)` — WIT 0.7.0 `book-entry.extra`, host validation +
   conversion, demo plugins rebuilt, fixtures refreshed,
   `docs/plugins.md` section.
4. `feat(formats)` *(optional)* — epub ISBN/publisher/date/translator
   extraction with unit test on a fixture epub.
5. `feat(web)` — book-detail meta section renders non-empty known fields
   (zh-CN labels: 出版社/ISBN/出版时间/译者/插画/页数/定价/装帧/语言);
   edit dialog + manual-acquire dialog gain the common inputs; i18n keys;
   `tsc` + `vite build`.

E2e additions: upload with manual `ext` → visible in `GET /api/books`;
patch set/clear round-trip; plugin entry carrying `extra` lands in
library metadata and survives refresh; old rows (pre-migration) read as
`{}`.
