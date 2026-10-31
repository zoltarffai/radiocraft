//! Rotation + criteria (AutoDJ rule) editing over WS — the RadioDJ
//! "Rotations" form surface.
//!
//! Storage model (`reference/schema/database.sql`): a rotation is a `rotations`
//! row; each rule line is a `rotations_list` row whose legacy columns cover the
//! RadioDJ-era fields, while `data` may hold a JSON object carrying the *full*
//! core `RotationCriteria` (dayparts, day masks, selection mode, weight floor —
//! the RadioCraft editor extension the `radiocraft-db::autodj` loader honours).
//!
//! `criteria_add` / `criteria_update` therefore accept the full core shape and
//! *denormalise*: the JSON goes to `data`, the RadioDJ-era subset is mirrored
//! into the legacy columns so external RadioDJ tools still see sane values.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};

use radiocraft_core::rotation::RotationCriteria as CoreCriteria;
use radiocraft_db::entities;

use crate::state::App;

/// `list_rotations`: every rotation header.
pub async fn op_list(app: &Arc<App>) -> Result<Value> {
    let db = app.require_db().await?;
    let rows = entities::list_rotations(db.pool()).await?;
    Ok(json!({
        "rotations": rows
            .iter()
            .map(|r| json!({ "ID": r.ID, "name": r.name }))
            .collect::<Vec<_>>(),
    }))
}

/// `rotation_create`: new empty rotation, args `{name}`.
pub async fn op_create(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let name = crate::dispatch::arg_str(args, "name")?;
    let id = entities::insert_rotation(db.pool(), &name).await?;
    Ok(json!({ "rotationId": id }))
}

/// `rotation_rename`: args `{rotationId, name}`.
pub async fn op_rename(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let id = crate::dispatch::arg_u32(args, "rotationId")?;
    let name = crate::dispatch::arg_str(args, "name")?;
    let rows = entities::rename_rotation(db.pool(), id, &name).await?;
    if rows == 0 {
        return Err(anyhow::anyhow!("no rotation with id {id}"));
    }
    Ok(json!({ "updated": rows }))
}

/// `rotation_delete`: drops the rotation plus its rule lines, args `{rotationId}`.
pub async fn op_delete(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let id = crate::dispatch::arg_u32(args, "rotationId")?;
    let rows = entities::delete_rotation(db.pool(), id).await?;
    if rows == 0 {
        return Err(anyhow::anyhow!("no rotation with id {id}"));
    }
    Ok(json!({ "removed": rows }))
}

/// `list_criteria`: the rule lines of one rotation, full core shape.
pub async fn op_list_criteria(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let id = crate::dispatch::arg_u32(args, "rotationId")?;
    let rows = entities::list_criteria(db.pool(), id).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let core = radiocraft_db::autodj::criteria_for_editor(row)?;
        out.push(serde_json::to_value(&core)?);
    }
    Ok(json!({ "criteria": out }))
}

/// Merge the wire payload onto the row's stored core criteria (`base`:
/// the current `data` JSON when editing, the core `Default` when adding;
/// unparseable bases fall back to defaults). `id`/`rotation_id` are
/// server-assigned below.
fn merge_criteria(args: &Value, id: u32, rotation_id: u32, base: &Value) -> Result<CoreCriteria> {
    let patch = args
        .get("criteria")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let mut merged = match base {
        Value::Object(map) => map.clone(),
        _ => match serde_json::to_value(CoreCriteria::default()) {
            Ok(Value::Object(map)) => map,
            Ok(_) => serde_json::Map::new(),
            Err(e) => return Err(anyhow::anyhow!("default criteria: {e}")),
        },
    };
    if let Some(patch_map) = patch.as_object() {
        for (k, v) in patch_map {
            merged.insert(k.clone(), v.clone());
        }
    }
    let mut core: CoreCriteria = serde_json::from_value(Value::Object(merged))
        .map_err(|e| anyhow::anyhow!("bad criteria payload: {e}"))?;
    core.id = i64::from(id);
    core.rotation_id = i64::from(rotation_id);
    Ok(core)
}

