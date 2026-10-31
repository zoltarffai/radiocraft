//! The single pump task: engine events and live meters fan out to the
//! broadcast hub from here (docs/05: "server pushes, clients render").
//!
//! Engine events (`Finished`/`SegueStarted`) drive now-playing bookkeeping;
//! everything else is polled at ~20 Hz and cheap to compare.

use std::sync::Arc;
use std::time::{Duration, Instant};

use radiocraft_audio::mixer::Zone;
use radiocraft_audio::player::PlayerEvent;

use crate::state::{App, MAIN_ZONE};

/// 20 Hz meter tick.
pub const PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// Spawn the pump on the current runtime; it runs until the app is dropped.
pub fn spawn_pump(app: Arc<App>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PUMP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // AutoDJ retry throttle so a permanently empty pool never hammers
        // the database at the meter rate.
        let mut last_fill: Option<Instant> = None;
        loop {
            ticker.tick().await;
            drain_events(&app).await;
            if app.autodj_enabled()
                && app.current_now_playing().is_none()
                && !app.engine.is_playing(MAIN_ZONE)
                && last_fill.is_none_or(|t| t.elapsed() >= AUTODJ_RETRY)
            {
                last_fill = Some(Instant::now());
                let app2 = Arc::clone(&app);
                // Off the meter tick: a fill may touch the database.
                tokio::spawn(async move { crate::autodj::fill_on_idle(&app2).await; });
            }
            let _ = app.tx.send(app.meters_frame());
        }
    })
}

/// Minimum gap between AutoDJ fill attempts.
const AUTODJ_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// Forward engine events: finished/segue transitions close out now-playing
/// and run the booking for the item that just ended.
async fn drain_events(app: &Arc<App>) {
    for (zone, event) in app.engine.take_events() {
        match event {
            PlayerEvent::Finished | PlayerEvent::SegueStarted => match zone {
                Zone::Main => app.refresh_now_playing().await,
                Zone::Cart => app.refresh_cart_now(),
                _ => {}
            },
        }
    }
    // The engine may have ended the item without an event (manual stop,
    // VDF target, stop_at); keep `now` honest either way.
    app.refresh_now_playing().await;
    app.refresh_cart_now();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::Frame;
    use radiocraft_audio::engine::EngineConfig;

    #[tokio::test]
    async fn pump_pushes_meters() {
        let dir = std::env::temp_dir().join(format!("rc-pump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::load(&dir).expect("load");
        // Headless engine variant: no audio device needed for the test.
        app.engine = radiocraft_audio::engine::PlayoutEngine::headless(EngineConfig::default());
        let app = Arc::new(app);
        let mut rx = app.subscribe();
        let handle = spawn_pump(Arc::clone(&app));
        let frame = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("a frame within 2s")
            .expect("open hub");
        assert!(matches!(frame, Frame::Meters { .. }), "{frame:?}");
        handle.abort();
    }
}
