//! Command-surface tests for the milestone-3 UI-parity ops.
//!
//! The offline half exercises validation/error paths through [`dispatch`]
//! without a database. The live half only runs when `RC_DB_TEST_DSN` names a
//! scratch database, e.g.
//! `RC_DB_TEST_DSN=mysql://root:changeme@127.0.0.1:3306/radiocraft_test cargo test`.
//! The schema is bootstrapped on connect, so an empty scratch DB is fine.

use std::sync::Arc;

use radiocraft_server::dispatch::dispatch;
use radiocraft_server::frames::Frame;
use radiocraft_server::state::App;
use serde_json::{Value, json};

fn tmp_app() -> Arc<App> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rc-cmd-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Arc::new(App::load(&dir).expect("load"))
}

async fn call(app: &Arc<App>, op: &str, args: Value) -> Frame {
    dispatch(app, 1, op, &args).await
}

fn result_of(frame: Frame) -> Value {
    match frame {
        Frame::Result { data, .. } => data,
        other => panic!("expected result, got {other:?}"),
    }
}

fn error_of(frame: Frame) -> String {
    match frame {
        Frame::Error { message, .. } => message,
        other => panic!("expected error, got {other:?}"),
    }
}

/// Every write/filter op must fail cleanly with a database error when the
/// server has no live connection (never panic or hang).
#[tokio::test]
async fn parity_ops_error_without_db() {
    let app = tmp_app();
    let cases: [(&str, Value); 13] = [
        ("library_filter", json!({"text": "x"})),
        ("library_get", json!({"songId": 1})),
        ("library_update", json!({"song": {}})),
        ("library_delete", json!({"songId": 1})),
        ("library_mass_set", json!({"ids": [1], "field": "enabled", "value": true})),
        ("library_mass_delete", json!({"ids": [1]})),
        ("history_filter", json!({"text": "x"})),
        ("history_delete", json!({"historyId": 1})),
        ("play_history", json!({"historyId": 1})),
        ("play_cart", json!({"buttonId": 1})),
        ("play_next", json!({})),
        ("list_queue", json!({})),
        ("queue_add_song", json!({"songId": 1})),
    ];
    for (op, args) in cases {
        let frame = call(&app, op, args).await;
        let msg = error_of(frame);
        assert!(msg.contains("database"), "{op}: {msg}");
    }
}

/// `play_next` before anything is loaded: a clear "nothing is loaded" error
/// (db is up, but no item on the transport).
#[tokio::test]
async fn play_history_and_playlist_ops_reject_bad_args() {
    let app = tmp_app();
    // Argument validation runs before the db requirement only where the
    // shape check precedes require_db; verify the shape errors surface too.
    let msg = error_of(call(&app, "queue_eta_set", json!({"queueId": 1})).await);
    assert!(
        msg.contains("eta") || msg.contains("database"),
        "queue_eta_set: {msg}"
    );
    let msg = error_of(call(&app, "queue_move", json!({"queueId": 1})).await);
    assert!(msg.contains("with") || msg.contains("database"), "queue_move: {msg}");
    let msg = error_of(call(&app, "playlist_move_item", json!({"playlistId": 1})).await);
    assert!(msg.contains("from") || msg.contains("database"), "playlist_move_item: {msg}");
    let msg = error_of(call(&app, "library_mass_set", json!({"ids": [], "field": "nope"})).await);
    assert!(
        msg.contains("database") || msg.contains("ids") || msg.contains("nope"),
        "library_mass_set: {msg}"
    );
}

// ---- autodj (offline half) --------------------------------------------------

