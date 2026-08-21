//! Bookshelf server entry point.

mod auth;
mod config;
mod db;
mod error;
mod rows;
mod routes;
mod service;
mod state;

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use bookshelf_core::source::BookSource;
use bookshelf_plugin::{load_dir, PluginManager};
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,bookshelf=debug")),
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
        tokio::fs::create_dir_all(&cfg.plugins_dir).await
            .with_context(|| format!("create plugins dir {}", cfg.plugins_dir.display()))?;
    }

    let pool = db::connect(&cfg).await?;
    db::seed_local_user(&pool, cfg.auth_enabled).await?;

    // Load wasm plugins (the wasmtime engine needs no async runtime).
    let manager: PluginManager = load_dir(&cfg.plugins_dir)?;
    let sources: Vec<Arc<dyn BookSource>> = manager
        .plugins()
        .iter()
        .map(|p| p.clone() as Arc<dyn BookSource>)
        .collect();
    tracing::info!(plugins = sources.len(), "loaded plugin sources");

    let library = service::Library::new(pool.clone(), sources.clone());
    let synced = library.sync_plugins().await?;
    tracing::info!(synced, "plugin catalog synced");

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