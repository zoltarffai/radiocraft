//! Command dispatch: the ops a UI invokes over `{"t":"cmd",...}`.
//!
//! Every op maps 1:1 onto the [`radiocraft_audio::engine::PlayoutEngine`]
//! or the db entity layer; nothing here duplicates business logic.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Local};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use radiocraft_audio::mixer::Zone;
use radiocraft_db::booking::{BookingParams, book_track};
use radiocraft_db::entities::{LibraryFilter, LibrarySort, NewSong};

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
        "play_cart" => play_cart(app, args).await,
        "segue_now" => {
            let zone = arg_zone(args)?;
            app.engine.segue_now(zone, arg_i32(args, "fade", -1))?;
            Ok(json!({}))
        }
        "stop" => {
            let zone = arg_zone(args)?;
            app.engine.stop(zone, arg_i32(args, "fade", -1));
            if matches!(zone, Zone::Cart) {
                app.clear_cart_now();
            }
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
        "get_db_config" => {
            let cfg = app.db_cfg.read().await;
            Ok(json!({
                "host": cfg.host,
                "port": cfg.port,
                "database": cfg.database,
                "username": cfg.username,
                "password": cfg.password,
            }))
        }
        "db_configure" => {
            {
                let mut cfg = app.db_cfg.write().await;
                if let Some(v) = args.get("host").and_then(Value::as_str) {
                    cfg.host = v.to_string();
                }
                if let Some(v) = args.get("port").and_then(Value::as_u64) {
                    cfg.port = v as u16;
                }
                if let Some(v) = args.get("database").and_then(Value::as_str) {
                    cfg.database = v.to_string();
                }
                if let Some(v) = args.get("username").and_then(Value::as_str) {
                    cfg.username = v.to_string();
                }
                if let Some(v) = args.get("password").and_then(Value::as_str) {
                    cfg.password = v.to_string();
                }
            }
            app.persist_db_cfg().await?;
            Ok(json!({}))
        }
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
        "library_filter" => {
            let db = app.require_db().await?;
            let f = library_filter_args(args);
            let (total, songs) = radiocraft_db::entities::filter_songs(db.pool(), &f).await?;
            Ok(json!({ "total": total, "songs": songs }))
        }
        "library_get" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "songId")?;
            let song = radiocraft_db::entities::song_by_id(db.pool(), id)
                .await?
                .ok_or_else(|| anyhow!("no song with id {id}"))?;
            Ok(json!({ "song": song }))
        }
        "library_add" => {
            let db = app.require_db().await?;
            let raw = arg_str(args, "path")?;
            let path = PathBuf::from(&raw);
            if !path.exists() {
                return Err(anyhow!("no such file: {raw}"));
            }
            if radiocraft_db::entities::song_by_path(db.pool(), &raw).await?.is_some() {
                return Err(anyhow!("file already in library: {raw}"));
            }
            // duration comes from the file itself; original_duration keeps
            // RadioDJ's "length = length" insert form
            let (_, duration) = request_for_file(&path, &CueKnobs::default())?;
            let (auto_artist, auto_title) = split_stem(&path);
            let artist = args
                .get("artist")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or(auto_artist);
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or(auto_title);
            let song = NewSong {
                path: raw.clone(),
                song_type: args
                    .get("songType")
                    .and_then(Value::as_i64)
                    .unwrap_or(0) as i8,
                id_subcat: args.get("idSubcat").and_then(Value::as_u64).unwrap_or(0) as u32,
                id_genre: args.get("idGenre").and_then(Value::as_u64).unwrap_or(0) as u32,
                artist: artist.clone(),
                original_artist: args
                    .get("originalArtist")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                title: title.clone(),
                album: args.get("album").and_then(Value::as_str).unwrap_or("").to_string(),
                duration: decimal_from_f64(duration),
                original_duration: decimal_from_f64(duration),
                enabled: true,
                weight: arg_decimal(args, "weight", Decimal::from(100)),
                ..NewSong::default()
            };
            let id = radiocraft_db::entities::insert_song(db.pool(), &song).await?;
            Ok(json!({ "songId": id, "artist": artist, "title": title }))
        }
        "library_update" => {
            let db = app.require_db().await?;
            let song: radiocraft_db::entities::Song =
                serde_json::from_value(args.get("song").cloned().unwrap_or(Value::Null))
                    .context("library_update needs a full 'song' row")?;
            radiocraft_db::entities::update_song(db.pool(), &song).await?;
            Ok(json!({ "songId": song.ID }))
        }
        "library_delete" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "songId")?;
            let rows = radiocraft_db::entities::delete_song(db.pool(), id).await?;
            Ok(json!({ "removed": rows }))
        }
        "library_mass_set" => {
            let db = app.require_db().await?;
            let ids = arg_ids(args)?;
            let field = args
                .get("field")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("library_mass_set needs a 'field'"))?;
            let rows = match field {
                "enabled" => {
                    let enabled = args
                        .get("value")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| anyhow!("enabled needs a boolean value"))?;
                    radiocraft_db::entities::mass_set_enabled(db.pool(), &ids, enabled).await?
                }
                "weight" => {
                    let weight = arg_decimal(args, "value", Decimal::ZERO);
                    radiocraft_db::entities::mass_set_weight(db.pool(), &ids, weight).await?
                }
                "genre" => {
                    let v = arg_value_u32(args)?;
                    radiocraft_db::entities::mass_set_genre(db.pool(), &ids, v).await?
                }
                "subcat" => {
                    let v = arg_value_u32(args)?;
                    radiocraft_db::entities::mass_set_subcategory(db.pool(), &ids, v).await?
                }
                other => return Err(anyhow!("unsupported mass field {other}")),
            };
            Ok(json!({ "updated": rows }))
        }
        "library_mass_delete" => {
            let db = app.require_db().await?;
            let ids = arg_ids(args)?;
            let rows = radiocraft_db::entities::mass_delete(db.pool(), &ids).await?;
            Ok(json!({ "removed": rows }))
        }

        // ---- history ------------------------------------------------------------
        "list_history" => {
            let db = app.require_db().await?;
            let (limit, offset) = page(args);
            let rows = radiocraft_db::entities::list_history(db.pool(), limit, offset).await?;
            Ok(json!({ "history": rows }))
        }
        "history_filter" => {
            let db = app.require_db().await?;
            let (limit, offset) = page(args);
            let text = args.get("text").and_then(Value::as_str).unwrap_or("");
            let (total, rows) = radiocraft_db::entities::filter_history(db.pool(), text, limit, offset).await?;
            Ok(json!({ "total": total, "history": rows }))
        }
        "history_delete" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "historyId")?;
            let rows = radiocraft_db::entities::delete_history_row(db.pool(), id).await?;
            if rows == 0 {
                return Err(anyhow!("no history row with id {id}"));
            }
            Ok(json!({ "removed": rows }))
        }
        "history_add" => {
            let db = app.require_db().await?;
            let song_id = arg_u32(args, "songId")?;
            let song_type = arg_i32(args, "songType", 0);
            let artist = args.get("artist").and_then(Value::as_str).unwrap_or("");
            let title = args.get("title").and_then(Value::as_str).unwrap_or("");
            let id = radiocraft_db::entities::add_history_row(db.pool(), song_id, song_type, artist, title).await?;
            Ok(json!({ "historyId": id }))
        }
        "play_history" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "historyId")?;
            let entry = radiocraft_db::entities::get_history_entry(db.pool(), id)
                .await?
                .ok_or_else(|| anyhow!("no history row with id {id}"))?;
            // The referenced song may have been deleted after the play was
            // booked: restore the history row and report, like RadioDJ's
            // "play from history" on a missing file.
            if radiocraft_db::entities::song_by_id(db.pool(), entry.trackID).await?.is_none() {
                let new_id = radiocraft_db::entities::add_history_row(
                    db.pool(),
                    entry.trackID,
                    i32::from(entry.song_type),
                    &entry.artist,
                    &entry.title,
                )
                .await?;
                return Ok(json!({ "restored": new_id }));
            }
            start(app, &json!({"songId": entry.trackID}), false).await
        }
        "play_next" => {
            // "Play Next" on the currently-loaded item: park it in the queue.
            let db = app.require_db().await?;
            let playing = app.current_now_playing().ok_or_else(|| anyhow!("nothing is loaded"))?;
            if playing.song_id <= 0 {
                return Err(anyhow!("the loaded item is not in the library"));
            }
            let song = radiocraft_db::entities::song_by_id(db.pool(), playing.song_id as u32)
                .await?
                .ok_or_else(|| anyhow!("the loaded song is no longer in the library"))?;
            let id = radiocraft_db::entities::requeue_song(db.pool(), &song).await?;
            Ok(json!({ "queued": id }))
        }

        // ---- playlists & carts -------------------------------------------------
        "list_playlists" => {
            let db = app.require_db().await?;
            let playlists = radiocraft_db::entities::list_playlists(db.pool()).await?;
            Ok(json!({ "playlists": playlists }))
        }
        "playlist_create" => {
            let db = app.require_db().await?;
            let name = arg_str(args, "name")?;
            let id = radiocraft_db::entities::insert_playlist(db.pool(), &name).await?;
            Ok(json!({ "playlistId": id }))
        }
        "playlist_rename" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "playlistId")?;
            let name = arg_str(args, "name")?;
            radiocraft_db::entities::rename_playlist(db.pool(), id, &name).await?;
            Ok(json!({ "playlistId": id }))
        }
        "playlist_delete" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "playlistId")?;
            radiocraft_db::entities::delete_playlist(db.pool(), id).await?;
            Ok(json!({ "playlistId": id }))
        }
        "list_playlist_items" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "playlistId")?;
            let items = radiocraft_db::entities::list_playlist_items_with_songs(db.pool(), id).await?;
            Ok(json!({ "items": items }))
        }
        "playlist_add_item" => {
            let db = app.require_db().await?;
            let pid = arg_u32(args, "playlistId")?;
            let song_id = arg_i32(args, "songId", 0);
            if song_id > 0 && radiocraft_db::entities::song_by_id(db.pool(), song_id as u32).await?.is_none() {
                return Err(anyhow!("no song with id {song_id}"));
            }
            let id = radiocraft_db::entities::append_playlist_song(db.pool(), pid, song_id).await?;
            Ok(json!({ "itemId": id }))
        }
        "playlist_remove_item" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "itemId")?;
            if id == 0 {
                return Err(anyhow!("itemId must identify a row"));
            }
            let rows = radiocraft_db::entities::delete_playlist_item(db.pool(), id).await?;
            if rows == 0 {
                return Err(anyhow!("no playlist row with id {id}"));
            }
            Ok(json!({ "removed": rows }))
        }
        "playlist_move_item" => {
            let db = app.require_db().await?;
            let pid = arg_u32(args, "playlistId")?;
            let from = arg_u32(args, "from")? as usize;
            let to = arg_u32(args, "to")? as usize;
            let rows = radiocraft_db::entities::list_playlist_items(db.pool(), pid).await?;
            let mut ids: Vec<u32> = rows.iter().map(|r| r.ID).collect();
            if from == 0 || from > ids.len() || to > ids.len() {
                return Err(anyhow!("position out of range"));
            }
            let item = ids.remove(from - 1);
            ids.insert(to - 1, item);
            radiocraft_db::entities::reindex_playlist_items(db.pool(), &ids).await?;
            Ok(json!({ "moved": item }))
        }
        "playlist_set_cues" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "itemId")?;
            if radiocraft_db::entities::get_playlist_item(db.pool(), id).await?.is_none() {
                return Err(anyhow!("no playlist row with id {id}"));
            }
            let rows = radiocraft_db::entities::update_playlist_item_cues(
                db.pool(),
                id,
                arg_decimal(args, "cstart", Decimal::ZERO),
                arg_decimal(args, "cnext", Decimal::ZERO),
                arg_decimal(args, "cend", Decimal::ZERO),
                arg_decimal(args, "fin", Decimal::ZERO),
                arg_decimal(args, "fout", Decimal::ZERO),
            )
            .await?;
            Ok(json!({ "updated": rows }))
        }
        "playlist_set_song" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "itemId")?;
            let song_id = arg_i32(args, "songId", 0);
            if song_id > 0 && radiocraft_db::entities::song_by_id(db.pool(), song_id as u32).await?.is_none() {
                return Err(anyhow!("no song with id {song_id}"));
            }
            let rows = radiocraft_db::entities::set_playlist_item_song(db.pool(), id, song_id).await?;
            if rows == 0 {
                return Err(anyhow!("no playlist row with id {id}"));
            }
            Ok(json!({ "updated": rows }))
        }
        "list_carts" => {
            let db = app.require_db().await?;
            let carts = radiocraft_db::entities::list_carts(db.pool()).await?;
            Ok(json!({ "carts": carts }))
        }
        "list_cart_buttons" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "cartId")?;
            let buttons = radiocraft_db::entities::list_cart_buttons_with_songs(db.pool(), id).await?;
            Ok(json!({ "buttons": buttons }))
        }

        // ---- play queue ----------------------------------------------------------
        "list_queue" => {
            let db = app.require_db().await?;
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(200);
            let items = radiocraft_db::entities::list_queue(db.pool(), limit).await?;
            Ok(json!({ "items": items }))
        }
        "queue_add_song" => {
            let db = app.require_db().await?;
            let song_id = arg_u32(args, "songId")?;
            let song = radiocraft_db::entities::song_by_id(db.pool(), song_id)
                .await?
                .ok_or_else(|| anyhow!("no song with id {song_id}"))?;
            let id = radiocraft_db::entities::requeue_song(db.pool(), &song).await?;
            Ok(json!({ "queued": id }))
        }
        "queue_add_file" => {
            let db = app.require_db().await?;
            let raw = arg_str(args, "path")?;
            let path = PathBuf::from(&raw);
            if !path.exists() {
                return Err(anyhow!("no such file: {raw}"));
            }
            let (duration, song_id) = match radiocraft_db::entities::song_by_path(db.pool(), &raw).await? {
                Some(s) => (s.duration, 0),
                None => {
                    let (_, d) = request_for_file(&path, &CueKnobs::default())?;
                    (decimal_from_f64(d), 0)
                }
            };
            let (artist, title) = split_stem(&path);
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or(title);
            let item = radiocraft_db::entities::NewQueueItem {
                songID: song_id,
                ETA: Some(Local::now().naive_local()),
                duration,
                artist,
                associated_artists: String::new(),
                title,
                album: String::new(),
                swID: -1,
                swPlay: Decimal::ZERO,
                vtID: -1,
                vtPlay: Decimal::ZERO,
            };
            let id = radiocraft_db::entities::enqueue(db.pool(), &item).await?;
            Ok(json!({ "queued": id }))
        }
        "queue_remove" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "queueId")?;
            let rows = radiocraft_db::entities::delete_queue_item(db.pool(), id).await?;
            if rows == 0 {
                return Err(anyhow!("no queue row with id {id}"));
            }
            Ok(json!({ "removed": rows }))
        }
        "queue_clear" => {
            let db = app.require_db().await?;
            let rows = radiocraft_db::entities::clear_queue(db.pool()).await?;
            Ok(json!({ "removed": rows }))
        }
        "queue_eta_set" => {
            let db = app.require_db().await?;
            let id = arg_u32(args, "queueId")?;
            let eta = arg_datetime(args, "eta")?;
            let rows = radiocraft_db::entities::set_queue_item_eta(db.pool(), id, eta).await?;
            if rows == 0 {
                return Err(anyhow!("no queue row with id {id}"));
            }
            Ok(json!({ "updated": rows }))
        }
        "queue_move" => {
            let db = app.require_db().await?;
            let a = arg_u32(args, "queueId")?;
            let b = arg_u32(args, "with")?;
            radiocraft_db::entities::swap_queue_item_etas(db.pool(), a, b).await?;
            Ok(json!({ "swapped": [a, b] }))
        }

        // ---- autodj ---------------------------------------------------------
        "autodj_set" => crate::autodj::op_set(app, args).await,
        "autodj_status" => Ok(crate::autodj::op_status(app)),
        "autodj_plan" => crate::autodj::op_plan(app).await,
        "autodj_play_next" => crate::autodj::op_play_next(app).await,

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

