//! REST surface, WebSocket route, and static hosting for the web UI.
//!
//! Phase 1 keeps REST thin: health/info/state plus library reads. Anything
//! stateful goes through the same engine/db layers the WS commands use.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::response::Html;
use axum::routing::get;
use serde::Deserialize;
use tower_http::services::{ServeDir, ServeFile};

use crate::frames::Frame;
use crate::state::App;
use crate::ws::{ServerSocket, handle_upgrade};

/// Build the whole HTTP app: `/ws`, `/api/*`, static web UI (optional).
pub fn router(app: Arc<App>, auth: Arc<crate::auth::Auth>, web_dist: Option<std::path::PathBuf>) -> Router {
    let socket = ServerSocket {
        app: Arc::clone(&app),
        auth: auth.clone(),
    };
    let mut router = Router::new()
        .route("/ws", get(handle_upgrade))
        .route("/health", get(health))
        .route("/api/state", get(state_snapshot))
        .route("/api/history", get(list_history))
        .with_state(socket.clone());

    if let Some(dist) = web_dist {
        let index = dist.join("index.html");
        let spa = ServeDir::new(&dist).fallback(ServeFile::new(index));
        router = router.fallback_service(spa);
    }

    // Static hosting of the RadioDJ-compatible now-playing export stays in
    // radiocraft-net; this server never replaces it.
    let _ = app;
    router
}

async fn health(State(socket): State<ServerSocket>) -> Html<String> {
    let app = &socket.app;
    let status = app.status_line().await;
    Html(format!(
        "RadioCraft {} — up, status: {} ({})",
        env!("CARGO_PKG_VERSION"),
        status.message,
        status.level
    ))
}

async fn state_snapshot(State(socket): State<ServerSocket>) -> axum::Json<Frame> {
    axum::Json(socket.app.state_frame())
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

async fn list_history(
    State(socket): State<ServerSocket>,
    Query(q): Query<HistoryQuery>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let app = &socket.app;
    let db = match app.db.read().await.clone() {
        Some(db) => db,
        None => return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE),
    };
    let limit = q.limit.unwrap_or(100).min(1000);
    let offset = q.offset.unwrap_or(0);
    match radiocraft_db::entities::list_history(db.pool(), limit, offset).await {
        Ok(rows) => Ok(axum::Json(serde_json::json!({ "history": rows }))),
        Err(e) => {
            tracing::warn!("GET /api/history failed: {e}");
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_builds_without_web_dist() {
        let dir = std::env::temp_dir().join(format!("rc-http-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let app = Arc::new(App::load(&dir).expect("load"));
        let auth = Arc::new(crate::auth::Auth::default());
        let _ = router(app, auth, None);
    }
}
