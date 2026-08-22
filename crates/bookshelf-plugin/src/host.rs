//! The wasmtime component host (plugin interface v2).
//!
//! Each `*.wasm` file in the plugins directory is compiled once at startup
//! into a [`WasmPlugin`]. The server registers *instances* of a plugin:
//! one wasm file can back several instances, each with its own validated
//! configuration, id, and enable flag.
//!
//! Calls into a plugin are synchronous and stateless: a fresh
//! [`wasmtime::Store`] is created per call, the instance's configuration
//! is injected through the `config::configure` import, and an
//! epoch-deadline guards against runaway plugins.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

use bookshelf_core::error::{Error, Result};
use bookshelf_core::source::{SourceBook, SourceBookFile, SourceChapter};

use crate::http_fetch::{self, FetchPolicy};

// Generates the bindings for the `bookshelf:plugin/bookshelf-plugin` world
// from `wit/bookshelf.wit`:
//   - `store::Host` + `config::Host` + `http::Host` traits for the
//     imported interfaces
//   - `BookshelfPlugin` world handle with `add_to_linker` / `instantiate`
//     and `call_name`, `call_config_schema`, `call_capabilities`,
//     `call_declare`, `call_search_books`, `call_get_book`,
//     `call_chapter_titles`, `call_get_chapter`, `call_identify_upload`,
//     `call_get_book_file`.
wasmtime::component::bindgen!({
    path: "wit",
    world: "bookshelf:plugin/bookshelf-plugin",
});

// The bindgen macro brings the world-scoped types (`ConfigField`,
// `ConfigValue`, `BookEntry`, `DeclaredBook`, `SearchResult`) into this
// module's scope directly, and generates the interface modules
// (`bookshelf::plugin::{store, config, types}`) with the `Host` traits.
// `ConfigKind` is only referenced *inside* the config interface, so it
// stays under the interface module.
pub use bookshelf::plugin::config::ConfigKind;

/// Host caps (design R4 guards).
pub const MAX_SEARCH_LIMIT: u32 = 50;
pub const MAX_SEARCH_OFFSET: u32 = 10_000;
pub const MAX_DECLARE_BOOKS: usize = 10_000;
pub const MAX_DECLARE_CHAPTERS: usize = 100_000;
pub const MAX_CHAPTER_CONTENT_BYTES: usize = 2 * 1024 * 1024;
/// Whole book files from `get-book-file` are capped at this size (the
/// http size cap usually kicks in first; this guards non-fetched files).
pub const MAX_BOOK_FILE_BYTES: usize = 256 * 1024 * 1024;

/// The epoch pump fires every `EPOCH_PUMP_MS`; a guest that keeps running
/// past its per-call epoch deadline gets trapped.
///
/// The deadline is set a couple of ticks beyond the current epoch, so a
/// call shorter than one full pump interval can never be trapped (a trap
/// needs the deadline to be exceeded at a wasm backedge), while runaway
/// plugins die within a few intervals.
const EPOCH_PUMP_MS: u64 = 200;

/// Deadline offset in ticks beyond the current epoch (see above).
const EPOCH_DEADLINE_TICKS: u64 = 2;

/// Host-side state handed to plugin imports for the duration of one call.
struct HostState {
    /// Instance id, used as the log tag.
    name: String,
    /// Validated configuration values, in `config-schema` field order.
    config: Vec<ConfigValue>,
    /// Outbound HTTP policy (allow list, caps) — shared by all plugins.
    policy: Arc<FetchPolicy>,
}

impl bookshelf::plugin::store::Host for HostState {
    // v48: synchronous host functions return `()` directly; errors are
    // signalled by panicking (which traps the plugin).
    fn log(&mut self, message: String) {
        info!(plugin = %self.name, "plugin: {message}");
    }
}

// The world `use`s the types interface, which generates a types-only
// import module; it has no functions.
impl bookshelf::plugin::types::Host for HostState {}

impl bookshelf::plugin::config::Host for HostState {
    /// The guest pulls its configuration at the start of every call.
    fn configure(&mut self) -> Vec<ConfigValue> {
        self.config.clone()
    }
}

