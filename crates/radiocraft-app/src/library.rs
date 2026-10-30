//! Library browse panel: the paged song grid, the search box, and the
//! "add file(s) to the library" flow (RadioDJ's `Add files` button).
//!
//! Search state (`LibraryUi`) is deliberately tiny so the query itself can
//! live in `AppState::load_library`, which `state.rs` already runs against
//! the final db entities API. The pure helpers here (`clamp_selection`,
//! `new_song_from_path`) are unit-tested without a GUI or a database.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use radiocraft_audio::decoder::FileSource;
use radiocraft_db::entities::{NewSong, Song};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::state::{AppState, SongRow};

/// Panel state the library query reads (`search`) and writes (`selected`).
#[derive(Debug, Clone, Default)]
pub struct LibraryUi {
    /// Search needle; empty = browse the whole table.
    pub search: String,
    /// Row the operator clicked in the grid.
    pub selected: Option<usize>,
    /// Whether the library panel is docked/visible.
    pub open: bool,
    /// File picker root override (usually the last used path).
    pub add_root: Option<PathBuf>,
}

/// Keep `selected` inside `rows`; returns the new selection.
pub fn clamp_selection(selected: Option<usize>, rows: &[SongRow]) -> Option<usize> {
    match selected {
        Some(i) if i < rows.len() => Some(i),
        _ => None,
    }
}

/// Next row below `selected` (wraps at the end).
pub fn next_selection(selected: Option<usize>, rows: &[SongRow]) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    Some(match selected {
        Some(i) if i + 1 < rows.len() => i + 1,
        Some(_) => 0,
        None => 0,
    })
}

/// Previous row above `selected` (wraps at the top).
pub fn prev_selection(selected: Option<usize>, rows: &[SongRow]) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    Some(match selected {
        Some(0) | None => rows.len() - 1,
        Some(i) => i - 1,
    })
}

/// Artist/title guess from a file stem: `Artist - Title` splits like RadioDJ.
pub fn split_stem(path: &Path) -> (String, String) {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match stem.split_once(" - ") {
        Some((artist, title)) => (artist.trim().to_string(), title.trim().to_string()),
        None => (String::new(), stem),
    }
}

/// Build the insert row for one audio file: duration comes from the
/// decoder, artist/title come from the file name.
pub fn new_song_from_path(path: &Path) -> Result<NewSong> {
    let source = FileSource::open(path).with_context(|| format!("opening {}", path.display()))?;
    let duration = source
        .duration()
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let (artist, title) = split_stem(path);
    let dec = Decimal::from_f64_retain(duration).unwrap_or_default();
    Ok(NewSong {
        path: path.display().to_string(),
        song_type: 3, // music, the RadioDJ default for added files
        id_subcat: 0,
        id_genre: 0,
        artist,
        original_artist: String::new(),
        title,
        album: String::new(),
        duration: dec,
        original_duration: dec,
        enabled: true,
        weight: Decimal::from(16),
    })
}

/// Insert files, skipping paths already in the table (RadioDJ's behaviour).
/// Returns the paths that were actually added.
pub fn add_files(state: &mut AppState, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let Some(db) = state.db.as_ref() else {
        state.set_status("Connect to the database first");
        return Ok(Vec::new());
    };
    let pool = db.pool();
    let mut added = Vec::new();
    let mut failures = 0usize;
    for path in paths {
        let song = match new_song_from_path(path) {
            Ok(song) => song,
            Err(err) => {
                tracing::warn!("adding {}: {err:#}", path.display());
                failures += 1;
                continue;
            }
        };
        let existing = state.rt.block_on(radiocraft_db::entities::song_by_path(
            pool,
            &song.path,
        ));
        match existing {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(err) => {
                state.report_error("checking the library for the file", &anyhow::Error::from(err));
                failures += 1;
                continue;
            }
        }
        match state
            .rt
            .block_on(radiocraft_db::entities::insert_song(pool, &song))
        {
            Ok(_) => added.push(path.clone()),
            Err(err) => {
                state.report_error("inserting the song", &anyhow::Error::from(err));
                failures += 1;
            }
        }
    }
    if added.is_empty() {
        if failures > 0 {
            state.set_status(format!("Nothing added ({failures} file(s) failed)"));
        } else {
            state.set_status("Files were already in the library");
        }
    } else {
        state.set_status(format!("Added {} file(s) to the library", added.len()));
        state.load_library();
    }
    Ok(added)
}

/// Pick files with the native dialog, rooted at `LastUsedPath`.
pub fn pick_audio_files(state: &mut AppState) -> Vec<PathBuf> {
    let root = state.last_used_path();
    let picked = rfd::FileDialog::new()
        .set_root(&root)
        .add_filter(
            "Audio",
            &["mp3", "flac", "ogg", "wav", "m4a", "mp4", "aac"],
        )
        .pick_files()
        .unwrap_or_default();
    if let Some(last) = picked.last() {
        if let Some(parent) = last.parent() {
            state
                .general
                .set_last_used_path(parent.display().to_string());
            if let Err(err) = state.save_general() {
                tracing::warn!("saving LastUsedPath: {err:#}");
            }
        }
    }
    picked
}

/// The row's path, for "play now".
pub fn row_path(row: &SongRow) -> Option<&Path> {
    if row.path.is_empty() {
        None
    } else {
        Some(Path::new(&row.path))
    }
}

