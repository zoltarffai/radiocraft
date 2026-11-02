//! Cartwall management parity with RadioDJ's built-in cart wall:
//! wall (cart) CRUD, per-wall button layouts, and the wall config
//! (grid size, stop-fade length, auto-roll).
//!
//! Walls and buttons live in the same `carts` / `carts_list` pair the
//! core player and the `Plugin_InstantPlayers` plugin use, so an
//! existing RadioDJ database carries over unchanged. Saving a layout
//! follows the plugin's `CartControl` pattern: one `DELETE FROM
//! carts_list WHERE pID=...` followed by one `INSERT ... SET pID, swID,
//! swButton, color` per filled button.
//!
//! The config is stored in `settings_cartwall.xml` (same
//! `NewDataSet/Settings` layout as every other settings file). The core
//! app's own wall settings live in encrypted string resources, so these
//! key names follow the `Plugin_InstantPlayers` config window
//! (`Columns`, `Rows`, `StopFadeDuration`) plus `AutoRoll` for the core
//! "roll the next button automatically" option.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context as _, Result};
use serde_json::{json, Value};

use radiocraft_settings::PluginSettings;

use crate::state::App;

/// Config file for the built-in cart wall (under the config dir).
pub const CARTWALL_SETTINGS_FILE: &str = "settings_cartwall.xml";

/// RadioDJ `Var.swColumns` default for the wall grid.
pub const DEFAULT_COLUMNS: i32 = 4;
/// RadioDJ `Var.swRows` default for the wall grid.
pub const DEFAULT_ROWS: i32 = 6;
/// Stop-fade length in seconds (`NMStopFadeDuration` in the config form).
pub const DEFAULT_FADE_SECS: i32 = 1;

/// The editable wall config, matching the numeric controls in RadioDJ's
/// cartwall/InstantPlayers config window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CartwallConfig {
    pub columns: i32,
    pub rows: i32,
    /// Fade length (seconds) used when a rolled button is pressed again.
    pub stop_fade_secs: i32,
    /// Automatically advance to the next button when one finishes.
    pub auto_roll: bool,
}

impl Default for CartwallConfig {
    fn default() -> Self {
        CartwallConfig {
            columns: DEFAULT_COLUMNS,
            rows: DEFAULT_ROWS,
            stop_fade_secs: DEFAULT_FADE_SECS,
            auto_roll: false,
        }
    }
}

fn settings_path(app: &App) -> PathBuf {
    app.config_dir.join(CARTWALL_SETTINGS_FILE)
}

/// Parse the config out of a raw key/value store, mirroring the
/// `GetSetting` defaults in the plugin's config form.
pub fn config_from_store(store: &PluginSettings) -> CartwallConfig {
    CartwallConfig {
        columns: store.get_as("Columns", DEFAULT_COLUMNS),
        rows: store.get_as("Rows", DEFAULT_ROWS),
        stop_fade_secs: store.get_as("StopFadeDuration", DEFAULT_FADE_SECS),
        auto_roll: store.get_as("AutoRoll", false),
    }
}

/// Read the stored config; missing file or keys fall back to defaults.
pub fn load_config(app: &App) -> Result<CartwallConfig> {
    let path = settings_path(app);
    if path.exists() {
        let store = PluginSettings::load(&path).with_context(|| CARTWALL_SETTINGS_FILE)?;
        Ok(config_from_store(&store))
    } else {
        Ok(CartwallConfig::default())
    }
}

/// Largest button slot the current grid can hold.
pub fn max_slots(cfg: &CartwallConfig) -> i32 {
    cfg.columns.max(0).saturating_mul(cfg.rows.max(0))
}

/// `cartwall_get_config`: the current wall config (defaults if unset).
pub fn op_get(app: &Arc<App>) -> Result<Value> {
    let cfg = *app.cartwall.read().expect("cartwall lock");
    Ok(json!({
        "columns": cfg.columns,
        "rows": cfg.rows,
        "fade": cfg.stop_fade_secs,
        "autoRoll": cfg.auto_roll,
    }))
}

