//! The wasmtime component host (plugin interface v2).
//!
//! Each `*.wasm` file in the plugins directory is compiled once at startup
//! through a shared [`PluginRuntime`] into a [`CompiledPlugin`]. The server registers *instances* of a plugin:
//! one wasm file can back several [`PluginInstance`] values, each with its own stored
//! configuration, id, and enable flag.
//!
//! Calls into a plugin are synchronous with fresh guest memory: a new
//! [`wasmtime::Store`] is created per call, the instance's configuration
//! is passed explicitly to business exports. Fuel bounds guest
//! computation without imposing a wall-clock deadline on network waits.
//! The instance-scoped `state` import provides explicit durable storage.

use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

use crate::error::{BookEntryError, Error, ImageStoreError, Result};
use mandara_core::source::{SourceBook, SourceBookFile, SourceChapter, SourceSeries};
use wasmtime::error::Context as _;

use crate::http_fetch::{self, FetchPolicy};
use crate::state::{StateStore, StateStoreError};

// Generates the bindings for the `mandara:plugin/mandara-plugin` world
// from `wit/mandara.wit`:
//   - `store::Host` + `state::Host` + `http::Host` traits for the
//     imported interfaces
//   - `MandaraPlugin` world handle with `add_to_linker` / `instantiate`
//     and `call_name`, `call_config_schema`, `call_capabilities`,
//     `call_declare`, `call_search_books`, `call_get_book`,
//     `call_get_content`, `call_get_chapter`, `call_identify_upload`,
//     `call_get_book_file`.
wasmtime::component::bindgen!({
    path: "wit",
    world: "mandara:plugin/mandara-plugin",
    imports: { "mandara:plugin/store.store-image": trappable },
});

// The bindgen macro brings the world-scoped types (`ConfigField`,
// `ConfigValue`, `BookEntry`, `DeclaredBook`, `SearchResult`) into this
// module's scope directly, and generates the interface modules
// (`mandara::plugin::{store, state, http, types}`) with the `Host` traits.
// `ConfigKind` is only referenced *inside* the types interface, so it
// stays under the interface module.
pub use mandara::plugin::types::ConfigKind;

/// Host caps (design R4 guards).
pub const MAX_SEARCH_LIMIT: u32 = 50;
pub const MAX_SEARCH_OFFSET: u32 = 10_000;
pub const MAX_DECLARE_BOOKS: usize = 10_000;
pub const MAX_DECLARE_CHAPTERS: usize = 100_000;
pub const MAX_CHAPTER_CONTENT_BYTES: usize = 2 * 1024 * 1024;
/// Whole book files from `get-book-file` are capped at this size (the
/// http size cap usually kicks in first; this guards non-fetched files).
pub const MAX_BOOK_FILE_BYTES: usize = 256 * 1024 * 1024;

/// Per-image byte cap for the `store-image` import (a plugin fetching its
/// own chapter images must not park arbitrarily large blobs).
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// Per-call total byte cap for `store-image` (a fresh [`HostState`] per
/// call makes this accounting free). Roughly a full illustration chapter's
/// worth of plates.
pub const MAX_CALL_IMAGE_BYTES: usize = 96 * 1024 * 1024;

/// Per-call guest instruction budget. Host I/O does not consume fuel, so
/// serial network requests have no aggregate wall-clock deadline. A spinning
/// guest still traps instead of consuming a blocking worker indefinitely.
const MAX_CALL_FUEL: u64 = 1_000_000_000;

/// Sink for the `store-image` import: implemented by the server (which
/// owns the DB pool and the files directory) and handed to every plugin
/// call. Synchronous — plugin calls already run on blocking threads.
pub trait ImageStore: Send + Sync {
    /// Store image bytes; returns the content address (sha256 hex). Must
    /// dedup: identical bytes return the same id. Backend failures retain
    /// their original source through [`ImageStoreError`].
    fn store_image(&self, bytes: &[u8], mime: &str)
    -> std::result::Result<String, ImageStoreError>;
}

