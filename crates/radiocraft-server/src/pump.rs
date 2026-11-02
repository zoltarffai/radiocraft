//! The single pump task: engine events and live meters fan out to the
//! broadcast hub from here (docs/05: "server pushes, clients render").
//!
//! Engine events (`Finished`/`SegueStarted`) drive now-playing bookkeeping;
//! everything else is polled at ~20 Hz and cheap to compare.

use std::sync::Arc;
use std::time::{Duration, Instant};

use radiocraft_audio::mixer::Zone;
use radiocraft_audio::player::PlayerEvent;
use radiocraft_plugin::host::{StateChange, StateInfo};

use crate::state::{App, CartNow, MAIN_ZONE, NowPlaying};

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
        // Plugin state-change fan-out: remember what sounded last tick.
        let mut prev_main: Option<NowPlaying> = None;
        let mut prev_cart: Option<CartNow> = None;
        let mut prev_instant: Option<CartNow> = None;
        let mut last_timer = Instant::now();
        loop {
            ticker.tick().await;
            drain_events(&app).await;
            app.check_duck_expiry();
            fan_state_changes(
                &app,
                &mut prev_main,
                &mut prev_cart,
                &mut prev_instant,
                &mut last_timer,
            );
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

/// SmartEvents-style plugin timer cadence.
const PLUGIN_TIMER: std::time::Duration = std::time::Duration::from_secs(1);

/// Diff last-tick transport state against now and broadcast the resulting
/// host `StateChanged` calls to plugins (`PlayerStarted/Ended`,
/// `CartStarted/Ended`, periodic `Timer`). Payloads mirror the reference:
/// a SongData JSON for the player, the button id for carts.
fn fan_state_changes(
    app: &Arc<App>,
    prev_main: &mut Option<NowPlaying>,
    prev_cart: &mut Option<CartNow>,
    prev_instant: &mut Option<CartNow>,
    last_timer: &mut Instant,
) {
    if app.plugin_manager().is_none() {
        return;
    }
    let cur_main = app.current_now_playing();
    if cur_main.as_ref().map(|n| n.song_id) != prev_main.as_ref().map(|n| n.song_id) {
        if let Some(old) = prev_main.take() {
            let data = serde_json::to_value(old.song_data()).unwrap_or_default();
            app.notify_plugins(StateInfo::with_data(StateChange::PlayerEnded, data));
        }
        if let Some(cur) = &cur_main {
            let data = serde_json::to_value(cur.song_data()).unwrap_or_default();
            app.notify_plugins(StateInfo::with_data(StateChange::PlayerStarted, data.clone()));
            app.notify_plugins(StateInfo::with_data(StateChange::CurrentlyPlayingChanged, data));
        }
        *prev_main = cur_main;
    }
    let cur_cart = app.cart_now();
    if cur_cart.as_ref().map(|c| c.button_id) != prev_cart.as_ref().map(|c| c.button_id) {
        if let Some(old) = prev_cart.take() {
            app.notify_plugins(StateInfo::with_data(
                StateChange::CartEnded,
                serde_json::json!(old.button_id),
            ));
        }
        if let Some(cur) = &cur_cart {
            app.notify_plugins(StateInfo::with_data(
                StateChange::CartStarted,
                serde_json::json!(cur.button_id),
            ));
        }
        *prev_cart = cur_cart;
    }
    // Instant/cart wall (Plugin_InstantPlayers): same state-change names but
    // the payload marks the source, since instant button ids live in their
    // own space (`weborama.ID`) and would otherwise collide with carts.
    let cur_instant = app.instant_now();
    if cur_instant.as_ref().map(|c| c.button_id) != prev_instant.as_ref().map(|c| c.button_id) {
        if let Some(old) = prev_instant.take() {
            app.notify_plugins(StateInfo::with_data(
                StateChange::CartEnded,
                serde_json::json!({ "id": old.button_id, "plugin": true }),
            ));
        }
        if let Some(cur) = &cur_instant {
            app.notify_plugins(StateInfo::with_data(
                StateChange::CartStarted,
                serde_json::json!({ "id": cur.button_id, "plugin": true }),
            ));
        }
        *prev_instant = cur_instant;
    }
    if last_timer.elapsed() >= PLUGIN_TIMER {
        *last_timer = Instant::now();
        app.notify_plugins(StateInfo::new(StateChange::Timer));
    }
}

/// Forward engine events: finished/segue transitions close out now-playing
/// and run the booking for the item that just ended.
async fn drain_events(app: &Arc<App>) {
    for (zone, event) in app.engine.take_events() {
        match event {
            PlayerEvent::Finished | PlayerEvent::SegueStarted => match zone {
                Zone::Main => app.refresh_now_playing().await,
                Zone::Cart => app.refresh_cart_now(),
                Zone::Instant => app.refresh_instant_now(),
                _ => {}
            },
        }
    }
    // The engine may have ended the item without an event (manual stop,
    // VDF target, stop_at); keep `now` honest either way.
    app.refresh_now_playing().await;
    app.refresh_cart_now();
    app.refresh_instant_now();
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