impl bookshelf::plugin::http::Host for HostState {
    /// The only way a plugin touches the outside world. The full policy
    /// (allow list, SSRF checks incl. redirect hops, timeout/size caps,
    /// stripped logging) lives in `http_fetch::fetch`; the server calls
    /// plugins on blocking threads, so a slow source never stalls a tokio
    /// worker.
    fn fetch(
        &mut self,
        request: bookshelf::plugin::http::Request,
    ) -> std::result::Result<bookshelf::plugin::http::Response, bookshelf::plugin::http::FetchError>
    {
        let req = http_fetch::FetchRequest {
            method: request.method,
            url: request.url,
            headers: request
                .headers
                .into_iter()
                .map(|h| (h.name, h.value))
                .collect(),
            body: request.body,
            timeout_ms: request.timeout_ms,
        };
        match http_fetch::fetch(&self.policy, req) {
            Ok(resp) => Ok(bookshelf::plugin::http::Response {
                status: resp.status,
                headers: resp
                    .headers
                    .into_iter()
                    .map(|(name, value)| bookshelf::plugin::http::Header { name, value })
                    .collect(),
                body: resp.body,
                final_url: resp.final_url,
            }),
            Err(e) => Err(match e {
                http_fetch::FetchError::InvalidUrl(m) => {
                    bookshelf::plugin::http::FetchError::InvalidUrl(m)
                }
                http_fetch::FetchError::Denied(m) => bookshelf::plugin::http::FetchError::Denied(m),
                http_fetch::FetchError::RedirectLimit(n) => {
                    bookshelf::plugin::http::FetchError::RedirectLimit(n)
                }
                http_fetch::FetchError::Timeout(ms) => {
                    bookshelf::plugin::http::FetchError::Timeout(ms)
                }
                http_fetch::FetchError::SizeLimit(n) => {
                    bookshelf::plugin::http::FetchError::SizeLimit(n)
                }
                http_fetch::FetchError::Transport(m) => {
                    bookshelf::plugin::http::FetchError::Transport(m)
                }
            }),
        }
    }
}

/// One compiled wasm component (per wasm file, shared by all instances).
pub struct WasmPlugin {
    /// Basename of the wasm file, e.g. `"wiki.wasm"` (instance rows refer
    /// to this).
    file: String,
    engine: Engine,
    component: Component,
    linker: Linker<HostState>,
    /// Outbound HTTP policy of the `http.fetch` import (allow list,
    /// timeout/size caps). Shared by every instance of this file.
    policy: Arc<FetchPolicy>,
}

