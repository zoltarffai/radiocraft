//! RadioCraft application library: the panels and shared state behind the
//! RadioDJ-style main window.
//!
//! `main.rs` is only the eframe bootstrap; everything else lives here so
//! integration tests (`tests/`) can reach the same modules the GUI does.
//! The GUI never runs in tests - only the pure logic in each module.

pub mod app;
pub mod library;
pub mod players_zones;
pub mod playlists;
pub mod state;
pub mod windows;

#[cfg(feature = "tray")]
pub mod tray;
