//! Plugin instance service: the bridge between the wasm host and the
//! library.
//!
//! The service compiles every `data/plugins/*.wasm` once and registers one
//! *instance* per row of `plugin_instances` (id = source id, wasm file +
//! validated config + enabled flag; one wasm file can back several
//! instances). Every call into a plugin:
//!
//!   1. loads the instance row (creation order matters for
//!      `identify-upload`),
//!   2. converts the stored config into the WIT `config-value` list in
//!      schema order,
//!   3. calls the export with a fresh store (statelessness, design R2),
//!      injected config (R3) and epoch-guarded execution,
//!   4. enforces capability policy (`capabilities()` decides which exports
//!      may be called).
//!
//! All durable state lives in SQLite under the host's control.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use sqlx::SqlitePool;

use bookshelf_core::error::Error as CoreError;
use bookshelf_core::source::{SourceBook, SourceChapter};
use bookshelf_plugin::{
    validate_config, values_from_config, BookEntry, ConfigErrors, ConfigField, ConfigKind,
    ConfigValue, SearchResult, WasmPlugin,
};

use crate::error::ApiError;

/// Facts about one wasm file, computed once at load time.
struct WasmInfo {
    wasm: Arc<WasmPlugin>,
    /// Human-readable plugin name.
    name: String,
    /// Declared capabilities (policy keyed off this list).
    capabilities: Vec<String>,
    /// Configuration schema (validation + value conversion).
    schema: Vec<ConfigField>,
}

/// One row of `plugin_instances`.
#[derive(Debug, Clone)]
pub struct InstanceRow {
    pub id: String,
    pub wasm_file: String,
    pub config: serde_json::Value,
    pub enabled: bool,
}

/// Instance as seen by `GET /api/plugins` (no config values — they may
/// contain secrets; only admins fetch them via the config-schema endpoint).
#[derive(Debug, Serialize)]
pub struct InstanceInfo {
    pub id: String,
    pub wasm_file: String,
    pub name: String,
    pub enabled: bool,
    pub capabilities: Vec<String>,
}

/// A serializable view of one `config-field` (the bindgen type itself is
/// not serializable).
#[derive(Debug, Serialize)]
pub struct ConfigFieldView {
    pub key: String,
    pub label: String,
    /// `"string"` | `"number"` | `"boolean"` | `"enum"` | `"list-of-string"`
    pub kind: String,
    /// Enum options (empty for other kinds).
    pub options: Vec<String>,
    pub default: Option<String>,
    pub required: bool,
    pub hint: Option<String>,
}

/// One declared book plus its declared chapters (sync path).
pub struct DeclaredCatalogBook {
    pub book: SourceBook,
    /// Declared chapters in reading order; non-empty `content` can be
    /// materialized eagerly, empty = title-only placeholder.
    pub chapters: Vec<SourceChapter>,
}

pub struct DeclaredCatalog {
    pub instance: String,
    pub books: Vec<DeclaredCatalogBook>,
}

/// Compiled wasm files + the instance registry.
pub struct PluginService {
    db: SqlitePool,
    wasm_by_file: HashMap<String, Arc<WasmInfo>>,
}

impl PluginService {
    /// Compile all wasm files and cache per-file facts. A file whose
    /// `name`/`capabilities`/`config-schema` cannot be fetched is kept
    /// with empty capabilities (its exports can never be reached through
    /// the capability gates) and a warning.
    pub fn new(db: SqlitePool, wasms: Vec<Arc<WasmPlugin>>) -> Self {
        let mut wasm_by_file = HashMap::new();
        for wasm in wasms {
            let (name, capabilities, schema) =
                match (wasm.name(), wasm.capabilities(), wasm.config_schema()) {
                    (Ok(name), Ok(capabilities), Ok(schema)) => (name, capabilities, schema),
                    (name, capabilities, schema) => {
                        tracing::warn!(
                            file = wasm.file(),
                            name_err = name.is_err(),
                            caps_err = capabilities.is_err(),
                            schema_err = schema.is_err(),
                            "could not introspect plugin; treating it as capability-less"
                        );
                        (
                            name.unwrap_or_else(|_| wasm.file().to_string()),
                            capabilities.unwrap_or_default(),
                            schema.unwrap_or_default(),
                        )
                    }
                };
            wasm_by_file.insert(
                wasm.file().to_string(),
                Arc::new(WasmInfo {
                    wasm,
                    name,
                    capabilities,
                    schema,
                }),
            );
        }
        Self { db, wasm_by_file }
    }

