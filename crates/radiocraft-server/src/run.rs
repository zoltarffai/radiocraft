//! Shared server bootstrap: web-dist resolution and a non-blocking `serve`
//! used by both the headless binary (which keeps its own graceful shutdown)
//! and the Tauri window shell (which owns the process lifetime).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};

use crate::{auth::Auth, http, state::App};

/// Resolve the built web UI directory: the `RADIOCRAFT_WEB_DIST` override if
/// set, otherwise the first `fallbacks` path that exists on disk.
pub fn resolve_web_dist(fallbacks: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    match std::env::var("RADIOCRAFT_WEB_DIST") {
        Ok(v) => Some(PathBuf::from(v)),
        Err(_) => fallbacks.into_iter().find(|p| p.exists()),
    }
}

/// Default config dir: `$XDG_CONFIG_HOME/radiocraft` or `~/.config/radiocraft`.
pub fn default_config_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{home}/.config")
    });
    PathBuf::from(base).join("radiocraft")
}

/// Bind `bind` (use port `0` for an OS-assigned port) and serve the router
/// on a spawned task. Returns the actual bound port.
pub async fn serve(
    app: Arc<App>,
    auth: Arc<Auth>,
    web_dist: Option<PathBuf>,
    bind: &str,
) -> Result<u16> {
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    let port = listener.local_addr().context("listener address")?.port();
    let router = http::router(app, auth, web_dist);
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, router).await {
            tracing::error!("server error: {err}");
        }
    });
    Ok(port)
}
