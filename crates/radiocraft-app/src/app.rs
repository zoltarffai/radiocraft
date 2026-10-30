//! The main RadioCraft window: status bar (station + now playing with the
//! remaining time), the library browser, the playlist panel, the transport
//! buttons, and the window menu. The playout zones live in their own
//! window ([`crate::players_zones::PlayersUi`]).
//!
//! Closing honours the `MinimizeToTray` setting through
//! [`radiocraft_net::autost::ClosePolicy`]: with it set, the X button hides
//! the window (`ViewportCommand::CancelClose` + `Visible(false)`); only the
//! menu's "Exit" (or the tray menu, under the `tray` feature) really quits.

use eframe::egui::{self, ViewportCommand};
use radiocraft_audio::player::PlayerEvent;
use radiocraft_core::template::format_hms;
use radiocraft_net::autost::{CloseAction, ClosePolicy};

use crate::library::{self, row_path};
use crate::players_zones::main_zone;
use crate::state::{AppState, StatusKind};

/// The eframe application.
pub struct RadioCraftApp {
    pub state: AppState,
    /// Set when the menu (or tray) asked for a real exit, so the close
    /// policy must not hide the window.
    exit_requested: bool,
    /// True after the close policy hid the window.
    hidden: bool,
    /// Real tray icon, only with the `tray` feature.
    #[cfg(feature = "tray")]
    _tray: Option<crate::tray::TrayGuard>,
}

impl RadioCraftApp {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            exit_requested: false,
            hidden: false,
            #[cfg(feature = "tray")]
            _tray: None,
        }
    }

    /// Attach the tray icon (no-op without the feature). Called by main.
    #[cfg(feature = "tray")]
    pub fn set_tray(&mut self, guard: crate::tray::TrayGuard) {
        self._tray = Some(guard);
    }

    /// Drain engine events and keep the now-playing header honest.
    fn poll_engine(&mut self, _ctx: &egui::Context) {
        let events = self.state.engine.take_events();
        for (zone, event) in events {
            if matches!(event, PlayerEvent::Finished) && zone == main_zone() {
                self.state.on_track_finished();
            }
        }
        self.state.refresh_now_playing();

        #[cfg(feature = "tray")]
        if let Some(tray) = &self._tray {
            if tray.poll(ctx) {
                self.hidden = false;
                self.state.set_status("Restored from the tray");
            }
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        let state = &mut self.state;
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Exit").clicked() {
                        self.exit_requested = true;
                        ui.close_menu();
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Players and zones").clicked() {
                        state.players_ui.open = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Configuration", |ui| {
                    if ui.button("Database").clicked() {
                        state.config_ui.db.open = true;
                        ui.close_menu();
                    }
                    if ui.button("Now playing").clicked() {
                        state.config_ui.now_playing.open = true;
                        ui.close_menu();
                    }
                    if ui.button("Web console").clicked() {
                        state.config_ui.web.open = true;
                        ui.close_menu();
                    }
                    if ui.button("Categories").clicked() {
                        state.config_ui.categories.open = true;
                        ui.close_menu();
                    }
                    if ui.button("Rotations").clicked() {
                        state.config_ui.rotations.open = true;
                        ui.close_menu();
                    }
                    if ui.button("Events").clicked() {
                        state.config_ui.events.open = true;
                        ui.close_menu();
                    }
                    if ui.button("Backup").clicked() {
                        state.config_ui.backup.open = true;
                        ui.close_menu();
                    }
                    if ui.button("Close behaviour").clicked() {
                        state.config_ui.close.open = true;
                        ui.close_menu();
                    }
                });
            });
        });
    }

    /// Station name, now playing with remaining time, and the status line.
    fn status_bar(&mut self, ctx: &egui::Context) {
        let state = &self.state;
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong(state.station_name().to_string());
                ui.separator();
                if let Some((elapsed, remaining)) = state.now_playing_clock() {
                    if let Some(np) = &state.now_playing {
                        ui.label(format!(
                            "Now playing: {} - {}   [{} / {}]",
                            np.artist,
                            np.title,
                            format_hms(elapsed),
                            format_hms(remaining)
                        ));
                    }
                } else {
                    ui.label("Nothing playing");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let color = match state.status.kind {
                        StatusKind::Info => egui::Color32::from_gray(190),
                        StatusKind::Warn => egui::Color32::from_rgb(220, 180, 60),
                        StatusKind::Error => egui::Color32::from_rgb(230, 90, 90),
                    };
                    ui.colored_label(color, state.status.message.clone());
                });
            });
        });
    }

    /// Play / Stop / Assist / AutoDJ plus a Restore button while hidden.
    fn transport_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("transport").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Play").clicked() {
                    play_selected(&mut self.state);
                }
                if ui.button("Stop").clicked() {
                    self.state.engine.stop(main_zone(), -1);
                    self.state.on_track_finished();
                    self.state.players_ui.panel_mut(main_zone()).playing = false;
                }
                ui.separator();
                if ui.button("Assist").clicked() {
                    assist_add(&mut self.state);
                }
                if ui.button("AutoDJ").clicked() {
                    self.state.config_ui.rotations.open = true;
                    self.state
                        .set_status("AutoDJ: review the rotations list here; the planner is not wired yet");
                }
                if self.hidden {
                    ui.separator();
                    if ui.button("Restore window").clicked() {
                        self.hidden = false;
                        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    }
                }
            });
        });
    }

    fn library_panel(&mut self, ctx: &egui::Context) {
        let state = &mut self.state;
        egui::SidePanel::left("library_panel")
            .default_width(380.0)
            .show(ctx, |ui| {
                let mut library_ui = std::mem::take(&mut state.library_ui);
                library_ui.draw(ui, state);
                state.library_ui = library_ui;
            });
    }

    fn playlist_panel(&mut self, ctx: &egui::Context) {
        let state = &mut self.state;
        egui::TopBottomPanel::bottom("playlist_panel")
            .default_height(240.0)
            .show(ctx, |ui| {
                let mut playlists_ui = std::mem::take(&mut state.playlists_ui);
                playlists_ui.draw(ui, state);
                state.playlists_ui = playlists_ui;
            });
    }

    /// The close request from the window manager or the menu bar.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        let policy = ClosePolicy::from_settings(&self.state.database);
        match policy.on_close(self.exit_requested) {
            CloseAction::HideToTray => {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                self.hidden = true;
                self.state.set_status("Hidden (close again to quit)");
            }
            CloseAction::Exit => {
                // eframe honours the close request on its own.
                self.exit_requested = true;
            }
        }
    }

    fn config_windows(&mut self, ctx: &egui::Context) {
        let state = &mut self.state;
        let mut windows = std::mem::take(&mut state.config_ui);
        windows.web.sync_now_playing(state);
        windows.db.show(ctx, state);
        windows.now_playing.show(ctx, state);
        windows.web.show(ctx, state);
        windows.categories.show(ctx, state);
        windows.rotations.show(ctx, state);
        windows.events.show(ctx, state);
        windows.backup.show(ctx, state);
        windows.close.show(ctx, state);
        state.config_ui = windows;
    }
}

