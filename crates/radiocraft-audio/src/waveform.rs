//! Waveform peak data + on-disk sidecar cache.
//!
//! [`WaveformData`] stores peak pairs (min/max) for fixed-size windows of
//! a track, the format RadioDJ uses for its waveform displays. Files are
//! cached next to the audio file (`.wfx`-style sidecar) so the GUI can
//! open waveforms without re-decoding.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::decoder::Source;
use crate::error::{AudioError, Result};

/// Number of peak windows the GUI asks for by default.
pub const DEFAULT_WINDOWS: usize = 800;

/// Peak data for one track: `windows` entries, each a (min, max) pair over
/// `samples_per_window` samples of the interleaved stereo stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaveformData {
    pub samples_per_window: usize,
    /// Interleaved (min, max) pairs, one per window.
    pub peaks: Vec<(f32, f32)>,
}

impl WaveformData {
    /// Build peak data by scanning `source` to completion.
    ///
    /// `windows` clamps the resolution of the display; a 3-hour file with
    /// 800 windows still produces a usable overview.
    pub fn scan_from(source: &mut dyn Source, windows: usize) -> Result<Self> {
        let windows = windows.clamp(16, 100_000);
        let total_frames = match source.duration() {
            Some(d) if d > Duration::ZERO => (d.as_secs_f64() * crate::SAMPLE_RATE as f64) as usize,
            _ => {
                // unknown length: read to EOF, then bucket the collected peaks
                return Self::scan_unknown(source, windows);
            }
        };
        let spw = (total_frames / windows).max(1);
        let actual_windows = total_frames.div_ceil(spw).max(1);

        let mut peaks = Vec::with_capacity(actual_windows);
        let mut buf = vec![0.0f32; 4096 * 2];
        let (mut wmin, mut wmax) = (0.0f32, 0.0f32);
        let mut in_window = 0usize;
        loop {
            let got = source.read(&mut buf);
            if got == 0 {
                break;
            }
            for frame in buf[..got * 2].chunks(2) {
                let lo = frame[0].min(frame[1]);
                let hi = frame[0].max(frame[1]);
                wmin = wmin.min(lo);
                wmax = wmax.max(hi);
                in_window += 1;
                if in_window >= spw {
                    peaks.push((wmin, wmax));
                    wmin = 0.0;
                    wmax = 0.0;
                    in_window = 0;
                }
            }
        }
        if in_window > 0 {
            peaks.push((wmin, wmax));
        }
        Ok(WaveformData { samples_per_window: spw, peaks })
    }

    fn scan_unknown(source: &mut dyn Source, windows: usize) -> Result<Self> {
        // Two-level rolling peak reduction so memory stays bounded even
        // when the stream is longer than the requested resolution.
        let mut peaks: Vec<(f32, f32)> = Vec::with_capacity(windows * 4);
        let mut buf = vec![0.0f32; 4096 * 2];
        loop {
            let got = source.read(&mut buf);
            if got == 0 {
                break;
            }
            for frame in buf[..got * 2].chunks(2) {
                peaks.push((frame[0].min(frame[1]), frame[0].max(frame[1])));
            }
            if peaks.len() >= windows * 4 {
                peaks = reduce(&peaks, windows);
            }
        }
        let spw = 1;
        let peaks = reduce(&peaks, windows);
        Ok(WaveformData { samples_per_window: spw, peaks })
    }

    /// Render `width` display columns as (min, max) pairs scaled for a GUI
    /// waveform of pixel `height` (values in [-1.0, 1.0]).
    pub fn render(&self, width: usize) -> Vec<(f32, f32)> {
        if self.peaks.is_empty() || width == 0 {
            return vec![(0.0, 0.0); width];
        }
        let mut out = Vec::with_capacity(width);
        for i in 0..width {
            let a = (i * self.peaks.len()) / width;
            let b = ((i + 1) * self.peaks.len()) / width;
            let (a, b) = (a.min(self.peaks.len() - 1), b.max(a + 1).min(self.peaks.len()));
            let lo = self.peaks[a..b].iter().map(|p| p.0).fold(0.0f32, f32::min);
            let hi = self.peaks[a..b].iter().map(|p| p.1).fold(0.0f32, f32::max);
            out.push((lo, hi));
        }
        out
    }