/// Persist a core criteria as a `rotations_list` row: JSON to `data`,
/// legacy-column mirror of the RadioDJ-era subset.
fn legacy_row(core: &CoreCriteria) -> entities::RotationCriteria {
    entities::RotationCriteria {
        ID: core.id as u32,
        pID: core.rotation_id as u32,
        catID: core.category_id as i32,
        subID: core.subcategory_id as i32,
        genID: core.genre_id as i32,
        mood: core.mood.clone(),
        gender: core.gender.clone(),
        language: core.language.clone(),
        // daypart columns stay 0 (= all day) in the RadioCraft model; the
        // JSON payload is the source of truth for the scheduler.
        start_type: 0,
        end_type: 0,
        // Legacy tinyint only distinguishes weighted vs shuffled.
        selType: matches!(core.selection, radiocraft_core::rotation::SelectionMode::Random),
        sweeper: false,
        repeatRule: core.repeat_rule > 0,
        ord: core.ord.clamp(0, 127) as i8,
        data: serde_json::to_string(core).unwrap_or_default(),
        track_separation: core.repeat_track_interval.clamp(0, i64::from(i32::MAX)) as i32,
        artist_separation: core.repeat_artist_interval.clamp(0, i64::from(i32::MAX)) as i32,
        title_separation: core.repeat_title_interval.clamp(0, i64::from(i32::MAX)) as i32,
        album_separation: core.repeat_album_interval.clamp(0, i64::from(i32::MAX)) as i32,
        advanced: None,
    }
}

/// `criteria_add`: append a rule line, args `{rotationId, criteria:{…}}`.
pub async fn op_criteria_add(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let rotation_id = crate::dispatch::arg_u32(args, "rotationId")?;
    let core = merge_criteria(args, 0, rotation_id, &Value::Null)?;
    let id = entities::insert_criteria(db.pool(), &legacy_row(&core)).await?;
    Ok(json!({ "criteriaId": id }))
}

/// `criteria_update`: rewrite one rule line, args `{criteria:{id,…}}`.
pub async fn op_criteria_update(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let id = args
        .get("criteria")
        .and_then(|c| c.get("id"))
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("criteria_update needs criteria.id"))? as u32;
    let existing = entities::get_criteria(db.pool(), id).await?;
    let Some(current) = existing.as_ref() else {
        return Err(anyhow::anyhow!("no criteria row with id {id}"));
    };
    // Base for the patch: decode the stored row first (legacy rows map in,
    // JSON `data` rows round-trip), then overlay the wire fields.
    let base = serde_json::to_value(radiocraft_db::autodj::criteria_for_editor(current)?)?;
    let core = merge_criteria(args, id, current.pID, &base)?;
    let affected = entities::update_criteria(db.pool(), &legacy_row(&core)).await?;
    Ok(json!({ "updated": affected }))
}

/// `criteria_delete`: drop one rule line, args `{criteriaId}`.
pub async fn op_criteria_delete(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let id = crate::dispatch::arg_u32(args, "criteriaId")?;
    let rows = entities::delete_criteria(db.pool(), id).await?;
    if rows == 0 {
        return Err(anyhow::anyhow!("no criteria row with id {id}"));
    }
    Ok(json!({ "removed": rows }))
}

/// `list_lookup`: picker data for the criteria editor (categories, all
/// subcategories, genres).
pub async fn op_lookup(app: &Arc<App>) -> Result<Value> {
    let db = app.require_db().await?;
    let cats = entities::list_categories(db.pool()).await?;
    let subs = entities::list_subcategories(db.pool(), None).await?;
    let gens = entities::list_genres(db.pool()).await?;
    Ok(json!({
        "categories": cats.iter().map(|c| json!({ "ID": c.ID, "name": c.name })).collect::<Vec<_>>(),
        "subcategories": subs.iter().map(|s| json!({
            "ID": s.ID, "parentid": s.parentid, "name": s.name,
        })).collect::<Vec<_>>(),
        "genres": gens.iter().map(|g| json!({ "ID": g.id, "name": g.name })).collect::<Vec<_>>(),
    }))
}
