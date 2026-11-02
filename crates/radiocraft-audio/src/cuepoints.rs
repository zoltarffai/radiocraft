//! Cue-point detection replicating Bass.Net `Utils.DetectCuePoints` and
//! `PluginInterface.Tracks.SetCuePoints` (the analyze-tracks flow).
//!
//! The RadioDJ "remove silence" / analyze-tracks feature runs two block
//! scans per track:
//!
//! 1. `DetectCuePoints(path, 0.1s, dbStart, dbNext)` → cue start / cue next,
//! 2. `DetectCuePoints(path, 0.1s, dbNext, dbEnd)`   → cue end.
//!
//! `DetectCuePoints` itself scans the decoded file in fixed blocks:
//!
//! - pass 1 walks forward and reports the *start* of the first frame whose
//!   channel-peak reaches `level(dbStart)`; the fallback is 0,
//! - pass 2 walks backward from EOF and reports the *end* of the last frame
//!   whose channel-peak reaches `level(dbEnd)`; the fallback is EOF.
//!
//! Semantics are bit-faithful to the original IL (validated 13/13 against a
//! live BASS oracle): dB thresholds clamp to [-90, 0], the block size
//! clamps to [0.1, 30] s, levels and sample peaks are single-precision,
//! block byte counts are `Seconds2Bytes` truncations, and candidate
//! positions are byte offsets converted back to seconds with an f32
//! round-trip. The zero-crossing refinement (`findZeroCrossing != 0`) is
//! never requested by RadioDJ (`zc = 0` at its single call site) and is
//! not implemented here.

use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::error::{AudioError, Result};

/// Thresholds (dB) for the three SetCuePoints levels, mirroring the
/// TracksManager analyzer defaults (`selDbIn` / `selDbNext` / `selDbOut`;
/// UI range is [-100, 0]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CueThresholds {
    /// "Start level threshold" (cueIn cue-out next pair uses S→N).
    pub start_db: f64,
    /// Middle threshold shared by both scans.
    pub next_db: f64,
    /// End level threshold (second scan uses N→E).
    pub end_db: f64,
}

impl Default for CueThresholds {
    fn default() -> Self {
        // Plugin_TracksManager designer defaults.
        CueThresholds {
            start_db: -24.0,
            next_db: -5.0,
            end_db: -30.0,
        }
    }
}

/// The three cue positions produced by `SetCuePoints`, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CueTriple {
    pub cue_start: f64,
    pub cue_next: f64,
    pub cue_end: f64,
}

/// `Utils.DetectCuePoints` clamps its dB arguments into [-90, 0].
fn clamp_db(db: f64) -> f64 {
    db.clamp(-90.0, 0.0)
}

