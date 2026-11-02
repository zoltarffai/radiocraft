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

use radiocraft_audio::is_stream_path;
use radiocraft_audio::mixer::Zone;
use radiocraft_db::booking::{BookingParams, book_track};
use radiocraft_db::entities::{LibraryFilter, LibrarySort, NewSong};

use crate::frames::Frame;
use crate::playback::{
    CueKnobs, build_cue_times, cue_tokens_from, knobs_from_song, request_for_file,
    request_for_source, split_stem,
};
use crate::state::{App, NowPlaying, Status, parse_zone, zone_name};

/// `songs.song_type` value marking an internet stream — RadioDJ's
/// `TrackTypes.InternetStream` enum index.
pub const STREAM_TRACK_TYPE: i32 = 5;

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
        "play_instant" => play_instant(app, args).await,
        "segue_now" => {
            let zone = arg_zone(args)?;
            app.engine.segue_now(zone, arg_i32(args, "fade", -1))?;
            Ok(json!({}))
        }
        "stop" => {
            let zone = arg_zone(args)?;
            app.engine.stop(zone, arg_i32(args, "fade", -1));
            match zone {
                Zone::Cart => app.clear_cart_now(),
                Zone::Instant => {
                    app.clear_instant_now();
                    app.release_program_duck();
                }
                _ => {}
            }
            Ok(json!({}))
        }
        "stop_all" => {
            app.engine.stop_all(arg_i32(args, "fade", -1));
            app.release_program_duck();
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
        "stream_add" => {
            // RadioDJ keeps internet streams as ordinary library rows with
            // `song_type = TrackTypes.InternetStream` (5) and the URL in
            // `path`; there is no separate streams table.
            let db = app.require_db().await?;
            let url = arg_str(args, "url")?;
            if !is_stream_path(&url) {
                return Err(anyhow!("stream url must start with http:// or https://"));
            }
            if radiocraft_db::entities::song_by_path(db.pool(), &url).await?.is_some() {
                return Err(anyhow!("stream already in library: {url}"));
            }
            let song = NewSong {
                path: url.clone(),
                song_type: STREAM_TRACK_TYPE as i8,
                id_subcat: args.get("idSubcat").and_then(Value::as_u64).unwrap_or(0) as u32,
                id_genre: args.get("idGenre").and_then(Value::as_u64).unwrap_or(0) as u32,
                artist: args
                    .get("artist")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("")
                    .to_string(),
                title: args
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&url)
                    .to_string(),
                album: args.get("album").and_then(Value::as_str).unwrap_or("").to_string(),
                duration: Decimal::ZERO,
                original_duration: Decimal::ZERO,
                enabled: true,
                weight: arg_decimal(args, "weight", Decimal::from(100)),
                ..NewSong::default()
            };
            let id = radiocraft_db::entities::insert_song(db.pool(), &song).await?;
            Ok(json!({ "songId": id, "path": url }))
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

        // ---- library analysis (Plugin_TracksManager analyze tracks) -----------
        "analyze_tracks" => {
            let db = app.require_db().await?;
            // TracksManager threshold spinners (dB, min -100); defaults
            // selDbIn=-24, selDbNext=-5, selDbOut=-30.
            let th = radiocraft_audio::cuepoints::CueThresholds {
                start_db: args.get("dbIn").and_then(Value::as_f64).unwrap_or(-24.0),
                next_db: args.get("dbNext").and_then(Value::as_f64).unwrap_or(-5.0),
                end_db: args.get("dbEnd").and_then(Value::as_f64).unwrap_or(-30.0),
            };
            // Selected rows: explicit ids. Otherwise RadioDJ's sweep query:
            // every library row except carts/jingles-type buckets 8,12,13,11,9,5.
            let ids: Vec<u32> = match args.get("songIds") {
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|v| {
                        v.as_u64()
                            .map(|v| u32::try_from(v).ok())
                            .ok_or_else(|| anyhow!("songIds must be positive ids"))
                            .and_then(|v| v.ok_or_else(|| anyhow!("songIds must be positive ids")))
                    })
                    .collect::<Result<Vec<u32>>>()?,
                _ => sqlx::query_scalar::<_, u32>(
                    "SELECT `ID` FROM `songs` WHERE `song_type` NOT IN (8,12,13,11,9,5)",
                )
                .fetch_all(db.pool())
                .await?,
            };
            // RadioDJ's `bwAnalyzeTracks` background worker: one sweep at a
            // time, and it must not block this connection's command loop
            // (decoding can run for minutes while transport keeps working).
            if app.analyze_busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
                anyhow::bail!("track analyzis already running");
            }
            let total = ids.len() as u64;
            let app = Arc::clone(app);
            tokio::spawn(async move {
                let report = analyze_sweep(&app, &db, &ids, &th).await;
                app.analyze_busy.store(false, std::sync::atomic::Ordering::SeqCst);
                if let Some(status) = report["status"].as_str() {
                    app.set_status(Status::info(status)).await;
                }
                let _ = app.tx.send(Frame::Result {
                    id: 0,
                    op: "analyze_tracks".into(),
                    data: report,
                });
            });
            Ok(json!({ "started": total }))
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

        // ---- cart wall ------------------------------------------------------------
        "cartwall_get_config" => crate::cartwall::op_get(app),
        "cartwall_set_config" => crate::cartwall::op_set(app, args),
        "aux_players_get_config" => crate::auxplayers::op_get_config(app),
        "aux_players_set_config" => crate::auxplayers::op_set_config(app, args),
        "aux_players_load" => crate::auxplayers::op_load(app, args).await,
        "aux_players_play" => crate::auxplayers::op_play(app, args).await,
        "aux_players_stop" => crate::auxplayers::op_stop(app, args),
        "aux_players_seek" => crate::auxplayers::op_seek(app, args),
        "aux_players_loop" => crate::auxplayers::op_loop(app, args),
        "aux_players_clear" => crate::auxplayers::op_clear(app, args),
        "cartwall_create" => crate::cartwall::op_create(app, args).await,
        "cartwall_rename" => crate::cartwall::op_rename(app, args).await,
        "cartwall_delete" => crate::cartwall::op_delete(app, args).await,
        "cartwall_save_buttons" => crate::cartwall::op_save_buttons(app, args).await,

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
            let is_stream = is_stream_path(&raw);
            let path = PathBuf::from(&raw);
            if !is_stream && !path.exists() {
                return Err(anyhow!("no such file: {raw}"));
            }
            let (duration, song_id) = match radiocraft_db::entities::song_by_path(db.pool(), &raw).await? {
                Some(s) => (s.duration, 0),
                None if is_stream => (Decimal::ZERO, 0),
                None => {
                    let (_, d) = request_for_file(&path, &CueKnobs::default())?;
                    (decimal_from_f64(d), 0)
                }
            };
            let (auto_artist, auto_title) = if is_stream {
                (String::new(), raw.clone())
            } else {
                split_stem(&path)
            };
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

        // ---- rotations / criteria ------------------------------------------
        "list_rotations" => crate::rotations::op_list(app).await,
        "rotation_create" => crate::rotations::op_create(app, args).await,
        "rotation_rename" => crate::rotations::op_rename(app, args).await,
        "rotation_delete" => crate::rotations::op_delete(app, args).await,
        "list_criteria" => crate::rotations::op_list_criteria(app, args).await,
        "criteria_add" => crate::rotations::op_criteria_add(app, args).await,
        "criteria_update" => crate::rotations::op_criteria_update(app, args).await,
        "criteria_delete" => crate::rotations::op_criteria_delete(app, args).await,
        "list_lookup" => crate::rotations::op_lookup(app).await,

        // ---- settings editors (XML-backed options) ----------------------------
        "list_devices" => crate::settings::op_list_devices(app),
        "set_device" => crate::settings::op_set_device(app, args),
        "soundcards_get" => crate::settings::op_soundcards_get(app),
        "soundcards_set" => crate::settings::op_soundcards_set(app, args),
        "sound_options_get" => crate::settings::op_sound_options_get(app),
        "sound_options_set" => crate::settings::op_sound_options_set(app, args),
        "fx_get" => crate::soundfx::op_fx_get(app),
        "fx_set" => crate::soundfx::op_fx_set(app, args),
        "export_get" => crate::settings::op_export_get(app),
        "export_set" => crate::settings::op_export_set(app, args),
        "export_test" => crate::settings::op_export_test(app).await,

        // ---- plugin manager (external cdylib plugins) ------------------------
        "plugin_list" => plugin_op_list(app).await,
        "plugin_load" => plugin_op_load(app, args).await,
        "plugin_unload" => plugin_op_unload(app, args).await,
        "plugin_invoke" => plugin_op_invoke(app, args).await,

        other => Err(anyhow!("unknown op {other}")),
    }
}

