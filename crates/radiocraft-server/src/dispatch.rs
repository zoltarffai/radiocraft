//! Command dispatch: the ops a UI invokes over `{"t":"cmd",...}`.
//!
//! Every op maps 1:1 onto the [`radiocraft_audio::engine::PlayoutEngine`]
//! or the db entity layer; nothing here duplicates business logic.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use chrono::Local;
use serde_json::{Value, json};

use radiocraft_audio::mixer::Zone;
use radiocraft_db::booking::{BookingParams, book_track};

use crate::frames::Frame;
use crate::playback::{CueKnobs, knobs_from_song, request_for_file, split_stem};
use crate::state::{App, NowPlaying, parse_zone, zone_name};

/// Handle one client command; always replies `result` or `error` with `id`.
pub async fn dispatch(app: &Arc<App>, id: u64, op: &str, args: &Value) -> Frame {
    match run(app, op, args).await {
        Ok(data) => Frame::Result {
            id,
            op: op.to_string(),
            data,
        },
        Err(e) => {
            tracing::warn!("cmd {op} failed: {e}");
            Frame::Error {
                id: Some(id),
                code: "cmd_failed".into(),
                message: e.to_string(),
            }
        }
    }
}

async fn run(app: &Arc<App>, op: &str, args: &Value) -> Result<Value> {
    match op {
        // ---- transport ------------------------------------------------------
        "play" => start(app, args, false).await,
        "cue" => start(app, args, true).await,
        "segue_now" => {
            let zone = arg_zone(args)?;
            app.engine.segue_now(zone, arg_i32(args, "fade", -1))?;
            Ok(json!({}))
        }
        "stop" => {
            let zone = arg_zone(args)?;
            app.engine.stop(zone, arg_i32(args, "fade", -1));
            Ok(json!({}))
        }
        "stop_all" => {
            app.engine.stop_all(arg_i32(args, "fade", -1));
            Ok(json!({}))
        }
        "stop_at" => {
            let zone = arg_zone(args)?;
            let secs = arg_f64(args, "secs")?;
            app.engine.stop_at(zone, secs, arg_i32(args, "fade", -1))?;
            Ok(json!({}))
        }
        "seek" => {
            let zone = arg_zone(args)?;
            let secs = arg_f64(args, "secs")?;
            app.engine.seek(zone, secs)?;
            Ok(json!({ "position_secs": secs }))
        }
        "pause" => {
            app.engine.pause(arg_zone(args)?);
            Ok(json!({}))
        }
        "resume" => {
            app.engine.resume(arg_zone(args)?);
            Ok(json!({}))
        }

        // ---- mixer ----------------------------------------------------------
        "set_master_db" => {
            let db = arg_f64(args, "db")?;
            app.engine.set_master_db(db);
            let _ = app.tx.send(app.state_frame());
            Ok(json!({ "master_db": app.engine.master_db() }))
        }
        "set_zone_fader_db" => {
            let zone = arg_zone(args)?;
            let db = arg_f64(args, "db")?;
            app.set_zone_mix(zone, Some(db), None);
            Ok(json!({ "zone": zone_name(zone), "fader_db": db }))
        }
        "set_zone_muted" => {
            let zone = arg_zone(args)?;
            let muted = args
                .get("muted")
                .and_then(Value::as_bool)
                .ok_or_else(|| anyhow!("set_zone_muted needs a boolean 'muted'"))?;
            app.set_zone_mix(zone, None, Some(muted));
            Ok(json!({ "zone": zone_name(zone), "muted": muted }))
        }

        // ---- settings ---------------------------------------------------------
        "get_setting" => {
            let key = arg_str(args, "key")?;
            let general = app.general.read().expect("general lock");
            let value = general
                .get_setting(&key)
                .ok_or_else(|| anyhow!("unknown setting {key}"))?;
            Ok(json!({ "key": key, "value": value }))
        }
        "set_setting" => {
            let key = arg_str(args, "key")?;
            let value = arg_str(args, "value")?;
            app.set_general_setting(&key, &value)?;
            Ok(json!({ "key": key, "value": value }))
        }

        // ---- database ---------------------------------------------------------
        "db_connect" => {
            app.connect_db().await?;
            let db = app.require_db().await?;
            let count = radiocraft_db::entities::count_songs(db.pool()).await.unwrap_or(0);
            Ok(json!({ "songs": count }))
        }

        // ---- library reads (thin REST also mirrors these) ---------------------
        "list_songs" => {
            let db = app.require_db().await?;
            let (limit, offset) = page(args);
            let songs = radiocraft_db::entities::list_songs(db.pool(), limit, offset).await?;
            Ok(json!({ "songs": songs }))
        }
        "search_songs" => {
            let db = app.require_db().await?;
            let needle = arg_str(args, "needle")?;
            let songs =
                radiocraft_db::entities::search_songs(db.pool(), &needle, args.get("limit").and_then(Value::as_i64).unwrap_or(100))
                    .await?;
            Ok(json!({ "songs": songs }))
        }
        "list_history" => {
            let db = app.require_db().await?;
            let (limit, offset) = page(args);
            let rows = radiocraft_db::entities::list_history(db.pool(), limit, offset).await?;
            Ok(json!({ "history": rows }))
        }

        other => Err(anyhow!("unknown op {other}")),
    }
}