/// Cheap per-call service handles, independent of the Wasmtime execution environment.
#[derive(Clone)]
struct HostServices {
    policy: FetchPolicy,
    images: Arc<dyn ImageStore>,
}

/// Per-call context: only identity, bound storage, services, and invocation data.
struct HostState {
    instance_id: Arc<str>,
    file: Arc<str>,
    state: Arc<dyn StateStore>,
    services: HostServices,
    image_bytes_stored: u64,
}

impl HostState {
    fn new(instance: &PluginInstance, services: HostServices) -> Self {
        Self {
            instance_id: instance.data.id.clone(),
            file: instance.data.plugin.file.clone(),
            state: instance.data.state.clone(),
            services,
            image_bytes_stored: 0,
        }
    }

    fn state_error(&self, error: StateStoreError) -> String {
        match error {
            StateStoreError::QuotaExceeded => "plugin state quota exceeded".into(),
            StateStoreError::Backend(error) => {
                warn!(plugin = %self.instance_id, error = %error, "plugin state storage failed");
                "plugin state storage unavailable".into()
            }
        }
    }
}

impl mandara::plugin::state::Host for HostState {
    fn get(&mut self, key: String) -> std::result::Result<Option<Vec<u8>>, String> {
        self.state.get(&key).map_err(|e| self.state_error(e))
    }

    fn set(&mut self, key: String, value: Vec<u8>) -> std::result::Result<(), String> {
        self.state
            .set(&key, &value)
            .map_err(|e| self.state_error(e))
    }

    fn delete(&mut self, key: String) -> std::result::Result<bool, String> {
        self.state.delete(&key).map_err(|e| self.state_error(e))
    }
}

impl mandara::plugin::store::Host for HostState {
    // Logging is infallible; image storage is a trappable host import.
    fn log(&mut self, message: String) {
        info!(plugin = %self.instance_id, "plugin: {message}");
    }

    fn store_image(&mut self, bytes: Vec<u8>, mime: String) -> wasmtime::Result<String> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(Error::ContentLimit {
                file: self.file.to_string(),
                resource: "store-image (per-image)",
                limit: MAX_IMAGE_BYTES,
            }
            .into());
        }
        if self.image_bytes_stored + bytes.len() as u64 > MAX_CALL_IMAGE_BYTES as u64 {
            return Err(Error::ContentLimit {
                file: self.file.to_string(),
                resource: "store-image (per-call)",
                limit: MAX_CALL_IMAGE_BYTES,
            }
            .into());
        }
        let id = self
            .services
            .images
            .store_image(&bytes, &mime)
            .with_context(|| format!("`{}` store-image failed", self.file))?;
        self.image_bytes_stored += bytes.len() as u64;
        Ok(id)
    }
}

// The world `use`s the types interface, which generates a types-only
// import module; it has no functions.
impl mandara::plugin::types::Host for HostState {}

impl mandara::plugin::http::Host for HostState {
    /// The only way a plugin touches the outside world. The full policy
    /// (timeout/size caps, redirect limit, stripped logging) lives in
    /// `http_fetch::fetch`; the server calls plugins on blocking threads,
    /// so a slow source never stalls a tokio worker.
    fn fetch(
        &mut self,
        request: mandara::plugin::http::Request,
    ) -> std::result::Result<mandara::plugin::http::Response, mandara::plugin::http::FetchError>
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
        match http_fetch::fetch(&self.services.policy, req) {
            Ok(resp) => Ok(mandara::plugin::http::Response {
                status: resp.status,
                headers: resp
                    .headers
                    .into_iter()
                    .map(|(name, value)| mandara::plugin::http::Header { name, value })
                    .collect(),
                body: resp.body,
                final_url: resp.final_url,
            }),
            Err(e) => Err(match e {
                http_fetch::FetchError::InvalidUrl(m) => {
                    mandara::plugin::http::FetchError::InvalidUrl(m.to_string())
                }
                http_fetch::FetchError::Denied(m) => mandara::plugin::http::FetchError::Denied(m),
                http_fetch::FetchError::RedirectLimit(n) => {
                    mandara::plugin::http::FetchError::RedirectLimit(n)
                }
                http_fetch::FetchError::Timeout(ms) => {
                    mandara::plugin::http::FetchError::Timeout(ms)
                }
                http_fetch::FetchError::SizeLimit(n) => {
                    mandara::plugin::http::FetchError::SizeLimit(n)
                }
                http_fetch::FetchError::Transport(m) => {
                    mandara::plugin::http::FetchError::Transport(m.to_string())
                }
            }),
        }
    }
}

