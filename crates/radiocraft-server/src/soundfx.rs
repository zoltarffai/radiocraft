//! Master sound-processing parity with RadioDJ's
//! `Plugin_SoundProcessing` (compressor, AGC/damper, 10-band EQ,
//! stereo enhancer, output trim).
//!
//! Settings round-trip through `Plugins/Plugin_SoundProcessing.xml` using
//! the exact key names RadioDJ's plugin reads via `GetSetting`, so an
//! existing RadioDJ `Plugins/` directory carries over. `fx_set` persists
//! the file, then live-swaps the chain through
//! [`radiocraft_audio::PlayoutEngine::set_sound_processor`].

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use radiocraft_audio::fx::{ProcessorConfig, SoundProcessor};
use radiocraft_settings::PluginSettings;

use crate::state::App;

/// Same file name (under `Plugins/`) as the RadioDJ plugin settings.
pub const FX_SETTINGS_FILE: &str = "Plugins/Plugin_SoundProcessing.xml";

fn settings_path(app: &App) -> PathBuf {
    app.config_dir.join(FX_SETTINGS_FILE)
}

/// Load the stored config, falling back to the plugin's own defaults.
fn load_settings(app: &App) -> Result<PluginSettings> {
    let path = settings_path(app);
    if path.exists() {
        PluginSettings::load(&path).with_context(|| FX_SETTINGS_FILE)
    } else {
        Ok(PluginSettings::new())
    }
}

/// Parse a [`ProcessorConfig`] out of the raw key/value store, mirroring
/// the `GetSetting` calls (and defaults) in `ClassPlugin.PluginInitialize`.
pub fn config_from_store(store: &PluginSettings) -> ProcessorConfig {
    // eqValues: "0;0;...;" semicolon-separated integers.
    let raw_eq = store.get_as::<String>("eqValues", "0;0;0;0;0;0;0;0;0;0;".into());
    let mut eq_values = [0.0f64; 10];
    for (i, part) in raw_eq
        .split(';')
        .filter(|s| !s.is_empty())
        .take(10)
        .enumerate()
    {
        if let Ok(v) = part.trim().parse::<f64>() {
            eq_values[i] = v;
        }
    }
    ProcessorConfig {
        compressor_enabled: store.get_as::<bool>("CompressorEnabled", false),
        eq_enabled: store.get_as::<bool>("eqON", false),
        agc_enabled: store.get_as::<bool>("AGCEnabled", false),
        stereo_enabled: store.get_as::<bool>("StereoEnhancerEnabled", false),
        compressor_by_preset: store.get_as::<bool>("fxCompByPreset", false),
        compressor_preset: store.get_as::<i64>("fxCompPreset", 0),
        damp_preset: store.get_as::<i64>("fxDampPreset", 0),
        // RadioDJ's raw default "30" is out of the control's -60..=0
        // range; the designer default (-15) is what the UI actually shows.
        threshold_db: store.get_as::<f64>("cThreshold", 30.0).clamp(-60.0, 0.0),
        ratio: store.get_as::<f64>("cRatio", 3.0),
        attack_ms: store.get_as::<f64>("cAttack", 5.0),
        release_ms: store.get_as::<f64>("cRelease", 200.0),
        gain_db: store.get_as::<f64>("cGain", 10.0),
        vol_out: store.get_as::<f64>("vol_Out", 50.0),
        stereo_amount: store.get_as::<i64>("fxStereo", 1),
        eq_values,
    }
}

