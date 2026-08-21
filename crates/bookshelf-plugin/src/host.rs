//! The wasmtime component host.
//!
//! Each `*.wasm` file in the plugins directory is loaded once at startup.
//! Calls into a plugin are synchronous (a plugin is a pure, short-lived
//! computation); a fresh [`wasmtime::Store`] is created per call so plugin
//! instances never share state with each other.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bookshelf_core::error::{Error, Result};
use bookshelf_core::source::{BookSource, SourceBook, SourceChapter};
use tracing::{info, warn};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

// Generates the bindings for the `bookshelf:plugin/bookshelf-plugin` world
// from `wit/bookshelf.wit`:
//   - `store::Host` trait for the imported `store` interface
//   - `BookshelfPlugin` world handle with `add_to_linker` / `instantiate`
//     and `call_name`, `call_list_books`, `call_chapter_titles`,
//     `call_get_chapter` for the world exports.
wasmtime::component::bindgen!({
    path: "wit",
    world: "bookshelf:plugin/bookshelf-plugin",
});

/// Host-side state passed to plugin imports.
#[derive(Default)]
struct HostState {
    name: String,
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

/// One loaded wasm component, exposed as a [`BookSource`].
pub struct WasmPlugin {
    id: String,
    engine: Engine,
    component: Component,
    linker: Linker<HostState>,
}

impl WasmPlugin {
    /// Load a component from raw wasm bytes. `id` identifies the source in
    /// the `books.source` column.
    pub fn load(id: String, wasm: Vec<u8>) -> anyhow::Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config)?;
        let component = Component::new(&engine, &wasm)
            .map_err(|e| anyhow::anyhow!("`{id}` is not a valid wasm component: {e}"))?;
        let mut linker = Linker::new(&engine);
        BookshelfPlugin::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut HostState| {
            state
        })?;
        Ok(Self {
            id,
            engine,
            component,
            linker,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Instantiate the component in a fresh store and run `f` against the
    /// typed bindings.
    fn call<T>(
        &self,
        f: impl FnOnce(&mut Store<HostState>, &BookshelfPlugin) -> wasmtime::Result<T>,
    ) -> Result<T> {
        let mut store = Store::new(&self.engine, HostState { name: self.id.clone() });
        let bindings = BookshelfPlugin::instantiate(&mut store, &self.component, &self.linker)
            .map_err(|e| Error::Plugin(format!("instantiate `{}`: {e}", self.id)))?;
        f(&mut store, &bindings)
            .map_err(|e| Error::Plugin(format!("call into `{}`: {e}", self.id)))
    }
}

#[async_trait]
impl BookSource for WasmPlugin {
    fn id(&self) -> &str {
        &self.id
    }

    async fn list_books(&self) -> Result<Vec<SourceBook>> {
        let books = self.call(|store, b| b.call_list_books(store))?;
        Ok(books
            .into_iter()
            .map(|b| SourceBook {
                id: b.id,
                title: b.title,
                authors: b.authors,
                description: b.description,
                cover_url: b.cover_url,
            })
            .collect())
    }

    async fn chapter_titles(&self, book_id: &str) -> Result<Vec<String>> {
        self.call(|store, b| b.call_chapter_titles(store, book_id))
    }

    async fn get_chapter(&self, book_id: &str, index: u32) -> Result<Option<SourceChapter>> {
        let chapter = self.call(|store, b| b.call_get_chapter(store, book_id, index))?;
        Ok(chapter.map(|c| SourceChapter {
            title: c.title,
            content: c.content,
        }))
    }
}

/// Loads and holds all plugins found in a directory.
#[derive(Default)]
pub struct PluginManager {
    plugins: Vec<Arc<WasmPlugin>>,
}

/// Load every `*.wasm` component in `dir`. Invalid files are skipped with a
/// warning so a bad plugin never prevents the server from starting.
pub fn load_dir(dir: &Path) -> anyhow::Result<PluginManager> {
    let mut manager = PluginManager::default();
    if !dir.is_dir() {
        return Ok(manager);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin")
            .to_string();
        let wasm = std::fs::read(&path)?;
        match WasmPlugin::load(id.clone(), wasm) {
            Ok(plugin) => {
                info!(plugin = %id, "loaded wasm plugin");
                manager.plugins.push(Arc::new(plugin));
            }
            Err(e) => {
                warn!(plugin = %id, "skipping plugin: {e:#}");
            }
        }
    }
    Ok(manager)
}

impl PluginManager {
    pub fn plugins(&self) -> &[Arc<WasmPlugin>] {
        &self.plugins
    }

    pub fn get(&self, id: &str) -> Option<Arc<WasmPlugin>> {
        self.plugins.iter().find(|p| p.id() == id).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}