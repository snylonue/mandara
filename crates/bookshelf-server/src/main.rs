//! Bookshelf server entry point.

mod auth;
mod config;
mod db;
mod error;
mod routes;
mod rows;
/// Diesel schema (generated from the migrations; see `just schema`).
#[allow(dead_code, unused_imports)]
mod schema;
mod service;
mod state;
mod time;

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use bookshelf_plugin::{FetchPolicy, load_dir};
use service::plugins::PluginService;
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,bookshelf=debug")),
        )
        .init();

    let cfg = config::Config::parse();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), addr = %cfg.addr, "starting bookshelf-server");

    let data_dir = cfg.data_dir();
    tokio::fs::create_dir_all(&data_dir).await?;
    if !cfg.plugins_dir.exists() {
        tokio::fs::create_dir_all(&cfg.plugins_dir)
            .await
            .with_context(|| format!("create plugins dir {}", cfg.plugins_dir.display()))?;
    }

    db::run_migrations(&cfg.db).await?;
    let diesel_db = db::connect_diesel(&cfg.db)?;

    // One-shot upgrade migration (`--reparse-originals`): re-parse every
    // retained original with the current parser, then exit. Runs before
    // plugin loading — it needs neither wasm nor the HTTP stack.
    if cfg.reparse_originals {
        let files_dir = data_dir.join("files");
        let (ok, failed) = service::library::reparse_originals(&diesel_db, &files_dir).await?;
        tracing::info!(reparsed = ok, failed, "reparse-originals finished");
        return Ok(());
    }

    // Operational caps of every plugin's `fetch` import
    // (BOOKSHELF_PLUGIN_FETCH_* env vars). Plugins get plain network
    // access; calls run on blocking threads, so a slow source never
    // stalls a worker.
    let fetch_policy = Arc::new(FetchPolicy::from_env());
    tracing::info!(
        timeout_ms = fetch_policy.timeout_ms,
        max_bytes = fetch_policy.max_bytes,
        "plugin fetch caps"
    );

    // Load wasm plugins (the wasmtime engine needs no async runtime).
    // The files dir backs the content-addressed image store that serves
    // the plugins' `store-image` import.
    let files_dir = data_dir.join("files");
    tokio::fs::create_dir_all(&files_dir).await?;
    // Backfill intrinsic dimensions for images stored before they were
    // sniffed (migration 0011): lets the read path annotate stored img
    // tags with width/height so image loads stop re-anchoring the view.
    match service::library::backfill_image_dimensions(&diesel_db, &files_dir).await {
        Ok(n) if n > 0 => tracing::info!(backfilled = n, "image dimensions backfilled"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "image dimension backfill failed"),
    }
    let image_store = service::images::DbImageStore::new(diesel_db.clone(), files_dir.clone());
    let wasms = load_dir(&cfg.plugins_dir, fetch_policy, image_store)?;
    tracing::info!(files = wasms.len(), "loaded wasm plugin files");

    let plugins = Arc::new(PluginService::new(diesel_db.clone(), wasms));
    let library = service::Library::new(diesel_db.clone(), plugins, files_dir);
    // Startup sync materializes the catalog of every enabled
    // `declare`-capable instance; search/lookup-only instances stay lazy.
    match library.sync_plugins().await {
        Ok(synced) => tracing::info!(synced, "plugin catalog synced"),
        Err(e) => tracing::warn!(error = %e, "startup plugin sync failed"),
    }

    let auth = auth::AuthService::new(cfg.allow_register, &cfg.jwt_secret);
    let state = Arc::new(AppState {
        cfg: cfg.clone(),
        diesel_db,
        auth,
        library,
    });

    let app = routes::router(state.clone());
    let listener = TcpListener::bind(&cfg.addr)
        .await
        .with_context(|| format!("bind {}", cfg.addr))?;
    tracing::info!("listening on http://{}", cfg.addr);
    axum::serve(listener, app).await?;
    Ok(())
}
