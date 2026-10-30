//! The RadioCraft playout server (docs/05).
//!
//! Single source of truth for playout: the engine and all business logic
//! stay in Rust; every UI (browser or Tauri window) talks the WebSocket
//! protocol in [`frames`] and the REST surface in [`http`].

pub mod auth;
pub mod dispatch;
pub mod frames;
pub mod http;
pub mod playback;
pub mod pump;
pub mod state;
pub mod ws;

pub use state::App;
