//! Aux-player parity with `Plugin_AuxPlayers`: N configurable extra
//! players (RadioDJ's `PlayersCount`, 1..15), each an independent
//! `TrackPlayer` on its own engine zone (`Zone::AuxPlayer(n)`).
//!
//! The reference plugin creates one `APlayer` per configured player
//! (`PlayersForm.SetupPlayers`), loads tracks by song id, path or URL,
//! plays with a per-track `UpdateHistory` booked at start, honours the
//! track's loop region when the loop button is selected, and stops with
//! the configured `StopFadeDuration` (ms) fade. Those settings live in
//! the plugin's `GetSetting`/`SaveSetting` store, which the host maps to
//! `settings_Plugin_AuxPlayers.xml`; defaults come from `Var.cs`.
//!
//! `PluginZone` selects the reference app's sound-card zone for the
//! plugin; RadioCraft renders every player through the engine mixer
//! instead, so the value is stored for config-file compatibility only.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Result};
use radiocraft_audio::mixer::Zone;
use radiocraft_audio::Source as _;
use radiocraft_db::entities;
use radiocraft_settings::PluginSettings;
use rust_decimal::prelude::ToPrimitive as _;
use serde_json::{json, Value};

use crate::playback::{knobs_from_song, request_for_source, split_stem, track_gain_for, CueKnobs};
use crate::state::{App, Status};

/// Config file for the aux players (under the config dir), matching the
/// host mapping of the plugin's `GetSetting`/`SaveSetting` calls.
pub const AUXPLAYERS_SETTINGS_FILE: &str = "settings_Plugin_AuxPlayers.xml";

/// RadioDJ `Var.playerCount` default (range 1..=15, `FormConfig`).
pub const DEFAULT_PLAYERS_COUNT: i32 = 3;
/// RadioDJ `Var.playerZoneNumber` default (range 1..=10, `FormConfig`).
pub const DEFAULT_PLUGIN_ZONE: i32 = 4;
/// RadioDJ `Var.StopFadeDuration` default, milliseconds
/// (range 0..=10000, `FormConfig`).
pub const DEFAULT_STOP_FADE_MS: i32 = 0;

/// Contract upper bounds from `FormConfig`'s numeric up/downs.
pub const MAX_PLAYERS: i32 = 15;

/// The editable aux-player config, matching `FormConfig`'s OK-time save
/// of `PlayersCount`, `PluginZone` and `StopFadeDuration`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuxPlayersConfig {
    pub players_count: i32,
    pub plugin_zone: i32,
    pub stop_fade_ms: i32,
}

impl Default for AuxPlayersConfig {
    fn default() -> Self {
        Self {
            players_count: DEFAULT_PLAYERS_COUNT,
            plugin_zone: DEFAULT_PLUGIN_ZONE,
            stop_fade_ms: DEFAULT_STOP_FADE_MS,
        }
    }
}

/// The track loaded into one aux player slot (`APlayer.AuxPlayerTrack`
/// and `LoadInfo`): play restarts from this record until another load
/// replaces it.
///
/// `song_id` is 0 for ad-hoc paths/URLs (out-of-library, like RadioDJ's
/// test play, so history is not booked for those).
#[derive(Debug, Clone, PartialEq)]
pub struct AuxTrack {
    /// 1-based player number ("Player N" in the plugin UI).
    pub player: u8,
    pub song_id: i64,
    pub artist: String,
    pub title: String,
    /// Raw library path (file path or stream URL).
    pub path: String,
    pub track_type: i32,
    pub duration_secs: f64,
    pub knobs: CueKnobs,
    /// `ButLoop.Selected`: restarts inside the track's loop region.
    pub loop_enabled: bool,
    /// Linear playout volume multiplier resolved at load time from the
    /// row's loudness columns + `UseBSLoudness` (docs/06 §8); 1.0 for
    /// ad-hoc path/URL loads with no library row.
    pub gain_lin: f64,
}

impl AuxTrack {
    /// The wire view of one loaded aux track.
    pub fn info(&self) -> crate::frames::AuxPlayerInfo {
        crate::frames::AuxPlayerInfo {
            player: self.player,
            song_id: self.song_id,
            artist: self.artist.clone(),
            title: self.title.clone(),
            duration_secs: self.duration_secs,
            loop_enabled: self.loop_enabled,
        }
    }

    /// The zone this player renders on.
    pub fn zone(&self) -> Zone {
        Zone::AuxPlayer(self.player)
    }
}

