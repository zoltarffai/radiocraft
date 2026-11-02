//! The RadioCraft playout server (docs/05).
//!
//! Single source of truth for playout: the engine and all business logic
//! stay in Rust; every UI (browser or Tauri window) talks the WebSocket
//! protocol in [`frames`] and the REST surface in [`http`].

pub mod auth;
pub mod autodj;
pub mod auxplayers;
pub mod cartwall;
pub mod dispatch;
pub mod frames;
pub mod grid;
pub mod http;
pub mod playback;
pub mod plugin_host;
pub mod rotations;
pub mod playlist;
pub mod pump;
pub mod settings;
pub mod run;
pub mod soundfx;
pub mod state;
pub mod ws;

pub use state::App;
