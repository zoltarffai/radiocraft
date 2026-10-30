//! Library playback path: RadioDJ `cue_times` strings, cue knobs, and
//! turning a `Song` row into a `PlayRequest`.
//!
//! The editable cue knobs (`CueKnobs`) mirror the players window: start,
//! cue-next (VDF), and end, plus fades. `CuePlan` is the same-value,
//! `None`-when-not-set form the engine consumes (migrated from the old
//! egui shell's `players_zones.rs`).

use std::path::Path;

use anyhow::Result;
use rust_decimal::Decimal;
use serde::Deserialize;

use radiocraft_audio::decoder::{FileSource, Source};
use radiocraft_audio::player::PlayRequest;
use radiocraft_db::entities::{NewSong, Song};

/// Raw cue knobs for one track, seconds; 0 = "not set" (RadioDJ convention).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CueKnobs {
    pub start: f64,
    pub next: f64,
    pub end: f64,
    #[serde(default)]
    pub loop_in: f64,
    #[serde(default)]
    pub loop_out: f64,
    #[serde(default)]
    pub fade_in_ms: i32,
    #[serde(default)]
    pub fade_out_ms: i32,
}

impl CueKnobs {
    /// The engine-side plan derived from these knobs.
    pub fn plan(&self) -> CuePlan {
        CuePlan {
            start_at: if self.start > 0.0 { Some(self.start) } else { None },
            vdf_target: if self.next > 0.0 { Some(self.next) } else { None },
            stop_at: if self.end > 0.0 { Some(self.end) } else { None },
            fade_in_ms: if self.fade_in_ms > 0 { Some(self.fade_in_ms) } else { None },
            fade_out_ms: if self.fade_out_ms > 0 { Some(self.fade_out_ms) } else { None },
        }
    }

    /// Knobs with explicit values (0 fields are left alone), the wire form
    /// the players UI edits.
    pub fn merge(&self, patch: &CueKnobs) -> CueKnobs {
        let mut out = *self;
        if patch.start != 0.0 {
            out.start = patch.start;
        }
        if patch.next != 0.0 {
            out.next = patch.next;
        }
        if patch.end != 0.0 {
            out.end = patch.end;
        }
        if patch.loop_in != 0.0 {
            out.loop_in = patch.loop_in;
        }
        if patch.loop_out != 0.0 {
            out.loop_out = patch.loop_out;
        }
        if patch.fade_in_ms != 0 {
            out.fade_in_ms = patch.fade_in_ms;
        }
        if patch.fade_out_ms != 0 {
            out.fade_out_ms = patch.fade_out_ms;
        }
        out
    }
}

/// The engine-side cue description: `None` = "not set, let the engine decide".
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CuePlan {
    pub start_at: Option<f64>,
    pub vdf_target: Option<f64>,
    pub stop_at: Option<f64>,
    pub fade_in_ms: Option<i32>,
    pub fade_out_ms: Option<i32>,
}

impl CuePlan {
    /// Apply the knobs to a fresh `PlayRequest`.
    ///
    /// Fades go through `with_fades`; a missing side passes `-1`, the
    /// engine's "use my default" marker.
    pub fn apply(&self, req: PlayRequest) -> PlayRequest {
        let mut req = req;
        match (self.fade_in_ms, self.fade_out_ms) {
            (Some(fin), Some(fout)) => req = req.with_fades(fin, fout),
            (Some(fin), None) => req = req.with_fades(fin, -1),
            (None, Some(fout)) => req = req.with_fades(-1, fout),
            (None, None) => {}
        }
        if let Some(secs) = self.start_at {
            req = req.starting_at(secs);
        }
        if let Some(secs) = self.vdf_target {
            req = req.vdf_target(secs);
        }
        req
    }

    /// The stop position relative to the sounding segment (seconds);
    /// RadioDJ schedules `stop_at` from where playback effectively starts.
    pub fn stop_at_secs(&self) -> Option<f64> {
        let start = self.start_at.unwrap_or(0.0);
        self.stop_at.map(|end| (end - start).max(0.0))
    }
}

/// Parse RadioDJ's `cue_times` string, e.g.
/// `&sta=1.2&int=0&pin=0&pou=0&hin=0&hou=0&out=0&xta=45&end=50&fin=250&fou=5000`.
///
/// Keys we care about: `sta` (start), `xta` (cue next / VDF target),
/// `end` (cut), `pin`/`pou` (loop region), `fin`/`fou` (fade ms).
/// Unknown or malformed keys are ignored; an empty string yields zeros.
pub fn parse_cue_times(cue_times: &str) -> CueKnobs {
    let mut knobs = CueKnobs::default();
    for part in cue_times.split('&') {
        let (key, raw) = match part.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        // RadioDJ stored seconds with a decimal comma; tolerate both.
        let value = raw.replace(',', ".");
        match key {
            "sta" => knobs.start = parse_f64(&value),
            "xta" => knobs.next = parse_f64(&value),
            "end" => knobs.end = parse_f64(&value),
            "pin" => knobs.loop_in = parse_f64(&value),
            "pou" => knobs.loop_out = parse_f64(&value),
            "fin" => knobs.fade_in_ms = parse_i32(&value),
            "fou" => knobs.fade_out_ms = parse_i32(&value),
            _ => {}
        }
    }
    knobs
}