/// Play a library song on the main zone (AutoDJ fill uses this so the
/// now-playing bookkeeping and `UpdateTracks2` booking are identical to
/// an operator-initiated play).
pub async fn play_song(app: &Arc<App>, song_id: i64) -> Result<Value> {
    start(app, &json!({ "songId": song_id }), false).await
}

/// Play (or stop) the track linked to a cartwall button, mirroring
/// RadioDJ's `CartButtonControl.set_PlayCart`: pressing the rolled button
/// stops it; otherwise the linked `songs` row rolls on the dedicated cart
/// zone (auto-ducking the main program) and history is booked at start via
/// the lightweight `UpdateHistory` insert — carts never enter the main
/// now-playing hub, so the program item keeps running underneath.
async fn play_cart(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let button = if let Some(button_id) = args.get("buttonId").and_then(Value::as_u64) {
        radiocraft_db::entities::cart_button_by_id(db.pool(), button_id as u32).await?
    } else if let Some(cart_id) = args.get("cartId").and_then(Value::as_u64) {
        let number = arg_u32(args, "number")?;
        radiocraft_db::entities::cart_button_by_number(db.pool(), cart_id as u32, number).await?
    } else {
        return Err(anyhow!("play_cart needs a 'buttonId', or a 'cartId' + 'number'"));
    }
    .ok_or_else(|| anyhow!("no such cart button"))?;
    if button.swID == 0 {
        return Err(anyhow!("cart button {} is not assigned", button.swButton));
    }

    // Toggle: pressing the rolled button stops the cart with a fade.
    if app.cart_now().is_some_and(|c| c.button_id == button.ID) && app.engine.is_playing(Zone::Cart)
    {
        app.engine.stop(Zone::Cart, arg_i32(args, "fade", -1));
        app.clear_cart_now();
        return Ok(json!({ "stopped": true, "buttonId": button.ID }));
    }

    let song = radiocraft_db::entities::song_by_id(db.pool(), button.swID)
        .await?
        .ok_or_else(|| anyhow!("cart button {} links to missing song {}", button.swButton, button.swID))?;
    let path = PathBuf::from(&song.path);
    if !path.exists() {
        return Err(anyhow!("no such file: {}", song.path));
    }

    // RadioDJ books carts at play start (UpdateHistory), not at stop.
    let history_id = radiocraft_db::entities::add_history_row(
        db.pool(),
        song.ID,
        i32::from(song.song_type),
        &song.artist,
        &song.title,
    )
    .await?;

    let knobs = knobs_from_song(&song);
    let (req, duration) = request_for_file(&path, &knobs)?;
    app.engine
        .play(Zone::Cart, req, i32::from(song.song_type), song.overlay)?;
    let cart = crate::state::CartNow {
        button_id: button.ID,
        cart_id: button.pID,
        song_id: i64::from(song.ID),
        title: song.title.clone(),
    };
    app.set_cart_now(cart.clone());
    Ok(json!({
        "cart": cart.info(),
        "historyId": history_id,
        "duration_secs": duration,
    }))
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

fn arg_u32(args: &Value, key: &str) -> Result<u32> {
    args.get(key)
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .map(|v| v as u32)
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

/// Mass-operation id lists: `{"ids":[1,2,3]}`, non-empty.
fn arg_ids(args: &Value) -> Result<Vec<u32>> {
    let ids = args
        .get("ids")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("ids must be an array"))?
        .iter()
        .map(|v| v.as_u64().and_then(|v| u32::try_from(v).ok()))
        .collect::<Option<Vec<u32>>>()
        .ok_or_else(|| anyhow!("ids must be positive ids"))?;
    if ids.is_empty() {
        return Err(anyhow!("ids must not be empty"));
    }
    Ok(ids)
}

/// Decimal argument with default (json numbers or numeric strings).
fn arg_decimal(args: &Value, name: &str, default: Decimal) -> Decimal {
    args.get(name)
        .map(|v| match v {
            Value::Number(_) => serde_json::from_value(v.clone()).unwrap_or(default),
            Value::String(s) => s.parse::<Decimal>().unwrap_or(default),
            _ => default,
        })
        .unwrap_or(default)
}

/// Required numeric `value` field (mass-set genre/subcat style).
fn arg_value_u32(args: &Value) -> Result<u32> {
    args.get("value")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| anyhow!("value must be a positive id"))
}

