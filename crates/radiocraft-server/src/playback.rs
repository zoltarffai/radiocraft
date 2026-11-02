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

use radiocraft_audio::decoder::{FileSource, Source, StreamSource};
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

/// Every cue token RadioDJ's `cue_times` string can carry — the set
/// `SaveTrackData` rewrites after an analyze pass. Seconds unless noted;
/// 0 means "not set" (except `fade_in_ms`, always written raw).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CueTokens {
    pub start: f64,     // sta
    pub intro: f64,     // int
    pub loop_in: f64,   // pin
    pub loop_out: f64,  // pou
    pub hook_in: f64,   // hin
    pub hook_out: f64,  // hou
    pub outro: f64,     // out
    pub next: f64,      // xta
    pub end: f64,       // end
    pub fade_in_ms: i64, // fin
}

/// Pull all ten tokens out of a `cue_times` string (missing keys stay 0).
pub fn cue_tokens_from(cue_times: &str) -> CueTokens {
    let mut t = CueTokens::default();
    for part in cue_times.split('&') {
        let (key, raw) = match part.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        let value = raw.replace(',', ".");
        match key {
            "sta" => t.start = parse_f64(&value),
            "int" => t.intro = parse_f64(&value),
            "pin" => t.loop_in = parse_f64(&value),
            "pou" => t.loop_out = parse_f64(&value),
            "hin" => t.hook_in = parse_f64(&value),
            "hou" => t.hook_out = parse_f64(&value),
            "out" => t.outro = parse_f64(&value),
            "xta" => t.next = parse_f64(&value),
            "end" => t.end = parse_f64(&value),
            "fin" => t.fade_in_ms = parse_f64(&value) as i64,
            _ => {}
        }
    }
    t
}

/// Rebuild the `cue_times` string exactly like `FormTracksManager.SaveTrackData`:
/// fixed token order `&sta &int &pin &pou &hin &hou &out &xta &end &fin &fou`,
/// `0` for any position that is not > 0, raw decimal otherwise. `fin` and
/// `fou` are written unconditionally; `fou` is the analyze-mode value
/// `CueEnd - CueNext` (seconds), which the players later read as a fade-out.
pub fn build_cue_times(t: &CueTokens) -> String {
    let mut out = String::new();
    let pos = |key: &str, v: f64, out: &mut String| {
        out.push('&');
        out.push_str(key);
        out.push('=');
        if v > 0.0 {
            out.push_str(&format_f64(v));
        } else {
            out.push('0');
        }
    };
    pos("sta", t.start, &mut out);
    pos("int", t.intro, &mut out);
    pos("pin", t.loop_in, &mut out);
    pos("pou", t.loop_out, &mut out);
    pos("hin", t.hook_in, &mut out);
    pos("hou", t.hook_out, &mut out);
    pos("out", t.outro, &mut out);
    pos("xta", t.next, &mut out);
    pos("end", t.end, &mut out);
    out.push_str(&format!("&fin={}", t.fade_in_ms));
    out.push_str(&format!("&fou={}", format_f64(t.end - t.next)));
    out
}

