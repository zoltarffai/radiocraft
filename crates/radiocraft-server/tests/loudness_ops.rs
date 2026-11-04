//! Web-surface coverage for the loudness scan ops: `scan_loudness` argument
//! handling (the Reset button, an empty selection, and the target guard).
//!
//! The scan half needs a database and only runs when `RC_DB_TEST_DSN` names
//! a scratch database, same convention as `commands.rs`. The `loudness scan
//! already running` guard is exercised by the playout e2e suite.

use std::sync::Arc;

use radiocraft_db::entities::NewSong;
use radiocraft_server::dispatch::dispatch;
use radiocraft_server::frames::Frame;
use radiocraft_server::state::App;
use serde_json::{Value, json};

fn tmp_app() -> Arc<App> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rc-loud-{}-{nanos}", std::process::id()));
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

// ---- live-database helpers (verbatim shape from commands.rs) --------------

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

fn live_guard() -> &'static std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCK: std::sync::OnceLock<std::sync::Arc<tokio::sync::Mutex<()>>> =
        std::sync::OnceLock::new();
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

/// Three enabled library rows (song_type 0) pointing at missing files: the
/// sweep skips them without touching their loudness columns.
async fn seed_scratch_songs(app: &Arc<App>) -> Vec<u32> {
    let db = app.require_db().await.expect("db");
    let ids = (0..3).map(|n| format!("loudness-web-{n}")).collect::<Vec<_>>();
    scrub(app).await;
    let mut inserted = Vec::new();
    for title in ids {
        let s = NewSong {
            song_type: 0,
            title: title.clone(),
            path: format!("/nonexistent/{title}.mp3"),
            ..Default::default()
        };
        inserted.push(
            radiocraft_db::entities::insert_song(db.pool(), &s)
                .await
                .expect("insert"),
        );
    }
    inserted
}

async fn scrub(app: &Arc<App>) {
    let db = app.require_db().await.expect("db");
    sqlx::raw_sql("DELETE FROM `songs` WHERE `title` LIKE 'loudness-web-%'")
        .execute(db.pool())
        .await
        .expect("scrub");
}

#[tokio::test]
async fn scan_loudness_reset_restores_factory_values() {
    let app = tmp_app();
    let _live = live_guard().lock().await;
    if !connect_live(&app).await {
        return;
    }
    let ids = seed_scratch_songs(&app).await;
    let db = app.require_db().await.expect("db");

    // Dirty the seeded rows first: the reset must find them at factory values.
    for id in &ids {
        sqlx::query(
            "UPDATE `songs` SET `loudness` = 1.20, `bs_loudness` = 1.20, `bs1770` = -16.20 \
             WHERE `ID` = ?",
        )
        .bind(id)
        .execute(db.pool())
        .await
        .expect("dirty");
    }

    let res = result_of(call(&app, "scan_loudness", json!({ "reset": true })).await);
    assert!(res["reset"].as_u64().unwrap() >= ids.len() as u64, "{res}");

    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT COUNT(*) FROM `songs` WHERE `ID` IN ({placeholders}) \
         AND `loudness` = 1.000 AND `bs_loudness` = 1.000 AND `bs1770` = 0.000"
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for id in &ids {
        q = q.bind(id);
    }
    assert_eq!(q.fetch_one(db.pool()).await.unwrap(), ids.len() as i64);
    scrub(&app).await;
}

#[tokio::test]
async fn scan_loudness_empty_selection_completes_with_nothing_scanned() {
    let app = tmp_app();
    let _live = live_guard().lock().await;
    if !connect_live(&app).await {
        return;
    }
    // The UI's "Scan selected" path with a `songIds: []` payload must not
    // be mistaken for the unscanned-row sweep (that is the no-`songIds` path).
    let mut rx = app.tx.subscribe();
    let res = result_of(call(&app, "scan_loudness", json!({ "songIds": [] })).await);
    assert_eq!(res["started"], 0);
    // The spawned sweep still broadcasts its completion report (id 0).
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
        match frame {
            Ok(Ok(Frame::Result { id: 0, op, data })) if op == "scan_loudness" => {
                assert_eq!(data["scanned"], 0);
                assert_eq!(data["total"], 0);
                assert!(data["errors"].as_array().unwrap().is_empty(), "{data}");
                return;
            }
            Ok(Ok(_)) => continue, // unrelated state/meter broadcasts
            Ok(Err(_)) | Err(_) => panic!("no scan_loudness broadcast report"),
        }
    }
}

#[tokio::test]
async fn scan_loudness_rejects_non_negative_target() {
    let app = tmp_app();
    let _live = live_guard().lock().await;
    if !connect_live(&app).await {
        return;
    }
    // BSTarget is dBFS below zero by definition (RadioDJ default -12); a
    // positive target would write astronomic multipliers to the rows.
    let msg = error_of(call(&app, "scan_loudness", json!({ "songIds": [1], "target": 42 })).await);
    assert!(msg.contains("target"), "{msg}");
}