fn settings_path(app: &App) -> PathBuf {
    app.config_dir.join(AUXPLAYERS_SETTINGS_FILE)
}

/// Parse the config out of a raw key/value store, mirroring the
/// `GetSetting` defaults in the plugin's `Var.cs`.
pub fn config_from_store(store: &PluginSettings) -> AuxPlayersConfig {
    AuxPlayersConfig {
        players_count: store
            .get_as("PlayersCount", DEFAULT_PLAYERS_COUNT)
            .clamp(1, MAX_PLAYERS),
        plugin_zone: store
            .get_as("PluginZone", DEFAULT_PLUGIN_ZONE)
            .clamp(1, 10),
        stop_fade_ms: store
            .get_as("StopFadeDuration", DEFAULT_STOP_FADE_MS)
            .clamp(0, 10_000),
    }
}

/// Read the stored config; missing file or keys fall back to defaults.
pub fn load_config(app: &App) -> Result<AuxPlayersConfig> {
    let path = settings_path(app);
    if path.exists() {
        let store = PluginSettings::load(&path).with_context(|| AUXPLAYERS_SETTINGS_FILE)?;
        Ok(config_from_store(&store))
    } else {
        Ok(AuxPlayersConfig::default())
    }
}

// ---- ops ----------------------------------------------------------------------

/// `aux_players_get_config`: the current aux-player config (defaults if unset).
pub fn op_get_config(app: &Arc<App>) -> Result<Value> {
    let cfg = app.aux_config();
    Ok(json!({
        "playersCount": cfg.players_count,
        "pluginZone": cfg.plugin_zone,
        "stopFadeMs": cfg.stop_fade_ms,
    }))
}

/// `aux_players_set_config`: validate, persist (same three keys as
/// `FormConfig`), live-swap the count, and retire players that fall off
/// the config (stop + unload).
pub fn op_set_config(app: &Arc<App>, args: &Value) -> Result<Value> {
    let mut cfg = app.aux_config();
    if let Some(v) = args.get("playersCount").and_then(Value::as_i64) {
        let v = i32::try_from(v).unwrap_or(i32::MAX);
        if !(1..=MAX_PLAYERS).contains(&v) {
            return Err(anyhow!("playersCount must be between 1 and {MAX_PLAYERS}"));
        }
        cfg.players_count = v;
    }
    if let Some(v) = args.get("pluginZone").and_then(Value::as_i64) {
        let v = i32::try_from(v).unwrap_or(i32::MAX);
        if !(1..=10).contains(&v) {
            return Err(anyhow!("pluginZone must be between 1 and 10"));
        }
        cfg.plugin_zone = v;
    }
    if let Some(v) = args.get("stopFadeMs").and_then(Value::as_i64) {
        let v = i32::try_from(v).unwrap_or(i32::MAX);
        if !(0..=10_000).contains(&v) {
            return Err(anyhow!("stopFadeMs must be between 0 and 10000"));
        }
        cfg.stop_fade_ms = v;
    }

    let path = settings_path(app);
    let mut store = if path.exists() {
        PluginSettings::load(&path).with_context(|| AUXPLAYERS_SETTINGS_FILE)?
    } else {
        PluginSettings::new()
    };
    store.set_typed("PlayersCount", cfg.players_count);
    store.set_typed("PluginZone", cfg.plugin_zone);
    store.set_typed("StopFadeDuration", cfg.stop_fade_ms);
    store
        .save_at(&path)
        .with_context(|| format!("saving {AUXPLAYERS_SETTINGS_FILE}"))?;

    app.set_aux_config(cfg);

    // Retire players that fell off the config (RadioDJ rebuilds the whole
    // player list on a config change: stop + dispose).
    for player in u8::try_from(cfg.players_count)
        .unwrap_or(u8::MAX)
        .saturating_add(1)..=MAX_PLAYERS as u8
    {
        app.engine.stop(Zone::AuxPlayer(player), 0);
        app.clear_aux_track(player);
    }
    let _ = app.tx.send(app.player_frame());

    Ok(json!({
        "playersCount": cfg.players_count,
        "pluginZone": cfg.plugin_zone,
        "stopFadeMs": cfg.stop_fade_ms,
    }))
}

