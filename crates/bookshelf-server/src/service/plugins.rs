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

use bookshelf_core::error::Error as CoreError;
use bookshelf_core::source::{SourceBook, SourceBookFile, SourceChapter};
use bookshelf_plugin::{
    BookEntry, ConfigErrors, ConfigField, ConfigKind, ConfigValue, SearchResult, SourceInfo,
    WasmPlugin, validate_config, values_from_config,
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
    /// Interaction hints for the frontend (`source-info` export).
    source_info: SourceInfo,
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
    /// Frontend interaction hints (`source-info`), serialized for the UI.
    pub source_info: SourceInfoView,
}

/// A serializable view of the WIT `source-info` record.
#[derive(Debug, Clone, Serialize)]
pub struct SourceInfoView {
    /// `"search"` | `"manual-id"` | `"browse"` | `""` = capability default.
    pub kind: String,
    /// `"numeric"` | `"slug"` | `"uuid"` | `"free"` …
    pub id_kind: String,
    /// zh-CN hint for the manual id entry box.
    pub id_hint: Option<String>,
    /// zh-CN placeholder for the search box.
    pub search_hint: Option<String>,
}

impl From<&SourceInfo> for SourceInfoView {
    fn from(s: &SourceInfo) -> Self {
        SourceInfoView {
            kind: s.kind.clone(),
            id_kind: s.id_kind.clone(),
            id_hint: s.id_hint.clone(),
            search_hint: s.search_hint.clone(),
        }
    }
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
    db: crate::db::DieselDb,
    wasm_by_file: HashMap<String, Arc<WasmInfo>>,
}