/// Write a [`ProcessorConfig`] back with RadioDJ's key names and
/// .NET-style value spellings (`True`/`False`).
fn store_from_config(store: &mut PluginSettings, cfg: &ProcessorConfig) {
    let flag = |b: bool| if b { "True" } else { "False" }.to_string();
    store.set("CompressorEnabled", flag(cfg.compressor_enabled));
    store.set("eqON", flag(cfg.eq_enabled));
    store.set("AGCEnabled", flag(cfg.agc_enabled));
    store.set("StereoEnhancerEnabled", flag(cfg.stereo_enabled));
    store.set("fxCompByPreset", flag(cfg.compressor_by_preset));
    store.set("fxCompPreset", cfg.compressor_preset.to_string());
    store.set("fxDampPreset", cfg.damp_preset.to_string());
    store.set("cThreshold", cfg.threshold_db.to_string());
    store.set("cRatio", cfg.ratio.to_string());
    store.set("cAttack", cfg.attack_ms.to_string());
    store.set("cRelease", cfg.release_ms.to_string());
    store.set("cGain", cfg.gain_db.to_string());
    store.set("vol_Out", cfg.vol_out.to_string());
    store.set("fxStereo", cfg.stereo_amount.to_string());
    let eq = cfg
        .eq_values
        .iter()
        .map(|v| format!("{}", *v as i64))
        .collect::<Vec<_>>()
        .join(";");
    store.set("eqValues", format!("{eq};"));
}

/// JSON view of a config, keys named like the RadioDJ settings.
fn cfg_json(cfg: &ProcessorConfig) -> Value {
    json!({
        "CompressorEnabled": cfg.compressor_enabled,
        "eqON": cfg.eq_enabled,
        "AGCEnabled": cfg.agc_enabled,
        "StereoEnhancerEnabled": cfg.stereo_enabled,
        "fxCompByPreset": cfg.compressor_by_preset,
        "fxCompPreset": cfg.compressor_preset,
        "fxDampPreset": cfg.damp_preset,
        "cThreshold": cfg.threshold_db,
        "cRatio": cfg.ratio,
        "cAttack": cfg.attack_ms,
        "cRelease": cfg.release_ms,
        "cGain": cfg.gain_db,
        "vol_Out": cfg.vol_out,
        "fxStereo": cfg.stereo_amount,
        "eqValues": cfg.eq_values,
    })
}

/// Overlay the keys present in `args` onto `cfg` (partial update).
fn merge_args(cfg: &mut ProcessorConfig, args: &Value) -> Result<()> {
    let b = |key: &str, slot: &mut bool| -> Result<()> {
        if let Some(v) = args.get(key) {
            *slot = v
                .as_bool()
                .with_context(|| format!("{key} must be a boolean"))?;
        }
        Ok(())
    };
    let n = |key: &str, slot: &mut f64| -> Result<()> {
        if let Some(v) = args.get(key) {
            *slot = v
                .as_f64()
                .with_context(|| format!("{key} must be a number"))?;
        }
        Ok(())
    };
    let i = |key: &str, slot: &mut i64| -> Result<()> {
        if let Some(v) = args.get(key) {
            *slot = v
                .as_i64()
                .with_context(|| format!("{key} must be an integer"))?;
        }
        Ok(())
    };
    b("CompressorEnabled", &mut cfg.compressor_enabled)?;
    b("eqON", &mut cfg.eq_enabled)?;
    b("AGCEnabled", &mut cfg.agc_enabled)?;
    b("StereoEnhancerEnabled", &mut cfg.stereo_enabled)?;
    b("fxCompByPreset", &mut cfg.compressor_by_preset)?;
    i("fxCompPreset", &mut cfg.compressor_preset)?;
    i("fxDampPreset", &mut cfg.damp_preset)?;
    n("cThreshold", &mut cfg.threshold_db)?;
    n("cRatio", &mut cfg.ratio)?;
    n("cAttack", &mut cfg.attack_ms)?;
    n("cRelease", &mut cfg.release_ms)?;
    n("cGain", &mut cfg.gain_db)?;
    n("vol_Out", &mut cfg.vol_out)?;
    i("fxStereo", &mut cfg.stereo_amount)?;
    if let Some(v) = args.get("eqValues") {
        let list = v
            .as_array()
            .with_context(|| "eqValues must be an array of 10 numbers")?;
        if list.len() != 10 {
            anyhow::bail!("eqValues must contain exactly 10 values");
        }
        for (slot, item) in cfg.eq_values.iter_mut().zip(list) {
            *slot = item
                .as_f64()
                .with_context(|| "eqValues entries must be numbers")?;
        }
    }
    Ok(())
}