/// Shared execution environment for all compiled plugins.
pub struct PluginRuntime {
    engine: Engine,
    linker: Linker<HostState>,
    services: HostServices,
}

impl PluginRuntime {
    pub fn new(policy: FetchPolicy, images: Arc<dyn ImageStore>) -> anyhow::Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        let engine = Engine::new(&config)?;
        let mut linker = Linker::new(&engine);
        MandaraPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut HostState| state)?;
        Ok(Self {
            engine,
            linker,
            services: HostServices { policy, images },
        })
    }

    /// Compile code without instantiating or executing guest exports.
    pub fn compile(
        &self,
        file: impl Into<String>,
        wasm: &[u8],
    ) -> anyhow::Result<Arc<CompiledPlugin>> {
        let file = file.into();
        let component = Component::new(&self.engine, wasm)
            .with_context(|| format!("{file} is not a valid wasm component"))?;
        Ok(Arc::new(CompiledPlugin {
            file: file.into(),
            component,
        }))
    }

    /// Bind code, configuration, and storage to one registered instance.
    pub fn instance(
        &self,
        id: impl Into<String>,
        plugin: Arc<CompiledPlugin>,
        config: serde_json::Value,
        state: Arc<dyn StateStore>,
    ) -> anyhow::Result<PluginInstance> {
        anyhow::ensure!(
            Engine::same(&self.engine, plugin.component.engine()),
            "plugin component belongs to another runtime"
        );
        Ok(PluginInstance {
            data: Arc::new(InstanceData {
                id: Arc::from(id.into()),
                plugin,
                config,
                state,
            }),
        })
    }

    /// Load directory entries through this runtime's shared engine.
    pub fn load_dir(&self, dir: &Path) -> anyhow::Result<Vec<Arc<CompiledPlugin>>> {
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
            match self.compile(file.clone(), &wasm) {
                Ok(plugin) => {
                    info!(file = %file, "loaded wasm plugin");
                    plugins.push(plugin);
                }
                Err(e) => {
                    warn!(file = %file, "skipping plugin: {e:#}");
                }
            }
        }
        plugins.sort_by(|a, b| a.file().cmp(b.file()));
        Ok(plugins)
    }
}

/// Compiled code and its identity, shared by registered instances.
pub struct CompiledPlugin {
    file: Arc<str>,
    component: Component,
}

impl CompiledPlugin {
    pub fn file(&self) -> &str {
        &self.file
    }
}

/// An immutable configuration snapshot of a registered plugin instance.
/// Clones and configuration updates retain the same instance-bound storage.
#[derive(Clone)]
pub struct PluginInstance {
    data: Arc<InstanceData>,
}

struct InstanceData {
    id: Arc<str>,
    plugin: Arc<CompiledPlugin>,
    config: serde_json::Value,
    state: Arc<dyn StateStore>,
}

impl PluginInstance {
    pub fn id(&self) -> &str {
        &self.data.id
    }
    pub fn file(&self) -> &str {
        self.data.plugin.file()
    }
    pub fn config(&self) -> &serde_json::Value {
        &self.data.config
    }

    /// Update configuration without rebinding the instance's storage.
    pub fn with_config(&self, config: serde_json::Value) -> Self {
        Self {
            data: Arc::new(InstanceData {
                id: self.data.id.clone(),
                plugin: self.data.plugin.clone(),
                config,
                state: self.data.state.clone(),
            }),
        }
    }

