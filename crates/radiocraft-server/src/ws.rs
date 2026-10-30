//! The WebSocket endpoint: upgrade, `ready`, then serve one client loop.
//!
//! Subscribed clients mirror the broadcast hub; commands run through
//! [`crate::dispatch`] serially on the connection so op order is kept.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::broadcast;

use crate::auth::Auth;
use crate::dispatch::dispatch;
use crate::frames::{ClientMsg, Frame, PROTOCOL_VERSION, ZONES};
use crate::state::App;

/// axum route: `/ws`.
pub async fn handle_upgrade(ws: WebSocketUpgrade, State(server): State<ServerSocket>) -> Response {
    ws.on_upgrade(move |socket| serve(socket, server))
}

/// Per-connection settings the router carries.
#[derive(Clone)]
pub struct ServerSocket {
    pub app: Arc<App>,
    pub auth: Arc<Auth>,
}

async fn serve(socket: WebSocket, server: ServerSocket) {
    let (mut sender, mut receiver) = socket.split();
    let app = Arc::clone(&server.app);

    let mut rx: Option<broadcast::Receiver<Frame>> = None;
    let mut authed = !server.auth.required;

    let first = Frame::Ready {
        protocol: PROTOCOL_VERSION,
        server: "radiocraft".into(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        zones: ZONES.iter().map(|z| (*z).to_string()).collect(),
        authenticated: authed || !server.auth.required,
    };
    if sender
        .send(Message::Text(serde_json::to_string(&first).expect("frame").into()))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            maybe = async { match rx.as_mut() { Some(rx) => rx.recv().await, None => std::future::pending().await } } => {
                let frame = match maybe {
                    Ok(frame) => frame,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let text = match serde_json::to_string(&frame) {
                    Ok(text) => text,
                    Err(_) => continue,
                };
                if sender.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            incoming = receiver.next() => {
                let msg = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Ping(data))) => {
                        if sender.send(Message::Pong(data)).await.is_err() { break; }
                        continue;
                    }
                    Some(Ok(Message::Pong(_)) | Ok(Message::Binary(_))) => continue,
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                };
                let parsed: ClientMsg = match serde_json::from_str(&msg) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        let frame = Frame::Error {
                            id: None,
                            code: "bad_json".into(),
                            message: format!("unparsable message: {e}"),
                        };
                        let _ = sender.send(frame_text(&frame)).await;
                        continue;
                    }
                };
                if let Some(frame) = handle_msg(&app, &server.auth, &mut rx, &mut authed, parsed).await {
                    if sender.send(frame_text(&frame)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

fn frame_text(frame: &Frame) -> Message {
    Message::Text(serde_json::to_string(frame).expect("frame serialises").into())
}

/// One client message; `None` when it produced no direct reply (sub).
async fn handle_msg(
    app: &Arc<App>,
    auth: &Arc<Auth>,
    rx: &mut Option<broadcast::Receiver<Frame>>,
    authed: &mut bool,
    msg: ClientMsg,
) -> Option<Frame> {
    match msg {
        ClientMsg::Ping {} => Some(Frame::Pong {}),
        ClientMsg::Hello { username, password } => {
            if auth.check_hello(username.as_deref(), password.as_deref()) {
                *authed = true;
                Some(Frame::Result {
                    id: 0,
                    op: "hello".into(),
                    data: serde_json::json!({ "authenticated": true }),
                })
            } else {
                Some(Frame::Error {
                    id: None,
                    code: "unauthorized".into(),
                    message: "bad token".into(),
                })
            }
        }
        ClientMsg::Sub {} => {
            *rx = Some(app.subscribe());
            // Immediate snapshot so the UI paints without waiting for a change.
            Some(Frame::Result {
                id: 0,
                op: "sub".into(),
                data: serde_json::json!({
                    "player": app.player_frame(),
                    "state": app.state_frame(),
                    "status": Frame::Status(app.status_line().await.into()),
                    "db_connected": app.db.read().await.is_some(),
                }),
            })
        }
        ClientMsg::Unsub {} => {
            *rx = None;
            None
        }
        ClientMsg::Cmd { id, op, args } => {
            if !*authed {
                return Some(Frame::Error {
                    id: Some(id),
                    code: "unauthorized".into(),
                    message: "send a hello with the token first".into(),
                });
            }
            Some(dispatch(app, id, &op, &args).await)
        }
    }
}