fn parse_f64(raw: &str) -> f64 {
    raw.trim().parse().unwrap_or(0.0)
}

fn parse_i32(raw: &str) -> i32 {
    raw.trim().parse().unwrap_or(0)
}

/// Cue knobs as stored on a library row.
pub fn knobs_from_song(song: &Song) -> CueKnobs {
    parse_cue_times(&song.cue_times)
}

/// Open a file and build the `PlayRequest` for it (cue plan + loop region).
///
/// Returns the request plus the decoded duration in seconds so callers
/// can fill in now-playing bookkeeping.
pub fn request_for_file(path: &Path, knobs: &CueKnobs) -> Result<(PlayRequest, f64)> {
    let source = FileSource::open(path)?;
    let duration = source
        .duration()
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let mut req = knobs.plan().apply(PlayRequest::new(Box::new(source)));
    if knobs.loop_in > 0.0 && knobs.loop_out > knobs.loop_in {
        req = req.looping(knobs.loop_in, knobs.loop_out);
    }
    Ok((req, duration))
}

/// File-name split used for newly added files: `Artist - Title`.
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
    let source = FileSource::open(path)
        .map_err(|e| anyhow::anyhow!("opening {}: {e}", path.display()))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_times_full_string() {
        let knobs = parse_cue_times("&sta=1.2&int=0&pin=3&pou=9&hin=0&hou=0&out=0&xta=45&end=50&fin=250&fou=5000");
        assert_eq!(
            knobs,
            CueKnobs {
                start: 1.2,
                next: 45.0,
                end: 50.0,
                loop_in: 3.0,
                loop_out: 9.0,
                fade_in_ms: 250,
                fade_out_ms: 5000,
            }
        );
    }

    #[test]
    fn cue_times_empty_and_garbage() {
        assert_eq!(parse_cue_times(""), CueKnobs::default());
        assert_eq!(parse_cue_times("not a cue").start, 0.0);
        let knobs = parse_cue_times("&sta=abc&end=7,5");
        assert_eq!(knobs.start, 0.0);
        assert_eq!(knobs.end, 7.5); // decimal comma tolerated
    }

    #[test]
    fn cue_times_unknown_keys_ignored() {
        let knobs = parse_cue_times("&hin=100&hou=200&sta=2");
        assert_eq!(knobs.start, 2.0);
        assert_eq!(knobs.next, 0.0);
    }

    #[test]
    fn plan_zero_knobs_are_none() {
        let plan = CueKnobs::default().plan();
        assert_eq!(
            plan,
            CuePlan {
                start_at: None,
                vdf_target: None,
                stop_at: None,
                fade_in_ms: None,
                fade_out_ms: None,
            }
        );
    }

    #[test]
    fn stop_at_relative_to_start() {
        let knobs = CueKnobs {
            start: 5.0,
            end: 40.0,
            ..Default::default()
        };
        assert_eq!(knobs.plan().stop_at_secs(), Some(35.0));
        // stop before start clamps to 0 (engine stops immediately).
        let knobs = CueKnobs {
            start: 40.0,
            end: 10.0,
            ..Default::default()
        };
        assert_eq!(knobs.plan().stop_at_secs(), Some(0.0));
    }

    #[test]
    fn patch_merge_only_nonzero() {
        let stored = CueKnobs {
            start: 3.0,
            end: 60.0,
            fade_out_ms: 5000,
            ..Default::default()
        };
        let patch = CueKnobs {
            end: 90.0,
            fade_in_ms: 250,
            ..Default::default()
        };
        let merged = stored.merge(&patch);
        assert_eq!(merged.start, 3.0);
        assert_eq!(merged.end, 90.0);
        assert_eq!(merged.fade_in_ms, 250);
        assert_eq!(merged.fade_out_ms, 5000);
    }

    #[test]
    fn stem_split_matches_radio_convention() {
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
    fn new_song_from_path_reports_open_error() {
        let err = new_song_from_path(Path::new("/nonexistent/track.mp3"))
            .expect_err("missing file must fail");
        assert!(err.to_string().contains("opening"), "{err}");
    }
}
