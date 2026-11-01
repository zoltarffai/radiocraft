//! Settings editors over WS — the RadioDJ "Options" surface that lives in
//! XML files rather than the database: sound device, sound options
//! (`settings_general.xml` + `settings_soundcards.xml`) and the now-playing
//! export (`Plugins/Plugin_NowPlaying.xml`).
//!
//! Every op is a read/apply pair: reads mirror the stored keys, writes
//! persist first, then push the new values into the live engine/exporter so
//! the change takes effect without a restart.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use radiocraft_audio::device::DeviceManager;
use radiocraft_core::model::SongData;
use radiocraft_net::nowplaying::ExportDraft;
use radiocraft_settings::SoundcardSettings;

use crate::state::App;

// ---- sound device -----------------------------------------------------------

/// `list_devices`: output device names plus the active sink.
pub fn op_list_devices(app: &Arc<App>) -> Result<Value> {
    Ok(json!({
        "devices": DeviceManager::output_device_names(),
        "current": app.engine.sink_info(),
    }))
}

/// `set_device`: switch the live output device.
/// Args `{name}` (empty/absent = system default); persists the choice to
/// `settings_soundcards.xml` (`audio_device`).
pub fn op_set_device(app: &Arc<App>, args: &Value) -> Result<Value> {
    let name = args.get("name").and_then(Value::as_str);
    let info = app.set_audio_device(name)?;
    Ok(json!({ "current": info }))
}

/// `soundcards_get`: the `settings_soundcards.xml` knobs.
pub fn op_soundcards_get(app: &Arc<App>) -> Result<Value> {
    let path = app.config_dir().join(SoundcardSettings::FILE);
    let sc = if path.exists() {
        SoundcardSettings::load(&path)?
    } else {
        SoundcardSettings::default()
    };
    let current = app.engine.sink_info();
    Ok(json!({
        "audio_mode": *sc.audio_mode(),
        "input_fade_in": *sc.input_fade_in(),
        "input_fade_out": *sc.input_fade_out(),
        "audio_device": sc
            .get_raw("audio_device")
            .unwrap_or_else(|| current.name.clone()),
        "current": current,
    }))
}

/// `soundcards_set`: args `{audioMode?, inputFadeIn?, inputFadeOut?}`.
/// The device itself is only changed through `set_device` so the engine
/// always ends up holding the persisted sink.
pub fn op_soundcards_set(app: &Arc<App>, args: &Value) -> Result<Value> {
    let path = app.config_dir().join(SoundcardSettings::FILE);
    let mut sc = if path.exists() {
        SoundcardSettings::load(&path)?
    } else {
        SoundcardSettings::default()
    };
    if let Some(v) = args.get("audioMode").and_then(Value::as_i64) {
        sc.set_audio_mode(v.clamp(0, i64::from(i32::MAX)) as i32);
    }
    if let Some(v) = args.get("inputFadeIn").and_then(Value::as_i64) {
        sc.set_input_fade_in(v.clamp(0, i64::from(i32::MAX)) as i32);
    }
    if let Some(v) = args.get("inputFadeOut").and_then(Value::as_i64) {
        sc.set_input_fade_out(v.clamp(0, i64::from(i32::MAX)) as i32);
    }
    sc.save(&path)?;
    drop(sc);
    op_soundcards_get(app)
}

// ---- sound options (settings_general.xml) ------------------------------------

/// `sound_options_get`: the knobs the sound-options form edits.
pub fn op_sound_options_get(app: &Arc<App>) -> Result<Value> {
    let g = app.general.read().expect("general lock");
    Ok(json!({
        "volume_out": *g.volume_out(),
        "dur_fadeout": *g.dur_fadeout(),
        "vol_overlay": *g.vol_overlay(),
        "vol_onsweeper": *g.vol_onsweeper(),
        "vol_vt": *g.vol_vt(),
    }))
}

/// `sound_options_set`: args carry any subset of the knob names; each key is
/// saved through `set_general_setting` (XML persist + live engine re-apply).
pub fn op_sound_options_set(app: &Arc<App>, args: &Value) -> Result<Value> {
    for key in [
        "volume_out",
        "dur_fadeout",
        "vol_overlay",
        "vol_onsweeper",
        "vol_vt",
    ] {
        if let Some(v) = args.get(key).and_then(Value::as_i64) {
            app.set_general_setting(key, &v.to_string())?;
        }
    }
    op_sound_options_get(app)
}

// ---- now-playing export (Plugins/Plugin_NowPlaying.xml) -----------------------

/// `export_get`: the current export settings as an editable draft.
pub fn op_export_get(app: &Arc<App>) -> Result<Value> {
    let plugin = app.plugin_export.read().expect("plugin export lock");
    let draft = ExportDraft::from_config(&radiocraft_net::nowplaying::ExportConfig::from_settings(
        &plugin,
    ));
    let value = serde_json::to_value(&draft)?;
    Ok(json!({ "export": value }))
}

/// `export_set`: validate + persist the draft, rebuild the exporter.
pub fn op_export_set(app: &Arc<App>, args: &Value) -> Result<Value> {
    let draft: ExportDraft = serde_json::from_value(args.get("export").cloned().unwrap_or(Value::Null))
        .map_err(|e| anyhow!("bad export settings: {e}"))?;
    app.save_export_draft(draft)?;
    op_export_get(app)
}

/// `export_test`: run the exporter against the live track (or a sample
/// song when nothing is on air) and report what each channel rendered.
pub async fn op_export_test(app: &Arc<App>) -> Result<Value> {
    let exporter = app.exporter.read().expect("exporter lock").clone();
    let song = match app.current_now_playing() {
        Some(now) => SongData {
            id: now.song_id,
            artist: now.artist.clone(),
            title: now.title.clone(),
            album: now.album.clone(),
            path: now.path.clone(),
            duration: now.duration_secs,
            track_type: now.track_type,
            ..SongData::default()
        },
        None => SongData {
            id: 0,
            artist: "RadioCraft".into(),
            title: "Test message".into(),
            duration: 30.0,
            track_type: 6, // promo/liner: always inside the default export set
            ..SongData::default()
        },
    };
    if !exporter.config.should_export(&song) {
        return Err(anyhow!(
            "track type {} is not in the export list",
            song.track_type
        ));
    }
    let snapshot = exporter.broadcast(&song)?;
    Ok(json!({
        "snapshot": snapshot,
        "file": exporter.config.export_to_file,
        "web": exporter.config.export_to_web,
        "serial": exporter.config.export_to_serial,
    }))
}
