//! The single pump task: engine events and live meters fan out to the
//! broadcast hub from here (docs/05: "server pushes, clients render").
//!
//! Engine events (`Finished`/`SegueStarted`) drive now-playing bookkeeping;
//! everything else is polled at ~20 Hz and cheap to compare.

use std::sync::Arc;
use std::time::Duration;

use radiocraft_audio::mixer::Zone;
use radiocraft_audio::player::PlayerEvent;

use crate::state::App;

/// 20 Hz meter tick.
pub const PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// Spawn the pump on the current runtime; it runs until the app is dropped.
pub fn spawn_pump(app: Arc<App>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PUMP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            drain_events(&app).await;
            let _ = app.tx.send(app.meters_frame());
        }
    })
}

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