    /// Distinguish a live registration from a deleted and recreated ID.
    pub fn same_registration(&self, other: &Self) -> bool {
        self.id() == other.id()
            && Arc::ptr_eq(&self.data.plugin, &other.data.plugin)
            && Arc::ptr_eq(&self.data.state, &other.data.state)
    }
}

impl PluginRuntime {
    fn invoke<T>(
        &self,
        instance: &PluginInstance,
        f: impl FnOnce(&mut Store<HostState>, &MandaraPlugin) -> wasmtime::Result<T>,
    ) -> Result<T> {
        let mut store = Store::new(
            instance.data.plugin.component.engine(),
            HostState::new(instance, self.services.clone()),
        );
        store.set_fuel(MAX_CALL_FUEL)?;
        let bindings =
            MandaraPlugin::instantiate(&mut store, &instance.data.plugin.component, &self.linker)
                .with_context(|| format!("instantiate '{}'", instance.file()))?;
        Ok(f(&mut store, &bindings).with_context(|| format!("call into '{}'", instance.file()))?)
    }

    /// Pass this instance's positional configuration to the business export.
    fn invoke_with_config<T>(
        &self,
        instance: &PluginInstance,
        f: impl FnOnce(&mut Store<HostState>, &MandaraPlugin, &[ConfigValue]) -> wasmtime::Result<T>,
    ) -> Result<T> {
        let schema = self.config_schema(instance)?;
        let config = values_from_config(&schema, &instance.data.config);
        self.invoke(instance, |store, bindings| f(store, bindings, &config))
    }

    /// Metadata exports require no configuration, but always have instance storage.
    pub fn name(&self, instance: &PluginInstance) -> Result<String> {
        self.invoke(instance, |store, b| b.call_name(store))
    }
    pub fn source_info(&self, instance: &PluginInstance) -> Result<SourceInfo> {
        self.invoke(instance, |store, b| b.call_source_info(store))
    }
    pub fn config_schema(&self, instance: &PluginInstance) -> Result<Vec<ConfigField>> {
        self.invoke(instance, |store, b| b.call_config_schema(store))
    }
    pub fn capabilities(&self, instance: &PluginInstance) -> Result<Vec<String>> {
        self.invoke(instance, |store, b| b.call_capabilities(store))
    }