// ---- play / cue -------------------------------------------------------------

/// Start (or segue-cue) a track: `songId` from the library or a raw `path`
/// for out-of-library testing (the old AutoDJ `Now playing` of one file).
async fn start(app: &Arc<App>, args: &Value, follow: bool) -> Result<Value> {
    let zone = arg_zone(args)?;
    let patch = arg_cues(args);

    let (path, mut playing) = if let Some(song_id) = args.get("songId").and_then(Value::as_i64) {
        let db = app.require_db().await?;
        let song = radiocraft_db::entities::song_by_id(db.pool(), song_id as u32)
            .await?
            .ok_or_else(|| anyhow!("no song with id {song_id}"))?;
        let path = PathBuf::from(&song.path);
        let mut knobs = knobs_from_song(&song);
        if let Some(patch) = &patch {
            knobs = knobs.merge(patch);
        }
        let playing = NowPlaying {
            song_id: i64::from(song.ID),
            artist: song.artist.clone(),
            title: song.title.clone(),
            album: song.album.clone(),
            path: song.path.clone(),
            track_type: i32::from(song.song_type),
            duration_secs: 0.0,
            started_at: Local::now(),
            cue_start: knobs.start,
            cue_next: knobs.next,
            cue_end: knobs.end,
            fade_in_ms: knobs.fade_in_ms,
            fade_out_ms: knobs.fade_out_ms,
        };
        (path, playing)
    } else if let Some(raw) = args.get("path").and_then(Value::as_str) {
        let path = PathBuf::from(raw);
        if !path.exists() {
            return Err(anyhow!("no such file: {raw}"));
        }
        let (artist, title) = split_stem(&path);
        let (artist, title) = match args.get("title").and_then(Value::as_str) {
            Some(t) => (artist, t.to_string()),
            None => (artist, title),
        };
        let playing = NowPlaying {
            song_id: 0,
            artist,
            title,
            album: String::new(),
            path: raw.to_string(),
            track_type: 0,
            duration_secs: 0.0,
            started_at: Local::now(),
            cue_start: 0.0,
            cue_next: 0.0,
            cue_end: 0.0,
            fade_in_ms: 0,
            fade_out_ms: 0,
        };
        (path, playing)
    } else {
        return Err(anyhow!("play needs a 'songId' or a 'path'"));
    };

    let (req, duration) = request_for_file(&path, &cue_knobs_of(&playing, patch.as_ref()))?;
    playing.duration_secs = duration;

    let plan = cue_knobs_of(&playing, None).plan();
    if follow {
        app.engine.cue(zone, req, playing.track_type, false)?;
    } else {
        app.engine.play(zone, req, playing.track_type, false)?;
        if let Some(secs) = plan.stop_at_secs() {
            app.engine.stop_at(zone, secs, -1)?;
        }
    }

    let info = playing.info();
    app.on_track_started(playing).await;
    let _ = app.tx.send(app.player_frame());
    Ok(json!({ "now": info }))
}

/// Rebuild a knob bundle from the (possibly patched) now-playing knobs.
fn cue_knobs_of(playing: &NowPlaying, patch: Option<&CueKnobs>) -> CueKnobs {
    let base = CueKnobs {
        start: playing.cue_start,
        next: playing.cue_next,
        end: playing.cue_end,
        loop_in: 0.0,
        loop_out: 0.0,
        fade_in_ms: playing.fade_in_ms,
        fade_out_ms: playing.fade_out_ms,
    };
    match patch {
        Some(patch) => base.merge(patch),
        None => base,
    }
}

// ---- arg helpers --------------------------------------------------------------

fn arg_zone(args: &Value) -> Result<Zone> {
    match args.get("zone").and_then(Value::as_str) {
        None => Ok(Zone::Main),
        Some(name) => parse_zone(name).ok_or_else(|| anyhow!("unknown zone {name}")),
    }
}

fn arg_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("missing string arg {key}"))
}

fn arg_f64(args: &Value, key: &str) -> Result<f64> {
    args.get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .ok_or_else(|| anyhow!("missing numeric arg {key}"))
}

fn arg_i32(args: &Value, key: &str, default: i32) -> i32 {
    args.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .map(|v| v as i32)
        .unwrap_or(default)
}