/// .NET Framework `Conversions.ToString(double)` = `Double.ToString()` =
/// "G15": 15 significant digits, trailing zeros dropped, plain decimal
/// unless the exponent is < -5 or >= 15 (then `d.dddE±NN`). So the
/// `end - next` subtraction noise prints as `-0.000500000000000611`,
/// not Rust's shortest-round-trip `-0.0005000000000006111`.
fn format_f64(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    if v == 0.0 {
        return "0".into();
    }
    // 15 significant digits, scientific, e.g. "-5.00000000000611e-4".
    let sci = format!("{:.14e}", v);
    let (m, e) = sci.split_once('e').expect("format produces e");
    let e: i32 = e.parse().expect("format produces integer exponent");
    let neg = m.starts_with('-');
    let digits = m.trim_start_matches('-').replace('.', "");
    let sign = if neg { "-" } else { "" };
    // G-style exponent rules: scientific outside [-4,15) exponents.
    if !(-4..15).contains(&e) {
        let frac = digits[1..].trim_end_matches('0').trim_end_matches('.');
        let mant =
            if frac.is_empty() { digits[..1].to_string() } else { format!("{}.{}", &digits[..1], frac) };
        return format!("{sign}{mant}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    let point = e + 1; // decimal point position within `digits`
    let s = if point <= 0 {
        format!("0.{}{}", "0".repeat(-point as usize), digits)
    } else if point >= digits.len() as i32 {
        format!("{}{}", digits, "0".repeat(point as usize - digits.len()))
    } else {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    };
    format!("{sign}{}", s.trim_end_matches('0').trim_end_matches('.'))
}

/// Open the media for `path` and build the `PlayRequest` for it (cue plan +
/// loop region). HTTP(S) paths (RadioDJ internet streams, `song_type = 5`)
/// open through [`StreamSource`]; everything else is a local file.
///
/// Returns the request plus the decoded duration in seconds so callers
/// can fill in now-playing bookkeeping (live streams report 0.0).
///
/// Blocking: opens the file or fetches the stream's HTTP header; call from
/// `spawn_blocking` on async contexts.
pub fn request_for_file(path: &Path, knobs: &CueKnobs) -> Result<(PlayRequest, f64)> {
    request_for_source(&path_string(path), knobs)
}

/// Same as [`request_for_file`] but keyed by the raw library `path` string,
/// which is how streams are stored (the URL itself).
pub fn request_for_source(raw_path: &str, knobs: &CueKnobs) -> Result<(PlayRequest, f64)> {
    if radiocraft_audio::is_stream_path(raw_path) {
        let source = StreamSource::open(raw_path)?;
        let duration = source.duration().map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let mut req = knobs.plan().apply(PlayRequest::new(Box::new(source)));
        if knobs.loop_in > 0.0 && knobs.loop_out > knobs.loop_in {
            req = req.looping(knobs.loop_in, knobs.loop_out);
        }
        return Ok((req, duration));
    }
    let source = FileSource::open(Path::new(raw_path))?;
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

/// Lossy path-to-string for dispatching file vs. stream; non-UTF-8 paths
/// (never produced by the library flows) fall back to the debug form.
fn path_string(path: &Path) -> String {
    path.to_str().map(str::to_string).unwrap_or_else(|| path.display().to_string())
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
        artist,
        title,
        duration: dec,
        original_duration: dec,
        ..NewSong::default()
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
    fn apply_marks_missing_fades_with_the_default_marker() {
        let source = Box::new(radiocraft_audio::decoder::MemorySource::from_mono(
            &[0.0; 8],
            44_100.0,
        ));
        let plan = CuePlan {
            start_at: Some(2.0),
            vdf_target: Some(30.0),
            stop_at: None,
            fade_in_ms: Some(400),
            fade_out_ms: None,
        };
        let req = plan.apply(PlayRequest::new(source));
        assert_eq!(req.fade_in_ms, 400);
        assert_eq!(req.fade_out_ms, -1);
        assert_eq!(req.start_at_secs, Some(2.0));
        assert_eq!(req.vdf_target_secs, Some(30.0));
    }

    #[test]
    fn apply_without_cues_leaves_the_request_untouched() {
        let source = Box::new(radiocraft_audio::decoder::MemorySource::from_mono(
            &[0.0; 8],
            44_100.0,
        ));
        let req = CuePlan::default().apply(PlayRequest::new(source));
        assert_eq!(req.fade_in_ms, 0);
        assert_eq!(req.fade_out_ms, -1);
        assert_eq!(req.start_at_secs, None);
        assert_eq!(req.vdf_target_secs, None);
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

    #[test]
    fn cue_times_tokens_round_trip() {
        let src = "&sta=1.2&int=0&pin=3&pou=8&hin=0&hou=0&out=0&xta=45&end=50&fin=250&fou=5000";
        let t = cue_tokens_from(src);
        assert_eq!(t.start, 1.2);
        assert_eq!(t.loop_in, 3.0);
        assert_eq!(t.next, 45.0);
        assert_eq!(t.end, 50.0);
        assert_eq!(t.fade_in_ms, 250);
        // Rebuild follows SaveTrackData: fou is recomputed as end - next.
        assert_eq!(
            build_cue_times(&t),
            "&sta=1.2&int=0&pin=3&pou=8&hin=0&hou=0&out=0&xta=45&end=50&fin=250&fou=5"
        );
    }

    #[test]
    fn cue_times_build_matches_analyze_layout() {
        // Fresh analysis result on an empty cue_times: zero positions print
        // `0`, VB-style whole seconds print without a fraction.
        let t = CueTokens {
            start: 0.0,
            next: 15.0,
            end: 14.9995,
            ..Default::default()
        };
        assert_eq!(
            build_cue_times(&t),
            "&sta=0&int=0&pin=0&pou=0&hin=0&hou=0&out=0&xta=15&end=14.9995&fin=0&fou=-0.000500000000000611"
        );
    }

    #[test]
    fn format_f64_matches_dotnet_g15() {
        // .NET Framework Double.ToString() = G15: whole numbers lose the
        // fraction, trailing zeros are dropped, 15 significant digits max.
        assert_eq!(format_f64(0.0), "0");
        assert_eq!(format_f64(15.0), "15");
        assert_eq!(format_f64(50.0 - 45.0), "5");
        assert_eq!(format_f64(1.2), "1.2");
        assert_eq!(format_f64(8.0001), "8.0001");
        // Subtraction noise rounds at the 15th significant digit.
        assert_eq!(format_f64(14.9995 - 15.0), "-0.000500000000000611");
        // Plain decimal down to 1e-4, scientific at 1e-5 and 1e15.
        assert_eq!(format_f64(0.0000226757369614512), "2.26757369614512E-05");
        assert_eq!(format_f64(0.0005), "0.0005");
        assert_eq!(format_f64(1e15), "1E+15");
        assert_eq!(format_f64(999999999999999.0), "999999999999999");
    }
}
