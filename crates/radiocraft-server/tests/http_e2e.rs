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