impl WasmPlugin {
    /// Compile a component from raw wasm bytes. `policy` governs the
    /// `http.fetch` import of every instance of this file.
    pub fn load(
        file: impl Into<String>,
        wasm: Vec<u8>,
        policy: Arc<FetchPolicy>,
    ) -> anyhow::Result<Self> {
        let file = file.into();
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config)?;
        let component = Component::new(&engine, &wasm)
            .map_err(|e| anyhow::anyhow!("{file} is not a valid wasm component: {e}"))?;
        let mut linker = Linker::new(&engine);
        BookshelfPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut HostState| {
            state
        })?;
        Ok(Self {
            file,
            engine,
            component,
            linker,
            policy,
        })
        .inspect(|plugin| plugin.start_epoch_pump())
    }

    pub fn file(&self) -> &str {
        &self.file
    }

    /// Start the epoch pump: a background thread increments the engine's
    /// epoch every few ms, so any call whose deadline expired gets
    /// trapped at the next wasm backedge.
    fn start_epoch_pump(&self) {
        let engine = self.engine.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(EPOCH_PUMP_MS));
            engine.increment_epoch();
        });
    }

    /// Instantiate the component in a fresh store and run `f` against the
    /// typed bindings. `config` is injected via the `configure` import.
    fn call<T>(
        &self,
        config: &[ConfigValue],
        f: impl FnOnce(&mut Store<HostState>, &BookshelfPlugin) -> wasmtime::Result<T>,
    ) -> Result<T> {
        let mut store = Store::new(
            &self.engine,
            HostState {
                name: self.file.clone(),
                config: config.to_vec(),
                policy: self.policy.clone(),
            },
        );
        // Epoch deadline: calls shorter than one pump interval are never
        // trapped; a runaway plugin is interrupted within a few intervals.
        store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
        let bindings = BookshelfPlugin::instantiate(&mut store, &self.component, &self.linker)
            .map_err(|e| Error::Plugin(format!("instantiate `{}`: {e}", self.file)))?;
        f(&mut store, &bindings)
            .map_err(|e| Error::Plugin(format!("call into `{}`: {e}", self.file)))
    }

    /// Human-readable plugin name (same for all instances).
    pub fn name(&self) -> Result<String> {
        self.call(&[], |store, b| b.call_name(store))
    }

    /// Configuration schema (checked with the empty config; used against
    /// every instance of this wasm file).
    pub fn config_schema(&self) -> Result<Vec<ConfigField>> {
        self.call(&[], |store, b| b.call_config_schema(store))
    }

    /// Declared capabilities, one call at load time. The host keys all
    /// policy (sync, browsing, reads) off this list.
    pub fn capabilities(&self) -> Result<Vec<String>> {
        self.call(&[], |store, b| b.call_capabilities(store))
    }

    /// Full static catalog (`declare` capability). Enforced: never more
    /// than [`MAX_DECLARE_BOOKS`] books / [`MAX_DECLARE_CHAPTERS`]
    /// chapters (warns beyond 90% of the cap).
    pub fn declare(&self, config: &[ConfigValue]) -> Result<Option<Vec<DeclaredBook>>> {
        let declared = self.call(config, |store, b| b.call_declare(store))?;
        if let Some(books) = &declared {
            let chapters = books.iter().map(|b| b.chapters.len()).sum::<usize>();
            if books.len() > MAX_DECLARE_BOOKS || chapters > MAX_DECLARE_CHAPTERS {
                return Err(Error::Plugin(format!(
                    "`{}` declare catalog too large: {}/{} books, {}/{} chapters (caps)",
                    self.file,
                    books.len(),
                    MAX_DECLARE_BOOKS,
                    chapters,
                    MAX_DECLARE_CHAPTERS
                )));
            }
            if books.len() > MAX_DECLARE_BOOKS * 9 / 10 || chapters > MAX_DECLARE_CHAPTERS * 9 / 10
            {
                warn!(
                    plugin = %self.file,
                    books = books.len(),
                    chapters,
                    "declare catalog close to the size cap"
                );
            }
        }
        Ok(declared)
    }

    /// Paginated catalog search (`search` capability). The host clamps
    /// `limit` and `offset` to [`MAX_SEARCH_LIMIT`] / [`MAX_SEARCH_OFFSET`].
    pub fn search_books(
        &self,
        config: &[ConfigValue],
        query: &str,
        offset: u32,
        limit: u32,
    ) -> Result<SearchResult> {
        let offset = offset.min(MAX_SEARCH_OFFSET);
        let limit = limit.min(MAX_SEARCH_LIMIT);
        self.call(config, |store, b| {
            b.call_search_books(store, query, offset, limit)
        })
    }

    /// Single book by id (`lookup` capability).
    pub fn get_book(&self, config: &[ConfigValue], book_id: &str) -> Result<Option<BookEntry>> {
        self.call(config, |store, b| b.call_get_book(store, book_id))
    }

    /// Chapter titles of a book (`content` capability).
    pub fn chapter_titles(&self, config: &[ConfigValue], book_id: &str) -> Result<Vec<String>> {
        self.call(config, |store, b| b.call_chapter_titles(store, book_id))
    }

    /// Fetch one chapter (`content` capability). Content is capped at
    /// [`MAX_CHAPTER_CONTENT_BYTES`] per call.
    pub fn get_chapter(
        &self,
        config: &[ConfigValue],
        book_id: &str,
        index: u32,
    ) -> Result<Option<SourceChapter>> {
        let chapter = self.call(config, |store, b| b.call_get_chapter(store, book_id, index))?;
        match chapter {
            Some(c) => {
                if c.content.len() > MAX_CHAPTER_CONTENT_BYTES {
                    return Err(Error::Plugin(format!(
                        "`{}` chapter {book_id}#{index} exceeds the {} byte content cap",
                        self.file, MAX_CHAPTER_CONTENT_BYTES
                    )));
                }
                Ok(Some(SourceChapter {
                    title: c.title,
                    content: c.content,
                }))
            }
            None => Ok(None),
        }
    }

    /// Upload identification (`identify` capability).
    pub fn identify_upload(
        &self,
        config: &[ConfigValue],
        filename: &str,
        file_hash: &str,
    ) -> Result<Option<SourceBook>> {
        let book = self.call(config, |store, b| {
            b.call_identify_upload(store, filename, file_hash)
        })?;
        Ok(book.map(SourceBook::from))
    }

    /// Fetch a whole book file (`book-file` capability). `None` = the
    /// source has no file for this book (chapter mode). Bytes are capped
    /// at [`MAX_BOOK_FILE_BYTES`].
    pub fn get_book_file(
        &self,
        config: &[ConfigValue],
        book_id: &str,
    ) -> Result<Option<SourceBookFile>> {
        let file = self.call(config, |store, b| b.call_get_book_file(store, book_id))?;
        match file {
            Some(f) => {
                if f.bytes.len() > MAX_BOOK_FILE_BYTES {
                    return Err(Error::Plugin(format!(
                        "`{}` book file {book_id} exceeds the {} byte cap",
                        self.file, MAX_BOOK_FILE_BYTES
                    )));
                }
                Ok(Some(SourceBookFile {
                    filename: f.filename,
                    mime: f.mime,
                    bytes: f.bytes,
                }))
            }
            None => Ok(None),
        }
    }
}