impl PluginService {
    /// Compile all wasm files and cache per-file facts. A file whose
    /// `name`/`capabilities`/`config-schema` cannot be fetched is kept
    /// with empty capabilities (its exports can never be reached through
    /// the capability gates) and a warning.
    pub fn new(db: crate::db::DieselDb, wasms: Vec<Arc<WasmPlugin>>) -> Self {
        let mut wasm_by_file = HashMap::new();
        for wasm in wasms {
            let (name, capabilities, schema, source_info) = match (
                wasm.name(),
                wasm.capabilities(),
                wasm.config_schema(),
                wasm.source_info(),
            ) {
                (Ok(name), Ok(capabilities), Ok(schema), Ok(source_info)) => {
                    (name, capabilities, schema, source_info)
                }
                (name, capabilities, schema, source_info) => {
                    tracing::warn!(
                        file = wasm.file(),
                        name_err = name.is_err(),
                        caps_err = capabilities.is_err(),
                        schema_err = schema.is_err(),
                        srcinfo_err = source_info.is_err(),
                        "could not introspect plugin; treating it as capability-less"
                    );
                    (
                        name.unwrap_or_else(|_| wasm.file().to_string()),
                        capabilities.unwrap_or_default(),
                        schema.unwrap_or_default(),
                        source_info.unwrap_or_else(|_| SourceInfo {
                            kind: String::new(),
                            id_kind: String::new(),
                            id_hint: None,
                            search_hint: None,
                        }),
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
                    source_info,
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
        use diesel::ExpressionMethods as _;
        use diesel::QueryDsl as _;
        use diesel_async::RunQueryDsl as _;

        let mut conn = self.db.get().await?;
        let rows: Vec<(String, String, String, i64)> = crate::schema::plugin_instances::table
            .order(crate::schema::plugin_instances::created_at.asc())
            .select((
                crate::schema::plugin_instances::id,
                crate::schema::plugin_instances::wasm_file,
                crate::schema::plugin_instances::config,
                crate::schema::plugin_instances::enabled,
            ))
            .load(&mut conn)
            .await?;
        rows.into_iter()
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
        use diesel::OptionalExtension as _;
        use diesel::QueryDsl as _;
        use diesel_async::RunQueryDsl as _;

        let mut conn = self.db.get().await?;
        let row: Option<(String, String, String, i64)> = crate::schema::plugin_instances::table
            .find(id)
            .select((
                crate::schema::plugin_instances::id,
                crate::schema::plugin_instances::wasm_file,
                crate::schema::plugin_instances::config,
                crate::schema::plugin_instances::enabled,
            ))
            .first(&mut conn)
            .await
            .optional()?;
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
            capabilities: wasm
                .as_ref()
                .map(|w| w.capabilities.clone())
                .unwrap_or_default(),
            source_info: wasm
                .as_ref()
                .map(|w| SourceInfoView::from(&w.source_info))
                .unwrap_or_else(|| SourceInfoView {
                    kind: String::new(),
                    id_kind: String::new(),
                    id_hint: None,
                    search_hint: None,
                }),
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
        let mut conn = self.db.get().await?;
        use diesel::ExpressionMethods as _;
        use diesel_async::RunQueryDsl as _;
        let result = diesel::insert_into(crate::schema::plugin_instances::table)
            .values((
                crate::schema::plugin_instances::id.eq(id),
                crate::schema::plugin_instances::wasm_file.eq(wasm_file),
                crate::schema::plugin_instances::config.eq(config.to_string()),
            ))
            .execute(&mut conn)
            .await;
        if matches!(
            &result,
            Err(diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _
            ))
        ) {
            return Err(ApiError::bad_request(format!(
                "instance id `{id}` is already in use"
            )));
        }
        result?;
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
        use diesel::BoolExpressionMethods as _;
        use diesel::ExpressionMethods as _;
        use diesel::QueryDsl as _;
        use diesel_async::RunQueryDsl as _;

        let mut conn = self.db.get().await?;
        let files: Vec<String> = crate::schema::book_files::table
            .filter(
                crate::schema::book_files::source
                    .eq(id)
                    .or(crate::schema::book_files::content_source.eq(id)),
            )
            .select(crate::schema::book_files::id)
            .load(&mut conn)
            .await?;
        if !files.is_empty() {
            return Err(ApiError::ConflictWithFiles(files));
        }
        let result = diesel::delete(crate::schema::plugin_instances::table.find(id))
            .execute(&mut conn)
            .await?;
        if result == 0 {
            return Err(ApiError::not_found("plugin instance"));
        }
        Ok(())
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<InstanceInfo, ApiError> {
        use diesel::ExpressionMethods as _;
        use diesel::QueryDsl as _;
        use diesel_async::RunQueryDsl as _;

        let mut conn = self.db.get().await?;
        let result = diesel::update(crate::schema::plugin_instances::table.find(id))
            .set(crate::schema::plugin_instances::enabled.eq(enabled as i64))
            .execute(&mut conn)
            .await?;
        if result == 0 {
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
        use diesel::ExpressionMethods as _;
        use diesel::QueryDsl as _;
        use diesel_async::RunQueryDsl as _;
        diesel::update(crate::schema::plugin_instances::table.find(id))
            .set(crate::schema::plugin_instances::config.eq(config.to_string()))
            .execute(&mut self.db.get().await?)
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

    /// Does the instance's wasm declare `cap`? (Used to pick acquisition
    /// modes before calling into the plugin, e.g. file mode vs chapter
    /// mode.)
    pub async fn declares(&self, id: &str, cap: &str) -> Result<bool, ApiError> {
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        let wasm = self.wasm_info(&row.wasm_file).ok_or_else(|| {
            ApiError::bad_request(format!("wasm file `{}` is not loaded", row.wasm_file))
        })?;
        Ok(wasm.capabilities.iter().any(|c| c == cap))
    }

    /// Whole-book file (`book-file` capability, file mode). Per the
    /// design, plugin errors are logged and treated as "no file" — the
    /// caller falls back to chapter mode.
    pub async fn get_book_file(
        &self,
        id: &str,
        book_id: &str,
    ) -> Result<Option<SourceBookFile>, ApiError> {
        let book_id = book_id.to_string();
        let result = self
            .call(id, true, {
                let book_id = book_id.clone();
                move |wasm, values| wasm.get_book_file(values, &book_id)
            })
            .await;
        match result {
            Ok(file) => Ok(file),
            Err(e) => {
                tracing::warn!(
                    source = %id,
                    book = %book_id,
                    error = %e,
                    "get-book-file failed; falling back to chapter mode"
                );
                Ok(None)
            }
        }
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