/// ETA timestamp: RFC3339 or `YYYY-MM-DD HH:MM:SS` (local time).
fn arg_datetime(args: &Value, name: &str) -> Result<chrono::NaiveDateTime> {
    let raw = arg_str(args, name)?;
    match DateTime::parse_from_rfc3339(&raw) {
        Ok(d) => Ok(d.naive_local()),
        Err(_) => chrono::NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S")
            .map_err(|_| anyhow!("{name} must be an RFC3339 or 'YYYY-MM-DD HH:MM:SS' timestamp")),
    }
}

/// Map a LibraryFilter from wire args (`{text,idCategory,idSubcat,idGenre,idRotation,enabled,sort,limit,offset}`).
fn library_filter_args(args: &Value) -> LibraryFilter {
    let (limit, offset) = page(args);
    LibraryFilter {
        text: args.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
        category: args.get("idCategory").and_then(Value::as_u64).unwrap_or(0) as u32,
        subcategory: args.get("idSubcat").and_then(Value::as_u64).unwrap_or(0) as u32,
        genre: args.get("idGenre").and_then(Value::as_u64).unwrap_or(0) as u32,
        rotation: args.get("idRotation").and_then(Value::as_u64).unwrap_or(0) as u32,
        enabled: args.get("enabled").and_then(Value::as_bool),
        order: match args.get("sort").and_then(Value::as_str) {
            Some("title") => LibrarySort::Title,
            Some("album") => LibrarySort::Album,
            Some("duration") => LibrarySort::Duration,
            Some("year") => LibrarySort::Year,
            Some("weight") => LibrarySort::Weight,
            Some("count") => LibrarySort::Count,
            _ => LibrarySort::Artist,
        },
        limit,
        offset,
    }
}