impl From<BookEntry> for SourceBook {
    fn from(b: BookEntry) -> Self {
        SourceBook {
            id: b.id,
            title: b.title,
            authors: b.authors,
            description: b.description,
            cover_url: b.cover_url,
            content_source: b.content_source,
            content_id: b.content_id,
        }
    }
}

/// Loads every `*.wasm` component in `dir`, skipping invalid files with a
/// warning so a bad plugin never prevents the server from starting.
/// The returned map is keyed by file basename (e.g. `"hello.wasm"`).
/// `policy` governs the `http.fetch` import of every plugin.
pub fn load_dir(dir: &Path, policy: Arc<FetchPolicy>) -> anyhow::Result<Vec<Arc<WasmPlugin>>> {
    let mut plugins = Vec::new();
    if !dir.is_dir() {
        return Ok(plugins);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            continue;
        }
        let file = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin.wasm")
            .to_string();
        let wasm = std::fs::read(&path)?;
        match WasmPlugin::load(file.clone(), wasm, policy.clone()) {
            Ok(plugin) => {
                info!(file = %file, "loaded wasm plugin");
                plugins.push(Arc::new(plugin));
            }
            Err(e) => {
                warn!(file = %file, "skipping plugin: {e:#}");
            }
        }
    }
    plugins.sort_by(|a, b| a.file().cmp(b.file()));
    Ok(plugins)
}

// ---- configuration validation ----------------------------------------------
//
// The server stores configuration as a JSON object keyed by field key,
// validates it against the plugin's `config-schema`, and converts it into
// the WIT `config-value` list (in schema order) before every call.

/// One structured validation error, rendered inline in the admin form.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigFieldError {
    pub field: String,
    pub message: String,
}

/// A structured list of configuration errors (400 response body:
/// `{"errors": [...]}`).
#[derive(Debug, Clone)]
pub struct ConfigErrors(pub Vec<ConfigFieldError>);

impl ConfigErrors {
    pub fn push(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.0.push(ConfigFieldError {
            field: field.into(),
            message: message.into(),
        });
    }
}

