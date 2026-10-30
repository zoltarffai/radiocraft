//! REST surface, WebSocket route, and static hosting for the web UI.
//!
//! Phase 1 keeps REST thin: health/info/state plus library reads. Anything
//! stateful goes through the same engine/db layers the WS commands use.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::routing::get;
use serde::Deserialize;
use tower_http::services::{ServeDir, ServeFile};

use crate::frames::Frame;
use crate::state::App;
use crate::ws::{ServerSocket, handle_upgrade};
use radiocraft_db::Db;

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
        .route("/api/songs", get(list_songs))
        .route("/api/playlists", get(list_playlists))
        .route("/api/playlists/{id}/items", get(list_playlist_items))
        .route("/api/carts", get(list_carts))
        .route("/api/carts/{id}/buttons", get(list_cart_buttons))
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
pub struct PageQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

/// The connected database handle, or 503 when the operator has not connected.
async fn db_of(
    socket: &ServerSocket,
) -> Result<Db, axum::http::StatusCode> {
    match socket.app.db.read().await.clone() {
        Some(db) => Ok(db),
        None => Err(axum::http::StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn list_history(
    State(socket): State<ServerSocket>,
    Query(q): Query<PageQuery>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let db = db_of(&socket).await?;
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

async fn list_songs(
    State(socket): State<ServerSocket>,
    Query(q): Query<PageQuery>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let db = db_of(&socket).await?;
    let limit = q.limit.unwrap_or(500).min(5000);
    let offset = q.offset.unwrap_or(0);
    match radiocraft_db::entities::list_songs(db.pool(), limit, offset).await {
        Ok(songs) => Ok(axum::Json(serde_json::json!({
            "songs": songs,
            "total": radiocraft_db::entities::count_songs(db.pool())
                .await
                .unwrap_or(0),
        }))),
        Err(e) => {
            tracing::warn!("GET /api/songs failed: {e}");
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn list_playlists(
    State(socket): State<ServerSocket>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let db = db_of(&socket).await?;
    match radiocraft_db::entities::list_playlists(db.pool()).await {
        Ok(playlists) => Ok(axum::Json(serde_json::json!({ "playlists": playlists }))),
        Err(e) => {
            tracing::warn!("GET /api/playlists failed: {e}");
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn list_playlist_items(
    State(socket): State<ServerSocket>,
    Path(id): Path<u32>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let db = db_of(&socket).await?;
    match radiocraft_db::entities::list_playlist_items_with_songs(db.pool(), id).await {
        Ok(items) => Ok(axum::Json(serde_json::json!({ "items": items }))),
        Err(e) => {
            tracing::warn!("GET /api/playlists/{id}/items failed: {e}");
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn list_carts(
    State(socket): State<ServerSocket>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let db = db_of(&socket).await?;
    match radiocraft_db::entities::list_carts(db.pool()).await {
        Ok(carts) => Ok(axum::Json(serde_json::json!({ "carts": carts }))),
        Err(e) => {
            tracing::warn!("GET /api/carts failed: {e}");
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn list_cart_buttons(
    State(socket): State<ServerSocket>,
    Path(id): Path<u32>,
) -> Result<axum::Json<serde_json::Value>, axum::http::StatusCode> {
    let db = db_of(&socket).await?;
    match radiocraft_db::entities::list_cart_buttons_with_songs(db.pool(), id).await {
        Ok(buttons) => Ok(axum::Json(serde_json::json!({ "buttons": buttons }))),
        Err(e) => {
            tracing::warn!("GET /api/carts/{id}/buttons failed: {e}");
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
