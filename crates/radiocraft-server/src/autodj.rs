//! AutoDJ: the automatic track pick that keeps the main zone rolling when
//! the operator queue runs dry (docs/02 §3 contract).
//!
//! Split of responsibilities (docs/04): `radiocraft-core::scheduler` owns
//! the decision logic, `radiocraft-db::autodj` owns the data access, and
//! this module glues the two to the playout hub: plan -> play through the
//! normal `start` path so booking and now-playing behave exactly like an
//! operator-initiated play.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use chrono::Local;
use serde_json::{Value, json};

use radiocraft_core::scheduler::{CandidatePool, Scheduler, SchedulerDecision, TrackCandidate};
use radiocraft_db::{Db, autodj as db_autodj};

use crate::state::{App, MAIN_ZONE};

/// Plan the next AutoDJ slot at the current clock.
///
/// The core `Scheduler::plan` walks rotations/criteria internally, so the
/// data layer pre-fetches each active criteria query's candidate pool at
/// the current clock; pools are merged (deduped by song id) and the core
/// re-checks each criteria's predicates against the rows itself.
pub async fn plan(app: &App, db: &Db) -> Result<SchedulerDecision> {
    let config = app.autodj_config();
    let scheduler = Scheduler::new(config.clone());
    let now = Local::now().naive_local();

    let rotations = db_autodj::load_rotations(db.pool()).await?;
    let mut pool = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for rotation in radiocraft_core::rotation::select_rotations(&rotations, now) {
        for criteria in rotation.active_criteria(now) {
            let query = criteria.query_for(now, config.forced_song_type);
            for c in db_autodj::fetch_candidates(db.pool(), &query).await? {
                if seen.insert(c.id) {
                    pool.push(c);
                }
            }
        }
    }
    Ok(scheduler.plan(&rotations, &CandidatePool::new(pool), now, app.autodj_last_song()))
}

/// Wire view of a planning decision (`autodj_plan` dry-run payload).
pub fn decision_wire(d: &SchedulerDecision) -> Value {
    let pick = d.pick.as_ref().map(|p| {
        let c = &p.candidate;
        json!({
            "songId": c.id,
            "artist": c.artist,
            "title": c.title,
            "album": c.album,
            "path": c.path,
            "duration": c.duration,
            "weight": c.weight,
            "sweeperBefore": p.sweeper_before.as_ref().map(sweeper_wire),
            "sweeperWithin": p.sweeper_within.as_ref().map(sweeper_wire),
            "voiceTrack": p.voice_track.as_ref().map(sweeper_wire),
            "sweeperOffset": p.sweeper_offset_seconds,
        })
    });
    json!({
        "rotationId": d.rotation_id,
        "rotationName": d.rotation_name,
        "criteriaId": d.criteria_id,
        "pick": pick,
        "ordered": d
            .ordered
            .iter()
            .map(|c| json!({ "songId": c.id, "artist": c.artist, "title": c.title }))
            .collect::<Vec<_>>(),
        "rejected": d
            .rejected
            .iter()
            .map(|(id, reason)| json!({ "songId": id, "reason": reason }))
            .collect::<Vec<_>>(),
    })
}

fn sweeper_wire(c: &TrackCandidate) -> Value {
    json!({ "songId": c.id, "title": c.title, "path": c.path, "duration": c.duration })
}

/// Plan and roll the next AutoDJ pick on the main zone through the normal
/// play path (so now-playing bookkeeping and the `UpdateTracks2` booking
/// work exactly like an operator play).
pub async fn play_next(app: &Arc<App>, db: &Db) -> Result<Value> {
    let decision = plan(app, db).await?;
    let wire = decision_wire(&decision);
    let song_id = decision
        .pick
        .as_ref()
        .map(|p| p.candidate.id)
        .ok_or_else(|| anyhow::anyhow!("AutoDJ found no playable track"))?;

    let data = crate::dispatch::play_song(app, song_id).await?;
    app.note_autodj_pick(song_id);
    Ok(json!({ "now": data["now"].clone(), "plan": wire }))
}

/// The pump hook: once the main program went silent, keep it rolling when
/// AutoDJ is enabled and nothing is queued. A fill that is already in
/// flight is never overlapped.
pub async fn fill_on_idle(app: &Arc<App>) {
    if !app.autodj_enabled()
        || app.engine.is_playing(MAIN_ZONE)
        || app.current_now_playing().is_some()
    {
        return;
    }
    if !app.begin_autodj_fill() {
        return;
    }
    let result = fill(app).await;
    app.end_autodj_fill();
    match result {
        Ok(()) => {}
        Err(e) => {
            tracing::warn!("autodj fill: {e}");
            app.set_status(crate::state::Status::warn(format!("AutoDJ: {e}")))
                .await;
        }
    }
}

async fn fill(app: &Arc<App>) -> Result<()> {
    let db = match app.db.read().await.clone() {
        Some(db) => db,
        None => return Ok(()),
    };
    // Operator-queued material always wins over the AutoDJ fill.
    let (queued, _) = radiocraft_db::entities::queue_counts(db.pool()).await?;
    if queued > 0 {
        return Ok(());
    }
    play_next(app, &db).await?;
    app.set_status(crate::state::Status::info("AutoDJ: continuing the program"))
        .await;
    Ok(())
}

// ---- ws ops ---------------------------------------------------------------

/// `autodj_set`: enable/disable AutoDJ (optionally replacing the tuning
/// knobs); the switch persists as `AutoDJStart` so it survives restarts.
pub async fn op_set(app: &Arc<App>, args: &Value) -> Result<Value> {
    let enabled = args
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow::anyhow!("autodj_set needs a boolean 'enabled'"))?;
    app.set_general_setting("AutoDJStart", if enabled { "True" } else { "False" })?;
    app.set_autodj_enabled(enabled);
    if let Some(config) = args.get("config") {
        let config: radiocraft_core::scheduler::SchedulerConfig =
            serde_json::from_value(config.clone())
                .map_err(|e| anyhow::anyhow!("bad autodj config: {e}"))?;
        app.set_autodj_config(config);
    }
    Ok(json!({ "enabled": enabled, "config": app.autodj_config() }))
}

/// `autodj_status`: the current switch + tuning knobs.
pub fn op_status(app: &Arc<App>) -> Value {
    json!({
        "enabled": app.autodj_enabled(),
        "lastPickSongId": app.autodj_last_song(),
        "config": app.autodj_config(),
    })
}

/// `autodj_plan`: dry run of the next slot (no state change, no play).
pub async fn op_plan(app: &Arc<App>) -> Result<Value> {
    let db = app.require_db().await?;
    let decision = plan(app, &db).await?;
    let mut wire = decision_wire(&decision);
    wire["lastPickSongId"] = json!(app.autodj_last_song());
    Ok(wire)
}

/// `autodj_play_next`: plan and roll the pick immediately (manual kick).
pub async fn op_play_next(app: &Arc<App>) -> Result<Value> {
    let db = app.require_db().await?;
    play_next(app, &db).await
}
