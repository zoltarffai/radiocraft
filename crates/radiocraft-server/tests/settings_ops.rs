//! Tests for the XML-backed settings-editor ops (no database needed):
//! sound device, soundcards, sound options, and the now-playing export.

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
    let dir = std::env::temp_dir().join(format!("rc-set-{}-{nanos}", std::process::id()));
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

#[tokio::test]
async fn soundcards_round_trip_and_device_ops() {
    let app = tmp_app();

    // Defaults before any file exists.
    let got = result_of(call(&app, "soundcards_get", json!({})).await);
    assert_eq!(got["audio_mode"], 0);
    assert_eq!(got["input_fade_out"], 1500);

    // Set + round-trip; the file lands in the config dir.
    let got = result_of(
        call(
            &app,
            "soundcards_set",
            json!({"audioMode": 1, "inputFadeIn": 20, "inputFadeOut": 900}),
        )
        .await,
    );
    assert_eq!(got["audio_mode"], 1);
    assert_eq!(got["input_fade_in"], 20);
    assert_eq!(got["input_fade_out"], 900);
    assert!(app.config_dir().join("settings_soundcards.xml").exists());

    // Device listing always answers (null/virtual devices included).
    let devs = result_of(call(&app, "list_devices", json!({})).await);
    assert!(devs["current"]["channels"].is_number());

    // An unknown device name is rejected without touching playout.
    let msg = error_of(call(&app, "set_device", json!({"name": "no-such-out-9"})).await);
    assert!(msg.contains("no-such-out-9"), "{msg}");
}

#[tokio::test]
async fn sound_options_set_persists_and_reraims_engine() {
    let app = tmp_app();
    let got = result_of(
        call(&app, "sound_options_set", json!({"dur_fadeout": 4200, "vol_overlay": 7})).await,
    );
    assert_eq!(got["dur_fadeout"], 4200);
    assert_eq!(got["vol_overlay"], 7);

    // Live engine picked the knobs up...
    let cfg = app.engine.config();
    assert_eq!(cfg.default_fade_out_ms, 4200);
    assert_eq!(cfg.overlay_db, 7.0);

    // ...and they survive a settings reload from disk.
    let app2 = Arc::new(App::load(app.config_dir()).expect("reload"));
    let got = result_of(call(&app2, "sound_options_get", json!({})).await);
    assert_eq!(got["dur_fadeout"], 4200);
}

#[tokio::test]
async fn export_draft_round_trip_validation_and_test() {
    let app = tmp_app();

    // Defaults: everything off.
    let got = result_of(call(&app, "export_get", json!({})).await);
    assert_eq!(got["export"]["export_to_file"], false);

    // File export without a path is rejected.
    let msg = error_of(
        call(
            &app,
            "export_set",
            json!({"export": {"export_to_file": true, "target_path": ""}}),
        )
        .await,
    );
    assert!(msg.contains("target path"), "{msg}");

    // Web export without any URL is rejected.
    let msg = error_of(
        call(
            &app,
            "export_set",
            json!({"export": {"export_to_web": true, "web_urls": ["  "]}}),
        )
        .await,
    );
    assert!(msg.contains("URL"), "{msg}");

    // A valid draft persists into the plugin file (Plugins/ is auto-created).
    let draft = json!({
        "export_to_file": true,
        "target_path": app.config_dir().join("np.txt").to_string_lossy(),
        "txt_file_template": "$artist$ - $title$",
        "types_text": "0;1;6;",
    });
    let got = result_of(call(&app, "export_set", json!({"export": draft})).await);
    assert_eq!(got["export"]["export_to_file"], true);
    assert!(app.config_dir().join("Plugins/Plugin_NowPlaying.xml").exists());

    // export_test with nothing on air renders the sample song.
    let got = result_of(call(&app, "export_test", json!({})).await);
    assert_eq!(got["snapshot"]["artist"], "RadioCraft");
    let written = std::fs::read_to_string(app.config_dir().join("np.txt")).expect("export file");
    assert_eq!(written, "RadioCraft - Test message");

    // A track type outside the export list is refused.
    let draft = json!({
        "export_to_file": true,
        "target_path": app.config_dir().join("np2.txt").to_string_lossy(),
        "types_text": "3;",
    });
    let _ = result_of(call(&app, "export_set", json!({"export": draft})).await);
    let msg = error_of(call(&app, "export_test", json!({})).await);
    assert!(msg.contains("not in the export list"), "{msg}");
}

#[tokio::test]
async fn station_name_rebuilds_exporter() {
    let app = tmp_app();
    let draft = json!({
        "export_to_file": true,
        "target_path": app.config_dir().join("np3.txt").to_string_lossy(),
        "txt_file_template": "$station_name$",
    });
    let _ = result_of(call(&app, "export_set", json!({"export": draft})).await);
    let got = result_of(call(&app, "export_test", json!({})).await);
    assert_eq!(got["snapshot"]["station"], "RadioDJ");

    let _ = result_of(call(&app, "set_setting", json!({"key": "StationName", "value": "K-Rust"})).await);
    let got = result_of(call(&app, "export_test", json!({})).await);
    assert_eq!(got["snapshot"]["station"], "K-Rust");
}
