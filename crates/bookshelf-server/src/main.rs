//! Bookshelf server entry point.

mod auth;
mod config;
mod db;
mod error;
mod routes;
mod rows;
mod service;
mod state;

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
    if !cfg.auth_enabled {
        tracing::warn!(
            "authentication is DISABLED: running as a single local admin (BOOKSHELF_AUTH_ENABLED=false)"
        );
    }

    let data_dir = cfg.data_dir();
    tokio::fs::create_dir_all(&data_dir).await?;
    if !cfg.plugins_dir.exists() {
        tokio::fs::create_dir_all(&cfg.plugins_dir)
            .await
            .with_context(|| format!("create plugins dir {}", cfg.plugins_dir.display()))?;
    }

    let pool = db::connect(&cfg).await?;
    db::seed_local_user(&pool, cfg.auth_enabled).await?;

    // Outbound HTTP policy of every plugin's `fetch` import
    // (BOOKSHELF_PLUGIN_FETCH_* env vars; empty allow list = all fetches
    // denied). Plugin calls run on blocking threads, so a slow source
    // never stalls a worker.
    let fetch_policy = Arc::new(FetchPolicy::from_env());
    if fetch_policy.allowed_hosts.is_empty() {
        tracing::warn!(
            "plugin fetch allow list is EMPTY: every plugin http.fetch call will be denied \
             (set BOOKSHELF_PLUGIN_FETCH_ALLOWED_HOSTS)"
        );
    } else {
        tracing::info!(
            hosts = ?fetch_policy.allowed_hosts,
            timeout_ms = fetch_policy.timeout_ms,
            max_bytes = fetch_policy.max_bytes,
            "plugin fetch policy"
        );
    }

    // Load wasm plugins (the wasmtime engine needs no async runtime).
    let wasms = load_dir(&cfg.plugins_dir, fetch_policy)?;
    tracing::info!(files = wasms.len(), "loaded wasm plugin files");

    let plugins = Arc::new(PluginService::new(pool.clone(), wasms));
    let files_dir = data_dir.join("files");
    tokio::fs::create_dir_all(&files_dir).await?;
    let library = service::Library::new(pool.clone(), plugins, files_dir);
    // Startup sync materializes the catalog of every enabled
    // `declare`-capable instance; search/lookup-only instances stay lazy.
    match library.sync_plugins().await {
        Ok(synced) => tracing::info!(synced, "plugin catalog synced"),
        Err(e) => tracing::warn!(error = %e, "startup plugin sync failed"),
    }

    let auth = auth::AuthService::new(cfg.auth_enabled, cfg.allow_register, &cfg.jwt_secret);
    let state = Arc::new(AppState {
        cfg: cfg.clone(),
        db: pool,
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
