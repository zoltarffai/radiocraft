//! RadioCraft application shell: eframe app, zone wiring, tray, settings paths.
//!
//! Mirrors the RadioDJ main window: the playout zones live in their own
//! window, and the main window carries the library browser, the playlist
//! and the transport buttons.  This binary just builds the state and hands
//! control to eframe.

use std::path::PathBuf;

use anyhow::{Context, Result};
use eframe::egui;

use radiocraft_app::app::RadioCraftApp;
use radiocraft_app::state::AppState;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config_dir = config_dir().context("figuring out the configuration directory")?;
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("creating {})", config_dir.display()))?;

    let state = AppState::load(&config_dir)
        .with_context(|| format!("loading settings from {}", config_dir.display()))?;
    tracing::info!("configuration loaded from {}", config_dir.display());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([800.0, 520.0])
            .with_title("RadioCraft"),
        ..eframe::NativeOptions::default()
    };

    let app = RadioCraftApp::new(state);

    eframe::run_native(
        "RadioCraft",
        options,
        Box::new(move |cc| {
            let app = app;

            #[cfg(feature = "tray")]
            {
                let icon = radiocraft_app::tray::placeholder_image();
                match radiocraft_app::tray::install(&cc.egui_ctx, icon) {
                    Ok(guard) => app.set_tray(guard),
                    Err(err) => tracing::warn!("tray icon unavailable: {err:#}"),
                }
            }

            #[cfg(not(feature = "tray"))]
            let _ = &cc;

            Ok(Box::new(app))
        }),
    )
    .map_err(|err| anyhow::anyhow!("running the eframe event loop: {err}"))?;
    Ok(())
}

/// Where settings live: `$XDG_CONFIG_HOME/radiocraft`, or `~/.config/radiocraft`.
fn config_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("RADIOCRAFT_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => {
            let home = std::env::var_os("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(base.join("radiocraft"))
}