/// `Utils.DBToLevel(dB, 1.0)` widened result narrowed to f32, as stored
/// by the DetectCuePoints locals.
fn db_to_level(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// `Utils.ScanSampleLevel`: absolute peak over `chans` samples starting at
/// sample `idx`; out-of-range lanes contribute 0.
fn scan_sample_level(buf: &[f32], idx: usize, chans: usize) -> f32 {
    let mut peak = 0f32;
    for c in 0..chans {
        let v = buf.get(idx + c).map_or(0.0, |s| s.abs());
        if v > peak {
            peak = v;
        }
    }
    peak
}

/// Block scan of already-decoded interleaved native-rate samples.
///
/// `samples` is interleaved by `chans` at the file's *native* `rate`
/// (the analyzer never resamples). Returns `(cue_in, cue_out)` seconds.
pub fn detect_cue_points(
    samples: &[f32],
    chans: usize,
    rate: usize,
    block_size: f32,
    db_in: f64,
    db_out: f64,
) -> (f64, f64) {
    let chans = chans.max(1);
    let rate = rate.max(1);
    let level_in = db_to_level(clamp_db(db_in));
    let level_out = db_to_level(clamp_db(db_out));
    // BASS block size in bytes: Seconds2Bytes truncation of the f32 seconds.
    let block = block_size.clamp(0.1, 30.0) as f64;
    let frame_bytes = 4 * chans; // f32 lanes per frame
    let bytes_per_sec = frame_bytes * rate;
    let block_bytes = (block * bytes_per_sec as f64) as i64;

    // The decoded samples stand in for the BASS stream: total bytes is the
    // sample count times 4; every block-aligned byte offset maps onto a
    // sample index exactly as `buffer[V_9 / 4]` did in the original.
    let total_bytes = (samples.len() / chans) as i64 * frame_bytes as i64;

    // ---- pass 1: forward, first frame reaching level_in -> cue start ----
    let mut base: i64 = 0;
    let mut cand: i64 = 0;
    let mut found = false;
    while !found && base < total_bytes {
        let avail = total_bytes - base;
        let mut got = block_bytes.min(avail);
        got -= got.rem_euclid(frame_bytes as i64);
        if got <= 0 {
            break;
        }
        let mut v9: i64 = 0; // byte offset of the scanned frame in the block
        while v9 < got {
            let sample_idx = ((base + v9) / 4) as usize;
            let peak = scan_sample_level(samples, sample_idx, chans);
            if peak >= level_in {
                found = true;
                cand = base + v9;
                break;
            }
            v9 += frame_bytes as i64;
        }
        base += got;
    }
    let cue_in = if !found || cand >= total_bytes {
        0.0
    } else {
        bytes_to_seconds(cand, bytes_per_sec)
    };

    // ---- pass 2: backward from EOF, last frame reaching level_out -> cue end ----
    let mut hi = total_bytes;
    let mut cue_bytes: i64 = 0;
    let mut found2 = false;
    while !found2 && hi > 0 {
        // The original seeks to `hi - block` and reads a full block; data
        // always ends exactly at `hi`.
        let got = block_bytes.min(hi);
        if got <= 0 {
            break;
        }
        // Scan frames from the end of the block backward (V_9 = V_10).
        let mut v9 = got;
        while v9 > 0 {
            let sample_idx = ((hi - got + v9) / 4) as usize - chans;
            let peak = scan_sample_level(samples, sample_idx, chans);
            if peak >= level_out {
                found2 = true;
                cue_bytes = hi - got + v9; // frame *end* offset
                break;
            }
            v9 -= frame_bytes as i64;
        }
        if !found2 {
            hi -= got;
        }
    }
    let cue_out = if !found2 || cue_bytes <= 0 || cue_bytes > total_bytes {
        bytes_to_seconds(total_bytes, bytes_per_sec)
    } else {
        bytes_to_seconds(cue_bytes, bytes_per_sec)
    };

    (cue_in, cue_out)
}

/// `BASS_ChannelBytes2Seconds` result stored into a float32 out param,
/// then widened for the double API (the f32 round-trip is observable).
fn bytes_to_seconds(bytes: i64, bytes_per_sec: usize) -> f64 {
    (bytes as f64 / bytes_per_sec as f64) as f32 as f64
}

/// `Tracks.SetCuePoints(path, ref cs, ref cn, ref ce, S, N, E)`:
/// one DCP(S, N) yields start+next, a second DCP(N, E) refines the end.
pub fn set_cue_points(
    samples: &[f32],
    chans: usize,
    rate: usize,
    th: &CueThresholds,
) -> CueTriple {
    let (cue_start, cue_next) = detect_cue_points(samples, chans, rate, 0.1, th.start_db, th.next_db);
    let (_, cue_end) = detect_cue_points(samples, chans, rate, 0.1, th.next_db, th.end_db);
    CueTriple {
        cue_start,
        cue_next,
        cue_end,
    }
}

/// Decode a whole file at its native format (channels + sample rate) into
/// interleaved f32 samples — the analyzer input the BASS float decode fed.
pub fn decode_for_analysis(path: &Path) -> Result<(Vec<f32>, usize, usize)> {
    let file = std::fs::File::open(path).map_err(|e| {
        AudioError::Io(std::io::Error::new(
            e.kind(),
            format!("{}: {e}", path.display()),
        ))
    })?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let mut hint = Hint::new();
    if !ext.is_empty() {
        hint.with_extension(&ext);
    }
    let probe = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| AudioError::Decode(format!("{}: {e}", path.display())))?;

    let mut reader = probe.format;
    let track = reader
        .default_track()
        .ok_or_else(|| AudioError::Unsupported(format!("{}: no audio track", path.display())))?;
    let params = track.codec_params.clone();
    let track_id = track.id;
    if params.codec == CODEC_TYPE_NULL {
        return Err(AudioError::Unsupported(format!(
            "{}: unknown codec",
            path.display()
        )));
    }
    let rate = params
        .sample_rate
        .ok_or_else(|| AudioError::Unsupported(format!("{}: no rate", path.display())))?
        as usize;
    let chans = params
        .channels
        .map_or(2, |c| c.count())
        .max(1);

    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions { verify: false })
        .map_err(|e| AudioError::Unsupported(format!("{}: {e}", path.display())))?;

    let mut samples: Vec<f32> = Vec::new();
    if let Some(n_frames) = params.n_frames {
        samples.reserve(n_frames as usize * chans);
    }
    let mut chans = chans;
    let mut soft_failures = 0usize;
    loop {
        let pkt = match reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::ResetRequired) => {
                return Err(AudioError::Decode(format!(
                    "{}: format change",
                    path.display()
                )))
            }
            Err(_) => break,
        };
        if pkt.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&pkt) {
            Ok(d) => d,
            Err(_) => {
                soft_failures += 1;
                if soft_failures > 16 {
                    break;
                }
                continue;
            }
        };
        let spec = *decoded.spec();
        let frames = decoded.frames() as usize;
        chans = spec.channels.count().max(1);
        let mut sb = SampleBuffer::<f32>::new(frames as u64, spec);
        sb.copy_interleaved_ref(decoded);
        samples.extend_from_slice(sb.samples());
    }
    if samples.is_empty() {
        return Err(AudioError::Decode(format!(
            "{}: decoded nothing",
            path.display()
        )));
    }
    Ok((samples, chans, rate))
}