    /// Basenames of the compiled wasm files (for the register form).
    pub fn wasm_files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.wasm_by_file.keys().cloned().collect();
        files.sort();
        files
    }

    // ---- instance rows ---------------------------------------------------

    async fn rows(&self) -> Result<Vec<InstanceRow>, ApiError> {
        sqlx::query_as::<_, (String, String, String, i64)>(
            "SELECT id, wasm_file, config, enabled \
             FROM plugin_instances ORDER BY created_at, rowid",
        )
        .fetch_all(&self.db)
        .await?
        .into_iter()
        .map(|(id, wasm_file, config, enabled)| {
            Ok(InstanceRow {
                id,
                wasm_file,
                config: serde_json::from_str(&config).unwrap_or_else(|_| serde_json::json!({})),
                enabled: enabled != 0,
            })
        })
        .collect()
    }

    async fn row(&self, id: &str) -> Result<Option<InstanceRow>, ApiError> {
        let row: Option<(String, String, String, i64)> = sqlx::query_as(
            "SELECT id, wasm_file, config, enabled \
             FROM plugin_instances WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db)
        .await?;
        Ok(row.map(|(id, wasm_file, config, enabled)| InstanceRow {
            id,
            wasm_file,
            config: serde_json::from_str(&config).unwrap_or_else(|_| serde_json::json!({})),
            enabled: enabled != 0,
        }))
    }

    fn wasm_info(&self, wasm_file: &str) -> Option<Arc<WasmInfo>> {
        self.wasm_by_file.get(wasm_file).cloned()
    }

    fn info(&self, row: &InstanceRow) -> InstanceInfo {
        let wasm = self.wasm_info(&row.wasm_file);
        InstanceInfo {
            id: row.id.clone(),
            wasm_file: row.wasm_file.clone(),
            name: wasm
                .as_ref()
                .map(|w| w.name.clone())
                .unwrap_or_else(|| row.wasm_file.clone()),
            enabled: row.enabled,
            capabilities: wasm.map(|w| w.capabilities.clone()).unwrap_or_default(),
        }
    }

    /// All instances in creation order.
    pub async fn infos(&self) -> Result<Vec<InstanceInfo>, ApiError> {
        Ok(self.rows().await?.iter().map(|r| self.info(r)).collect())
    }

    // ---- instance management (admin) -------------------------------------

    fn validate(
        &self,
        wasm: &WasmInfo,
        input: &serde_json::Value,
    ) -> Result<serde_json::Value, ApiError> {
        validate_config(&wasm.schema, input).map_err(ApiError::ConfigErrors)
    }

    /// Register an instance for a compiled wasm file. Config defaults to
    /// `{}` (schema defaults fill the rest).
    pub async fn register(
        &self,
        id: &str,
        wasm_file: &str,
        config: serde_json::Value,
    ) -> Result<InstanceInfo, ApiError> {
        let wasm = self.wasm_info(wasm_file).ok_or_else(|| {
            ApiError::bad_request(format!("no wasm file `{wasm_file}` is loaded"))
        })?;
        let config = self.validate(&wasm, &config)?;
        sqlx::query("INSERT INTO plugin_instances (id, wasm_file, config) VALUES (?, ?, ?)")
            .bind(id)
            .bind(wasm_file)
            .bind(config.to_string())
            .execute(&self.db)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(db) if db.is_unique_violation() => {
                    ApiError::bad_request(format!("instance id `{id}` is already in use"))
                }
                other => other.into(),
            })?;
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        Ok(self.info(&row))
    }

    /// Delete an instance. Files that still reference it (as source or
    /// content source) must be deleted first (409, like metadata
    /// deletion).
    pub async fn unregister(&self, id: &str) -> Result<(), ApiError> {
        let files: Vec<String> =
            sqlx::query_scalar("SELECT id FROM book_files WHERE source = ? OR content_source = ?")
                .bind(id)
                .bind(id)
                .fetch_all(&self.db)
                .await?;
        if !files.is_empty() {
            return Err(ApiError::ConflictWithFiles(files));
        }
        let result = sqlx::query("DELETE FROM plugin_instances WHERE id = ?")
            .bind(id)
            .execute(&self.db)
            .await?;
        if result.rows_affected() == 0 {
            return Err(ApiError::not_found("plugin instance"));
        }
        Ok(())
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<InstanceInfo, ApiError> {
        let result = sqlx::query("UPDATE plugin_instances SET enabled = ? WHERE id = ?")
            .bind(enabled as i64)
            .bind(id)
            .execute(&self.db)
            .await?;
        if result.rows_affected() == 0 {
            return Err(ApiError::not_found("plugin instance"));
        }
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        Ok(self.info(&row))
    }

    /// Validate and store an instance's configuration.
    pub async fn set_config(
        &self,
        id: &str,
        config: serde_json::Value,
    ) -> Result<InstanceInfo, ApiError> {
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        let wasm = self.wasm_info(&row.wasm_file).ok_or_else(|| {
            ApiError::bad_request(format!("wasm file `{}` is not loaded", row.wasm_file))
        })?;
        let config = self.validate(&wasm, &config)?;
        sqlx::query("UPDATE plugin_instances SET config = ? WHERE id = ?")
            .bind(config.to_string())
            .bind(id)
            .execute(&self.db)
            .await?;
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        Ok(self.info(&row))
    }

    /// The config schema plus the current values, for the admin form.
    pub async fn config_schema(
        &self,
        id: &str,
    ) -> Result<(Vec<ConfigFieldView>, serde_json::Value), ApiError> {
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        let wasm = self.wasm_info(&row.wasm_file).ok_or_else(|| {
            ApiError::bad_request(format!("wasm file `{}` is not loaded", row.wasm_file))
        })?;
        let fields = wasm
            .schema
            .iter()
            .map(|f| ConfigFieldView {
                key: f.key.clone(),
                label: f.label.clone(),
                kind: match f.kind {
                    ConfigKind::Text => "string",
                    ConfigKind::Number => "number",
                    ConfigKind::Boolean => "boolean",
                    ConfigKind::EnumOptions(_) => "enum",
                    ConfigKind::ListOfString => "list-of-string",
                }
                .to_string(),
                options: match &f.kind {
                    ConfigKind::EnumOptions(options) => options.clone(),
                    _ => Vec::new(),
                },
                default: f.default.clone(),
                required: f.required,
                hint: f.hint.clone(),
            })
            .collect();
        Ok((fields, row.config))
    }

    // ---- capability-gated calls ------------------------------------------

    fn requires_cap(&self, wasm: &WasmInfo, cap: &str) -> Result<(), ApiError> {
        if wasm.capabilities.iter().any(|c| c == cap) {
            Ok(())
        } else {
            Err(ApiError::bad_request(format!(
                "plugin `{}` does not support `{cap}`",
                wasm.name
            )))
        }
    }

    /// Run `f` against an instance's wasm with its stored config injected.
    /// `require_enabled` gates sync/browse/materialize/identify; reading
    /// already-materialized chapters of a disabled instance stays allowed.
    ///
    /// The wasm call (and any `http.fetch` it makes) executes on a
    /// `spawn_blocking` thread, so a slow plugin/source never stalls a
    /// tokio worker.
    async fn call<T: Send + 'static>(
        &self,
        id: &str,
        require_enabled: bool,
        f: impl FnOnce(&WasmPlugin, &[ConfigValue]) -> Result<T, CoreError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        if require_enabled && !row.enabled {
            return Err(ApiError::bad_request(format!(
                "plugin instance `{id}` is disabled"
            )));
        }
        let wasm = self.wasm_info(&row.wasm_file).ok_or_else(|| {
            ApiError::bad_request(format!("wasm file `{}` is not loaded", row.wasm_file))
        })?;
        let values = values_from_config(&wasm.schema, &row.config);
        let plugin = wasm.wasm.clone();
        run_wasm(plugin, values, f).await
    }

    /// Declared catalogs of every enabled `declare`-capable instance
    /// (startup sync / `POST /api/plugins/sync`).
    pub async fn declared_catalogs(&self) -> Result<Vec<DeclaredCatalog>, ApiError> {
        let mut out = Vec::new();
        for row in self.rows().await? {
            if !row.enabled {
                continue;
            }
            let Some(wasm) = self.wasm_info(&row.wasm_file) else {
                continue;
            };
            if !wasm.capabilities.iter().any(|c| c == "declare") {
                continue;
            }
            self.requires_cap(&wasm, "declare")?;
            let values = values_from_config(&wasm.schema, &row.config);
            let declared = run_wasm(wasm.wasm.clone(), values, |wasm, values| {
                wasm.declare(values)
            })
            .await?
            .unwrap_or_default();
            out.push(DeclaredCatalog {
                instance: row.id,
                books: declared
                    .into_iter()
                    .map(|b| DeclaredCatalogBook {
                        book: SourceBook::from(b.book),
                        chapters: b
                            .chapters
                            .into_iter()
                            .map(|c| SourceChapter {
                                title: c.title,
                                content: c.content,
                            })
                            .collect(),
                    })
                    .collect(),
            });
        }
        Ok(out)
    }

    /// Declared catalog of one instance (`None` when it has no `declare`
    /// capability or is disabled).
    pub async fn declared_catalog(&self, id: &str) -> Result<Option<DeclaredCatalog>, ApiError> {
        for row in self.rows().await? {
            if row.id != id || !row.enabled {
                continue;
            }
            let Some(wasm) = self.wasm_info(&row.wasm_file) else {
                return Ok(None);
            };
            if !wasm.capabilities.iter().any(|c| c == "declare") {
                return Ok(None);
            }
            self.requires_cap(&wasm, "declare")?;
            let values = values_from_config(&wasm.schema, &row.config);
            let declared = run_wasm(wasm.wasm.clone(), values, |wasm, values| {
                wasm.declare(values)
            })
            .await?
            .unwrap_or_default();
            return Ok(Some(DeclaredCatalog {
                instance: row.id,
                books: declared
                    .into_iter()
                    .map(|b| DeclaredCatalogBook {
                        book: SourceBook::from(b.book),
                        chapters: b
                            .chapters
                            .into_iter()
                            .map(|c| SourceChapter {
                                title: c.title,
                                content: c.content,
                            })
                            .collect(),
                    })
                    .collect(),
            }));
        }
        Ok(None)
    }

    /// Paginated catalog search (requires the `search` capability).
    pub async fn search_books(
        &self,
        id: &str,
        query: &str,
        offset: u32,
        limit: u32,
    ) -> Result<SearchResult, ApiError> {
        let query = query.to_string();
        self.call(id, true, move |wasm, values| {
            wasm.search_books(values, &query, offset, limit)
        })
        .await
    }

    /// Single book by id (requires `lookup`).
    pub async fn get_book(&self, id: &str, book_id: &str) -> Result<Option<BookEntry>, ApiError> {
        let book_id = book_id.to_string();
        self.call(id, true, move |wasm, values| {
            wasm.get_book(values, &book_id)
        })
        .await
    }

    /// Chapter titles (requires `content`; allowed for disabled instances).
    pub async fn chapter_titles(&self, id: &str, book_id: &str) -> Result<Vec<String>, ApiError> {
        let book_id = book_id.to_string();
        self.call(id, false, move |wasm, values| {
            wasm.chapter_titles(values, &book_id)
        })
        .await
    }

    /// One chapter (requires `content`; allowed for disabled instances).
    pub async fn get_chapter(
        &self,
        id: &str,
        book_id: &str,
        index: u32,
    ) -> Result<Option<SourceChapter>, ApiError> {
        let book_id = book_id.to_string();
        self.call(id, false, move |wasm, values| {
            wasm.get_chapter(values, &book_id, index)
        })
        .await
    }

    /// Ask every enabled `identify`-capable instance in creation order;
    /// the first match supplies the upload's metadata.
    pub async fn identify_upload(
        &self,
        filename: &str,
        file_hash: &str,
    ) -> Result<Option<(String, SourceBook)>, ApiError> {
        for row in self.rows().await? {
            if !row.enabled {
                continue;
            }
            let Some(wasm) = self.wasm_info(&row.wasm_file) else {
                continue;
            };
            if !wasm.capabilities.iter().any(|c| c == "identify") {
                continue;
            }
            let values = values_from_config(&wasm.schema, &row.config);
            let filename = filename.to_string();
            let file_hash = file_hash.to_string();
            if let Some(book) = run_wasm(wasm.wasm.clone(), values, move |wasm, values| {
                wasm.identify_upload(values, &filename, &file_hash)
            })
            .await?
            {
                return Ok(Some((row.id, book)));
            }
        }
        Ok(None)
    }

    /// Resolve the effective content-target instance for a book entry:
    /// `content-source` ?? the entry's own instance, `content-id` ?? `id`.
    pub fn content_target<'a>(
        &self,
        instance: &'a str,
        entry: &'a SourceBook,
    ) -> (&'a str, &'a str) {
        (
            entry.content_source.as_deref().unwrap_or(instance),
            entry.content_id.as_deref().unwrap_or(&entry.id),
        )
    }

    /// Validate a content-target instance for rebinding: it must exist,
    /// and when it has the `lookup` capability the book must resolve via
    /// `get-book` (catches typos early).
    pub async fn check_content_target(&self, source: &str, book_id: &str) -> Result<(), ApiError> {
        let row = self
            .row(source)
            .await?
            .ok_or_else(|| ApiError::bad_request(format!("unknown plugin instance `{source}`")))?;
        let wasm = self.wasm_info(&row.wasm_file).ok_or_else(|| {
            ApiError::bad_request(format!("wasm file `{}` is not loaded", row.wasm_file))
        })?;
        if wasm.capabilities.iter().any(|c| c == "lookup") {
            let values = values_from_config(&wasm.schema, &row.config);
            let book_id = book_id.to_string();
            let book_id_2 = book_id.clone();
            let found = run_wasm(wasm.wasm.clone(), values, move |wasm, values| {
                wasm.get_book(values, &book_id)
            })
            .await?;
            if found.is_none() {
                return Err(ApiError::bad_request(format!(
                    "content source `{source}` does not offer book `{book_id_2}`"
                )));
            }
        }
        Ok(())
    }
}

impl From<ConfigErrors> for ApiError {
    fn from(e: ConfigErrors) -> Self {
        ApiError::ConfigErrors(e)
    }
}

/// Run a sync call into a compiled plugin on a blocking thread, so a slow
/// plugin (e.g. one waiting on `http.fetch`) never stalls a tokio worker.
/// The plugin's epoch deadline still traps runaway guest code; a fetch
/// blocked on the network outlives the deadline but never a worker.
async fn run_wasm<T: Send + 'static>(
    wasm: Arc<WasmPlugin>,
    values: Vec<ConfigValue>,
    f: impl FnOnce(&WasmPlugin, &[ConfigValue]) -> Result<T, CoreError> + Send + 'static,
) -> Result<T, ApiError> {
    let inner: Result<T, CoreError> = tokio::task::spawn_blocking(move || f(&wasm, &values))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("plugin call panicked: {e}")))?;
    inner.map_err(ApiError::from)
}
