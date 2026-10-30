//! Playlist panel: the saved playlist list and the running-order item
//! grid. RadioDJ keeps `ord` as the display/order column; editing the
//! order here (move up/down, remove) rewrites it on save, so the grid
//! stays exactly what the playout reads next.

use radiocraft_db::entities::{self, NewPlaylistItem, PlaylistHeader, PlaylistItem};
use rust_decimal::Decimal;

use crate::state::{AppState, NowPlaying, SongRow};
use chrono::Local;

/// One grid row for the item grid (DB row + resolved labels).
#[derive(Debug, Clone)]
pub struct PlaylistRow {
    pub item: PlaylistItem,
    pub artist: String,
    pub title: String,
    pub duration_secs: f64,
}

/// Panel state for the playlist dock.
#[derive(Debug, Default)]
pub struct PlaylistsUi {
    /// Saved playlists from the `playlists` table.
    pub lists: Vec<PlaylistHeader>,
    /// Currently selected playlist ID.
    pub current: Option<u32>,
    /// Name field for "new playlist".
    pub new_name: String,
    /// Rows (with labels already looked up) in `ord` order.
    pub rows: Vec<PlaylistRow>,
    pub selected: Option<usize>,
    /// Panel visibility (the View menu toggles it).
    pub open: bool,
}

/// Rewrite `ord` of these inserts to 1..=len, in the given order.
pub fn reindex_ords(mut items: Vec<NewPlaylistItem>) -> Vec<NewPlaylistItem> {
    for (i, item) in items.iter_mut().enumerate() {
        item.ord = (i + 1) as u32;
    }
    items.shrink_to_fit();
    items
}

/// The insert row `items[idx]` moves to after/below `items[target]`.
///
/// Returns the new index of the moved row, or `None` if it cannot move.
pub fn swap_rows(rows: &mut Vec<PlaylistRow>, idx: usize, target: usize) -> Option<usize> {
    if idx >= rows.len() || target >= rows.len() || idx == target {
        return None;
    }
    let row = rows.remove(idx);
    rows.insert(target, row);
    Some(target)
}

/// Turn current rows into re-insertable items with a fresh order.
pub fn rows_to_items(rows: &[PlaylistRow], playlist_id: u32) -> Vec<NewPlaylistItem> {
    let items: Vec<NewPlaylistItem> = rows
        .iter()
        .map(|r| NewPlaylistItem {
            pID: playlist_id,
            sID: r.item.sID,
            cstart: r.item.cstart,
            cnext: r.item.cnext,
            cend: r.item.cend,
            fin: r.item.fin,
            fout: r.item.fout,
            swID: r.item.swID,
            swplay: r.item.swplay,
            vtID: r.item.vtID,
            vtplay: r.item.vtplay,
            swfirst: r.item.swfirst,
            ord: 0,
        })
        .collect();
    reindex_ords(items)
}

/// A fresh playlist row for a song ID: cue columns are zero, which is
/// RadioDJ's "not set" encoding.
pub fn item_for_song(playlist_id: u32, song_id: i32, ord: u32) -> NewPlaylistItem {
    NewPlaylistItem {
        pID: playlist_id,
        sID: song_id,
        cstart: Decimal::ZERO,
        cnext: Decimal::ZERO,
        cend: Decimal::ZERO,
        fin: Decimal::ZERO,
        fout: Decimal::ZERO,
        swID: 0,
        swplay: Decimal::ZERO,
        vtID: 0,
        vtplay: Decimal::ZERO,
        swfirst: false,
        ord,
    }
}

/// Play a library row on the main zone right now (the "Play" button).
///
/// Mirs the current playlist-item cue semantics: no row cues, defaults.
pub fn play_song_now(state: &mut AppState, row: &SongRow, path: &std::path::Path) {
    let plan = crate::players_zones::CuePlan::default();
    match crate::players_zones::start_file(state, crate::players_zones::main_zone(), path, plan) {
        Ok(duration) => {
            let duration = if duration > 0.0 {
                duration
            } else {
                row.duration_secs
            };
            state.on_track_started(NowPlaying {
                song_id: row.id,
                path: path.display().to_string(),
                artist: row.artist.clone(),
                title: row.title.clone(),
                album: row.album.clone(),
                duration_secs: duration,
                track_type: 0,
                started_at: Local::now(),
                cue_start: 0.0,
                cue_next: 0.0,
                cue_end: duration,
                fade_in_ms: 0,
                fade_out_ms: *state.general.dur_fadeout(),
            });
        }
        Err(err) => state.report_error("playing the row", &err),
    }
}