/// Does this row match the search needle (client-side sanity check the
/// grid uses before trusting the server result).
pub fn row_matches(row: &SongRow, needle: &str) -> bool {
    let needle = needle.trim().to_lowercase();
    needle.is_empty()
        || row.artist.to_lowercase().contains(&needle)
        || row.title.to_lowercase().contains(&needle)
        || row.album.to_lowercase().contains(&needle)
}

/// Cache row for a song fetched by ID (used after playlist edits).
pub fn row_from_song(song: &Song) -> SongRow {
    SongRow {
        id: i64::from(song.ID),
        artist: song.artist.clone(),
        title: song.title.clone(),
        album: song.album.clone(),
        duration_secs: song.duration.to_f64().unwrap_or(0.0),
        path: song.path.clone(),
    }
}

impl LibraryUi {
    /// Grid keys: down/up move the selection.
    pub fn handle_keys(&mut self, rows: &[SongRow], key: egui::Key) {
        match key {
            egui::Key::ArrowDown => self.selected = next_selection(self.selected, rows),
            egui::Key::ArrowUp => self.selected = prev_selection(self.selected, rows),
            _ => {}
        }
    }

    /// The library side panel.
    pub fn draw(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        ui.horizontal(|ui| {
            ui.label("Search");
            let changed = ui
                .add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("artist, title or album")
                        .desired_width(160.0),
                )
                .changed();
            if ui.button("Go").clicked() || changed {
                state.load_library();
            }
            if ui.button("Refresh").clicked() {
                state.load_library();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Add file").clicked() {
                let picked = pick_audio_files(state);
                if !picked.is_empty() {
                    if let Err(err) = add_files(state, &picked) {
                        state.report_error("adding files", &err);
                    }
                }
            }
            let have_sel = self.selected.is_some() && self.selected.unwrap() < state.library.rows.len();
            if ui.button("Add to playlist").clicked() && have_sel {
                let row_id = i32::try_from(state.library.rows[self.selected.unwrap()].id)
                    .unwrap_or(0);
                state.playlists_ui.add_songs(state, &[row_id]);
            }
            if ui.button("Play now").clicked() && have_sel {
                let row = state.library.rows[self.selected.unwrap()].clone();
                if let Some(path) = row_path(&row) {
                    let path = path.to_path_buf();
                    crate::playlists::play_song_now(state, &row, &path);
                } else {
                    state.set_status("This row has no file path");
                }
            }
        });
        if state.library.rows.is_empty() {
            ui.label("Library is empty (or not connected).");
            return;
        }
        let response = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ui.text_style_height(&egui::TextStyle::Body), state.library.rows.len(), |ui, row_range| {
                for i in row_range {
                    let row = &state.library.rows[i];
                    let sel = self.selected == Some(i);
                    let text = format!(
                        "{} - {}   [{}]",
                        row.artist,
                        row.title,
                        radiocraft_core::template::format_hms(row.duration_secs)
                    );
                    let button = ui.selectable_label(sel, text);
                    if button.clicked() {
                        self.selected = Some(i);
                    }
                    if button.double_clicked() {
                        self.selected = Some(i);
                        let row_id = i32::try_from(row.id).unwrap_or(0);
                        state.playlists_ui.add_songs(state, &[row_id]);
                    }
                }
            });
        let _response = response;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, title: &str) -> SongRow {
        SongRow {
            id,
            artist: "Artist".into(),
            title: title.into(),
            album: String::new(),
            duration_secs: 10.0,
            path: format!("/music/{title}.mp3"),
        }
    }

    #[test]
    fn selection_navigation_wraps() {
        let rows = [row(1, "a"), row(2, "b"), row(3, "c")];
        assert_eq!(next_selection(Some(0), &rows), Some(1));
        assert_eq!(next_selection(Some(2), &rows), Some(0));
        assert_eq!(next_selection(None, &rows), Some(0));
        assert_eq!(prev_selection(Some(0), &rows), Some(2));
        assert_eq!(prev_selection(None, &rows), Some(2));
        assert_eq!(next_selection(None, &[]), None);
    }

    #[test]
    fn clamp_drops_out_of_range() {
        let rows = [row(1, "a")];
        assert_eq!(clamp_selection(Some(0), &rows), Some(0));
        assert_eq!(clamp_selection(Some(5), &rows), None);
        assert_eq!(clamp_selection(None, &rows), None);
    }

    #[test]
    fn stem_splits_artist_title() {
        assert_eq!(
            split_stem(Path::new("/x/Metallica - Enter Sandman.mp3")),
            ("Metallica".into(), "Enter Sandman".into())
        );
        assert_eq!(
            split_stem(Path::new("/x/solo track.wav")),
            (String::new(), "solo track".into())
        );
    }

    #[test]
    fn row_matches_is_case_insensitive() {
        let r = row(1, "Song");
        assert!(row_matches(&r, ""));
        assert!(row_matches(&r, "song"));
        assert!(row_matches(&r, "ARTIST"));
        assert!(!row_matches(&r, "zzz"));
    }

    #[test]
    fn new_song_from_missing_file_errors() {
        let err = new_song_from_path(Path::new("/nonexistent/track.mp3"));
        assert!(err.is_err());
    }
}
