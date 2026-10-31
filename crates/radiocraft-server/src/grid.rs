//! Library grid helpers: selection navigation and the client-side row match
//! the grid uses before trusting the server result (migrated from the old
//! egui shell's `library.rs`). Generic over any row type; the selection
//! helpers only care about the row count.

/// Keep `selected` inside `rows`; returns the new selection.
pub fn clamp_selection<T>(selected: Option<usize>, rows: &[T]) -> Option<usize> {
    match selected {
        Some(i) if i < rows.len() => Some(i),
        _ => None,
    }
}

/// Next row below `selected` (wraps at the end).
pub fn next_selection<T>(selected: Option<usize>, rows: &[T]) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    Some(match selected {
        Some(i) if i + 1 < rows.len() => i + 1,
        Some(_) | None => 0,
    })
}

/// Previous row above `selected` (wraps at the top).
pub fn prev_selection<T>(selected: Option<usize>, rows: &[T]) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    Some(match selected {
        Some(0) | None => rows.len() - 1,
        Some(i) => i - 1,
    })
}

/// Does a row's artist/title/album text match the search needle?
pub fn row_matches(artist: &str, title: &str, album: &str, needle: &str) -> bool {
    let needle = needle.trim().to_lowercase();
    needle.is_empty()
        || artist.to_lowercase().contains(&needle)
        || title.to_lowercase().contains(&needle)
        || album.to_lowercase().contains(&needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> [u32; 3] {
        [1, 2, 3]
    }

    #[test]
    fn selection_navigation_wraps() {
        let rows = rows();
        assert_eq!(next_selection(Some(0), &rows), Some(1));
        assert_eq!(next_selection(Some(2), &rows), Some(0));
        assert_eq!(next_selection(None, &rows), Some(0));
        assert_eq!(prev_selection(Some(0), &rows), Some(2));
        assert_eq!(prev_selection(None, &rows), Some(2));
        assert_eq!(next_selection::<u32>(None, &[]), None);
    }

    #[test]
    fn clamp_drops_out_of_range() {
        let rows = [1u32];
        assert_eq!(clamp_selection(Some(0), &rows), Some(0));
        assert_eq!(clamp_selection(Some(5), &rows), None);
        assert_eq!(clamp_selection(None, &rows), None);
    }

    #[test]
    fn row_matches_is_case_insensitive() {
        let (artist, title, album) = ("Artist", "Song", "");
        assert!(row_matches(artist, title, album, ""));
        assert!(row_matches(artist, title, album, "song"));
        assert!(row_matches(artist, title, album, "ARTIST"));
        assert!(!row_matches(artist, title, album, "zzz"));
    }
}
