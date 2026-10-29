//! Stored playlists (`playlists` + `playlists_list`): entry model, reorder /
//! lock operations, and the by-date filename expansion used when a playlist
//! is backed by a dated `.m3u` file (docs/02 section 3).

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::booking::sentinel;
use crate::template::render_date_mask;

/// One playlist row (`playlists_list`): either a song (`s_id`), an event
/// (`s_id` < 0 with the event id in `event_id`), or a line marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistEntry {
    /// Stable row id (`playlists_list.ID`); 0 = not yet persisted.
    pub id: i64,
    /// Owner playlist (`playlists_list.pID`).
    pub playlist_id: i64,
    /// Song id (`sID`); `sentinel::NO_REF` when this row is an event line.
    pub song_id: i64,
    /// Event id when `song_id == NO_REF` (`events.startEvent`-style link).
    pub event_id: Option<i64>,
    /// Position within the playlist (`ord`).
    pub ord: u32,
    // cue times (playlists_list decimals, seconds):
    pub cstart: f64,
    pub cnext: f64,
    pub cend: f64,
    pub fin: f64,
    pub fout: f64,
    // assigned sweeper / voice-track (0 in schema = none; NO_REF = none here):
    pub sw_id: i64,
    pub sw_play: f64,
    pub vt_id: i64,
    pub vt_play: f64,
    pub sw_first: bool,
    /// UI-side lock (RestorePlaylistAt/LockPlaylist); the core keeps it as a
    /// flag so reorder math can skip locked rows.
    pub locked: bool,
}

impl PlaylistEntry {
    /// A plain song row.
    pub fn song(playlist_id: i64, song_id: i64, ord: u32) -> Self {
        PlaylistEntry {
            id: 0,
            playlist_id,
            song_id,
            event_id: None,
            ord,
            cstart: 0.0,
            cnext: 0.0,
            cend: 0.0,
            fin: 0.0,
            fout: 0.0,
            sw_id: sentinel::NO_REF,
            sw_play: 0.0,
            vt_id: sentinel::NO_REF,
            vt_play: 0.0,
            sw_first: false,
            locked: false,
        }
    }

    /// An event marker row (sID = -1 convention).
    pub fn event(playlist_id: i64, event_id: i64, ord: u32) -> Self {
        PlaylistEntry {
            song_id: sentinel::NO_REF,
            event_id: Some(event_id),
            ..Self::song(playlist_id, sentinel::NO_REF, ord)
        }
    }

    pub fn is_event(&self) -> bool {
        self.song_id == sentinel::NO_REF
    }
}

/// Where a playlist's *contents* come from (the `playlists` row is just the
/// name; these sources mirror the app-side "playlist file modes").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlaylistSource {
    /// Ordinary stored rows in `playlists_list`.
    Stored { playlist_id: i64 },
    /// Dated `.m3u` file: folder + date mask (mask excludes the extension;
    /// `.m3u` is appended by [`expand_playlist_file`]).
    FileByDate {
        folder: String,
        /// Date mask of docs/02 `filebydate` tokens, e.g. `{YYYY}\{MM}\playlist`.
        mask: String,
    },
}

impl Default for PlaylistSource {
    fn default() -> Self {
        PlaylistSource::Stored { playlist_id: 0 }
    }
}

/// In-memory playlist builder: keeps entries in display order and supports
/// the drag/drop + lock operations the UI needs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlaylistBuilder {
    pub source: PlaylistSource,
    pub name: String,
    pub entries: Vec<PlaylistEntry>,
}