/// `aux_players_load`: load a track into one player slot, by `songId`,
/// `path` (file or stream URL), or library file path. Loading replaces
/// any playing track immediately (the reference `ClearPlayer` + `Create`
/// path, no fade).
pub async fn op_load(app: &Arc<App>, args: &Value) -> Result<Value> {
    let player = arg_player(args)?;
    let zone = Zone::AuxPlayer(player);
    let track = if let Some(song_id) = args.get("songId").and_then(Value::as_i64) {
        load_from_song(app, player, song_id).await?
    } else if let Some(path) = args.get("path").and_then(Value::as_str) {
        load_from_path(player, path)?
    } else {
        return Err(anyhow!("aux_players_load needs a 'songId' or a 'path'"));
    };

    app.engine.stop(zone, 0);
    app.set_aux_track(track.clone());
    Ok(json!({ "player": player, "track": track.info() }))
}

/// Resolve and open-check a library song into an aux slot record.
async fn load_from_song(app: &Arc<App>, player: u8, song_id: i64) -> Result<AuxTrack> {
    if song_id <= 0 {
        return Err(anyhow!("songId must be a positive song id"));
    }
    let db = app.require_db().await?;
    let song = entities::song_by_id(db.pool(), song_id as u32)
        .await?
        .ok_or_else(|| anyhow!("no such song {song_id}"))?;
    let knobs = knobs_from_song(&song);
    let duration = song.duration.to_f64().unwrap_or(0.0).max(0.0);
    let gain_lin = track_gain_for(&song, &app.general.read().expect("general lock"));
    Ok(AuxTrack {
        player,
        song_id,
        artist: song.artist.clone(),
        title: song.title.clone(),
        path: song.path.clone(),
        track_type: i32::from(song.song_type),
        duration_secs: duration,
        knobs,
        loop_enabled: false,
        gain_lin,
    })
}

/// Resolve a raw file path or stream URL into an aux slot record, using
/// the file's decoded duration (0 for streams) and name-split title.
fn load_from_path(player: u8, raw_path: &str) -> Result<AuxTrack> {
    if radiocraft_audio::is_stream_path(raw_path) {
        return Ok(AuxTrack {
            player,
            song_id: 0,
            artist: String::new(),
            title: raw_path.to_string(),
            path: raw_path.to_string(),
            track_type: 5,
            duration_secs: 0.0,
            knobs: CueKnobs::default(),
            loop_enabled: false,
            gain_lin: 1.0,
        });
    }
    let path = std::path::Path::new(raw_path);
    if !path.exists() {
        return Err(anyhow!("no such file: {raw_path}"));
    }
    let source = radiocraft_audio::decoder::FileSource::open(path)
        .map_err(|e| anyhow!("opening {raw_path}: {e}"))?;
    let duration = source.duration().map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let (artist, title) = split_stem(path);
    Ok(AuxTrack {
        player,
        song_id: 0,
        artist,
        title,
        path: raw_path.to_string(),
        track_type: 0,
        duration_secs: duration,
        knobs: CueKnobs::default(),
        loop_enabled: false,
        gain_lin: 1.0,
    })
}

/// `aux_players_play`: play/stop toggle for one player
/// (`PlayersForm.PlayAUX`): start the loaded track from 0, or stop with
/// the configured `StopFadeDuration` fade. Library tracks book history
/// at play start (`APlayer.PlayAux` -> `UpdateHistory`).
pub async fn op_play(app: &Arc<App>, args: &Value) -> Result<Value> {
    let player = arg_player(args)?;
    let zone = Zone::AuxPlayer(player);
    let cfg = app.aux_config();

    // Stop side of the toggle.
    if app.engine.is_playing(zone) {
        app.engine.stop(zone, cfg.stop_fade_ms);
        let _ = app.tx.send(app.player_frame());
        return Ok(json!({ "player": player, "playing": false }));
    }

    let track = app
        .aux_track(player)
        .ok_or_else(|| anyhow!("player {player} has no track loaded"))?;

    // Loop region (`ButLoop`): re-open with the region armed when the
    // loop button is selected; tracks without a region play straight.
    let knobs = track.knobs;
    if track.loop_enabled && !(knobs.loop_in > 0.0 && knobs.loop_out > knobs.loop_in) {
        return Err(anyhow!("track has no loop region to play"));
    }
    let raw_path = track.path.clone();
    let (req, duration) = tokio::task::spawn_blocking(move || {
        let (mut req, duration) = request_for_source(&raw_path, &knobs)?;
        if track.loop_enabled {
            req = req.looping(knobs.loop_in, knobs.loop_out);
        }
        Ok::<_, anyhow::Error>((req, duration))
    })
    .await
    .context("open source thread join")??;

    let req = req.with_track_gain_lin(track.gain_lin);
    app.engine.play(zone, req, track.track_type, false)?;

    // History at play start, like `APlayer.PlayAux` booking the track.
    // Ad-hoc path/URL loads (song_id 0) are out-of-library and skipped.
    let mut history_id = 0u32;
    if track.song_id > 0 {
        if let Some(db) = app.db.read().await.clone() {
            match entities::add_history_row(
                db.pool(),
                track.song_id as u32,
                track.track_type,
                &track.artist,
                &track.title,
            )
            .await
            {
                Ok(id) => history_id = id,
                Err(e) => tracing::warn!("booking aux track {} failed: {e}", track.song_id),
            }
        }
    }

    let status = format!(
        "Player {} now playing: {} {}",
        player,
        track.artist,
        track.title
    );
    app.set_status(Status::info(status)).await;
    let _ = app.tx.send(app.player_frame());
    Ok(json!({
        "player": player,
        "playing": true,
        "historyId": history_id,
        "duration_secs": duration.max(track.duration_secs),
    }))
}