impl eframe::App for RadioCraftApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Keep the clock ticking; egui 0.29's continuous repaint would also
        // cover this, but explicit requests keep the frame cost honest.
        ctx.request_repaint();

        self.poll_engine(ctx);
        self.menu_bar(ctx);
        self.status_bar(ctx);
        self.transport_bar(ctx);
        self.library_panel(ctx);
        self.playlist_panel(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("Players and zones window: open it from the View menu.");
        });

        let state = &mut self.state;
        let mut players_ui = std::mem::take(&mut state.players_ui);
        players_ui.draw(ctx, state);
        state.players_ui = players_ui;
        self.config_windows(ctx);

        // Close policy last so the buttons/windows above settle first.
        self.handle_close(ctx);
    }
}

/// Play the selected library row on the main zone.
fn play_selected(state: &mut AppState) {
    let index = match state.library_ui.selected {
        Some(i) if i < state.library.rows.len() => i,
        _ => {
            state.set_status("Select a library row first");
            return;
        }
    };
    let row = state.library.rows[index].clone();
    let path = match row_path(&row) {
        Some(p) if p.exists() => p.to_path_buf(),
        Some(_) => {
            state.set_status("The file for this row is missing from disk");
            return;
        }
        None => {
            state.set_status("This row has no file path");
            return;
        }
    };
    crate::playlists::play_song_now(state, &row, &path);
}

/// RadioDJ "Assist": pick a file, add it to the library, and (when one is
/// selected) append it to the current playlist so it plays next.
fn assist_add(state: &mut AppState) {
    let picked = library::pick_audio_files(state);
    if picked.is_empty() {
        return;
    }
    let added = match library::add_files(state, &picked) {
        Ok(added) => added,
        Err(err) => {
            state.report_error("assist add", &err);
            return;
        }
    };
    state.load_library();
    if added.is_empty() {
        state.set_status("Assist: nothing new (all files already in the library)");
        return;
    }
    if state.playlists_ui.current.is_some() {
        let db = match state.db.take() {
            Some(db) => db,
            None => {
                state.set_status(format!("Assist added {} file(s)", added.len()));
                return;
            }
        };
        let mut ids: Vec<i32> = Vec::new();
        for path in &added {
            let as_string = path.display().to_string();
            if let Ok(Some(song)) = state
                .rt
                .block_on(radiocraft_db::entities::song_by_path(db.pool(), &as_string))
            {
                if let Ok(id) = i32::try_from(song.ID) {
                    ids.push(id);
                }
            }
        }
        state.db = Some(db);
        if !ids.is_empty() {
            let mut playlists_ui = std::mem::take(&mut state.playlists_ui);
            playlists_ui.add_songs(state, &ids);
            state.playlists_ui = playlists_ui;
        }
    }
    state.set_status(format!("Assist added {} file(s)", added.len()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_policy_hide_honours_setting() {
        let mut settings = radiocraft_settings::DatabaseSettings::default();
        settings.set_minimize_to_tray(true);
        let policy = ClosePolicy::from_settings(&settings);
        assert!(matches!(policy.on_close(false), CloseAction::HideToTray));
        assert!(matches!(policy.on_close(true), CloseAction::Exit));
    }

    #[test]
    fn close_policy_exit_without_setting() {
        let settings = radiocraft_settings::DatabaseSettings::default();
        let policy = ClosePolicy::from_settings(&settings);
        assert!(matches!(policy.on_close(false), CloseAction::Exit));
    }
}