// ---- plugin ops -------------------------------------------------------------
//
// The manager methods round-trip over a channel to the plugin thread with
// `blocking_recv`, so they run on the blocking pool, never on a worker.

/// Shared: clone the manager handle out of `App` (error if not attached).
fn plugin_mgr(app: &Arc<App>) -> Result<Arc<crate::plugin_host::PluginManager>> {
    app.plugin_manager()
        .map(Arc::clone)
        .context("plugin manager is not running")
}

async fn plugin_op_list(app: &Arc<App>) -> Result<Value> {
    let mgr = plugin_mgr(app)?;
    tokio::task::spawn_blocking(move || mgr.list())
        .await
        .context("plugin thread join")
}

async fn plugin_op_load(app: &Arc<App>, args: &Value) -> Result<Value> {
    let mgr = plugin_mgr(app)?;
    let raw = args.get("path").and_then(Value::as_str).context("path required")?;
    // Relative paths resolve against the Plugins folder the scan uses.
    let path = {
        let p = PathBuf::from(raw);
        if p.is_absolute() {
            p
        } else {
            app.config_dir.join("Plugins").join(p)
        }
    };
    let id = tokio::task::spawn_blocking(move || mgr.load(path))
        .await
        .context("plugin thread join")?
        .map_err(anyhow::Error::msg)?;
    Ok(json!({ "id": id }))
}

