//! REST smoke tests: health/state are always up, db-backed reads answer 503
//! until the operator connects.

use axum_test::TestServer;
use radiocraft_server::auth::Auth;
use radiocraft_server::http::router;
use radiocraft_server::state::App;
use std::sync::Arc;

fn test_app() -> Arc<App> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("rc-http-e2e-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    Arc::new(App::load(&dir).expect("load"))
}

#[tokio::test]
async fn health_and_state_answer() {
    let server = TestServer::builder()
        .http_transport()
        .build(router(test_app(), Arc::new(Auth::default()), None));

    let res = server.get("/health").await;
    res.assert_status_ok();
    assert!(res.text().contains("RadioCraft"));

    let res = server.get("/api/state").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["t"], "state");
}

#[tokio::test]
async fn db_reads_are_503_until_connected() {
    let server = TestServer::builder()
        .http_transport()
        .build(router(test_app(), Arc::new(Auth::default()), None));

    for path in [
        "/api/history",
        "/api/songs",
        "/api/playlists",
        "/api/playlists/1/items",
        "/api/carts",
        "/api/carts/1/buttons",
    ] {
        let res = server.get(path).await;
        res.assert_status(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
}

#[tokio::test]
async fn rest_cmd_mirror_returns_flat_payloads() {
    let server = TestServer::builder()
        .http_transport()
        .build(router(test_app(), Arc::new(Auth::default()), None));

    // db-free ops answer with the same flat payload the WS result frame carries.
    let res = server.post("/api/cmd/sound_options_get").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert!(body.get("volume_out").is_some(), "{body}");
    assert!(body.get("dur_fadeout").is_some(), "{body}");

    let res = server.post("/api/cmd/autodj_status").await;
    res.assert_status_ok();
    let body: serde_json::Value = res.json();
    assert_eq!(body["enabled"], serde_json::json!(false));

    // db-backed op without a connection: the dispatch error maps to 400.
    let res = server.post("/api/cmd/list_rotations").await;
    res.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = res.json();
    assert_eq!(body["code"], "cmd_failed");

    let res = server.post("/api/cmd/not_an_op").await;
    res.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = res.json();
    assert_eq!(body["code"], "cmd_failed");
    assert!(body["message"].as_str().unwrap().contains("unknown op"));

    // unparsable args are rejected before dispatch.
    let res = server
        .post("/api/cmd/soundcards_set")
        .text("{oops")
        .await;
    res.assert_status(axum::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = res.json();
    assert_eq!(body["code"], "bad_json");
}

#[tokio::test]
async fn rest_token_gate_applies_to_api() {
    let server = TestServer::builder()
        .http_transport()
        .build(router(test_app(), Arc::new(Auth::with_token("s3cret")), None));

    // health stays open for monitoring; the rest of /api/* needs the token.
    server.get("/health").await.assert_status_ok();

    let res = server.get("/api/state").await;
    res.assert_status(axum::http::StatusCode::UNAUTHORIZED);

    let res = server
        .get("/api/state")
        .add_header("authorization", "Bearer s3cret")
        .await;
    res.assert_status_ok();

    let res = server
        .post("/api/cmd/autodj_status")
        .add_header("x-auth-token", "s3cret")
        .await;
    res.assert_status_ok();

    let res = server
        .post("/api/cmd/autodj_status")
        .add_header("authorization", "Bearer wrong")
        .await;
    res.assert_status(axum::http::StatusCode::UNAUTHORIZED);
}