/// f64 -> Decimal helper for durations parsed from audio files.
fn decimal_from_f64(v: f64) -> Decimal {
    use std::str::FromStr as _;
    let v = v.max(0.0);
    // round-trip through the shortest string form to avoid binary noise
    Decimal::from_str(&format!("{v:.3}")).unwrap_or(Decimal::ZERO)
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
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rc-dispatch-{}-{nanos}", std::process::id()));
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
    async fn db_reads_error_without_connection() {
        let app = tmp_app();
        for (op, args) in [
            ("list_playlists", json!({})),
            ("list_playlist_items", json!({"playlistId": 1})),
            ("list_carts", json!({})),
            ("list_cart_buttons", json!({"cartId": 1})),
        ] {
            let frame = dispatch(&app, 9, op, &args).await;
            match frame {
                Frame::Error { message, .. } => {
                    assert!(message.contains("database"), "{op}: {message}");
                }
                other => panic!("{op}: expected error, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn db_configure_round_trips() {
        let app = tmp_app();
        let frame = dispatch(
            &app,
            10,
            "db_configure",
            &json!({"host": "db.example", "port": 3307, "database": "radio", "username": "u", "password": "p"}),
        )
        .await;
        assert!(matches!(frame, Frame::Result { .. }), "{frame:?}");
        // persisted and re-readable via the same dir
        let reopened = Arc::new(App::load(app.config_dir()).expect("reload"));
        let frame = dispatch(&reopened, 11, "get_db_config", &json!({})).await;
        match frame {
            Frame::Result { data, .. } => {
                assert_eq!(data["host"], "db.example");
                assert_eq!(data["port"], 3307);
                assert_eq!(data["database"], "radio");
                assert_eq!(data["username"], "u");
                assert_eq!(data["password"], "p");
            }
            other => panic!("expected result, got {other:?}"),
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
