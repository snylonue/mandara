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
//!      injected config (R3) and fuel-metered execution,
//!   4. enforces capability policy (`capabilities()` decides which exports
//!      may be called).
//!
//! All durable state lives in SQLite under the host's control.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;

use mandara_core::source::{SourceBook, SourceBookFile, SourceChapter, SourceDeclaredContent};
use mandara_plugin::error::Error as PluginError;
use mandara_plugin::{
    BookEntry, CompiledPlugin, ConfigErrors, ConfigField, ConfigKind, PluginInstance,
    PluginRuntime, SearchResult, SourceInfo, validate_config,
};

use crate::error::ApiError;

/// Plugin information read in a registered instance context.
struct PluginInfo {
    instance: PluginInstance,
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
    pub content: Option<SourceDeclaredContent>,
}

pub struct DeclaredCatalog {
    pub instance: String,
    pub books: Vec<DeclaredCatalogBook>,
}

/// Compiled wasm files + the instance registry.
pub struct PluginService {
    db: crate::db::DieselDb,
    runtime: Arc<PluginRuntime>,
    wasm_by_file: HashMap<String, Arc<CompiledPlugin>>,
    instances: tokio::sync::RwLock<HashMap<String, PluginInstance>>,
}

impl PluginService {
    /// Restore registered instances without executing any guest exports.
    pub async fn new(
        db: crate::db::DieselDb,
        runtime: Arc<PluginRuntime>,
        wasms: Vec<Arc<CompiledPlugin>>,
    ) -> Result<Self, ApiError> {
        let wasm_by_file = wasms
            .into_iter()
            .map(|wasm| (wasm.file().to_string(), wasm))
            .collect();
        let service = Self {
            db,
            runtime,
            wasm_by_file,
            instances: Default::default(),
        };
        for row in service.rows().await? {
            if service.wasm_by_file.contains_key(&row.wasm_file) {
                let instance = service.bind_instance(&row)?;
                service.instances.write().await.insert(row.id, instance);
            }
        }
        Ok(service)
    }

    fn bind_instance(&self, row: &InstanceRow) -> Result<PluginInstance, ApiError> {
        let wasm = self.wasm(&row.wasm_file).ok_or_else(|| {
            ApiError::bad_request(format!("wasm file '{}' is not loaded", row.wasm_file))
        })?;
        Ok(self.runtime.instance(
            row.id.clone(),
            wasm,
            row.config.clone(),
            super::plugin_state::DbPluginState::new(self.db.clone(), row.id.clone()),
        )?)
    }