impl std::fmt::Display for ConfigErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for e in &self.0 {
            writeln!(f, "{}: {}", e.field, e.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigErrors {}

/// Default JSON value of a field (from its serialized `default`), or
/// `null` when a field has no default and is optional.
///
/// Defaults are JSON-serialized (e.g. `"10"`, `"false"`, `"\"x\""`);
/// when a text field's default does not parse as JSON it is treated as
/// the literal string (friendlier for zh-CN labels).
fn field_default(field: &ConfigField) -> serde_json::Value {
    match &field.default {
        Some(raw) => match serde_json::from_str(raw) {
            Ok(value) => value,
            Err(_) if matches!(field.kind, ConfigKind::Text) => {
                serde_json::Value::String(raw.clone())
            }
            Err(_) => serde_json::Value::Null,
        },
        None => serde_json::Value::Null,
    }
}

/// Validate `input` (a JSON object) against the schema. Unknown keys are
/// rejected, `required` fields must be present, defaults fill in missing
/// optional fields, and every value must match its field kind. The
/// normalized object (with defaults applied) is returned.
pub fn validate_config(
    fields: &[ConfigField],
    input: &serde_json::Value,
) -> std::result::Result<serde_json::Value, ConfigErrors> {
    let mut errors = ConfigErrors(Vec::new());
    let obj = match input {
        // `null`/missing counts as an empty object (defaults fill it).
        serde_json::Value::Null => &serde_json::Map::new(),
        serde_json::Value::Object(map) => map,
        _ => {
            errors.push("$", "config must be a JSON object");
            return Err(errors);
        }
    };

    // Unknown keys.
    for key in obj.keys() {
        if !fields.iter().any(|f| &f.key == key) {
            errors.push(key, "unknown config key");
        }
    }

    let mut out = serde_json::Map::new();
    for field in fields {
        let value = match obj.get(&field.key) {
            Some(v) => {
                if v.is_null() {
                    // Explicit null = unset (defaults may kick in).
                    field_default(field)
                } else {
                    v.clone()
                }
            }
            None => field_default(field),
        };
        if value.is_null() && field.required {
            errors.push(&field.key, "required field is missing");
        }
        match (&field.kind, &value) {
            (ConfigKind::Text, serde_json::Value::String(_))
            | (ConfigKind::Number, serde_json::Value::Number(_))
            | (ConfigKind::Boolean, serde_json::Value::Bool(_)) => {}
            (ConfigKind::EnumOptions(options), serde_json::Value::Number(n)) => {
                if n.as_u64()
                    .map(|i| (i as usize) < options.len())
                    .unwrap_or(false)
                {
                    // valid index
                } else {
                    errors.push(
                        &field.key,
                        format!("must be an index in 0..{} (enum options)", options.len()),
                    );
                }
            }
            (ConfigKind::ListOfString, serde_json::Value::Array(items)) => {
                if !items.iter().all(|i| i.is_string()) {
                    errors.push(&field.key, "must be an array of strings");
                }
            }
            _ => {
                errors.push(&field.key, format!("expected {}", kind_name(&field.kind)));
            }
        }
        if !value.is_null() {
            out.insert(field.key.clone(), value);
        }
    }

    if errors.0.is_empty() {
        Ok(serde_json::Value::Object(out))
    } else {
        Err(errors)
    }
}

fn kind_name(kind: &ConfigKind) -> &'static str {
    match kind {
        ConfigKind::Text => "string",
        ConfigKind::Number => "number",
        ConfigKind::Boolean => "boolean",
        ConfigKind::EnumOptions(_) => "enum index",
        ConfigKind::ListOfString => "array of strings",
    }
}

/// Convert a validated config object into the WIT `config-value` list in
/// schema order, ready to be injected via `configure`. Missing optional
/// fields yield a sensible neutral value (`""` / `0` / `false` / `[]`).
pub fn values_from_config(fields: &[ConfigField], config: &serde_json::Value) -> Vec<ConfigValue> {
    fields
        .iter()
        .map(|field| match (&field.kind, config.get(&field.key)) {
            (ConfigKind::Text, Some(serde_json::Value::String(s))) => ConfigValue::Text(s.clone()),
            (ConfigKind::Number, Some(serde_json::Value::Number(n))) => {
                ConfigValue::Number(n.as_f64().unwrap_or(0.0))
            }
            (ConfigKind::Boolean, Some(serde_json::Value::Bool(b))) => ConfigValue::Boolean(*b),
            (ConfigKind::EnumOptions(_), Some(serde_json::Value::Number(n))) => {
                ConfigValue::EnumIndex(n.as_u64().unwrap_or(0) as u32)
            }
            (ConfigKind::ListOfString, Some(serde_json::Value::Array(items))) => {
                ConfigValue::StringList(
                    items
                        .iter()
                        .filter_map(|i| i.as_str().map(String::from))
                        .collect(),
                )
            }
            // Defaults were applied during validation; anything else gets
            // a neutral value (the guest sees the field as unset).
            (ConfigKind::Text, _) => ConfigValue::Text(String::new()),
            (ConfigKind::Number, _) => ConfigValue::Number(0.0),
            (ConfigKind::Boolean, _) => ConfigValue::Boolean(false),
            (ConfigKind::EnumOptions(_), _) => ConfigValue::EnumIndex(0),
            (ConfigKind::ListOfString, _) => ConfigValue::StringList(Vec::new()),
        })
        .collect()
}