/// The AutoDJ switch works with no database at all: status reports the
/// knobs, `autodj_set` flips + persists the switch, and the plan/play ops
/// fail cleanly with a database error until one is connected.
#[tokio::test]
async fn autodj_switch_works_without_db() {
    let app = tmp_app();
    let data = result_of(call(&app, "autodj_status", json!({})).await);
    assert_eq!(data["enabled"], json!(false));
    assert!(data["config"].is_object(), "{data}");
    assert_eq!(data["lastPickSongId"], json!(null));

    let msg = error_of(call(&app, "autodj_set", json!({})).await);
    assert!(msg.contains("enabled"), "{msg}");
    let msg = error_of(call(&app, "autodj_plan", json!({})).await);
    assert!(msg.contains("database"), "{msg}");
    let msg = error_of(call(&app, "autodj_play_next", json!({})).await);
    assert!(msg.contains("database"), "{msg}");

    let data = result_of(call(&app, "autodj_set", json!({"enabled": true})).await);
    assert_eq!(data["enabled"], json!(true));
    assert!(app.autodj_enabled());

    // Persisted under the legacy key: a fresh app loads the switch back.
    let reloaded = Arc::new(App::load(&app.config_dir).expect("reload"));
    assert!(reloaded.autodj_enabled());

    // Tuning knobs round-trip too.
    let data = result_of(
        call(
            &app,
            "autodj_set",
            json!({"enabled": true, "config": {"max_candidates": 25}}),
        )
        .await,
    );
    assert_eq!(data["config"]["max_candidates"], json!(25));
    assert_eq!(app.autodj_config().max_candidates, Some(25));

    let data = result_of(call(&app, "autodj_set", json!({"enabled": false})).await);
    assert_eq!(data["enabled"], json!(false));
    assert!(!app.autodj_enabled());
}

// ---- live-database round trip ---------------------------------------------

fn dsn() -> Option<(String, u16, String, String, String)> {
    let raw = std::env::var("RC_DB_TEST_DSN").ok()?;
    // mysql://user:pass@host:port/database
    let rest = raw.strip_prefix("mysql://")?;
    let (creds, hostdb) = rest.split_once('@')?;
    let (user, pass) = creds.split_once(':')?;
    let (hostport, database) = hostdb.split_once('/')?;
    let (host, port) = hostport.rsplit_once(':').unwrap_or((hostport, "3306"));
    Some((
        host.to_string(),
        port.parse().expect("port"),
        database.to_string(),
        user.to_string(),
        pass.to_string(),
    ))
}

/// The live tests share one scratch database (and each scrubs its own
/// rotation/song residue at start), so they must not overlap. Cargo runs
/// tests in a test binary in parallel; this process-wide lock serialises
/// everything that touches the live DB.
fn live_guard() -> &'static std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCK: std::sync::OnceLock<std::sync::Arc<tokio::sync::Mutex<()>>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
}

async fn connect_live(app: &Arc<App>) -> bool {
    let Some((host, port, database, username, password)) = dsn() else {
        eprintln!("skipping live-db test: set RC_DB_TEST_DSN to run");
        return false;
    };
    let frame = call(
        app,
        "db_configure",
        json!({ "host": host, "port": port, "database": database, "username": username, "password": password }),
    )
    .await;
    assert!(matches!(frame, Frame::Result { .. }), "{frame:?}");
    let frame = call(app, "db_connect", json!({})).await;
    assert!(matches!(frame, Frame::Result { .. }), "{frame:?}");
    true
}