/// Optional cue patch in the cmd args (`cue` object, camelCase fields).
fn arg_cues(args: &Value) -> Option<CueKnobs> {
    args.get("cue")
        .and_then(|v| serde_json::from_value::<CueKnobs>(v.clone()).ok())
}

fn page(args: &Value) -> (i64, i64) {
    (
        args.get("limit").and_then(Value::as_i64).unwrap_or(200),
        args.get("offset").and_then(Value::as_i64).unwrap_or(0),
    )
}

// ---- booking -------------------------------------------------------------------

impl App {
    /// One play booked with `UpdateTracks2` semantics when a library item
    /// ends (or is stopped); skipped for out-of-library test plays.
    pub async fn book_finished(&self, playing: &NowPlaying) {
        if playing.song_id <= 0 {
            return;
        }
        let db = match self.db.read().await.clone() {
            Some(db) => db,
            None => return,
        };
        let params = {
            let general = self.general.read().expect("general lock");
            BookingParams {
                track_id: playing.song_id as u32,
                cur_artist: playing.artist.clone(),
                cur_title: playing.title.clone(),
                song_type: playing.track_type,
                cur_listeners: None,
                history_days: *general.days_2_keep_history(),
                p_weight: general
                    .get_setting("pWeight")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0),
            }
        };
        match book_track(db.pool(), &params).await {
            Ok(out) => tracing::debug!(
                "booked song {} (disabled {}, deleted {}, relocated {}, trimmed {})",
                playing.song_id,
                out.songs_disabled,
                out.songs_deleted,
                out.songs_relocated,
                out.history_trimmed
            ),
            Err(e) => tracing::warn!("booking song {} failed: {e}", playing.song_id),
        }
    }

    /// Persist one general-settings key and re-apply engine knobs.
    pub fn set_general_setting(&self, key: &str, value: &str) -> Result<()> {
        {
            let mut general = self.general.write().expect("general lock");
            if !general.save_setting(key, value) {
                return Err(anyhow!("unknown setting {key}"));
            }
            general
                .save(self.config_dir.join(radiocraft_settings::GeneralSettings::FILE))
                .context("saving settings_general.xml")?;
        }
        self.reapply_engine_knobs();
        let _ = self.tx.send(self.state_frame());
        Ok(())
    }

    /// Push the settings knobs the engine reads live back into it.
    pub fn reapply_engine_knobs(&self) {
        let general = self.general.read().expect("general lock");
        self.engine
            .set_duck_db(f64::from(*general.vol_onsweeper()));
        self.engine.set_vt_duck_db(f64::from(*general.vol_vt()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_app() -> Arc<App> {
        let dir = std::env::temp_dir().join(format!("rc-dispatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Arc::new(App::load(&dir).expect("load"))
    }

    #[tokio::test]
    async fn unknown_op_is_error() {
        let app = tmp_app();
        let frame = dispatch(&app, 1, "nope", &json!({})).await;
        match frame {
            Frame::Error { id, message, .. } => {
                assert_eq!(id, Some(1));
                assert!(message.contains("nope"));
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_ops_shape_ok() {
        let app = tmp_app();
        for (op, args) in [
            ("stop", json!({})),
            ("stop_all", json!({"fade": -999})),
            ("pause", json!({"zone": "aux"})),
            ("resume", json!({"zone": "aux"})),
        ] {
            let frame = dispatch(&app, 2, op, &args).await;
            assert!(matches!(frame, Frame::Result { .. }), "{op}: {frame:?}");
        }
    }

    #[tokio::test]
    async fn mixer_ops_round_trip() {
        let app = tmp_app();
        let frame = dispatch(&app, 3, "set_zone_fader_db", &json!({"zone": "cart", "db": -6.0})).await;
        assert!(matches!(frame, Frame::Result { .. }), "{frame:?}");
        let state = match app.state_frame() {
            Frame::State { zones, .. } => zones,
            other => panic!("{other:?}"),
        };
        let cart = state.iter().find(|z| z.zone == "cart").expect("cart zone");
        assert!((cart.fader_db - -6.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let app = tmp_app();
        let frame = dispatch(&app, 4, "set_setting", &json!({"key": "StationName", "value": "RC"})).await;
        assert!(matches!(frame, Frame::Result { .. }), "{frame:?}");
        let frame = dispatch(&app, 5, "get_setting", &json!({"key": "StationName"})).await;
        match frame {
            Frame::Result { data, .. } => assert_eq!(data["value"], json!("RC")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn play_requires_db_or_file() {
        let app = tmp_app();
        let frame = dispatch(&app, 6, "play", &json!({"songId": 1})).await;
        assert!(matches!(frame, Frame::Error { .. }), "{frame:?}");
        let frame = dispatch(&app, 7, "play", &json!({"path": "/nonexistent.mp3"})).await;
        match frame {
            Frame::Error { message, .. } => assert!(message.contains("no such file")),
            other => panic!("{other:?}"),
        }
    }
}