    // ---- sidecar cache (.wfx-style little-endian binary) ----

    /// Sidecar path for an audio file: `audio.wav` -> `audio.wav.wfx`.
    pub fn sidecar_path(audio: &Path) -> PathBuf {
        let mut p = audio.as_os_str().to_os_string();
        p.push(".wfx");
        PathBuf::from(p)
    }

    /// Persist the sidecar next to `audio`.
    pub fn save_for(&self, audio: &Path) -> Result<()> {
        let path = Self::sidecar_path(audio);
        let mut bytes = Vec::with_capacity(8 + self.peaks.len() * 8);
        bytes.extend_from_slice(&(self.samples_per_window as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.peaks.len() as u32).to_le_bytes());
        for (lo, hi) in &self.peaks {
            bytes.extend_from_slice(&lo.to_le_bytes());
            bytes.extend_from_slice(&hi.to_le_bytes());
        }
        fs::write(&path, bytes).map_err(|e| io_err(&path, e))
    }

    /// Load a sidecar for `audio` if it exists and is fresher than the
    /// audio file itself.
    pub fn load_for(audio: &Path) -> Result<Option<Self>> {
        let path = Self::sidecar_path(audio);
        if !path.exists() {
            return Ok(None);
        }
        if let (Ok(audio_t), Ok(side_t)) = (mtime(audio), mtime(&path)) {
            if side_t < audio_t {
                return Ok(None); // stale
            }
        }
        let data = fs::read(&path).map_err(|e| io_err(&path, e))?;
        Self::from_bytes(&data)
            .map(Some)
            .ok_or_else(|| AudioError::Decode("bad waveform sidecar".into()))
    }

    /// Parse sidecar bytes (`None` on malformed input).
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        let spw = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let n = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        if data.len() < 8 + n * 8 || spw == 0 {
            return None;
        }
        let mut peaks = Vec::with_capacity(n);
        for i in 0..n {
            let o = 8 + i * 8;
            let lo = f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
            let hi = f32::from_le_bytes([data[o + 4], data[o + 5], data[o + 6], data[o + 7]]);
            peaks.push((lo, hi));
        }
        Some(WaveformData { samples_per_window: spw, peaks })
    }

    /// Serialise to the same bytes `save_for` writes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8 + self.peaks.len() * 8);
        bytes.extend_from_slice(&(self.samples_per_window as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.peaks.len() as u32).to_le_bytes());
        for (lo, hi) in &self.peaks {
            bytes.extend_from_slice(&lo.to_le_bytes());
            bytes.extend_from_slice(&hi.to_le_bytes());
        }
        bytes
    }
}

fn reduce(peaks: &[(f32, f32)], target: usize) -> Vec<(f32, f32)> {
    if peaks.len() <= target || target == 0 {
        return peaks.to_vec();
    }
    let step = peaks.len() / target + 1;
    let mut out = Vec::with_capacity(target);
    for chunk in peaks.chunks(step) {
        let lo = chunk.iter().map(|p| p.0).fold(0.0f32, f32::min);
        let hi = chunk.iter().map(|p| p.1).fold(0.0f32, f32::max);
        out.push((lo, hi));
        if out.len() >= target {
            break;
        }
    }
    out
}

fn io_err(path: &Path, e: std::io::Error) -> AudioError {
    AudioError::Io(std::io::Error::new(
        e.kind(),
        format!("{}: {e}", path.display()),
    ))
}

fn mtime(path: &Path) -> std::io::Result<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified())
}

// ---------------------------------------------------------------------------
// Memory cache with TTL (Radiotify-style display cache)
// ---------------------------------------------------------------------------

/// In-memory waveform cache keyed by file path, with a TTL so modified
/// files are refreshed and long-idle entries are dropped.
#[derive(Debug)]
pub struct WaveformCache {
    entries: Vec<(PathBuf, Instant, WaveformData)>,
    ttl: Duration,
    max_entries: usize,
}