/// `cartwall_set_config`: validate, persist, and live-swap the config.
pub fn op_set(app: &Arc<App>, args: &Value) -> Result<Value> {
    let mut cfg = *app.cartwall.read().expect("cartwall lock");
    if let Some(v) = args.get("columns").and_then(Value::as_i64) {
        let v = i32::try_from(v).unwrap_or(i32::MAX);
        if !(1..=20).contains(&v) {
            return Err(anyhow!("columns must be between 1 and 20"));
        }
        cfg.columns = v;
    }
    if let Some(v) = args.get("rows").and_then(Value::as_i64) {
        let v = i32::try_from(v).unwrap_or(i32::MAX);
        if !(1..=20).contains(&v) {
            return Err(anyhow!("rows must be between 1 and 20"));
        }
        cfg.rows = v;
    }
    if let Some(v) = args.get("fade").and_then(Value::as_i64) {
        let v = i32::try_from(v).unwrap_or(-1);
        if !(0..=30).contains(&v) {
            return Err(anyhow!("fade must be between 0 and 30 seconds"));
        }
        cfg.stop_fade_secs = v;
    }
    if let Some(v) = args.get("autoRoll").and_then(Value::as_bool) {
        cfg.auto_roll = v;
    }

    let path = settings_path(app);
    let mut store = if path.exists() {
        PluginSettings::load(&path).with_context(|| CARTWALL_SETTINGS_FILE)?
    } else {
        PluginSettings::new()
    };
    store.set_typed("Columns", cfg.columns);
    store.set_typed("Rows", cfg.rows);
    store.set_typed("StopFadeDuration", cfg.stop_fade_secs);
    store.set_typed("AutoRoll", cfg.auto_roll);
    store
        .save_at(&path)
        .with_context(|| format!("saving {CARTWALL_SETTINGS_FILE}"))?;

    *app.cartwall.write().expect("cartwall lock") = cfg;
    Ok(json!({
        "columns": cfg.columns,
        "rows": cfg.rows,
        "fade": cfg.stop_fade_secs,
        "autoRoll": cfg.auto_roll,
    }))
}

/// `cartwall_create`: add a `carts` wall row.
pub async fn op_create(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("cartwall_create needs a non-empty 'name'"))?;
    let id = radiocraft_db::entities::insert_cart(db.pool(), name).await?;
    Ok(json!({ "cartId": id, "name": name }))
}

/// `cartwall_rename`: rename one `carts` row.
pub async fn op_rename(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let id = arg_u32(args, "cartId")?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("cartwall_rename needs a non-empty 'name'"))?;
    let rows = radiocraft_db::entities::rename_cart(db.pool(), id, name).await?;
    if rows == 0 {
        return Err(anyhow!("no cart wall with id {id}"));
    }
    Ok(json!({ "updated": rows }))
}

/// `cartwall_delete`: drop the wall and all of its buttons, matching the
/// plugin's `DELETE FROM carts_list ...; DELETE FROM carts ...` pair.
pub async fn op_delete(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let id = arg_u32(args, "cartId")?;
    let rows = radiocraft_db::entities::delete_cart(db.pool(), id).await?;
    if rows == 0 {
        return Err(anyhow!("no cart wall with id {id}"));
    }
    Ok(json!({ "deleted": rows }))
}

/// `cartwall_save_buttons`: replace a wall's full button layout. Each
/// entry is `{"slot": n, "songId": id, "color": argb}`; only assigned
/// buttons are inserted, exactly like the plugin's save loop.
pub async fn op_save_buttons(app: &Arc<App>, args: &Value) -> Result<Value> {
    let cart_id = arg_u32(args, "cartId")?;
    let buttons = args
        .get("buttons")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("cartwall_save_buttons needs a 'buttons' array"))?;

    let cfg = *app.cartwall.read().expect("cartwall lock");
    let limit = max_slots(&cfg);
    let mut specs = Vec::with_capacity(buttons.len());
    let mut seen_slots = Vec::with_capacity(buttons.len());
    for entry in buttons {
        let slot = entry
            .get("slot")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("every cart button needs a 'slot'"))?;
        if slot < 1 || slot > i64::from(limit) {
            return Err(anyhow!(
                "slot must be between 1 and {limit} for a {}x{} wall",
                cfg.columns,
                cfg.rows
            ));
        }
        let slot = slot as u32;
        if seen_slots.contains(&slot) {
            return Err(anyhow!("duplicate button slot {slot}"));
        }
        seen_slots.push(slot);
        let song_id = entry.get("songId").and_then(Value::as_i64).unwrap_or(0);
        if song_id < 1 {
            return Err(anyhow!("button {slot} is not assigned to a song"));
        }
        let color = entry
            .get("color")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        specs.push(radiocraft_db::entities::NewCartButton {
            swID: song_id as u32,
            swButton: slot,
            color,
        });
    }

    let db = app.require_db().await?;
    if radiocraft_db::entities::cart_by_id(db.pool(), cart_id)
        .await?
        .is_none()
    {
        return Err(anyhow!("no cart wall with id {cart_id}"));
    }
    for spec in &specs {
        if radiocraft_db::entities::song_by_id(db.pool(), spec.swID)
            .await?
            .is_none()
        {
            return Err(anyhow!("no song with id {}", spec.swID));
        }
    }
    let inserted =
        radiocraft_db::entities::replace_cart_buttons(db.pool(), cart_id, &specs).await?;
    Ok(json!({ "cartId": cart_id, "buttons": inserted }))
}

fn arg_u32(args: &Value, key: &str) -> Result<u32> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| anyhow!("{key} must be a positive integer"))
}
