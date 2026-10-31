//! Playlist item helpers: order rewrites and row->insert conversions for
//! `playlists_list` (migrated from the old egui shell's `playlists.rs`).
//! RadioDJ keeps `ord` as the display/order column; editing the order here
//! (move up/down, remove) rewrites it on save, so the grid stays exactly
//! what the playout reads next.

use radiocraft_db::entities::{NewPlaylistItem, PlaylistItem};
use rust_decimal::Decimal;

/// One grid row for the item grid (DB row + resolved labels).
#[derive(Debug, Clone)]
pub struct PlaylistRow {
    pub item: PlaylistItem,
    pub artist: String,
    pub title: String,
    pub duration_secs: f64,
}

/// Rewrite `ord` of these inserts to 1..=len, in the given order.
pub fn reindex_ords(mut items: Vec<NewPlaylistItem>) -> Vec<NewPlaylistItem> {
    for (i, item) in items.iter_mut().enumerate() {
        item.ord = (i + 1) as u32;
    }
    items.shrink_to_fit();
    items
}

/// The row at `idx` moves to position `target` (remove + reinsert, so rows
/// between shift one way). Returns the new index of the moved row, or
/// `None` if it cannot move.
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
        assert_eq!(
            rows.iter().map(|r| r.item.sID).collect::<Vec<_>>(),
            [2, 3, 1]
        );
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
    }
}