/// Persist `cfg` and rebuild the live engine chain.
fn apply(app: &App, cfg: ProcessorConfig) -> Result<()> {
    let path = settings_path(app);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut store = load_settings(app)?;
    store_from_config(&mut store, &cfg);
    store
        .save_at(&path)
        .with_context(|| format!("save {}", FX_SETTINGS_FILE))?;
    let processor = SoundProcessor::new(cfg.clone(), radiocraft_audio::SAMPLE_RATE as f64);
    app.engine.set_sound_processor(processor);
    *app.soundfx.write().expect("soundfx lock") = cfg;
    Ok(())
}

/// Boot-time hook (`App::load`): install the saved chain, if any effect
/// is enabled. With everything off the engine keeps its pass-through
/// (no processor), matching RadioDJ not attaching dormant BASS FX.
pub fn apply_at_boot(app: &App) -> Result<()> {
    let store = load_settings(app)?;
    let cfg = config_from_store(&store);
    *app.soundfx.write().expect("soundfx lock") = cfg.clone();
    if cfg.compressor_enabled
        || cfg.eq_enabled
        || cfg.agc_enabled
        || (cfg.stereo_enabled && cfg.stereo_amount > 1)
        || (cfg.vol_out - 50.0).abs() > f64::EPSILON
    {
        let processor = SoundProcessor::new(cfg, radiocraft_audio::SAMPLE_RATE as f64);
        app.engine.set_sound_processor(processor);
    }
    Ok(())
}

// ---- ops --------------------------------------------------------------------

/// `fx_get`: current master sound-processing config.
pub fn op_fx_get(app: &Arc<App>) -> Result<Value> {
    let cfg = app.soundfx.read().expect("soundfx lock").clone();
    Ok(json!({ "config": cfg_json(&cfg) }))
}

/// `fx_set`: partial update of the config; persists the settings file and
/// live-applies the new chain. Returns the full effective config.
pub fn op_fx_set(app: &Arc<App>, args: &Value) -> Result<Value> {
    let mut cfg = app.soundfx.read().expect("soundfx lock").clone();
    merge_args(&mut cfg, args)?;
    apply(app, cfg)?;
    let cfg = app.soundfx.read().expect("soundfx lock").clone();
    Ok(json!({ "config": cfg_json(&cfg) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_through_store() {
        let cfg = ProcessorConfig {
            compressor_enabled: true,
            eq_enabled: true,
            agc_enabled: true,
            stereo_enabled: true,
            compressor_by_preset: true,
            compressor_preset: 4,
            damp_preset: 2,
            threshold_db: -24.0,
            ratio: 6.0,
            attack_ms: 12.0,
            release_ms: 350.0,
            gain_db: 4.0,
            vol_out: 72.0,
            stereo_amount: 5,
            eq_values: [1.0, -2.0, 3.0, -4.0, 5.0, -6.0, 7.0, -8.0, 9.0, -10.0],
        };
        let mut store = PluginSettings::new();
        store_from_config(&mut store, &cfg);
        let back = config_from_store(&store);
        assert_eq!(back, cfg);
    }

    #[test]
    fn defaults_match_plugin_getsetting_defaults() {
        let store = PluginSettings::new();
        let cfg = config_from_store(&store);
        assert!(!cfg.compressor_enabled);
        assert!(!cfg.eq_enabled);
        assert!(!cfg.agc_enabled);
        assert!(!cfg.stereo_enabled);
        assert_eq!(cfg.stereo_amount, 1);
        assert_eq!(cfg.vol_out, 50.0);
        assert_eq!(cfg.ratio, 3.0);
        assert_eq!(cfg.attack_ms, 5.0);
        assert_eq!(cfg.release_ms, 200.0);
        assert_eq!(cfg.gain_db, 10.0);
        // Out-of-range raw default ("30") clamps into the UI range.
        assert_eq!(cfg.threshold_db, 0.0);
        assert_eq!(cfg.eq_values, [0.0; 10]);
    }
}