impl PlaylistBuilder {
    pub fn new(source: PlaylistSource, name: impl Into<String>) -> Self {
        PlaylistBuilder {
            source,
            name: name.into(),
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, mut entry: PlaylistEntry) -> &mut Self {
        entry.ord = self.entries.len() as u32;
        self.entries.push(entry);
        self
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert at `pos` (clamped) and renumber.
    pub fn insert(&mut self, pos: usize, mut entry: PlaylistEntry) {
        let pos = pos.min(self.entries.len());
        entry.ord = pos as u32;
        self.entries.insert(pos, entry);
        self.renumber();
    }

    pub fn remove(&mut self, pos: usize) -> Option<PlaylistEntry> {
        if pos >= self.entries.len() {
            return None;
        }
        let e = self.entries.remove(pos);
        self.renumber();
        Some(e)
    }

    /// Drag-and-drop move: take the row at `from`, place at `to`, renumber.
    /// Locked rows refuse moves *into* them (the target slot may not be a
    /// locked row) and a locked row may not move itself.
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.entries.len() {
            return false;
        }
        if self.entries[from].locked {
            return false;
        }
        let to = to.min(self.entries.len().saturating_sub(1));
        if self.entries.get(to).is_some_and(|e| e.locked) {
            return false; // dropping onto a locked row is refused
        }
        let e = self.entries.remove(from);
        self.entries.insert(to, e);
        self.renumber();
        true
    }

    /// LockPlaylist toggle.
    pub fn set_locked(&mut self, pos: usize, locked: bool) -> bool {
        match self.entries.get_mut(pos) {
            Some(e) => {
                e.locked = locked;
                true
            }
            None => false,
        }
    }

    /// RestorePlaylistAt: replace the row at `pos` (keeping its lock flag)
    /// with `entry`, keeping the rest of the order intact.
    pub fn restore_at(&mut self, pos: usize, mut entry: PlaylistEntry) -> bool {
        match self.entries.get_mut(pos) {
            Some(slot) => {
                entry.ord = slot.ord;
                entry.locked = slot.locked;
                *slot = entry;
                true
            }
            None => false,
        }
    }

    /// SavePlaylistAt: write the row at `pos` and clear its lock.
    pub fn save_at(&mut self, pos: usize, entry: PlaylistEntry) -> bool {
        if self.restore_at(pos, entry) {
            self.entries[pos].locked = false;
            true
        } else {
            false
        }
    }

    /// Renumber `ord` to the current display order.
    pub fn renumber(&mut self) {
        for (i, e) in self.entries.iter_mut().enumerate() {
            e.ord = i as u32;
        }
    }
}

/// Expand a by-date playlist source into the concrete file path for `date`:
/// folder + rendered mask + `.m3u`.
pub fn expand_playlist_file(folder: &str, mask: &str, date: NaiveDate) -> String {
    let rendered = render_date_mask(mask, date);
    let sep = if folder.ends_with('/') || folder.is_empty() {
        ""
    } else {
        "/"
    };
    format!("{folder}{sep}{rendered}.m3u")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(pid: i64, sid: i64) -> PlaylistEntry {
        PlaylistEntry::song(pid, sid, 0)
    }

    #[test]
    fn push_insert_remove_renumber() {
        let mut b = PlaylistBuilder::new(PlaylistSource::Stored { playlist_id: 1 }, "news");
        b.push(song(1, 10));
        b.push(song(1, 20));
        b.push(song(1, 30));
        assert_eq!(b.entries.iter().map(|e| e.ord).collect::<Vec<_>>(), vec![0, 1, 2]);

        b.insert(1, song(1, 99));
        assert_eq!(b.entries[1].song_id, 99);
        assert_eq!(b.entries.iter().map(|e| e.ord).collect::<Vec<_>>(), vec![0, 1, 2, 3]);

        let removed = b.remove(0).unwrap();
        assert_eq!(removed.song_id, 10);
        assert_eq!(b.entries.len(), 3);
        assert_eq!(b.entries.iter().map(|e| e.ord).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(b.remove(99).is_none());
    }

    #[test]
    fn drag_drop_reorder_and_locks() {
        let mut b = PlaylistBuilder::new(PlaylistSource::Stored { playlist_id: 2 }, "music");
        for sid in [1, 2, 3, 4] {
            b.push(song(2, sid));
        }
        // move row 0 -> 2
        assert!(b.reorder(0, 2));
        assert_eq!(b.entries.iter().map(|e| e.song_id).collect::<Vec<_>>(), vec![2, 3, 1, 4]);
        assert_eq!(b.entries.iter().map(|e| e.ord).collect::<Vec<_>>(), vec![0, 1, 2, 3]);

        // lock row 1; dropping onto it fails, and moving it fails.
        assert!(b.set_locked(1, true));
        assert!(!b.reorder(0, 1));
        assert!(!b.reorder(1, 3));

        // save_at clears the lock; restore_at keeps it.
        assert!(b.save_at(1, song(2, 55)));
        assert!(!b.entries[1].locked);
        assert!(b.set_locked(1, true));
        assert!(b.restore_at(1, song(2, 66)));
        assert!(b.entries[1].locked);
        assert_eq!(b.entries[1].song_id, 66);
    }

    #[test]
    fn event_rows_and_defaults() {
        let e = PlaylistEntry::event(3, 42, 0);
        assert!(e.is_event());
        assert_eq!(e.event_id, Some(42));
        assert_eq!(e.song_id, sentinel::NO_REF);
        assert_eq!(e.sw_id, sentinel::NO_REF);
        assert_eq!(e.vt_id, sentinel::NO_REF);
    }

    #[test]
    fn playlist_by_date_mask_expansion() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        assert_eq!(
            expand_playlist_file("/playlists", "{YYYY}\\{MM}\\list_{DD}", d),
            "/playlists/2026\\10\\list_29.m3u"
        );
        // mask with month name, folder already ends with separator
        assert_eq!(
            expand_playlist_file("/pl/", "{MONTHNAMESHORT}-{D}", d),
            "/pl/Oct-29.m3u"
        );
    }

    #[test]
    fn source_roundtrips_through_builder() {
        let src = PlaylistSource::FileByDate {
            folder: "/pl".into(),
            mask: "{YYYY}".into(),
        };
        let b = PlaylistBuilder::new(src.clone(), "dated");
        assert_eq!(b.source, src);
        assert!(b.is_empty());
    }
}