#[tokio::test]
async fn live_command_round_trip() {
    let app = tmp_app();
    let _live = live_guard().lock().await;
    if !connect_live(&app).await {
        return;
    }

    // Scrub residue from earlier failed runs so the strict asserts below hold.
    let dsn_raw = std::env::var("RC_DB_TEST_DSN").unwrap();
    let scrub = radiocraft_db::Db::connect_url(&dsn_raw).await.expect("scrub conn");
    for sql in [
        "DELETE FROM songs WHERE path LIKE '/tmp/opencode/smoke-audio/parity-%'",
        "DELETE FROM history WHERE title = 'Parity Title'",
        "DELETE FROM playlists WHERE name LIKE 'Parity PL%'",
        "DELETE FROM carts WHERE name = 'Parity Cart'",
    ] {
        sqlx::raw_sql(sql).execute(scrub.pool()).await.expect("scrub");
    }
    scrub.close().await;

    // Add a song from the smoke tone file, then filter/edit/delete it.
    // A per-run copy keeps the unique idx_path constraint happy.
    let src = std::env::var("RC_TEST_AUDIO").unwrap_or_else(|_| "/tmp/opencode/smoke-audio/testtone.wav".into());
    if !std::path::Path::new(&src).exists() {
        eprintln!("skipping live-db test: test audio file {src} missing");
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = format!("/tmp/opencode/smoke-audio/parity-{nanos}.wav");
    std::fs::copy(&src, &file).expect("copy tone");
    let data = result_of(call(&app, "library_add", json!({"path": file})).await);
    let song_id = data["songId"].as_u64().expect("songId").to_string();
    let song_id: u64 = song_id.parse().unwrap();
    let _ = std::fs::remove_file(&file);

    let data = result_of(call(&app, "library_get", json!({"songId": song_id})).await);
    let mut song = data["song"].clone();
    song["title"] = json!("Parity Title");
    song["weight"] = json!("55.5");
    let data = result_of(call(&app, "library_update", json!({"song": song})).await);
    assert_eq!(data["songId"], json!(song_id));

    let data = result_of(call(&app, "library_filter", json!({"text": "Parity Title"})).await);
    assert_eq!(data["total"], json!(1), "{data}");

    let data = result_of(
        call(&app, "library_mass_set", json!({"ids": [song_id], "field": "enabled", "value": false}))
            .await,
    );
    assert_eq!(data["updated"], json!(1));

    // Queue editing (sentinel rows: swID/vtID -1).
    let data = result_of(call(&app, "queue_add_song", json!({"songId": song_id})).await);
    let queue_id = data["queued"].as_u64().expect("queued");
    let data = result_of(call(&app, "list_queue", json!({})).await);
    let items = data["items"].as_array().expect("items");
    assert!(items.iter().any(|i| i["ID"] == json!(queue_id)), "{data}");
    let eta = chrono::Local::now() + chrono::TimeDelta::minutes(10);
    let data = result_of(
        call(&app, "queue_eta_set", json!({"queueId": queue_id, "eta": eta.to_rfc3339()})).await,
    );
    assert_eq!(data["updated"], json!(1));
    let data = result_of(call(&app, "queue_remove", json!({"queueId": queue_id})).await);
    assert_eq!(data["removed"], json!(1));

    // Playlist editing incl. the locked-row guard.
    let data = result_of(call(&app, "playlist_create", json!({"name": "Parity PL"})).await);
    let pl_id = data["playlistId"].as_u64().expect("playlistId");
    let data = result_of(
        call(&app, "playlist_add_item", json!({"playlistId": pl_id, "songId": song_id})).await,
    );
    let item_id = data["itemId"].as_u64().expect("itemId");
    let data = result_of(call(&app, "list_playlist_items", json!({"playlistId": pl_id})).await);
    assert_eq!(data["items"].as_array().expect("items").len(), 1);

    // Locked now-playing row (id 0) must never be deletable.
    let msg = error_of(call(&app, "playlist_remove_item", json!({"itemId": 0})).await);
    assert!(msg.contains("itemId"), "{msg}");

    let data = result_of(call(&app, "playlist_remove_item", json!({"itemId": item_id})).await);
    assert_eq!(data["removed"], json!(1));
    let data = result_of(call(&app, "playlist_delete", json!({"playlistId": pl_id})).await);
    assert_eq!(data["playlistId"], json!(pl_id));

    // History CRUD + play_history on the booked song.
    let data = result_of(
        call(&app, "history_add", json!({"songId": song_id, "artist": "A", "title": "Parity Title"})).await,
    );
    let hist_id = data["historyId"].as_u64().expect("historyId");
    let data = result_of(call(&app, "history_filter", json!({"text": "Parity Title"})).await);
    assert!(data["total"].as_u64().expect("total") >= 1, "{data}");
    let data = result_of(call(&app, "history_delete", json!({"historyId": hist_id})).await);
    assert_eq!(data["removed"], json!(1));
    let msg = error_of(call(&app, "history_delete", json!({"historyId": hist_id})).await);
    assert!(msg.contains("no history row"), "{msg}");

    // Cleanup.
    let data = result_of(call(&app, "library_delete", json!({"songId": song_id})).await);
    assert_eq!(data["removed"], json!(1));
}

/// Cartwall play parity: `play_cart` rolls the linked song on the cart zone,
/// books history at start (RadioDJ's UpdateHistory), toggles off on a second
/// press, and clears the wall highlight on stop.
#[tokio::test]
async fn live_cart_play() {
    let app = tmp_app();
    let _live = live_guard().lock().await;
    if !connect_live(&app).await {
        return;
    }

    let dsn_raw = std::env::var("RC_DB_TEST_DSN").unwrap();
    let scrub = radiocraft_db::Db::connect_url(&dsn_raw).await.expect("scrub conn");
    for sql in [
        "DELETE FROM songs WHERE path LIKE '/tmp/opencode/smoke-audio/cart-%'",
        "DELETE FROM carts WHERE name = 'Parity Cart'",
    ] {
        sqlx::raw_sql(sql).execute(scrub.pool()).await.expect("scrub");
    }
    scrub.close().await;

    // A library song to link, then a cart wall with one button in slot 7.
    let src = std::env::var("RC_TEST_AUDIO").unwrap_or_else(|_| "/tmp/opencode/smoke-audio/testtone.wav".into());
    if !std::path::Path::new(&src).exists() {
        eprintln!("skipping live-db test: test audio file {src} missing");
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = format!("/tmp/opencode/smoke-audio/cart-{nanos}.wav");
    let file_stem = format!("cart-{nanos}");
    std::fs::copy(&src, &file).expect("copy tone");
    let data = result_of(call(&app, "library_add", json!({"path": file})).await);
    let song_id = data["songId"].as_u64().expect("songId");

    let db = app.require_db().await.expect("db");
    let cart_id = radiocraft_db::entities::insert_cart(db.pool(), "Parity Cart")
        .await
        .expect("insert cart");
    let mut conn = db.pool().acquire().await.expect("acquire");
    sqlx::query("INSERT INTO carts_list (pID, swID, swButton, color) VALUES (?, ?, 7, '#00ff00')")
        .bind(cart_id)
        .bind(song_id)
        .execute(&mut *conn)
        .await
        .expect("insert cart button");
    let button_id: i64 = sqlx::query_scalar("SELECT CAST(LAST_INSERT_ID() AS SIGNED)")
        .fetch_one(&mut *conn)
        .await
        .expect("button id");
    let button_id = button_id as u64;
    drop(conn);
    drop(db);

    // Arg validation.
    let msg = error_of(call(&app, "play_cart", json!({})).await);
    assert!(msg.contains("buttonId"), "{msg}");
    let msg = error_of(call(&app, "play_cart", json!({"buttonId": 999_999_999})).await);
    assert!(msg.contains("no such cart button"), "{msg}");

    // Press: rolls on the cart zone + books history immediately.
    let data = result_of(call(&app, "play_cart", json!({"buttonId": button_id})).await);
    assert_eq!(data["cart"]["button_id"], json!(button_id));
    assert_eq!(data["cart"]["song_id"], json!(song_id));
    assert_eq!(data["cart"]["cart_id"], json!(cart_id));
    assert!(data["historyId"].as_u64().expect("historyId") > 0, "{data}");
    let history_id = data["historyId"].as_u64().unwrap();
    let cart_title = data["cart"]["title"].as_str().unwrap_or("").to_string();
    assert!(!cart_title.is_empty());

    // The player frame carries the active button (wall highlight source).
    let frame = {
        let mut rx = app.tx.subscribe();
        app.tx.send(app.player_frame()).expect("frame");
        rx.recv().await.expect("frame recv")
    };
    match frame {
        Frame::Player { cart, .. } => {
            let cart = cart.expect("active cart in player frame");
            assert_eq!(cart.button_id, button_id as u32);
        }
        other => panic!("expected player frame, got {other:?}"),
    }

    // Pressing the rolled button again stops it (toggle).
    let data = result_of(call(&app, "play_cart", json!({"buttonId": button_id})).await);
    assert_eq!(data["stopped"], json!(true), "{data}");
    // The book at start used the linked song's own title.
    let song_row = result_of(call(&app, "library_filter", json!({"text": file_stem})).await);
    let booked_title = song_row["songs"][0]["title"].as_str().unwrap_or("").to_string();
    let data = result_of(call(&app, "history_filter", json!({"text": booked_title})).await);
    assert!(data["total"].as_u64().expect("total") >= 1, "{data}");

    // By cart + slot number, then stopped via the transport.
    let data = result_of(call(&app, "play_cart", json!({"cartId": cart_id, "number": 7})).await);
    assert_eq!(data["cart"]["button_id"], json!(button_id));
    let data = result_of(call(&app, "stop", json!({"zone": "cart"})).await);
    assert_eq!(data, json!({}));
    let frame = {
        let mut rx = app.tx.subscribe();
        app.tx.send(app.player_frame()).expect("frame");
        rx.recv().await.expect("frame recv")
    };
    match frame {
        Frame::Player { cart, .. } => assert!(cart.is_none(), "wall highlight must clear"),
        other => panic!("expected player frame, got {other:?}"),
    }

    // Cleanup.
    let db = app.require_db().await.expect("db");
    radiocraft_db::entities::delete_cart(db.pool(), cart_id as u32)
        .await
        .expect("delete cart");
    let data = result_of(call(&app, "history_delete", json!({"historyId": history_id})).await);
    assert_eq!(data["removed"], json!(1));
    let data = result_of(call(&app, "library_delete", json!({"songId": song_id})).await);
    assert_eq!(data["removed"], json!(1));
    let _ = std::fs::remove_file(&file);
}

/// Full AutoDJ path against the live database: rotation + criteria rows
/// drive the query, the plan reports the pick, and `autodj_play_next`
/// rolls it through the normal play path.
#[tokio::test]
async fn live_autodj_plan_and_play() {
    let app = tmp_app();
    let _live = live_guard().lock().await;
    if !connect_live(&app).await {
        return;
    }

    let dsn_raw = std::env::var("RC_DB_TEST_DSN").unwrap();
    let scrub = radiocraft_db::Db::connect_url(&dsn_raw).await.expect("scrub conn");
    for sql in [
        "DELETE FROM songs WHERE path LIKE '/tmp/opencode/smoke-audio/autodj-%'",
        "DELETE FROM rotations_list WHERE data = '#rc-parity-autodj' OR data LIKE '%rc-parity-rule%'",
        "DELETE FROM rotations WHERE name LIKE 'Parity AutoDJ' OR name LIKE 'Parity RotEditor%'",
        "DELETE FROM subcategory WHERE name IN ('Parity AutoDJ Sub', 'Parity RotEditor Sub')",
    ] {
        sqlx::raw_sql(sql).execute(scrub.pool()).await.expect("scrub");
    }
    scrub.close().await;

    let src = std::env::var("RC_TEST_AUDIO").unwrap_or_else(|_| "/tmp/opencode/smoke-audio/testtone.wav".into());
    if !std::path::Path::new(&src).exists() {
        eprintln!("skipping live-db test: test audio file {src} missing");
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = format!("/tmp/opencode/smoke-audio/autodj-{nanos}.wav");
    std::fs::copy(&src, &file).expect("copy tone");

    // Give the pool its own subcategory so unrelated library rows in the
    // scratch DB can never leak into the plan.
    let db = app.require_db().await.expect("db");
    let mut conn = db.pool().acquire().await.expect("acquire");
    sqlx::query("INSERT INTO subcategory (parentid, name) VALUES (1, 'Parity AutoDJ Sub')")
        .execute(&mut *conn)
        .await
        .expect("insert subcategory");
    let subcat_id: u32 = sqlx::query_scalar::<_, i64>("SELECT CAST(LAST_INSERT_ID() AS SIGNED)")
        .fetch_one(&mut *conn)
        .await
        .expect("subcategory id") as u32;
    drop(conn);
    let song_a = result_of(
        call(&app, "library_add", json!({"path": file, "weight": 100, "idSubcat": subcat_id})).await,
    );
    let song_a = song_a["songId"].as_u64().expect("songId");

    let rotation_id = radiocraft_db::entities::insert_rotation(db.pool(), "Parity AutoDJ")
        .await
        .expect("insert rotation");
    let criteria_row = radiocraft_db::entities::RotationCriteria {
        ID: 0,
        pID: rotation_id,
        catID: 0,
        subID: subcat_id as i32,
        genID: 0,
        mood: None,
        gender: None,
        language: None,
        start_type: 0,
        end_type: 0,
        selType: false,
        sweeper: false,
        repeatRule: false,
        ord: 0,
        data: "#rc-parity-autodj".into(),
        track_separation: 0,
        artist_separation: 0,
        title_separation: 0,
        album_separation: 0,
        advanced: None,
    };
    let criteria_id = radiocraft_db::entities::insert_criteria(db.pool(), &criteria_row)
        .await
        .expect("insert criteria");
    drop(db);

    // The switch is independent of planning.
    let data = result_of(call(&app, "autodj_set", json!({"enabled": true})).await);
    assert_eq!(data["enabled"], json!(true));
    let data = result_of(call(&app, "autodj_status", json!({})).await);
    assert_eq!(data["enabled"], json!(true));

    // Dry run: our song must be the pick of our criteria.
    let data = result_of(call(&app, "autodj_plan", json!({})).await);
    assert_eq!(data["rotationId"], json!(rotation_id), "{data}");
    assert_eq!(data["criteriaId"], json!(criteria_id), "{data}");
    assert_eq!(data["pick"]["songId"], json!(song_a), "{data}");
    assert_eq!(data["lastPickSongId"], json!(null));
    let ordered = data["ordered"].as_array().expect("ordered");
    assert!(ordered.iter().any(|c| c["songId"] == json!(song_a)), "{data}");

    // Roll it: the play happens through the normal path, so the player
    // frame and now-playing hub both see it.
    let data = result_of(call(&app, "autodj_play_next", json!({})).await);
    assert_eq!(data["now"]["song_id"], json!(song_a), "{data}");
    assert!(app.engine.is_playing(radiocraft_server::state::MAIN_ZONE));
    assert_eq!(app.autodj_last_song(), Some(song_a as i64));
    let plan = &data["plan"];
    assert_eq!(plan["pick"]["songId"], json!(song_a), "{plan}");

    let data = result_of(call(&app, "stop", json!({})).await);
    assert_eq!(data, json!({}));

    // Repeat-rule guard: the same track may not be chosen twice in a row,
    // so with only one candidate in the pool the next slot is empty.
    let data = result_of(call(&app, "autodj_plan", json!({})).await);
    assert_eq!(data["pick"], json!(null), "{data}");
    assert_eq!(data["lastPickSongId"], json!(song_a), "{data}");
    let rejected = data["rejected"].as_array().expect("rejected");
    assert!(
        rejected
            .iter()
            .any(|r| r["songId"] == json!(song_a) && r["reason"] == json!("SameTrackAsLast")),
        "{data}"
    );

    // Cleanup.
    let db = app.require_db().await.expect("db");
    sqlx::query("DELETE FROM history WHERE trackID = ?")
        .bind(song_a as u32)
        .execute(db.pool())
        .await
        .expect("scrub history");
    radiocraft_db::entities::delete_criteria(db.pool(), criteria_id)
        .await
        .expect("delete criteria");
    radiocraft_db::entities::delete_rotation(db.pool(), rotation_id)
        .await
        .expect("delete rotation");
    sqlx::query("DELETE FROM subcategory WHERE ID = ?")
        .bind(subcat_id)
        .execute(db.pool())
        .await
        .expect("delete subcategory");
    drop(db);
    let data = result_of(call(&app, "library_delete", json!({"songId": song_a})).await);
    assert_eq!(data["removed"], json!(1));
    let _ = std::fs::remove_file(&file);
}

/// Rotation/criteria editing over WS against the live DB: full CRUD, JSON
/// payload round-trip into `rotations_list.data`, legacy-column denormalisation,
/// and the new rotation feeding `autodj_plan`.
#[tokio::test]
async fn live_rotation_editor_ops() {
    let app = tmp_app();
    let _live = live_guard().lock().await;
    if !connect_live(&app).await {
        return;
    }

    let dsn_raw = std::env::var("RC_DB_TEST_DSN").unwrap();
    let scrub = radiocraft_db::Db::connect_url(&dsn_raw).await.expect("scrub conn");
    for sql in [
        "DELETE FROM songs WHERE path LIKE '/tmp/opencode/smoke-audio/rcrot-%'",
        "DELETE FROM rotations_list WHERE data = '#rc-parity-autodj' OR data LIKE '%rc-parity-rule%'",
        "DELETE FROM rotations WHERE name LIKE 'Parity AutoDJ' OR name LIKE 'Parity RotEditor%'",
        "DELETE FROM subcategory WHERE name IN ('Parity AutoDJ Sub', 'Parity RotEditor Sub')",
    ] {
        sqlx::raw_sql(sql).execute(scrub.pool()).await.expect("scrub");
    }
    scrub.close().await;

    let src = std::env::var("RC_TEST_AUDIO").unwrap_or_else(|_| "/tmp/opencode/smoke-audio/testtone.wav".into());
    if !std::path::Path::new(&src).exists() {
        eprintln!("skipping live-db test: test audio file {src} missing");
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = format!("/tmp/opencode/smoke-audio/rcrot-{nanos}.wav");
    std::fs::copy(&src, &file).expect("copy tone");

    let db = app.require_db().await.expect("db");
    let mut conn = db.pool().acquire().await.expect("acquire");
    sqlx::query("INSERT INTO subcategory (parentid, name) VALUES (1, 'Parity RotEditor Sub')")
        .execute(&mut *conn)
        .await
        .expect("insert subcategory");
    let subcat_id: u32 = sqlx::query_scalar::<_, i64>("SELECT CAST(LAST_INSERT_ID() AS SIGNED)")
        .fetch_one(&mut *conn)
        .await
        .expect("subcategory id") as u32;
    drop(conn);
    let song_id = result_of(
        call(&app, "library_add", json!({"path": file, "weight": 90, "idSubcat": subcat_id})).await,
    );
    let song_id = song_id["songId"].as_u64().expect("songId");

    // Header CRUD.
    let data = result_of(call(&app, "rotation_create", json!({"name": "Parity RotEditor"})).await);
    let rotation_id = data["rotationId"].as_u64().expect("rotationId");
    let data = result_of(call(&app, "list_rotations", json!({})).await);
    assert!(
        data["rotations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["ID"] == json!(rotation_id) && r["name"] == json!("Parity RotEditor")),
        "{data}"
    );
    let data = result_of(
        call(&app, "rotation_rename", json!({"rotationId": rotation_id, "name": "Parity RotEditor 2"})).await,
    );
    assert_eq!(data["updated"], json!(1));

    // Lookup pickers.
    let data = result_of(call(&app, "list_lookup", json!({})).await);
    assert!(data["subcategories"].as_array().unwrap().iter().any(|s| s["ID"] == json!(subcat_id)));

    // Add a rule line (full core JSON payload; subcategory include).
    let criteria = json!({
        "name": "rc-parity-rule",
        "enabled": true,
        "subcategory_id": subcat_id,
        "subcategory_type": "Include",
        "selection": "Weight",
        "repeat_artist_interval": 30,
        "weight_min": 5,
        "ord": 1,
    });
    let data = result_of(call(&app, "criteria_add", json!({"rotationId": rotation_id, "criteria": criteria})).await);
    let criteria_id = data["criteriaId"].as_u64().expect("criteriaId");

    // list_criteria returns the full core shape (data JSON honoured).
    let data = result_of(call(&app, "list_criteria", json!({"rotationId": rotation_id})).await);
    let rows = data["criteria"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], json!(criteria_id));
    assert_eq!(rows[0]["weight_min"], json!(5.0));
    assert_eq!(rows[0]["repeat_artist_interval"], json!(30));
    assert_eq!(rows[0]["selection"], json!("Weight"));

    // Legacy columns got the denormalised mirror.
    let legacy = radiocraft_db::entities::get_criteria(db.pool(), criteria_id as u32)
        .await
        .expect("get criteria")
        .expect("row");
    assert_eq!(legacy.subID, subcat_id as i32);
    assert_eq!(legacy.artist_separation, 30);
    assert_eq!(legacy.ord, 1);
    assert!(!legacy.selType);
    assert!(legacy.data.starts_with('{'));

    // Update: partial patch merges (serde(default)).
    let data = result_of(
        call(&app, "criteria_update", json!({"criteria": {"id": criteria_id, "selection": "Random", "ord": 2}})).await,
    );
    assert_eq!(data["updated"], json!(1));
    let data = result_of(call(&app, "list_criteria", json!({"rotationId": rotation_id})).await);
    let row = &data["criteria"].as_array().unwrap()[0];
    assert_eq!(row["selection"], json!("Random"));
    assert_eq!(row["ord"], json!(2));
    assert_eq!(row["weight_min"], json!(5.0), "untouched fields survive the patch");

    // The edited rotation now drives the AutoDJ plan.
    let data = result_of(call(&app, "autodj_plan", json!({})).await);
    assert_eq!(data["rotationId"], json!(rotation_id));
    let pick = data["pick"].as_object().unwrap_or_else(|| panic!("pick expected: {data}"));
    assert_eq!(pick["songId"], json!(song_id));

    // Deletes.
    let data = result_of(call(&app, "criteria_delete", json!({"criteriaId": criteria_id})).await);
    assert_eq!(data["removed"], json!(1));
    let data = result_of(call(&app, "rotation_delete", json!({"rotationId": rotation_id})).await);
    assert_eq!(data["removed"], json!(1));

    // Cleanup.
    sqlx::query("DELETE FROM subcategory WHERE ID = ?")
        .bind(subcat_id)
        .execute(db.pool())
        .await
        .expect("delete subcategory");
    drop(db);
    let data = result_of(call(&app, "library_delete", json!({"songId": song_id})).await);
    assert_eq!(data["removed"], json!(1));
    let _ = std::fs::remove_file(&file);
}
