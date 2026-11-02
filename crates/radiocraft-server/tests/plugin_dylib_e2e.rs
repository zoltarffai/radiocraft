//! End-to-end external-plugin test: the real cdylib built from
//! `radiocraft-plugin-example` is loaded through `PluginManager` (libloading
//! + ABI gate) and driven through the server's host shim.

use std::path::PathBuf;
use std::sync::Arc;

use radiocraft_plugin::host::{StateChange, StateInfo};
use radiocraft_server::{plugin_host::PluginManager, state::App};

/// The cdylib cargo produced for the dev-dependency next to our test binary.
fn example_dylib() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("../../target/debug");
    let lib = dir.join("libradiocraft_plugin_example.so");
    assert!(
        lib.is_file(),
        "expected {} — build it with `cargo build -p radiocraft-plugin-example`",
        lib.display()
    );
    lib
}

async fn wait_status(app: &App, needle: &str) -> String {
    for _ in 0..100 {
        let s = app.status_line().await;
        if s.message.contains(needle) {
            return s.message;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!(
        "status never contained {needle:?} (last: {:?})",
        app.status_line().await
    );
}

/// Manager calls block on the plugin thread (`blocking_recv`), so async tests
/// must run them on the blocking pool, exactly like the dispatch ops do.
async fn call_mgr<T: Send + 'static>(
    mgr: &Arc<PluginManager>,
    f: impl FnOnce(&PluginManager) -> T + Send + 'static,
) -> T {
    let mgr = Arc::clone(mgr);
    tokio::task::spawn_blocking(move || f(&mgr))
        .await
        .expect("blocking join")
}

#[tokio::test]
async fn example_dylib_loads_and_round_trips_host() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rc-dylib-e2e-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let app = Arc::new(App::load(&dir).expect("load"));
    let mgr = PluginManager::start(Arc::clone(&app));

    let id = call_mgr(&mgr, move |m| m.load(example_dylib()))
        .await
        .expect("load example dylib");

    // The scan registered the plugin's declared actions and metadata.
    let list = call_mgr(&mgr, |m| m.list()).await;
    let plugins = list["plugins"].as_array().expect("plugins array");
    let entry = plugins.iter().find(|p| p["id"] == id).expect("entry");
    assert_eq!(entry["name"], "Example");
    assert_eq!(
        entry["actions"],
        serde_json::json!([
            "Echo (Text)",
            "Remember (Key, Value)",
            "Recall (Key)",
            "CountStates"
        ])
    );

    // Action -> host DebugAddLine -> status line (async hop via outer rt).
    call_mgr(&mgr, |m| m.invoke("Echo", &[String::from("hello")]))
        .await
        .expect("echo");
    assert!(wait_status(&app, "example echo: hello")
        .await
        .contains("hello"));

    // Plugin settings round-trip through Plugin_Example.xml on disk.
    call_mgr(&mgr, |m| {
        m.invoke("Remember", &["colour".into(), "teal".into()])
    })
    .await
    .expect("remember");
    wait_status(&app, "example remembered colour=teal").await;
    call_mgr(&mgr, |m| m.invoke("Recall", &[String::from("colour")]))
        .await
        .expect("recall");
    assert!(wait_status(&app, "example recall colour=teal")
        .await
        .contains("teal"));
    let saved = std::fs::read_to_string(dir.join("Plugin_Example.xml")).expect("config file");
    assert!(saved.contains("teal"), "{saved}");

    // Host state fan-out reaches on_state_changed; CountStates reports it.
    call_mgr(&mgr, |m| {
        m.notify(StateInfo::new(StateChange::Timer));
        m.notify(StateInfo::with_data(
            StateChange::PlayerStarted,
            serde_json::json!({}),
        ));
    })
    .await;
    // Invoke is itself a round-trip to the plugin thread, so the notify
    // queue is drained by the time CountStates runs.
    call_mgr(&mgr, |m| m.invoke("CountStates", &[] as &[String]))
        .await
        .expect("count");
    let msg = wait_status(&app, "example states=").await;
    let seen: usize = msg
        .rsplit("example states=")
        .next()
        .expect("suffix")
        .parse()
        .expect("number");
    assert!(seen >= 2, "plugin saw only {seen} state changes");

    // Unload drops the entry; further dispatches no longer match.
    assert!(
        call_mgr(&mgr, move |m| m.unload(id)).await,
        "unload should report the entry"
    );
    assert_eq!(
        call_mgr(&mgr, |m| m.invoke("Echo", &[String::from("x")]))
            .await
            .unwrap_or_default(),
        0
    );

    mgr.shutdown();
}

#[tokio::test]
async fn bogus_dylib_fails_the_abi_gate() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rc-dylib-bogus-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let app = Arc::new(App::load(&dir).expect("load"));
    let mgr = PluginManager::start(Arc::clone(&app));

    // A real cdylib without the ABI exports must be rejected, not crash.
    let libc = PathBuf::from("/lib/x86_64-linux-gnu/libc.so.6");
    if libc.is_file() {
        let err = call_mgr(&mgr, move |m| m.load(libc))
            .await
            .expect_err("libc is not a plugin");
        assert!(!err.is_empty());
    }

    mgr.shutdown();
}