impl PlaylistsUi {
    /// Load the playlist names after a DB (re)connect.
    pub fn load_lists(&mut self, state: &mut AppState) {
        let Some(db) = state.db.as_ref() else {
            self.lists.clear();
            self.current = None;
            self.rows.clear();
            return;
        };
        match state.rt.block_on(entities::list_playlists(db.pool())) {
            Ok(lists) => {
                self.current = lists.first().map(|l| l.ID);
                self.lists = lists;
                self.reload_items(state);
            }
            Err(err) => state.report_error("listing the playlists", &anyhow::Error::from(err)),
        }
    }

    /// Re-fetch the current playlist's items and resolve song labels.
    pub fn reload_items(&mut self, state: &mut AppState) {
        self.rows.clear();
        self.selected = None;
        let Some(db) = state.db.as_ref() else {
            return;
        };
        let Some(pid) = self.current else {
            return;
        };
        let pool = db.pool();
        let fetched = state.rt.block_on(async {
            let items = entities::list_playlist_items(pool, pid).await?;
            let mut rows = Vec::with_capacity(items.len());
            for item in items {
                let song = if item.sID > 0 {
                    entities::song_by_id(pool, item.sID as u32).await?
                } else {
                    None
                };
                rows.push(PlaylistRow {
                    item,
                    artist: song.as_ref().map(|s| s.artist.clone()).unwrap_or_default(),
                    title: song
                        .as_ref()
                        .map(|s| s.title.clone())
                        .unwrap_or_else(|| "(missing song)".to_string()),
                    duration_secs: song
                        .as_ref()
                        .and_then(|s| rust_decimal::prelude::ToPrimitive::to_f64(&s.duration))
                        .unwrap_or(0.0),
                });
            }
            anyhow::Ok(rows)
        });
        match fetched {
            Ok(rows) => self.rows = rows,
            Err(err) => state.report_error("loading the playlist items", &err),
        }
    }

    pub fn select_playlist(&mut self, state: &mut AppState, id: u32) {
        self.current = Some(id);
        self.reload_items(state);
    }

    /// Append songs (library row IDs) to the current playlist.
    pub fn add_songs(&mut self, state: &mut AppState, song_ids: &[i32]) {
        let Some(db) = state.db.as_ref() else {
            state.set_status("Connect to the database first");
            return;
        };
        let Some(pid) = self.current else {
            state.set_status("Pick a playlist first (or create one)");
            return;
        };
        if song_ids.is_empty() {
            return;
        }
        let pool = db.pool();
        let first_ord = self.rows.len() as u32 + 1;
        let mut added = 0usize;
        for (i, sid) in song_ids.iter().enumerate() {
            let item = item_for_song(pid, *sid, first_ord + i as u32);
            match state.rt.block_on(entities::insert_playlist_item(pool, &item)) {
                Ok(_) => added += 1,
                Err(err) => {
                    state.report_error("adding the item", &anyhow::Error::from(err));
                    break;
                }
            }
        }
        if added > 0 {
            self.reload_items(state);
            state.set_status(format!("Added {added} item(s) to the playlist"));
        }
    }

    /// Remove the selected item (order is rewritten afterwards).
    pub fn remove_selected(&mut self, state: &mut AppState) {
        let Some(index) = self.selected else {
            state.set_status("Select a playlist row first");
            return;
        };
        let Some(db) = state.db.as_ref() else {
            return;
        };
        let Some(pid) = self.current else {
            return;
        };
        let item_id = self.rows[index].item.ID;
        match state
            .rt
            .block_on(entities::delete_playlist_item(db.pool(), item_id))
        {
            Ok(_) => {}
            Err(err) => {
                state.report_error("removing the item", &anyhow::Error::from(err));
                return;
            }
        }
        self.save_order(state, pid);
        self.selected = Some(index.min(self.rows.len().saturating_sub(1)));
    }