impl WaveformCache {
    pub fn new(ttl: Duration, max_entries: usize) -> Self {
        Self { entries: Vec::new(), ttl, max_entries }
    }

    pub fn get(&mut self, path: &Path) -> Option<&WaveformData> {
        let now = Instant::now();
        let hit = self
            .entries
            .iter()
            .position(|(p, t, _)| p == path && now.duration_since(*t) < self.ttl)?;
        Some(&self.entries[hit].2)
    }

    /// Read-only cache probe usable while holding the engine mutex.
    pub fn peek(&self, path: &Path) -> Option<&WaveformData> {
        let now = Instant::now();
        self.entries
            .iter()
            .find(|(p, t, _)| p == path && now.duration_since(*t) < self.ttl)
            .map(|(_, _, d)| d)
    }

    pub fn put(&mut self, path: &Path, data: WaveformData) {
        self.entries.retain(|(p, t, _)| {
            p != path && Instant::now().duration_since(*t) < self.ttl * 4
        });
        self.entries.push((path.to_path_buf(), Instant::now(), data));
        while self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
    }

    pub fn invalidate(&mut self, path: &Path) {
        self.entries.retain(|(p, _, _)| p != path);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for WaveformCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(30 * 60), 256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::{sine_source, MemorySource};

    #[test]
    fn scans_peaks_from_source() {
        let mut src = sine_source(440.0, 1.0, 0.5, 44_100.0);
        let wf = WaveformData::scan_from(&mut src, 100).unwrap();
        assert!(!wf.peaks.is_empty());
        let max = wf.peaks.iter().map(|p| p.1).fold(0.0f32, f32::max);
        assert!(max > 0.4, "max {max}");
    }

    #[test]
    fn scans_unknown_length_source() {
        // MemorySource with no reported duration exercises the unknown path.
        let data = vec![0.5f32; 44_100 * 2];
        let mut src = MemorySource::new(data, 44_100.0);
        let wf = WaveformData::scan_from(&mut src, 50).unwrap();
        assert!(!wf.peaks.is_empty());
    }

    #[test]
    fn sidecar_round_trip() {
        let wf = WaveformData {
            samples_per_window: 128,
            peaks: vec![(-0.5, 0.5), (-0.25, 0.75)],
        };
        let bytes = wf.to_bytes();
        let back = WaveformData::from_bytes(&bytes).unwrap();
        assert_eq!(back.samples_per_window, 128);
        assert_eq!(back.peaks, wf.peaks);
    }

    #[test]
    fn sidecar_file_round_trip() {
        let dir = std::env::temp_dir().join("rc_waveform_test");
        fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("track.mp3");
        fs::write(&audio, b"not really an mp3").unwrap();

        let wf = WaveformData { samples_per_window: 64, peaks: vec![(0.0, 1.0)] };
        wf.save_for(&audio).unwrap();
        let loaded = WaveformData::load_for(&audio).unwrap().unwrap();
        assert_eq!(loaded.peaks, wf.peaks);
        assert!(WaveformData::sidecar_path(&audio).exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_resamples_to_width() {
        let wf = WaveformData {
            samples_per_window: 1,
            peaks: vec![(-0.1, 0.1), (-0.2, 0.2), (-0.3, 0.3), (-0.4, 0.4)],
        };
        let out = wf.render(2);
        assert_eq!(out.len(), 2);
        assert!((out[1].1 - 0.4).abs() < 1e-6);
    }

    #[test]
    fn cache_put_get_invalidate() {
        let mut c = WaveformCache::default();
        let wf = WaveformData { samples_per_window: 1, peaks: vec![(0.0, 0.5)] };
        c.put(Path::new("a.wav"), wf.clone());
        assert!(c.get(Path::new("a.wav")).is_some());
        c.invalidate(Path::new("a.wav"));
        assert!(c.get(Path::new("a.wav")).is_none());
    }
}
