//! RadioCraft playout server binary.
//!
//! Usage: `radiocraft-server [config-dir]` (default: `~/.config/radiocraft`).
//! Listens on `127.0.0.1:8030` (override with `RADIOCRAFT_BIND`). When
//! `RADIOCRAFT_AUTH_TOKEN` is set the LAN can be opened safely; without it
//! the server stays loopback-only.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};

use radiocraft_server::{http, pump, state::App};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,radiocraft=debug".into()),
        )
        .with_target(false)
        .init();

    let config_dir = match std::env::args().nth(1) {
        Some(dir) => PathBuf::from(dir),
        None => default_config_dir(),
    };
    std::fs::create_dir_all(&config_dir).with_context(|| format!("{}", config_dir.display()))?;
    tracing::info!("config dir: {}", config_dir.display());

    let app = Arc::new(App::load(&config_dir).context("loading settings")?);
    app.set_status(radiocraft_server::state::Status::info("Server starting")).await;

    // Serve the built web UI when it exists next to the binary.
    let web_dist = std::env::var("RADIOCRAFT_WEB_DIST")
        .map(PathBuf::from)
        .or_else(|_| {
            let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web/dist");
            if candidate.exists() {
                Ok(candidate)
            } else {
                Err(std::env::VarError::NotPresent)
            }
        })
        .ok();
    if let Some(dist) = &web_dist {
        tracing::info!("serving web UI from {}", dist.display());
    }

    let auth = Arc::new(radiocraft_server::auth::Auth::from_env());
    if auth.required {
        tracing::info!("token auth is ON (RADIOCRAFT_AUTH_TOKEN set)");
    }

    pump::spawn_pump(Arc::clone(&Arc::clone(&app)));

    let bind = std::env::var("RADIOCRAFT_BIND").unwrap_or_else(|_| "127.0.0.1:8030".to_string());
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!("RadioCraft listening on http://{bind} (ws at /ws)");

    let router = http::router(Arc::clone(&app), auth, web_dist);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(Arc::clone(&app)))
        .await
        .context("serving")?;
    Ok(())
}

/// Default config dir: `$XDG_CONFIG_HOME/radiocraft` or `~/.config/radiocraft`.
fn default_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("radiocraft");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("radiocraft")
}

/// Wait for SIGINT/SIGTERM, then book the still-rolling item before exiting.
async fn shutdown_signal(app: Arc<App>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("SIGINT received"),
        () = terminate => tracing::info!("SIGTERM received"),
    }

    let taken = app.take_now_for_booking();
    if let Some(playing) = taken {
        app.book_finished(&playing).await;
    }
}