    /// Full static catalog (`declare` capability). Enforced: never more
    /// than [`MAX_DECLARE_BOOKS`] books / [`MAX_DECLARE_CHAPTERS`]
    /// chapters (warns beyond 90% of the cap).
    pub fn declare(&self, instance: &PluginInstance) -> Result<Option<Vec<DeclaredBook>>> {
        let declared = self
            .invoke_with_config(instance, |store, b, config| b.call_declare(store, config))?;
        if let Some(books) = &declared {
            let chapters = books
                .iter()
                .filter_map(|b| b.content.as_ref())
                .map(|c| c.chapters.len())
                .sum::<usize>();
            if books.len() > MAX_DECLARE_BOOKS || chapters > MAX_DECLARE_CHAPTERS {
                return Err(Error::CatalogLimit {
                    file: instance.file().to_string(),
                    books: books.len(),
                    max_books: MAX_DECLARE_BOOKS,
                    chapters,
                    max_chapters: MAX_DECLARE_CHAPTERS,
                });
            }
            if books.len() > MAX_DECLARE_BOOKS * 9 / 10 || chapters > MAX_DECLARE_CHAPTERS * 9 / 10
            {
                warn!(
                    plugin = %instance.file(),
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
        instance: &PluginInstance,
        query: &str,
        offset: u32,
        limit: u32,
    ) -> Result<SearchResult> {
        let offset = offset.min(MAX_SEARCH_OFFSET);
        let limit = limit.min(MAX_SEARCH_LIMIT);
        self.invoke_with_config(instance, |store, b, config| {
            b.call_search_books(store, config, query, offset, limit)
        })
    }

    /// Single book by id (`lookup` capability).
    pub fn get_book(&self, instance: &PluginInstance, book_id: &str) -> Result<Option<BookEntry>> {
        self.invoke_with_config(instance, |store, b, config| {
            b.call_get_book(store, config, book_id)
        })
    }

    /// Fetch one chapter (`content` capability). Content is capped at
    /// [`MAX_CHAPTER_CONTENT_BYTES`] per call.
    pub fn get_chapter(
        &self,
        instance: &PluginInstance,
        book_id: &str,
        index: u32,
    ) -> Result<Option<SourceChapter>> {
        let chapter = self.invoke_with_config(instance, |store, b, config| {
            b.call_get_chapter(store, config, book_id, index)
        })?;
        match chapter {
            Some(c) => {
                if c.content.len() > MAX_CHAPTER_CONTENT_BYTES {
                    return Err(Error::ContentLimit {
                        file: instance.file().to_string(),
                        resource: "chapter",
                        limit: MAX_CHAPTER_CONTENT_BYTES,
                    });
                }
                Ok(Some(SourceChapter {
                    title: c.title,
                    format: c.format,
                    content: c.content,
                }))
            }
            None => Ok(None),
        }
    }

    /// Content identity, loading and chapter declarations from one plugin call.
    pub fn get_content(
        &self,
        instance: &PluginInstance,
        content_id: &str,
    ) -> Result<Option<mandara_core::source::SourceDeclaredContent>> {
        let content = self.invoke_with_config(instance, |store, bindings, config| {
            bindings.call_get_content(store, config, content_id)
        })?;
        if let Some(content) = &content
            && content.chapters.len() > MAX_DECLARE_CHAPTERS
        {
            return Err(Error::ContentLimit {
                file: instance.file().to_string(),
                resource: "content chapters",
                limit: MAX_DECLARE_CHAPTERS,
            });
        }
        Ok(content.map(Into::into))
    }

    /// Upload identification (`identify` capability).
    pub fn identify_upload(
        &self,
        instance: &PluginInstance,
        filename: &str,
        file_hash: &str,
    ) -> Result<Option<SourceBook>> {
        let book = self.invoke_with_config(instance, |store, b, config| {
            b.call_identify_upload(store, config, filename, file_hash)
        })?;
        Ok(book.map(SourceBook::try_from).transpose()?)
    }

    /// Resolve a series into individually identified, validated member books.
    pub fn get_series(
        &self,
        instance: &PluginInstance,
        series_id: &str,
    ) -> Result<Option<SeriesResult>> {
        let result = self.invoke_with_config(instance, |store, bindings, config| {
            bindings.call_get_series(store, config, series_id)
        })?;
        if let Some(series) = &result {
            SourceSeries::try_from(series.series.clone())?;
            if series.books.len() > MAX_DECLARE_BOOKS {
                return Err(BookEntryError::Identity("series member limit exceeded".into()).into());
            }
            let mut ids = std::collections::HashSet::new();
            for book in &series.books {
                SourceBook::try_from(book.clone())?;
                if !ids.insert(&book.id)
                    || book.series.as_ref().map(|s| &s.id) != Some(&series.series.id)
                {
                    return Err(BookEntryError::Identity(
                        "duplicate book ID or mismatched parent series".into(),
                    )
                    .into());
                }
            }
        }
        Ok(result)
    }

    /// Fetch a whole content file. `None` means content is unavailable.
    /// Bytes are capped at [`MAX_BOOK_FILE_BYTES`].
    pub fn get_book_file(
        &self,
        instance: &PluginInstance,
        book_id: &str,
    ) -> Result<Option<SourceBookFile>> {
        let file = self.invoke_with_config(instance, |store, b, config| {
            b.call_get_book_file(store, config, book_id)
        })?;
        match file {
            Some(f) => {
                if f.bytes.len() > MAX_BOOK_FILE_BYTES {
                    return Err(Error::ContentLimit {
                        file: instance.file().to_string(),
                        resource: "book file",
                        limit: MAX_BOOK_FILE_BYTES,
                    });
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

impl From<ContentMode> for mandara_core::model::ContentMode {
    fn from(mode: ContentMode) -> Self {
        match mode {
            ContentMode::Chapters => Self::Chapters,
            ContentMode::File => Self::File,
        }
    }
}

impl From<DeclaredContent> for mandara_core::source::SourceDeclaredContent {
    fn from(content: DeclaredContent) -> Self {
        Self {
            id: content.id,
            mode: content.mode.into(),
            chapters: content
                .chapters
                .into_iter()
                .map(|chapter| mandara_core::source::SourceDeclaredChapter {
                    title: chapter.title,
                    format: chapter.format,
                    content: chapter.content,
                })
                .collect(),
        }
    }
}

/// Convert a plugin's `book-entry` into the core source model,
/// validating + parsing the optional extended-metadata JSON object
/// (`book-entry.extra`): it must be a valid `BookExt`-shaped object
/// (unknown keys allowed); anything else fails the plugin call so bad
/// data never lands.
impl TryFrom<BookEntry> for SourceBook {
    type Error = BookEntryError;

    fn try_from(b: BookEntry) -> std::result::Result<Self, Self::Error> {
        if b.id.is_empty()
            || (b.series.is_none() && b.volume_no != 0)
            || (b.series.is_some() && b.volume_no == 0)
        {
            return Err(BookEntryError::Identity(
                "book ID and volume order must agree with series membership".into(),
            ));
        }
        let ext = match b.extra {
            None => None,
            Some(raw) => {
                let value: serde_json::Value = serde_json::from_str(&raw)?;
                let validated = mandara_core::ext::BookExt::from_value(value.clone())?;
                Some(serde_json::to_value(validated).unwrap_or(value))
            }
        };
        Ok(Self {
            id: b.id,
            title: b.title,
            authors: b.authors,
            description: b.description,
            cover_url: b.cover_url,
            ext,
            series: b.series.map(SourceSeries::try_from).transpose()?,
            volume_no: b.volume_no,
        })
    }
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
        // Optional fields without a default stay `null` (absent) — only
        // present values are type-checked.
        if !value.is_null() {
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

/// Convert stored configuration into the WIT `config-value` list in schema
/// order. Unsaved fields use schema defaults; fields without a default use
/// a neutral value (`""` / `0` / `false` / `[]`) until configuration is saved.
pub fn values_from_config(fields: &[ConfigField], config: &serde_json::Value) -> Vec<ConfigValue> {
    fields
        .iter()
        .map(|field| {
            let fallback = field_default(field);
            let value = config
                .get(&field.key)
                .filter(|v| !v.is_null())
                .unwrap_or(&fallback);
            match (&field.kind, Some(value)) {
                (ConfigKind::Text, Some(serde_json::Value::String(s))) => {
                    ConfigValue::Text(s.clone())
                }
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
                // Required fields can remain unset before configuration is saved.
                (ConfigKind::Text, _) => ConfigValue::Text(String::new()),
                (ConfigKind::Number, _) => ConfigValue::Number(0.0),
                (ConfigKind::Boolean, _) => ConfigValue::Boolean(false),
                (ConfigKind::EnumOptions(_), _) => ConfigValue::EnumIndex(0),
                (ConfigKind::ListOfString, _) => ConfigValue::StringList(Vec::new()),
            }
        })
        .collect()
}
impl TryFrom<SeriesEntry> for SourceSeries {
    type Error = BookEntryError;
    fn try_from(s: SeriesEntry) -> std::result::Result<Self, Self::Error> {
        if s.id.is_empty() {
            return Err(BookEntryError::Identity("empty series ID".into()));
        }
        let ext = s
            .extra
            .map(|raw| -> std::result::Result<_, BookEntryError> {
                let value = serde_json::from_str(&raw)?;
                let ext = mandara_core::ext::SeriesExt::from_value(value)?;
                Ok(serde_json::to_value(ext).expect("series metadata is serializable"))
            })
            .transpose()?;
        Ok(Self {
            id: s.id,
            title: s.title,
            authors: s.authors,
            description: s.description,
            cover_url: s.cover_url,
            ext,
        })
    }
}