/// File-level `SetCuePoints` parity: decode at native format, run both
/// block scans.
pub fn set_cue_points_file(path: &Path, th: &CueThresholds) -> Result<CueTriple> {
    let (samples, chans, rate) = decode_for_analysis(path)?;
    Ok(set_cue_points(&samples, chans, rate, th))
}

/// File-level `DetectCuePoints(path, 0.1, dbIn, dbOut, zc=0)` parity.
pub fn detect_cue_points_file(path: &Path, db_in: f64, db_out: f64) -> Result<(f64, f64)> {
    let (samples, chans, rate) = decode_for_analysis(path)?;
    Ok(detect_cue_points(&samples, chans, rate, 0.1, db_in, db_out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/assets")
            .join(name)
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 5e-5
    }

    #[test]
    fn thresholds_match_designer_defaults() {
        let t = CueThresholds::default();
        assert_eq!((t.start_db, t.next_db, t.end_db), (-24.0, -5.0, -30.0));
    }

    #[test]
    fn db_to_level_is_f32_pow() {
        assert_eq!(db_to_level(0.0), 1.0);
        assert_eq!(db_to_level(-90.0), (10f64.powf(-4.5)) as f32);
        assert_eq!(clamp_db(5.0), 0.0);
        assert_eq!(clamp_db(-120.0), -90.0);
    }

    #[test]
    fn scan_level_peaks_over_channels() {
        let buf = [0.1, -0.5, 0.2, 0.0];
        assert_eq!(scan_sample_level(&buf, 0, 2), 0.5);
        assert_eq!(scan_sample_level(&buf, 2, 2), 0.2); // out-of-range lane -> 0
        assert_eq!(scan_sample_level(&buf, 1, 1), 0.5);
    }

    #[test]
    fn detect_bassutils_rows_zc0() {
        // oracle2 BASSUTILS rows (BASS 2.4.12 oracle, t1.wav).
        let t1 = asset("t1.wav");
        for (s, e, c1, c2) in [
            (-20.0, -40.0, 8.0001, 14.9995),
            (-40.0, -60.0, 4.0001, 15.0000),
            (-55.0, -20.0, 4.0000, 11.9999),
        ] {
            let (a, b) = detect_cue_points_file(&t1, s, e).unwrap();
            assert!(approx(a, c1), "in {s}..{e}: {a} vs {c1}");
            assert!(approx(b, c2), "out {s}..{e}: {b} vs {c2}");
        }
    }

    #[test]
    fn set_cue_points_oracle_triples() {
        // oracle3 Tracks.SetCuePoints sweep: (S, N, E, cs, cn, ce).
        let cases = [
            ("t1.wav", -20.0, -40.0, -60.0, 8.0001, 14.9995, 15.0000),
            ("t1.wav", -40.0, -20.0, -60.0, 4.0001, 11.9999, 15.0000),
            ("t1.wav", -20.0, -60.0, -40.0, 8.0001, 15.0000, 14.9995),
            ("t1.wav", -20.0, -40.0, -40.0, 8.0001, 14.9995, 14.9995),
            ("t1.wav", -55.0, -20.0, -40.0, 4.0000, 11.9999, 14.9995),
            ("t1.wav", -60.0, -60.0, -60.0, 2.0005, 15.0000, 15.0000),
            ("t1.wav", -70.0, -70.0, -70.0, 2.0001, 15.0000, 15.0000),
            ("t1.wav", -10.0, -10.0, -10.0, 0.0000, 17.0000, 17.0000),
            ("t1.wav", -95.0, -95.0, -95.0, 2.0000, 15.0000, 15.0000),
            ("t2.wav", -20.0, -60.0, -40.0, 5.0002, 10.0000, 10.0000),
            ("t2.wav", -20.0, -40.0, -40.0, 5.0002, 10.0000, 10.0000),
        ];
        for (file, s, n, e, cs, cn, ce) in cases {
            let (samples, chans, rate) = decode_for_analysis(&asset(file)).unwrap();
            let th = CueThresholds {
                start_db: s,
                next_db: n,
                end_db: e,
            };
            let got = set_cue_points(&samples, chans, rate, &th);
            assert!(approx(got.cue_start, cs), "{file} {s}/{n}/{e} cs {got:?} vs {cs}");
            assert!(approx(got.cue_next, cn), "{file} {s}/{n}/{e} cn {got:?} vs {cn}");
            assert!(approx(got.cue_end, ce), "{file} {s}/{n}/{e} ce {got:?} vs {ce}");
        }
    }

    #[test]
    fn no_forward_hit_falls_back_to_zero() {
        // A file whose samples never reach the threshold: cue in = 0,
        // cue out = end of file.
        let samples = vec![0.001f32; 44100 * 2];
        let (ci, co) = detect_cue_points(&samples, 1, 44100, 0.1, -20.0, -20.0);
        assert_eq!(ci, 0.0);
        assert!(approx(co, 2.0));
    }

    #[test]
    fn stereo_hit_uses_frame_stride() {
        // Silence (1 s) then a loud alternating-sign tone: cue in at the
        // tone start, cue out at the end of the loud region (one sample
        // before the trailing frame of silence).
        let rate = 44100usize;
        let mut samples = Vec::new();
        for _ in 0..rate {
            samples.extend_from_slice(&[0.0, 0.0]);
        }
        for i in 0..rate / 2 {
            let v = if i % 2 == 0 { 0.5 } else { -0.5 };
            samples.extend_from_slice(&[v, v]);
        }
        for _ in 0..rate / 4 {
            samples.extend_from_slice(&[0.0, 0.0]);
        }
        let (ci, co) = detect_cue_points(&samples, 2, rate, 0.1, -20.0, -20.0);
        assert!(approx(ci, 1.0), "cue in {ci}");
        assert!(approx(co, 1.5), "cue out {co}");
    }
}