async fn plugin_op_unload(app: &Arc<App>, args: &Value) -> Result<Value> {
    let mgr = plugin_mgr(app)?;
    let id = args.get("id").and_then(Value::as_u64).context("id required")?;
    let done = tokio::task::spawn_blocking(move || mgr.unload(id))
        .await
        .context("plugin thread join")?;
    Ok(json!({ "unloaded": done }))
}

async fn plugin_op_invoke(app: &Arc<App>, args: &Value) -> Result<Value> {
    let mgr = plugin_mgr(app)?;
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .context("action required")?
        .to_string();
    let call_args: Vec<String> = args
        .get("args")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| v.to_string()).collect())
        .unwrap_or_default();
    let matched = tokio::task::spawn_blocking(move || mgr.invoke(&action, &call_args))
        .await
        .context("plugin thread join")?
        .map_err(anyhow::Error::msg)?;
    Ok(json!({ "matched": matched }))
}

// ---- play / cue -------------------------------------------------------------

/// Start (or segue-cue) a track: `songId` from the library or a raw `path`
/// for out-of-library testing (the old AutoDJ `Now playing` of one file).
async fn start(app: &Arc<App>, args: &Value, follow: bool) -> Result<Value> {
    let zone = arg_zone(args)?;
    let patch = arg_cues(args);

    let (raw_path, mut playing) = if let Some(song_id) = args.get("songId").and_then(Value::as_i64) {
        let db = app.require_db().await?;
        let song = radiocraft_db::entities::song_by_id(db.pool(), song_id as u32)
            .await?
            .ok_or_else(|| anyhow!("no song with id {song_id}"))?;
        let raw_path = song.path.clone();
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
        (raw_path, playing)
    } else if let Some(raw) = args.get("path").and_then(Value::as_str) {
        if radiocraft_audio::is_stream_path(raw) {
            // Ad-hoc stream play (no library row): the title is the URL
            // unless the caller supplies one.
            let playing = NowPlaying {
                song_id: 0,
                artist: String::new(),
                title: args
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .unwrap_or(raw)
                    .to_string(),
                album: String::new(),
                path: raw.to_string(),
                track_type: STREAM_TRACK_TYPE,
                duration_secs: 0.0,
                started_at: Local::now(),
                cue_start: 0.0,
                cue_next: 0.0,
                cue_end: 0.0,
                fade_in_ms: 0,
                fade_out_ms: 0,
            };
            (raw.to_string(), playing)
        } else {
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
            (raw.to_string(), playing)
        }
    } else {
        return Err(anyhow!("play needs a 'songId' or a 'path'"));
    };

    // Opening a stream fetches the HTTP header on this (blocking) thread,
    // never a Tokio worker.
    let knobs = cue_knobs_of(&playing, patch.as_ref());
    let (req, duration) = tokio::task::spawn_blocking(move || request_for_source(&raw_path, &knobs))
        .await
        .context("open source thread join")??;
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
    if !is_stream_path(&song.path) && !PathBuf::from(&song.path).exists() {
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
    let (raw_path, knobs) = (song.path.clone(), knobs);
    let (req, duration) = tokio::task::spawn_blocking(move || request_for_source(&raw_path, &knobs))
        .await
        .context("open source thread join")??;
    app.engine
        .play(Zone::Cart, req, i32::from(song.song_type), song.overlay)?;
    let cart = crate::state::CartNow {
        button_id: button.ID,
        cart_id: button.pID,
        song_id: i64::from(song.ID),
        title: song.title.clone(),
    };
    app.set_cart_now(cart.clone());
    // RadioDJ's sweeper button ducks the main program for the track length
    // (`ReduceVolumeForSeconds(duration, true, -10f)`).
    // Infinite streams have no duration to time the duck against; leave the
    // program level alone and let the button toggle release it.
    if duration > 0.0 {
        app.apply_program_duck((duration * 1000.0) as i64);
    }
    Ok(json!({
        "cart": cart.info(),
        "historyId": history_id,
        "duration_secs": duration,
    }))
}

/// Play (or stop) the track behind an instant/cart wall button, mirroring
/// `Plugin_InstantPlayers.CartControl.ButtonClick` +
/// `CartButtonControl.set_PlayCart`: the linked `songs` row rolls on the
/// dedicated instant zone (own pause/seek controls, main program ducked for
/// the track length), history is booked at start, and the rolled button
/// toggles off with the configured `StopFadeDuration` fade. Buttons resolve
/// through the shared `carts`/`carts_list` pair like cartwalls do.
async fn play_instant(app: &Arc<App>, args: &Value) -> Result<Value> {
    let db = app.require_db().await?;
    let button = if let Some(button_id) = args.get("buttonId").and_then(Value::as_u64) {
        radiocraft_db::entities::cart_button_by_id(db.pool(), button_id as u32).await?
    } else if let Some(cart_id) = args.get("cartId").and_then(Value::as_u64) {
        let number = arg_u32(args, "number")?;
        radiocraft_db::entities::cart_button_by_number(db.pool(), cart_id as u32, number).await?
    } else {
        return Err(anyhow!("play_instant needs a 'buttonId', or a 'cartId' + 'number'"));
    }
    .ok_or_else(|| anyhow!("no such instant button"))?;

    // Toggle: pressing the rolled button stops the insert with a fade.
    if app.instant_now().is_some_and(|c| c.button_id == button.ID)
        && app.engine.is_playing(Zone::Instant)
    {
        let fade = {
            let cfg = app.cartwall.read().expect("cartwall lock");
            (cfg.stop_fade_secs.max(0) as i64) * 1000
        };
        app.engine.stop(Zone::Instant, fade as i32);
        app.clear_instant_now();
        app.release_program_duck();
        return Ok(json!({ "stopped": true, "buttonId": button.ID }));
    }

    let song = radiocraft_db::entities::song_by_id(db.pool(), button.swID)
        .await?
        .ok_or_else(|| {
            anyhow!("instant button {} links to missing song {}", button.swButton, button.swID)
        })?;
    if !is_stream_path(&song.path) && !PathBuf::from(&song.path).exists() {
        return Err(anyhow!("no such file: {}", song.path));
    }

    // History is booked at play start, like cart buttons do.
    let history_id = radiocraft_db::entities::add_history_row(
        db.pool(),
        song.ID,
        i32::from(song.song_type),
        &song.artist,
        &song.title,
    )
    .await?;

    let knobs = knobs_from_song(&song);
    let (raw_path, knobs) = (song.path.clone(), knobs);
    let (req, duration) = tokio::task::spawn_blocking(move || request_for_source(&raw_path, &knobs))
        .await
        .context("open source thread join")??;
    app.engine
        .play(Zone::Instant, req, i32::from(song.song_type), song.overlay)?;
    let instant = crate::state::CartNow {
        button_id: button.ID,
        cart_id: button.pID,
        song_id: i64::from(song.ID),
        title: song.title.clone(),
    };
    app.set_instant_now(instant.clone());
    // Infinite streams have no duration to time the duck against; leave the
    // program level alone and let the button toggle release it.
    if duration > 0.0 {
        app.apply_program_duck((duration * 1000.0) as i64);
    }
    Ok(json!({
        "instant": instant.info(),
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

// ---- analyze sweep (RadioDJ bwAnalyzeTracks_DoWork) --------------------------

/// Decode + re-cue every id, writing `cue_times`/`duration` per row.
/// Progress and the completion line are pushed through `set_status`
/// (RadioDJ's `lblStatus`/`Application.DoEvents` loop).
async fn analyze_sweep(
    app: &App,
    db: &radiocraft_db::Db,
    ids: &[u32],
    th: &radiocraft_audio::cuepoints::CueThresholds,
) -> Value {
    let total = ids.len();
    let mut analyzed = 0usize;
    let mut skipped = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for (n, id) in ids.iter().enumerate() {
        let Some(mut song) = (match radiocraft_db::entities::song_by_id(db.pool(), *id).await {
            Ok(song) => song,
            Err(e) => {
                errors.push(format!("id {id}: {e}"));
                skipped += 1;
                continue;
            }
        }) else {
            skipped += 1;
            continue;
        };
        // SaveTrackData's switch: song types 5,8,9,11,12,13 are
        // never analyzed, and missing files are skipped silently.
        if matches!(song.song_type, 5 | 8 | 9 | 11 | 12 | 13)
            || song.path.is_empty()
            || !std::path::Path::new(&song.path).is_file()
        {
            skipped += 1;
            continue;
        }
        app.set_status(Status::info(format!(
            "Analyzing {}/{total}: {}",
            n + 1,
            song.path
        )))
        .await;
        let path = std::path::PathBuf::from(&song.path);
        let th = *th;
        let triple = match tokio::task::spawn_blocking(move || {
            radiocraft_audio::cuepoints::set_cue_points_file(&path, &th)
        })
        .await
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                // SetCuePoints returning false: row left untouched.
                errors.push(format!("{}: {e}", song.path));
                skipped += 1;
                continue;
            }
            Err(e) => {
                errors.push(format!("{}: {e}", song.path));
                skipped += 1;
                continue;
            }
        };
        // Rebuild cue_times in SaveTrackData's token layout, keeping
        // the tokens the analyzer does not touch, and book the
        // analyzed play length into `duration` (CueNext - CueStart).
        let mut tokens = cue_tokens_from(&song.cue_times);
        tokens.start = triple.cue_start;
        tokens.next = triple.cue_next;
        tokens.end = triple.cue_end;
        song.cue_times = build_cue_times(&tokens);
        song.duration = decimal_from_f64(triple.cue_next - triple.cue_start);
        if let Err(e) = radiocraft_db::entities::update_song(db.pool(), &song).await {
            errors.push(format!("{}: save failed: {e}", song.path));
            skipped += 1;
            continue;
        }
        analyzed += 1;
    }
    json!({
        "analyzed": analyzed,
        "skipped": skipped,
        "total": total,
        "errors": errors,
        "status": format!("Tracks analyzis completed! ({analyzed} analyzed, {skipped} skipped)"),
    })
}

// ---- arg helpers --------------------------------------------------------------

fn arg_zone(args: &Value) -> Result<Zone> {
    match args.get("zone").and_then(Value::as_str) {
        None => Ok(Zone::Main),
        Some(name) => parse_zone(name).ok_or_else(|| anyhow!("unknown zone {name}")),
    }
}

pub fn arg_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("missing string arg {key}"))
}

pub(crate) fn arg_f64(args: &Value, key: &str) -> Result<f64> {
    args.get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .ok_or_else(|| anyhow!("missing numeric arg {key}"))
}

pub fn arg_u32(args: &Value, key: &str) -> Result<u32> {
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
    // RadioDJ assigns durations through `new decimal(double)`, which is the
    // 15-significant-digit G15 form; the DECIMAL(11,5) column then rounds to
    // 5 places. Matching that keeps e.g. 6.99984073638916 -> 6.99984 instead
    // of pre-rounding to 3 places (which would give 7.00000).
    Decimal::from_str(&crate::playback::format_f64(v)).unwrap_or(Decimal::ZERO)
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
        if key == "StationName" {
            // The now-playing templates render $station_name$; rebuild so
            // the new name takes effect without a restart.
            self.rebuild_exporter();
        }
        let _ = self.tx.send(self.state_frame());
        Ok(())
    }

    /// Push the settings knobs the engine reads live back into it.
    pub fn reapply_engine_knobs(&self) {
        let general = self.general.read().expect("general lock");
        self.engine
            .set_master_db(crate::state::volume_out_to_db(*general.volume_out()));
        self.engine
            .set_default_fade_out_ms(*general.dur_fadeout());
        self.engine
            .set_overlay_db(f64::from(*general.vol_overlay()));
        self.engine
            .set_duck_db(f64::from(*general.vol_onsweeper()));
        self.engine.set_vt_duck_db(f64::from(*general.vol_vt()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_from_f64_keeps_five_place_durations() {
        // A sweep computes duration = cue_next - cue_start in f64. Storage must
        // keep G15 precision so the DECIMAL(11,5) column rounds to 6.99984;
        // the old 3-place pre-round turned it into 7.00000.
        assert_eq!(decimal_from_f64(6.99984073638916), "6.99984073638916".parse::<Decimal>().unwrap());
        assert_eq!(decimal_from_f64(-1.0), Decimal::ZERO);
        // Round-trips through scientific notation too (tiny durations).
        assert_eq!(decimal_from_f64(0.0000226757369614512), "0.0000226757369614512".parse::<Decimal>().unwrap());
    }

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
            ("cartwall_create", json!({"name": "x"})),
            ("cartwall_rename", json!({"cartId": 1, "name": "y"})),
            ("cartwall_delete", json!({"cartId": 1})),
            ("cartwall_save_buttons", json!({"cartId": 1, "buttons": []})),
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

    /// Cartwall config round-trips through the plugin's settings file with
    /// RadioDJ's key names and defaults (Columns 4 / Rows 6).
    #[tokio::test]
    async fn cartwall_config_round_trips() {
        let app = tmp_app();
        let frame = dispatch(&app, 1, "cartwall_get_config", &json!({})).await;
        let cfg = match frame {
            Frame::Result { data, .. } => data,
            other => panic!("expected ok, got {other:?}"),
        };
        assert_eq!(cfg["columns"], 4);
        assert_eq!(cfg["rows"], 6);
        assert_eq!(cfg["fade"], 1);

        let frame = dispatch(
            &app,
            2,
            "cartwall_set_config",
            &json!({"columns": 8, "rows": 3, "fade": 2, "autoRoll": true}),
        )
        .await;
        assert!(matches!(frame, Frame::Result { .. }));
        assert!(app
            .config_dir
            .join(crate::cartwall::CARTWALL_SETTINGS_FILE)
            .exists());

        let frame = dispatch(&app, 3, "cartwall_get_config", &json!({})).await;
        match frame {
            Frame::Result { data, .. } => {
                assert_eq!(data["columns"], 8);
                assert_eq!(data["rows"], 3);
                assert_eq!(data["fade"], 2);
                assert_eq!(data["autoRoll"], true);
            }
            other => panic!("expected ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cartwall_buttons_reject_bad_slots() {
        let app = tmp_app();
        for (buttons, needle) in [
            (json!([{"slot": 0, "songId": 1}]), "slot must be"),
            (json!([{"slot": 1, "songId": 0}]), "not assigned"),
        ] {
            let frame = dispatch(
                &app,
                4,
                "cartwall_save_buttons",
                &json!({"cartId": 1, "buttons": buttons}),
            )
            .await;
            match frame {
                Frame::Error { message, .. } => {
                    assert!(message.contains(needle), "{message}");
                }
                other => panic!("expected error, got {other:?}"),
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
    async fn plugin_ops_without_manager_are_errors() {
        let app = tmp_app();
        for (op, args) in [
            ("plugin_list", json!({})),
            ("plugin_load", json!({"path": "foo.so"})),
            ("plugin_unload", json!({"id": 1})),
            ("plugin_invoke", json!({"action": "Ping", "args": []})),
        ] {
            let frame = dispatch(&app, 4, op, &args).await;
            match frame {
                Frame::Error { message, .. } => {
                    assert!(message.contains("plugin manager"), "{op}: {message}");
                }
                other => panic!("{op}: expected error, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn plugin_ops_answer_with_manager_attached() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rc-dispatch-plug-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let app = Arc::new(App::load(&dir).expect("load"));
        let mgr = crate::plugin_host::PluginManager::start(Arc::clone(&app));
        app.attach_plugins(mgr);

        // list round-trips to the plugin thread through the manager's own
        // spawn_blocking inside the op; the test just awaits the frame.
        let frame = dispatch(&app, 5, "plugin_list", &json!({})).await;
        match frame {
            Frame::Result { data, .. } => assert_eq!(data["plugins"], json!([])),
            other => panic!("expected result, got {other:?}"),
        }

        // A non-library path fails the ABI gate instead of panicking.
        let bogus = dir.join("Plugins").join("bogus.so");
        std::fs::create_dir_all(dir.join("Plugins")).unwrap();
        std::fs::write(&bogus, b"not a shared object").unwrap();
        let frame = dispatch(&app, 6, "plugin_load", &json!({"path": "bogus.so"})).await;
        match frame {
            Frame::Error { message, .. } => assert!(!message.is_empty(), "{message}"),
            other => panic!("expected error, got {other:?}"),
        }

        // Unknown ids report unloaded:false rather than an error.
        let frame = dispatch(&app, 7, "plugin_unload", &json!({"id": 999})).await;
        match frame {
            Frame::Result { data, .. } => assert_eq!(data["unloaded"], json!(false)),
            other => panic!("expected result, got {other:?}"),
        }

        app.plugin_manager().expect("manager").shutdown();
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
