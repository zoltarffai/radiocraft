//! Output level metering (VU bridges and peak/RMS meters) used by the
//! playout engine to drive the on-air meters and the auto-fill/assist
//! logic.

use serde::{Deserialize, Serialize};

use crate::util::lin_to_db_f32;

/// Sample-peak meter with peak-hold and decaying peak display, like the
/// RadioDJ "VU meter" columns (peak sample + hold).
#[derive(Debug, Clone)]
pub struct PeakMeter {
    pub channels: usize,
    per_ch: Vec<f32>,
    holds: Vec<f32>,
    hold_until: f64,
    rms_sum: f64,
    rms_frames: u64,
    pub rms: Vec<f32>,
    /// Peak-hold duration (ms) before the hold decays.
    pub hold_ms: f64,
    /// dB per second the held peak decays (RadioDJ draws a soft
    /// "mMs" style decaying max bar).
    pub decay_db_per_sec: f64,
    rate: f64,
    frames: u64,
}

impl PeakMeter {
    pub fn new(channels: usize, rate: f64) -> Self {
        Self::with_hold(channels, 2000.0, 6.0, rate)
    }

    pub fn with_hold(channels: usize, hold_ms: f64, decay_db_per_sec: f64, rate: f64) -> Self {
        Self {
            channels,
            per_ch: vec![0.0; channels],
            holds: vec![0.0; channels],
            hold_until: 0.0,
            rms_sum: 0.0,
            rms_frames: 0,
            rms: vec![0.0; channels],
            hold_ms,
            decay_db_per_sec,
            rate: rate.max(1.0),
            frames: 0,
        }
    }

    /// Observe one interleaved frame pair (stereo).
    #[inline]
    pub fn observe_frame(&mut self, left: f32, right: f32) {
        self.observe(&[left, right]);
    }

    pub fn observe(&mut self, frame: &[f32]) {
        let now_sec = self.frames as f64 / self.rate;
        self.frames += 1;
        for (i, s) in frame.iter().take(self.channels).enumerate() {
            let a = s.abs();
            if a > self.per_ch[i] {
                self.per_ch[i] = a;
            }
            if a > self.holds[i] {
                self.holds[i] = a;
                self.hold_until = now_sec + self.hold_ms / 1000.0;
            } else if now_sec > self.hold_until {
                let db = lin_to_db_f32(self.holds[i].max(1e-7))
                    - self.decay_db_per_sec * (now_sec - self.hold_until) as f32;
                self.holds[i] = 10f32.powf(db / 20.0).max(0.0);
            }
            self.rms_sum += (a * a) as f64;
        }
        self.rms_frames += 1;
        // refresh RMS twice per second
        if self.rms_frames >= (self.rate as u64 / 2).max(1) {
            let n = self.rms_frames.max(1) as f64 / self.channels as f64;
            for c in self.rms.iter_mut() {
                *c = (self.rms_sum / n).sqrt() as f32;
            }
            self.rms_sum = 0.0;
            self.rms_frames = 0;
        }
    }

    /// Feed an interleaved buffer of frames.
    pub fn observe_buffer(&mut self, buf: &[f32]) {
        for frame in buf.chunks_exact(self.channels.max(2).min(2).max(1)) {
            self.observe(frame);
        }
    }

    pub fn peak(&self, ch: usize) -> f32 {
        self.per_ch.get(ch).copied().unwrap_or(0.0)
    }
    pub fn peak_hold(&self, ch: usize) -> f32 {
        self.holds.get(ch).copied().unwrap_or(0.0)
    }
    pub fn rms(&self, ch: usize) -> f32 {
        self.rms.get(ch).copied().unwrap_or(0.0)
    }
    /// Peak in dBFS.
    pub fn peak_db(&self, ch: usize) -> f32 {
        lin_to_db_f32(self.peak(ch))
    }
    pub fn peak_hold_db(&self, ch: usize) -> f32 {
        lin_to_db_f32(self.peak_hold(ch))
    }