    async fn instance(&self, row: &InstanceRow) -> Result<PluginInstance, ApiError> {
        self.instances
            .read()
            .await
            .get(&row.id)
            .cloned()
            .ok_or_else(|| {
                ApiError::bad_request(format!("wasm file '{}' is not loaded", row.wasm_file))
            })
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

    fn wasm(&self, wasm_file: &str) -> Option<Arc<CompiledPlugin>> {
        self.wasm_by_file.get(wasm_file).cloned()
    }

    async fn plugin_info(&self, row: &InstanceRow) -> Result<PluginInfo, ApiError> {
        let instance = self.instance(row).await?;
        run_instance(self.runtime.clone(), instance, |runtime, instance| {
            Ok(PluginInfo {
                name: runtime.name(&instance)?,
                capabilities: runtime.capabilities(&instance)?,
                schema: runtime.config_schema(&instance)?,
                source_info: runtime.source_info(&instance)?,
                instance,
            })
        })
        .await
    }

    async fn info(&self, row: &InstanceRow) -> InstanceInfo {
        let wasm = match self.plugin_info(row).await {
            Ok(info) => Some(info),
            Err(error) => {
                tracing::warn!(instance = %row.id, %error, "could not read plugin information");
                None
            }
        };
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
        let mut out = Vec::new();
        for row in self.rows().await? {
            out.push(self.info(&row).await);
        }
        Ok(out)
    }

    // ---- instance management (admin) -------------------------------------

    fn validate(
        &self,
        wasm: &PluginInfo,
        input: &serde_json::Value,
    ) -> Result<serde_json::Value, ApiError> {
        Ok(validate_config(&wasm.schema, input)?)
    }

    /// Register an instance before executing plugin code. Configuration is
    /// empty until saved through the configuration endpoint.
    pub async fn register(&self, id: &str, wasm_file: &str) -> Result<InstanceInfo, ApiError> {
        self.wasm(wasm_file).ok_or_else(|| {
            ApiError::bad_request(format!("no wasm file `{wasm_file}` is loaded"))
        })?;
        let mut instances = self.instances.write().await;
        let mut conn = self.db.get().await?;
        use diesel::ExpressionMethods as _;
        use diesel_async::RunQueryDsl as _;
        let result = diesel::insert_into(crate::schema::plugin_instances::table)
            .values((
                crate::schema::plugin_instances::id.eq(id),
                crate::schema::plugin_instances::created_at.eq(chrono::Utc::now()),
                crate::schema::plugin_instances::wasm_file.eq(wasm_file),
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
        drop(conn);
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        let instance = self.bind_instance(&row)?;
        instances.insert(row.id.clone(), instance);
        drop(instances);
        Ok(self.info(&row).await)
    }

    /// Delete an instance only after removing its content items (409 otherwise).
    pub async fn unregister(&self, id: &str) -> Result<(), ApiError> {
        use diesel::ExpressionMethods as _;
        use diesel::QueryDsl as _;
        use diesel_async::RunQueryDsl as _;

        let mut instances = self.instances.write().await;
        let mut conn = self.db.get().await?;
        let files: Vec<String> = crate::schema::book_files::table
            .filter(crate::schema::book_files::source.eq(id))
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
        instances.remove(id);
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
        drop(conn);
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        Ok(self.info(&row).await)
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
        let wasm = self.plugin_info(&row).await?;
        let config = self.validate(&wasm, &config)?;
        use diesel::ExpressionMethods as _;
        use diesel::QueryDsl as _;
        use diesel_async::RunQueryDsl as _;
        let mut instances = self.instances.write().await;
        let current = instances
            .get(id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        if !wasm.instance.same_registration(&current) {
            return Err(ApiError::bad_request(
                "plugin instance changed while saving configuration",
            ));
        }
        diesel::update(crate::schema::plugin_instances::table.find(id))
            .set(crate::schema::plugin_instances::config.eq(config.to_string()))
            .execute(&mut self.db.get().await?)
            .await?;
        instances.insert(id.to_string(), current.with_config(config));
        drop(instances);
        let row = self
            .row(id)
            .await?
            .ok_or_else(|| ApiError::not_found("plugin instance"))?;
        Ok(self.info(&row).await)
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
        let wasm = self.plugin_info(&row).await?;
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

    fn requires_cap(&self, wasm: &PluginInfo, cap: &str) -> Result<(), ApiError> {
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
        f: impl FnOnce(Arc<PluginRuntime>, PluginInstance) -> Result<T, PluginError> + Send + 'static,
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
        run_instance(self.runtime.clone(), self.instance(&row).await?, f).await
    }

    /// Declared catalogs of every enabled `declare`-capable instance
    /// (startup sync / `POST /api/plugins/sync`).
    pub async fn declared_catalogs(&self) -> Result<Vec<DeclaredCatalog>, ApiError> {
        let mut out = Vec::new();
        for row in self.rows().await? {
            if !row.enabled {
                continue;
            }
            let Some(_) = self.wasm(&row.wasm_file) else {
                continue;
            };
            let wasm = self.plugin_info(&row).await?;
            if !wasm.capabilities.iter().any(|c| c == "declare") {
                continue;
            }
            self.requires_cap(&wasm, "declare")?;
            let declared =
                run_instance(self.runtime.clone(), wasm.instance, |runtime, instance| {
                    runtime.declare(&instance)
                })
                .await?
                .unwrap_or_default();
            out.push(DeclaredCatalog {
                instance: row.id,
                books: declared
                    .into_iter()
                    .map(|b| -> Result<_, ApiError> {
                        let book = SourceBook::try_from(b.book)?;
                        Ok(DeclaredCatalogBook {
                            book,
                            content: b.content.map(Into::into),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
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
            let Some(_) = self.wasm(&row.wasm_file) else {
                return Ok(None);
            };
            let wasm = self.plugin_info(&row).await?;
            if !wasm.capabilities.iter().any(|c| c == "declare") {
                return Ok(None);
            }
            self.requires_cap(&wasm, "declare")?;
            let declared =
                run_instance(self.runtime.clone(), wasm.instance, |runtime, instance| {
                    runtime.declare(&instance)
                })
                .await?
                .unwrap_or_default();
            return Ok(Some(DeclaredCatalog {
                instance: row.id,
                books: declared
                    .into_iter()
                    .map(|b| -> Result<_, ApiError> {
                        let book = SourceBook::try_from(b.book)?;
                        Ok(DeclaredCatalogBook {
                            book,
                            content: b.content.map(Into::into),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
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
        self.call(id, true, move |runtime, instance| {
            runtime.search_books(&instance, &query, offset, limit)
        })
        .await
    }

    /// Single book by id (requires `lookup`).
    pub async fn get_book(&self, id: &str, book_id: &str) -> Result<Option<BookEntry>, ApiError> {
        let book_id = book_id.to_string();
        self.call(id, true, move |runtime, instance| {
            runtime.get_book(&instance, &book_id)
        })
        .await
    }

    /// Chapter titles (requires `content`; allowed for disabled instances).
    pub async fn get_series(
        &self,
        id: &str,
        series_id: &str,
    ) -> Result<Option<mandara_plugin::SeriesResult>, ApiError> {
        let series_id = series_id.to_string();
        self.call(id, true, move |runtime, instance| {
            runtime.get_series(&instance, &series_id)
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
        self.call(id, false, move |runtime, instance| {
            runtime.get_chapter(&instance, &book_id, index)
        })
        .await
    }

    /// Fetch one complete content declaration, independently of book metadata.
    pub async fn get_content(
        &self,
        id: &str,
        content_id: &str,
        require_enabled: bool,
    ) -> Result<Option<SourceDeclaredContent>, ApiError> {
        let content_id = content_id.to_string();
        self.call(id, require_enabled, move |runtime, instance| {
            runtime.get_content(&instance, &content_id)
        })
        .await
    }

    /// Execute the plugin's file provider without host-owned fallback policy.
    pub async fn get_book_file(
        &self,
        id: &str,
        book_id: &str,
    ) -> Result<Option<SourceBookFile>, ApiError> {
        let book_id = book_id.to_string();
        self.call(id, false, move |runtime, instance| {
            runtime.get_book_file(&instance, &book_id)
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
            let Some(_) = self.wasm(&row.wasm_file) else {
                continue;
            };
            let wasm = self.plugin_info(&row).await?;
            if !wasm.capabilities.iter().any(|c| c == "identify") {
                continue;
            }
            let filename = filename.to_string();
            let file_hash = file_hash.to_string();
            if let Some(book) = run_instance(
                self.runtime.clone(),
                wasm.instance,
                move |runtime, instance| runtime.identify_upload(&instance, &filename, &file_hash),
            )
            .await?
            {
                return Ok(Some((row.id, book)));
            }
        }
        Ok(None)
    }
}

impl From<ConfigErrors> for ApiError {
    fn from(e: ConfigErrors) -> Self {
        ApiError::ConfigErrors(e)
    }
}

/// Run a sync call into a compiled plugin on a blocking thread, so a slow
/// plugin (e.g. one waiting on `http.fetch`) never stalls a tokio worker.
/// Guest computation is fuel-metered; each HTTP request has its own timeout.
/// There is no aggregate wall-clock deadline for a plugin call.
async fn run_instance<T: Send + 'static>(
    runtime: Arc<PluginRuntime>,
    instance: PluginInstance,
    f: impl FnOnce(Arc<PluginRuntime>, PluginInstance) -> Result<T, PluginError> + Send + 'static,
) -> Result<T, ApiError> {
    Ok(tokio::task::spawn_blocking(move || f(runtime, instance)).await??)
}
