//! RadioCraft application shell: eframe app, zone wiring, tray, settings paths.
//!
//! Mirrors the RadioDJ main window: 4 playback zones (main / aux / cart+instant /
//! plugin zone), playlist view, library browse, status now-playing. This binary
//! composes radiocraft-{core,db,audio,settings,net,plugin}.

mod app;

fn main() -> anyhow::Result<()> {
    env_logger_like_init();
    tracing_subscriber::fmt::init();
    let native = eframe::NativeOptions::default();
    eframe::run_native(
        "RadioCraft",
        native,
        Box::new(|_cc| Ok(Box::new(app::RadioCraftApp::default()))),
    )?;
    Ok(())
}

fn env_logger_like_init() {}
