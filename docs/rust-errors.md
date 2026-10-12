# Rust error boundaries

## Error ownership

Reusable modules expose enums implementing `std::error::Error` through
`thiserror`. Validation errors retain only their category, not rejected values
or input-derived details. Resource errors retain limits and operation context,
not book ids or uploaded filenames. Foreign
errors are retained with `#[from]` or `#[source]` so callers can inspect both
the category and the cause.

- Core models use `ModelError` for textual domain values and image ids.
- Extended metadata uses `MetadataError` and `IsbnError`. JSON, invalid
  digits, invalid length, and checksum failures remain distinguishable.
- Format parsers use their own `error::Error`, retaining ZIP, EPUB, and UTF-8
  errors instead of converting them to core invalid-argument messages.
- The plugin host separates `BookEntryError` (advertised metadata validation)
  from runtime and resource-limit failures. Wasmtime's native context API
  preserves its errors before the lossless conversion into `anyhow::Error`.
- Authentication uses `AuthError` for password hashing and token encoding.
  Upload parsing uses `UploadError` for multipart and field-specific JSON.
- Guest HTTP helpers use the generated WIT `FetchError` directly. Bangumi's
  application error enum also covers JSON decoding and HTTP status failures.

Application orchestration and startup may use `anyhow::Result` and
`Context`/`with_context`. These add operation context without discarding the
original error. Add context only when it identifies a missing operation or
resource, not to repeat an existing error category. Propagation uses `?` and
centralized `From` implementations;
`map_err` is not needed just to stringify a foreign error.

The core plugin extension error accepts a boxed `dyn Error + Send + Sync`.
The host's `ImageStore` returns `ImageStoreError`, whose backend variant
retains a boxed `dyn Error + Send + Sync`. Implementations do not expose
`anyhow` or database-specific errors in the trait signature. The Wasmtime
import converts failures to runtime traps with context: storage failures and
size limits return errors, never host panics.
A failed image write does not consume the call's image budget.

## HTTP and WIT boundaries

`ApiError` is responsible for HTTP policy, not library error construction.
Client validation and malformed uploads return 400 with their original source.
Database row conversions return `Result<_, ApiError>` and explicitly classify
invalid stored domain values as internal errors rather than client validation.
Plugin calls and resource failures remain internal; validation of an advertised
book entry at the acquisition boundary remains 400.
Authentication rejection is 401, invalid image paths are 404, and structured
configuration/conflict responses retain their existing shapes.

Internal failures log the complete error chain but return only
`{"error":"internal server error"}`. `ApiError::source()` points to the
wrapped error itself, including errors that have no further source.
`BadRequest(String)` is reserved for deliberate client-facing messages;
foreign errors go through `InvalidInput` or `Internal` instead.

WIT transport errors have string payloads by protocol design. Host HTTP errors
retain native causes until conversion to those payloads. Serialization into
HTTP JSON or WIT strings is the boundary where error formatting belongs.
Neither protocol needs a schema change for typed Rust errors.

## Intentional adapters and recovery

Not every `map_err` is an antipattern. Retain adapters that actually select
semantics or satisfy external callback signatures:

- Database domain parsing selects `Internal` instead of the same error
  type's client-validation mapping, while retaining the source.
- Upload JSON errors select the authors/ext field while retaining the source.
- JWT rejection and invalid image paths deliberately hide details with 401/404.
- Diesel connection setup must wrap a query error in its required connection
  error type; the boxed cause remains available.
- CSS parser callbacks adapt basic parser errors to grammar errors.
- HTTP transport classification selects retry/timeout/size/redirect outcomes;
  invalid URL/request and transport failures retain native errors.

Optional EPUB resources and metadata enrichment remain tolerant: missing or
unsupported data is handled by the existing diagnostics/fallback policy. Guest
exports without a WIT error result still log failures and return their specified
empty/absent results. These are explicit recovery/ABI decisions, not reasons to
return `Result<T, String>` inside Rust modules.
