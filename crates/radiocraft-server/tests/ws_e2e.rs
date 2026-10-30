//! End-to-end protocol test over a real socket: connect, `ready`,
//! subscribe, run a mixer command, and watch mirrored frames arrive.

use std::sync::Arc;

use radiocraft_server::auth::Auth;
use radiocraft_server::frames::{ClientMsg, Frame};
use radiocraft_server::http::router;
use radiocraft_server::state::App;

#[tokio::test]
async fn ws_round_trip() {
    let dir = std::env::temp_dir().join(format!("rc-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let app = Arc::new(App::load(&dir).expect("load"));
    let _pump = radiocraft_server::pump::spawn_pump(Arc::clone(&app));

    let server = axum_test::TestServer::builder()
        .http_transport()
        .build(router(Arc::clone(&app), Arc::new(Auth::default()), None));

    let mut ws = server.get_websocket("/ws").await.into_websocket().await;

    // 1. Ready frame on connect.
    let ready: Frame = serde_json::from_str(&ws.receive_text().await).expect("ready");
    match ready {
        Frame::Ready { protocol, zones, .. } => {
            assert_eq!(protocol, radiocraft_server::frames::PROTOCOL_VERSION);
            assert_eq!(zones, vec!["main", "aux", "cart", "instant"]);
        }
        other => panic!("expected ready, got {other:?}"),
    }

    // 2. Ping/pong.
    ws.send_json(&ClientMsg::Ping {}).await;
    let pong: Frame = serde_json::from_str(&ws.receive_text().await).expect("pong");
    assert!(matches!(pong, Frame::Pong {}));

    // 3. Sub returns a snapshot, then mirrored frames keep flowing.
    ws.send_json(&ClientMsg::Sub {}).await;
    // The direct snapshot reply.
    let snapshot = ws.receive_text().await;
    let frame: Frame = serde_json::from_str(&snapshot).expect("sub reply");
    assert!(matches!(frame, Frame::Result { .. }), "{frame:?}");

    // 4. A command with id correlation.
    ws.send_json(&serde_json::json!({
        "t": "cmd", "id": 7, "op": "set_zone_fader_db", "args": { "zone": "cart", "db": -3.0 }
    }))
    .await;
    let mut saw_result = false;
    for _ in 0..40 {
        let frame: Frame = serde_json::from_str(&ws.receive_text().await).expect("frame");
        if let Frame::Result { id, op, data } = &frame {
            if *id == 7 && op == "set_zone_fader_db" {
                assert_eq!(data["fader_db"], serde_json::json!(-3.0));
                saw_result = true;
                break;
            }
        }
    }
    assert!(saw_result, "cmd result with id 7 never arrived");

    // 5. Unknown op replies with an error frame.
    ws.send_json(&serde_json::json!({ "t": "cmd", "id": 8, "op": "bogus" }))
        .await;
    let mut saw_error = false;
    for _ in 0..40 {
        let frame: Frame = serde_json::from_str(&ws.receive_text().await).expect("frame");
        if let Frame::Error { id: Some(8), .. } = frame {
            saw_error = true;
            break;
        }
    }
    assert!(saw_error, "error frame with id 8 never arrived");
}
