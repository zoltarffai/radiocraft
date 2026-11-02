//! The WebSocket/JSON protocol frames (docs/05 §"Wire protocol").
//!
//! Every message is a single JSON object with a tag field `t`.
//! Client → server: `{"t":"cmd","id":7,"op":"play","args":{...}}`.
//! Server → client: `{"t":"player",...}`, `{"t":"meters",...}` etc.
//! `Result`/`Error` frames carry the command `id` for correlation.

use serde::{Deserialize, Serialize};

/// Wire codec for dB values: JSON has no infinity, so silent (`-inf`) is
/// sent as `null` and read back as `-inf`.
pub mod db_wire {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        if v.is_finite() {
            s.serialize_f64(*v)
        } else {
            s.serialize_none()
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        let value = Option::<f64>::deserialize(d)?;
        Ok(value.unwrap_or(f64::NEG_INFINITY))
    }
}

/// The playout zones by their wire names (mirror of [`radiocraft_audio::mixer::Zone`]).
pub const ZONES: [&str; 4] = ["main", "aux", "cart", "instant"];

/// Current protocol revision; clients check it in the `ready` frame.
pub const PROTOCOL_VERSION: u32 = 1;

/// One zone's transport state (sent inside `player` frames).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneState {
    pub zone: String,
    pub playing: bool,
    pub paused: bool,
    /// Live position in seconds, when something is loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_secs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_secs: Option<f64>,
}

/// Cartwall button currently rolled on the cart zone (null in `player`
/// frames when no cart is playing). Mirrors RadioDJ's `CartStarted` host
/// signal: the wall highlights the active button until it stops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CartNowInfo {
    /// `carts_list.ID` of the button.
    pub button_id: u32,
    /// `carts.ID` of the cart wall the button belongs to.
    pub cart_id: u32,
    /// `songs.ID` of the linked track.
    pub song_id: i64,
    pub title: String,
}

/// Now-playing metadata block (null in `player` frames when idle).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NowInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song_id: Option<i64>,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub path: String,
    pub track_type: i32,
    pub duration_secs: f64,
    /// Segment (hook) length in seconds, when a segment was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment_secs: Option<f64>,
    /// Item start, local wall clock, RFC 3339.
    pub started_at: String,
}

/// Master bus meters (20 Hz).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelInfo {
    #[serde(with = "db_wire")]
    pub level_db: f64,
    #[serde(with = "db_wire")]
    pub peak_db: f64,
    #[serde(with = "db_wire")]
    pub peak_hold_db: f64,
    #[serde(with = "db_wire")]
    pub rms_db: f64,
}

/// One zone's mixer knobs (mirrored server-side so GET state is complete).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneMix {
    pub zone: String,
    #[serde(with = "db_wire")]
    pub fader_db: f64,
    pub muted: bool,
}

/// Status line content (the RadioDJ "what happened" line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusInfo {
    pub level: String,
    pub message: String,
}

/// Server → client frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Frame {
    /// First frame on a new WebSocket: protocol and feature banner.
    Ready {
        protocol: u32,
        server: String,
        version: String,
        zones: Vec<String>,
        authenticated: bool,
    },
    /// Reply to a `cmd`; the payload fields are merged into the frame.
    Result {
        id: u64,
        op: String,
        #[serde(flatten)]
        data: serde_json::Value,
    },
    /// Reply to a `cmd` (or a protocol error): `id` is null for parse errors.
    Error {
        id: Option<u64>,
        code: String,
        message: String,
    },
    /// Now-playing + per-zone transport state (pushed on change).
    Player {
        now: Option<NowInfo>,
        zones: Vec<ZoneState>,
        /// Active cartwall button, when a cart is rolled.
        cart: Option<CartNowInfo>,
    },
    /// Master meters + live positions, pushed at 20 Hz.
    Meters {
        master: LevelInfo,
        zones: Vec<ZoneState>,
    },
    /// Master gain + per-zone faders/mutes (pushed on change).
    State {
        #[serde(with = "db_wire")]
        master_db: f64,
        zones: Vec<ZoneMix>,
    },
    /// Status line update.
    Status(StatusInfo),
    /// Plugin-originated event: a host `StateChanged` fan-out or a UI
    /// window request (see `radiocraft-plugin` host contract). The payloads
    /// are boxed to keep the frame enum lean (they are usually absent).
    PluginEvent {
        plugin: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<Box<radiocraft_plugin::host::StateInfo>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        ui: Option<Box<radiocraft_plugin::host::PluginUiRequest>>,
    },
    /// Reply to `{"t":"ping"}`.
    Pong {},
}

/// Client → server messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ClientMsg {
    /// Keep-alive; answers with `pong`.
    Ping {},
    /// Offer credentials; required before commands when the server demands login.
    Hello {
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<String>,
    },
    /// Start receiving `player`/`meters`/`state`/`status` frames.
    Sub {},
    /// Stop receiving pushes (replies `result` for the cmd path only).
    Unsub {},
    /// Invoke an operation; `args` is op-specific.
    Cmd {
        id: u64,
        op: String,
        #[serde(default)]
        args: serde_json::Value,
    },
}

impl ClientMsg {
    /// The command id for correlation in replies, when this is a `cmd`.
    pub fn cmd_id(&self) -> Option<u64> {
        match self {
            Self::Cmd { id, .. } => Some(*id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_frame_tag() {
        let f = Frame::Ready {
            protocol: PROTOCOL_VERSION,
            server: "radiocraft-server".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            zones: ZONES.iter().map(|z| (*z).to_string()).collect(),
            authenticated: false,
        };
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.starts_with(r#"{"t":"ready","protocol":1"#), "{s}");
    }

    #[test]
    fn result_frame_merges_payload() {
        let f = Frame::Result {
            id: 7,
            op: "play".into(),
            data: serde_json::json!({"playing": true, "positionSecs": 1.5}),
        };
        let v: serde_json::Value = serde_json::to_string(&f)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(v["t"], "result");
        assert_eq!(v["id"], 7);
        assert_eq!(v["op"], "play");
        assert_eq!(v["playing"], true);
        assert_eq!(v["positionSecs"], 1.5);
    }

    #[test]
    fn client_cmd_decodes() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"t":"cmd","id":3,"op":"playFile","args":{"path":"/a/b.mp3"}}"#)
                .unwrap();
        match m {
            ClientMsg::Cmd { id, op, args } => {
                assert_eq!(id, 3);
                assert_eq!(op, "playFile");
                assert_eq!(args["path"], "/a/b.mp3");
            }
            other => panic!("expected cmd, got {other:?}"),
        }
    }

    #[test]
    fn client_cmd_without_args() {
        let m: ClientMsg = serde_json::from_str(r#"{"t":"cmd","id":3,"op":"stopAll"}"#).unwrap();
        match m {
            ClientMsg::Cmd { args, .. } => assert!(args.is_null()),
            other => panic!("expected cmd, got {other:?}"),
        }
    }
}