/// `aux_players_stop`: fade-stop one player with the configured
/// `StopFadeDuration`, leaving the track loaded (the reference plugin's
/// stop button keeps the track and resets the bar).
pub fn op_stop(app: &Arc<App>, args: &Value) -> Result<Value> {
    let player = arg_player(args)?;
    let cfg = app.aux_config();
    app.engine.stop(Zone::AuxPlayer(player), cfg.stop_fade_ms);
    let _ = app.tx.send(app.player_frame());
    Ok(json!({ "player": player, "stopped": true }))
}

/// `aux_players_seek`: seek the loaded track to an absolute position.
pub fn op_seek(app: &Arc<App>, args: &Value) -> Result<Value> {
    let player = arg_player(args)?;
    let secs = crate::dispatch::arg_f64(args, "secs")?;
    app.engine.seek(Zone::AuxPlayer(player), secs.max(0.0))?;
    Ok(json!({ "player": player, "position_secs": secs }))
}

/// `aux_players_loop`: select/deselect the track's loop region
/// (`APlayer.ButLoopClick`); while playing, the track restarts at the
/// loop-in point like the reference button does.
pub fn op_loop(app: &Arc<App>, args: &Value) -> Result<Value> {
    let player = arg_player(args)?;
    let on = args
        .get("on")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow!("aux_players_loop needs a boolean 'on'"))?;
    let mut track = app
        .aux_track(player)
        .ok_or_else(|| anyhow!("player {player} has no track loaded"))?;
    if on && !(track.knobs.loop_in > 0.0 && track.knobs.loop_out > track.knobs.loop_in) {
        return Err(anyhow!("track has no loop region"));
    }
    track.loop_enabled = on;
    let zone = track.zone();
    if app.engine.is_playing(zone) {
        if on {
            let _ = app.engine.seek(zone, track.knobs.loop_in);
        } else {
            let _ = app.engine.seek(zone, 0.0);
        }
    }
    app.set_aux_track(track.clone());
    Ok(json!({ "player": player, "loopEnabled": on }))
}

/// `aux_players_clear`: stop and unload one player (`APlayer.ClearPlayer`).
pub fn op_clear(app: &Arc<App>, args: &Value) -> Result<Value> {
    let player = arg_player(args)?;
    app.engine.stop(Zone::AuxPlayer(player), 0);
    app.clear_aux_track(player);
    Ok(json!({ "player": player, "cleared": true }))
}

/// Player number arg (1-based, within the configured count).
fn arg_player(args: &Value) -> Result<u8> {
    let v = args
        .get("player")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("aux player op needs a 'player' number"))?;
    if !(1..=MAX_PLAYERS as i64).contains(&v) {
        return Err(anyhow!("player must be between 1 and {MAX_PLAYERS}"));
    }
    Ok(v as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_var_cs() {
        let cfg = AuxPlayersConfig::default();
        assert_eq!(cfg.players_count, 3);
        assert_eq!(cfg.plugin_zone, 4);
        assert_eq!(cfg.stop_fade_ms, 0);
        assert_eq!(config_from_store(&PluginSettings::new()), cfg);
    }

    #[test]
    fn store_values_are_clamped_to_form_ranges() {
        let mut store = PluginSettings::new();
        store.set_typed("PlayersCount", 999);
        store.set_typed("PluginZone", 0);
        store.set_typed("StopFadeDuration", -5);
        let cfg = config_from_store(&store);
        assert_eq!(cfg.players_count, MAX_PLAYERS);
        assert_eq!(cfg.plugin_zone, 1);
        assert_eq!(cfg.stop_fade_ms, 0);
    }
}