    /// Move the selected row up/down and persist the new order.
    pub fn move_selected(&mut self, state: &mut AppState, delta: isize) {
        let Some(index) = self.selected else {
            state.set_status("Select a playlist row first");
            return;
        };
        let target = match delta {
            -1 => index.saturating_sub(1),
            1 => (index + 1).min(self.rows.len().saturating_sub(1)),
            _ => return,
        };
        if swap_rows(&mut self.rows, index, target).is_none() {
            return;
        }
        self.selected = Some(target);
        if let Some(pid) = self.current {
            self.save_order(state, pid);
        }
    }

    /// Clear + reinsert every row of `pid` with ord = grid position.
    fn save_order(&mut self, state: &mut AppState, pid: u32) {
        let Some(db) = state.db.as_ref() else {
            return;
        };
        let items = rows_to_items(&self.rows, pid);
        let pool = db.pool();
        let result = state.rt.block_on(async {
            let rows = entities::list_playlist_items(pool, pid).await?;
            self.rows = rows
                .into_iter()
                .map(|item| PlaylistRow {
                    item,
                    artist: String::new(),
                    title: String::new(),
                    duration_secs: 0.0,
                })
                .collect();
            anyhow::Ok(())
        });
        if let Err(err) = result {
            state.report_error("re-reading the playlist", &err);
            return;
        }
        let result = state.rt.block_on(async {
            entities::clear_playlist_items(pool, pid).await?;
            for item in &items {
                entities::insert_playlist_item(pool, item).await?;
            }
            anyhow::Ok(())
        });
        if let Err(err) = result {
            state.report_error("saving the playlist order", &err);
        }
        self.reload_items(state);
    }

    /// Create a playlist from `new_name`.
    pub fn create_playlist(&mut self, state: &mut AppState) {
        let Some(db) = state.db.as_ref() else {
            state.set_status("Connect to the database first");
            return;
        };
        let name = self.new_name.trim().to_string();
        if name.is_empty() {
            state.set_status("Enter a playlist name first");
            return;
        }
        match state.rt.block_on(entities::insert_playlist(db.pool(), &name)) {
            Ok(id) => {
                self.new_name.clear();
                self.load_lists(state);
                self.current = Some(id);
                self.reload_items(state);
                state.set_status(format!("Created playlist {name}"));
            }
            Err(err) => state.report_error("creating the playlist", &anyhow::Error::from(err)),
        }
    }

    /// Rename the current playlist.
    pub fn rename_playlist(&mut self, state: &mut AppState, name: &str) {
        let Some(db) = state.db.as_ref() else {
            return;
        };
        let Some(pid) = self.current else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        match state
            .rt
            .block_on(entities::rename_playlist(db.pool(), pid, &name))
        {
            Ok(_) => {
                self.load_lists(state);
                self.current = Some(pid);
                state.set_status(format!("Renamed playlist to {name}"));
            }
            Err(err) => state.report_error("renaming the playlist", &anyhow::Error::from(err)),
        }
    }

    /// Delete the current playlist (items included).
    pub fn delete_current(&mut self, state: &mut AppState) {
        let Some(db) = state.db.as_ref() else {
            return;
        };
        let Some(pid) = self.current else {
            return;
        };
        match state.rt.block_on(entities::delete_playlist(db.pool(), pid)) {
            Ok(_) => {
                self.load_lists(state);
                state.set_status("Playlist deleted");
            }
            Err(err) => state.report_error("deleting the playlist", &anyhow::Error::from(err)),
        }
    }