    pub fn reset(&mut self) {
        for v in self.per_ch.iter_mut() {
            *v = 0.0;
        }
        for v in self.holds.iter_mut() {
            *v = 0.0;
        }
        self.rms_sum = 0.0;
        self.rms_frames = 0;
        for v in self.rms.iter_mut() {
            *v = 0.0;
        }
    }
}

/// Classic VU meter ballistics: ~300 ms integration (analyst movement).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VuConfig {
    /// Integration time constant (ms); VU standard is 300 ms.
    pub integration_ms: f64,
    /// Electrical full-scale reference: 0 VU at this linear level.
    pub reference: f64,
}

impl Default for VuConfig {
    fn default() -> Self {
        Self {
            integration_ms: 300.0,
            reference: 0.7, // 0 VU ~ -3 dBFS in broadcast practice
        }
    }
}

/// Single-channel moving-average VU meter.
#[derive(Debug)]
pub struct VuMeter {
    cfg: VuConfig,
    coef: f64,
    env: f64,
}

impl VuMeter {
    pub fn new(cfg: VuConfig, rate: f64) -> Self {
        let rate = rate.max(1.0);
        Self {
            cfg,
            coef: 1.0 - (-1.0 / (cfg.integration_ms.max(1.0) * 0.001 * rate)).exp(),
            env: 0.0,
        }
    }

    /// Feed a sample; returns the current VU deflection (0..=1.5).
    #[inline]
    pub fn observe(&mut self, sample: f32) -> f64 {
        let a = sample.abs() as f64;
        self.env += self.coef * (a - self.env);
        self.env / self.cfg.reference.max(1e-6)
    }

    /// Current reading in VU dB (0 VU = `reference`).
    #[inline]
    pub fn db(&self) -> f64 {
        let v = self.env / self.cfg.reference.max(1e-6);
        if v <= 1e-7 {
            f64::NEG_INFINITY
        } else {
            20.0 * v.log10()
        }
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
    }
}

/// Rolling signal-presence detector used by voice-track assist logic.
#[derive(Debug)]
pub struct LevelTracker {
    meter: VuMeter,
    rate: f64,
}

impl LevelTracker {
    pub fn new(rate: f64) -> Self {
        Self {
            meter: VuMeter::new(VuConfig::default(), rate),
            rate,
        }
    }

    pub fn observe_frame(&mut self, left: f32, right: f32) {
        let m = ((left.abs() + right.abs()) * 0.5) as f64;
        self.meter.observe(m as f32);
    }

    /// Current level in dBFS (same reference as the samples themselves).
    pub fn level_db(&self) -> f64 {
        let v = self.meter.env;
        if v <= 1e-7 {
            f64::NEG_INFINITY
        } else {
            20.0 * v.log10()
        }
    }

    #[allow(dead_code)]
    fn rate(&self) -> f64 {
        self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_meter_tracks_peaks() {
        let mut m = PeakMeter::new(2, 44100.0);
        for _ in 0..1000 {
            m.observe_frame(0.5, -0.25);
        }
        assert!((m.peak(0) - 0.5).abs() < 1e-6);
        assert!((m.peak(1) - 0.25).abs() < 1e-6);
        assert!((m.rms(0) - 0.5).abs() < 0.02);
    }

    #[test]
    fn vu_settles_to_steady_level() {
        let mut v = VuMeter::new(VuConfig::default(), 44100.0);
        for _ in 0..44100 {
            v.observe(0.5);
        }
        // after 1 s the ballistics converge to the steady value
        assert!((v.env - 0.5).abs() < 0.02, "env {}", v.env);
    }

    #[test]
    fn tracker_db_is_neg_inf_on_silence() {
        let mut t = LevelTracker::new(44100.0);
        for _ in 0..44100 {
            t.observe_frame(0.0, 0.0);
        }
        assert_eq!(t.level_db(), f64::NEG_INFINITY);
    }

    #[test]
    fn tracker_db_around_expected() {
        let mut t = LevelTracker::new(44100.0);
        for _ in 0..44100 {
            t.observe_frame(0.5, 0.5);
        }
        let db = t.level_db();
        assert!((db - (-6.02)).abs() < 0.5, "db {}", db);
    }
}