    /// The playlist panel.
    pub fn draw(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("playlist picker")
                .selected_text(
                    self.current
                        .and_then(|id| self.lists.iter().find(|l| l.ID == id))
                        .map(|l| l.name.clone())
                        .unwrap_or_else(|| "(no playlist)".into()),
                )
                .show_ui(ui, |ui| {
                    for list in &self.lists {
                        ui.selectable_value(&mut self.current, Some(list.ID), &list.name);
                    }
                });
            if ui.button("New").clicked() {
                self.create_playlist(state);
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.new_name)
                    .hint_text("new name")
                    .desired_width(110.0),
            );
            if ui.button("Delete").clicked() {
                self.delete_current(state);
            }
            if ui.button("Refresh").clicked() {
                self.reload_items(state);
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Move up").clicked() {
                self.move_selected(state, -1);
            }
            if ui.button("Move down").clicked() {
                self.move_selected(state, 1);
            }
            if ui.button("Remove").clicked() {
                self.remove_selected(state);
            }
            if ui.button("Play item").clicked() {
                if let Some(index) = self.selected {
                    let row = self.rows[index].item.sID;
                    if row > 0 {
                        if let Some(db) = state.db.as_ref() {
                            let song = state
                                .rt
                                .block_on(entities::song_by_id(db.pool(), row as u32));
                            match song {
                                Ok(Some(song)) => {
                                    let lib_row = crate::library::row_from_song(&song);
                                    if let Some(path) = crate::library::row_path(&lib_row) {
                                        let path = path.to_path_buf();
                                        play_song_now(state, &lib_row, &path);
                                    }
                                }
                                Ok(None) => state.set_status("That item references a missing song"),
                                Err(err) => {
                                    state.report_error("reading the song", &anyhow::Error::from(err))
                                }
                            }
                        }
                    }
                }
            }
        });
        if self.rows.is_empty() {
            ui.label("Playlist is empty.");
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("playlist grid")
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("#");
                        ui.strong("Artist");
                        ui.strong("Title");
                        ui.strong("Length");
                        for (i, row) in self.rows.iter().enumerate() {
                            let sel = self.selected == Some(i);
                            let _ = ui.selectable_label(sel, (i + 1).to_string());
                            if ui
                                .selectable_label(sel, &row.artist)
                                .clicked()
                            {
                                self.selected = Some(i);
                            }
                            if ui
                                .selectable_label(sel, &row.title)
                                .clicked()
                            {
                                self.selected = Some(i);
                            }
                            ui.label(radiocraft_core::template::format_hms(
                                row.duration_secs,
                            ));
                            ui.end_row();
                        }
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ord: u32, sid: i32) -> PlaylistRow {
        PlaylistRow {
            item: PlaylistItem {
                ID: sid as u32,
                pID: 1,
                sID: sid,
                cstart: Decimal::ZERO,
                cnext: Decimal::ZERO,
                cend: Decimal::ZERO,
                fin: Decimal::ZERO,
                fout: Decimal::ZERO,
                swID: 0,
                swplay: Decimal::ZERO,
                vtID: 0,
                vtplay: Decimal::ZERO,
                swfirst: false,
                ord,
            },
            artist: "a".into(),
            title: format!("t{sid}"),
            duration_secs: 1.0,
        }
    }

    #[test]
    fn reindex_rewrites_ords_in_order() {
        let items = vec![
            item_for_song(7, 10, 99),
            item_for_song(7, 11, 5),
            item_for_song(7, 12, 1),
        ];
        let out = reindex_ords(items);
        assert_eq!(out.iter().map(|i| i.ord).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(out.iter().map(|i| i.sID).collect::<Vec<_>>(), [10, 11, 12]);
    }

    #[test]
    fn rows_to_items_carries_cue_values_and_reindexes() {
        let rows = vec![row(9, 3), row(4, 5)];
        let items = rows_to_items(&rows, 42);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].ord, 1);
        assert_eq!(items[1].ord, 2);
        assert_eq!(items[0].pID, 42);
        assert_eq!(items[0].sID, 3);
        assert_eq!(items[1].sID, 5);
    }

    #[test]
    fn swap_moves_rows() {
        let mut rows = vec![row(1, 1), row(2, 2), row(3, 3)];
        assert_eq!(swap_rows(&mut rows, 0, 2), Some(2));
        assert_eq!(rows.iter().map(|r| r.item.sID).collect::<Vec<_>>(), [2, 3, 1]);
        assert_eq!(swap_rows(&mut rows, 1, 1), None);
        assert_eq!(swap_rows(&mut rows, 9, 0), None);
    }

    #[test]
    fn item_for_song_zeroes_cues() {
        let item = item_for_song(1, 9, 4);
        assert!(item.cstart.is_zero());
        assert!(item.cnext.is_zero());
        assert!(item.cend.is_zero());
        assert!(item.fin.is_zero());
        assert!(item.fout.is_zero());
        assert_eq!(item.ord, 4);
        assert!(!item.swfirst);
    }
}
