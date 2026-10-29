//! Effects chain, mirroring the RadioDJ "Sound processing" chain
//! (PAMS AudioProcessor + Plugin_SoundProcessing settings).
//!
//! The chain order is: soft clip / saturation, tilt EQs + 10-band
//! parametric EQ, mid/side width, compressor (threshold/ratio/attack/
//! release/makeup with soft knee), limiter (ceiling), and finally the
//! `vol_Out` output trimp. Settings key names follow the RadioDJ
//! `Plugin_SoundProcessing` table: `ON`, `cThreshold`, `cRatio`,
//! `cAttack`, `cRelease`, `cGain`, `eqON`, `eqValues`, `vol_Out`,
//! `ThresholdStart`, `ThresholdNext`, `ThresholdEnd`.

use serde::{Deserialize, Serialize};

use crate::util::{db_to_lin, lin_to_db};

/// Default compressor lookahead (samples). Processing samples with a
/// lookahead lets the envelope do gain changes without clipping the
/// attack transient.
pub const LOOKAHEAD: usize = 64;

// ---------------------------------------------------------------------------
// Soft clipping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SoftClipConfig {
    /// Input gain in dB driving the tanh curve (0 = off).
    pub saturation_db: f64,
}

impl Default for SoftClipConfig {
    fn default() -> Self {
        Self { saturation_db: 0.0 }
    }
}

#[derive(Debug, Default)]
pub struct SoftClip {
    drive: f64,
}

impl SoftClip {
    pub fn new(cfg: SoftClipConfig) -> Self {
        Self {
            drive: db_to_lin(cfg.saturation_db),
        }
    }

    #[inline]
    pub fn process(&self, x: f32) -> f32 {
        if self.drive <= 1.0001 {
            return x;
        }
        (self.drive * x as f64).tanh() as f32 / (self.drive * 0.95).tanh() as f32
    }
}

// ---------------------------------------------------------------------------
// Tilt EQ (high/low shelf, AudioThing style)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HighShelfConfig {
    /// Shelf frequency in Hz.
    pub frequency: f64,
    /// Gain in dB applied to the high band (positive = tilt up).
    pub tilt_db: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LowShelfConfig {
    pub frequency: f64,
    pub tilt_db: f64,
}

/// One-pole first-order lowpass used by the shelving EQs.
#[derive(Debug)]
struct OnePole {
    coef: f64,
    state: f64,
}

impl OnePole {
    fn new(freq_hz: f64, rate: f64) -> Self {
        let coef = (-std::f64::consts::FRAC_2_PI * freq_hz / rate).exp();
        Self { coef, state: 0.0 }
    }
    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        self.state = self.coef * (x - self.state) + self.state;
        // AudioThing returns the *high* component (x - lowpass).
        x - self.state
    }
    fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// High shelf: adds `tilt_db` energy above `frequency`.
#[derive(Debug)]
pub struct HighShelf {
    band: OnePole,
    gain: f64,
}

impl HighShelf {
    pub fn new(cfg: HighShelfConfig, rate: f64) -> Self {
        Self {
            band: OnePole::new(cfg.frequency.max(20.0), rate),
            gain: db_to_lin(cfg.tilt_db) - 1.0,
        }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if self.gain.abs() < 1e-9 {
            self.band.reset();
            return x;
        }
        let hi = self.band.process(x as f64);
        (x as f64 + self.gain * hi) as f32
    }
}

/// Low shelf: adds `tilt_db` energy below `frequency`.
#[derive(Debug)]
pub struct LowShelf {
    hp: OnePole,
    lp: OnePole,
    gain: f64,
}

impl LowShelf {
    pub fn new(cfg: LowShelfConfig, rate: f64) -> Self {
        Self {
            hp: OnePole::new(cfg.frequency.max(20.0), rate),
            lp: OnePole::new(cfg.frequency.max(20.0), rate),
            gain: db_to_lin(cfg.tilt_db) - 1.0,
        }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if self.gain.abs() < 1e-9 {
            self.hp.reset();
            self.lp.reset();
            return x;
        }
        let hi = self.hp.process(x as f64);
        // low component = x - hi, but `hi` here is the high part of the
        // one-pole; the low part is the internal state.
        let low = x as f64 - hi;
        (x as f64 + self.gain * low) as f32
    }
}

// ---------------------------------------------------------------------------
// Parametric EQ (10 bands, RBJ peaching filters)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PeakEqBand {
    pub frequency: f64,
    pub gain_db: f64,
    pub q: f64,
}

impl Default for PeakEqBand {
    fn default() -> Self {
        Self {
            frequency: 1000.0,
            gain_db: 0.0,
            q: 1.0,
        }
    }
}

#[derive(Debug)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn peaching(freq: f64, gain_db: f64, q: f64, rate: f64) -> Self {
        // RBJ audio EQ cookbook peaking shelf: A = 10^(dB/40)
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * freq / rate;
        let (sin_w0, cos_w0) = (w0.sin(), w0.cos());
        let alpha = sin_w0 / (2.0 * q.max(0.1));
        let a0_inv = 1.0 / (1.0 + alpha / a.max(1e-9));
        Self {
            b0: a0_inv * (1.0 + alpha * a),
            b1: a0_inv * (-2.0 * a),
            b2: a0_inv * (1.0 - alpha * a),
            a1: a0_inv * (-2.0 * cos_w0),
            a2: a0_inv * (1.0 - alpha / a.max(1e-9)),
            z1: 0.0,
            z2: 0.0,
        }
    }
    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// 10-band parametric EQ matching the RadioDJ `eqValues` array.
#[derive(Debug)]
pub struct ParametricEq {
    bands: Vec<(PeakEqBand, Biquad, Biquad)>,
}

/// RadioDJ default EQ frequencies (Hz) for the 10 bands in `eqValues`.
pub const EQ_FREQS: [f64; 10] = [
    40.0, 80.0, 150.0, 300.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 14000.0,
];

impl ParametricEq {
    pub fn new(gains_db: &[f64], rate: f64) -> Self {
        let bands = gains_db
            .iter()
            .enumerate()
            .filter(|(_, g)| g.abs() > 1e-6)
            .map(|(i, g)| {
                let f = EQ_FREQS.get(i).copied().unwrap_or(1000.0);
                let band = PeakEqBand {
                    frequency: f,
                    gain_db: *g,
                    q: 1.0,
                };
                (
                    band,
                    Biquad::peaching(f, *g, 1.0, rate),
                    Biquad::peaching(f, *g, 1.0, rate),
                )
            })
            .collect();
        Self { bands }
    }

    pub fn is_active(&self) -> bool {
        !self.bands.is_empty()
    }

    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut l = left;
        let mut r = right;
        for (_, bq, _) in self.bands.iter_mut() {
            l = bq.process(l as f64) as f32;
        }
        for (_, _, bq) in self.bands.iter_mut() {
            r = bq.process(r as f64) as f32;
        }
        (l, r)
    }
}

// ---------------------------------------------------------------------------
// Mid/Side width
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WidthConfig {
    /// 0 = mono, 1 = unchanged, >1 = wider.
    pub width: f64,
}

impl Default for WidthConfig {
    fn default() -> Self {
        Self { width: 1.0 }
    }
}

#[derive(Debug)]
pub struct MidSideWidth {
    width: f64,
}

impl MidSideWidth {
    pub fn new(cfg: WidthConfig) -> Self {
        Self { width: cfg.width }
    }
    #[inline]
    pub fn process(&self, left: f32, right: f32) -> (f32, f32) {
        if (self.width - 1.0).abs() < 1e-9 {
            return (left, right);
        }
        let mid = (left + right) * 0.5;
        let side = (left - right) * 0.5 * self.width;
        (mid + side, mid - side)
    }
}

// ---------------------------------------------------------------------------
// Compressor (AudioThing "Compressor ModeChains")
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CompressorConfig {
    /// Threshold in dBFS (-60..=0).
    pub threshold_db: f64,
    /// Ratio, 1..=20.
    pub ratio: f64,
    /// Attack in ms.
    pub attack_ms: f64,
    /// Release in ms.
    pub release_ms: f64,
    /// Makeup gain in dB applied after compression.
    pub makeup_db: f64,
    /// Knee width in dB (0 = hard knee, 12 = soft knee).
    pub knee_db: f64,
}

impl Default for CompressorConfig {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 3.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_db: 0.0,
            knee_db: 6.0,
        }
    }
}

/// Peak-based compressor with lookahead and soft knee. The RadioDJ
/// `cAttack`/`cRelease` settings are control speeds in ms and `cGain`
/// is the makeup gain.
#[derive(Debug)]
pub struct Compressor {
    cfg: CompressorConfig,
    attack_coef: f64,
    release_coef: f64,
    makeup: f64,
    envelope: f64,
    gain_smooth: f64,
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write: usize,
}

impl Compressor {
    pub fn new(cfg: CompressorConfig, rate: f64) -> Self {
        let rate = rate.max(1.0);
        let attack_ms = cfg.attack_ms.max(0.05);
        let release_ms = cfg.release_ms.max(1.0);
        Self {
            cfg,
            attack_coef: (-1.0 / (attack_ms * 0.001 * rate)).exp(),
            release_coef: (-1.0 / (release_ms * 0.001 * rate)).exp(),
            makeup: db_to_lin(cfg.makeup_db),
            envelope: 0.0,
            gain_smooth: 1.0,
            delay_l: vec![0.0; LOOKAHEAD + 1],
            delay_r: vec![0.0; LOOKAHEAD + 1],
            write: 0,
        }
    }

    /// Static curve: linear gain (not dB) for an envelope level in dB.
    fn curve(&self, level_db: f64) -> f64 {
        let th = self.cfg.threshold_db;
        let ratio = self.cfg.ratio.clamp(1.0, 20.0);
        let knee = self.cfg.knee_db.max(0.0);
        let over = level_db - th;
        // reduction (dB of attenuation, always >= 0)
        let applied = if knee > 0.0 && over > -knee / 2.0 && over < knee / 2.0 {
            // soft knee: quadratic ramp from 0 at th-k/2 to full at th+k/2
            (1.0 - 1.0 / ratio) * (over + knee / 2.0).powi(2) / (2.0 * knee)
        } else if over > 0.0 {
            (1.0 - 1.0 / ratio) * over
        } else {
            0.0
        };
        db_to_lin(-applied)
    }

    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        // write into the delay rings
        self.delay_l[self.write] = left;
        self.delay_r[self.write] = right;
        let read = (self.write + 1) % (LOOKAHEAD + 1);
        let dl = self.delay_l[read];
        let dr = self.delay_r[read];
        self.write = (self.write + 1) % (LOOKAHEAD + 1);

        // stereo peak in dB
        let peak = dl.abs().max(dr.abs()) as f64;
        let peak_db = lin_to_db(peak.max(1e-10));
        if peak_db > self.envelope {
            self.envelope = self.attack_coef * self.envelope + (1.0 - self.attack_coef) * peak_db;
        } else {
            self.envelope = self.release_coef * self.envelope + (1.0 - self.release_coef) * peak_db;
        }
        let target = self.curve(self.envelope) * self.makeup;
        self.gain_smooth =
            0.01 * self.gain_smooth + 0.99 * target; // slow smoothing to avoid zipper noise
        (
            (dl as f64 * self.gain_smooth) as f32,
            (dr as f64 * self.gain_smooth) as f32,
        )
    }

    /// Current gain reduction in dB (<= 0), for meters.
    pub fn gain_reduction_db(&self) -> f64 {
        lin_to_db(self.gain_smooth.max(1e-9))
    }
}

// ---------------------------------------------------------------------------
// Limiter (PAMS "Maximiter"-style)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LimiterConfig {
    /// Ceiling in dBFS (-12..=0, RadioDJ `Limiter_ceiling`).
    pub ceiling_db: f64,
    /// Attack in ms.
    pub attack_ms: f64,
    /// Release in ms.
    pub release_ms: f64,
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            ceiling_db: -0.3,
            attack_ms: 0.5,
            release_ms: 50.0,
        }
    }
}

#[derive(Debug)]
pub struct Limiter {
    ceiling: f64,
    attack_coef: f64,
    release_coef: f64,
    envelope: f64,
    gain_smooth: f64,
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write: usize,
}

impl Limiter {
    pub fn new(cfg: LimiterConfig, rate: f64) -> Self {
        let rate = rate.max(1.0);
        let ceiling_db = cfg.ceiling_db.clamp(-12.0, 0.0);
        Self {
            ceiling: db_to_lin(ceiling_db),
            attack_coef: (-1.0 / (cfg.attack_ms.max(0.05) * 0.001 * rate)).exp(),
            release_coef: (-1.0 / (cfg.release_ms.max(1.0) * 0.001 * rate)).exp(),
            envelope: 0.0,
            gain_smooth: 1.0,
            delay_l: vec![0.0; LOOKAHEAD + 1],
            delay_r: vec![0.0; LOOKAHEAD + 1],
            write: 0,
        }
    }

    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.delay_l[self.write] = left;
        self.delay_r[self.write] = right;
        let read = (self.write + 1) % (LOOKAHEAD + 1);
        let dl = self.delay_l[read];
        let dr = self.delay_r[read];
        self.write = (self.write + 1) % (LOOKAHEAD + 1);

        let peak = dl.abs().max(dr.abs()) as f64;
        if peak > self.envelope {
            self.envelope = self.attack_coef * self.envelope + (1.0 - self.attack_coef) * peak;
        } else {
            self.envelope = self.release_coef * self.envelope + (1.0 - self.release_coef) * peak;
        }
        let target = if self.envelope > self.ceiling {
            self.ceiling / self.envelope
        } else {
            1.0
        };
        self.gain_smooth = 0.999 * self.gain_smooth + 0.001 * target;
        (
            (dl as f64 * self.gain_smooth) as f32,
            (dr as f64 * self.gain_smooth) as f32,
        )
    }
}

// ---------------------------------------------------------------------------
// Sound processor (Plugin_SoundProcessing)
// ---------------------------------------------------------------------------

/// Full sound-processing chain configured from the RadioDJ
/// `Plugin_SoundProcessing` settings keys.
#[derive(Debug)]
pub struct SoundProcessor {
    pub enabled: bool,
    compressor: Option<Compressor>,
    eq: Option<ParametricEq>,
    limiter: Limiter,
    vol_out: f64,
}

impl SoundProcessor {
    /// Build from `Plugin_SoundProcessing` keys:
    /// - `on`: `ON` (chain master switch)
    /// - `threshold_db`: `cThreshold` (compressor threshold dBFS)
    /// - `ratio`: `cRatio`
    /// - `attack_ms`: `cAttack`
    /// - `release_ms`: `cRelease`
    /// - `gain_db`: `cGain` (makeup gain)
    /// - `eq_on`: `eqON`
    /// - `eq_values`: `eqValues` (10 x dB)
    /// - `vol_out`: `vol_Out` (output trim dB, master)
    #[allow(clippy::too_many_arguments)]
    pub fn from_settings(
        on: bool,
        threshold_db: f64,
        ratio: f64,
        attack_ms: f64,
        release_ms: f64,
        gain_db: f64,
        eq_on: bool,
        eq_values: &[f64],
        vol_out_db: f64,
        rate: f64,
    ) -> Self {
        let compressor = if on {
            Some(Compressor::new(
                CompressorConfig {
                    threshold_db: threshold_db.clamp(-80.0, 0.0),
                    ratio: ratio.clamp(1.0, 20.0),
                    attack_ms: attack_ms.max(0.0),
                    release_ms: release_ms.max(0.0),
                    makeup_db: gain_db,
                    knee_db: 6.0,
                },
                rate,
            ))
        } else {
            None
        };
        let eq = if on && eq_on && eq_values.iter().any(|v| v.abs() > 1e-6) {
            Some(ParametricEq::new(eq_values, rate))
        } else {
            None
        };
        Self {
            enabled: on,
            compressor,
            eq,
            limiter: Limiter::new(LimiterConfig::default(), rate),
            vol_out: db_to_lin(vol_out_db),
        }
    }

    #[inline]
    pub fn process(&mut self, mut left: f32, mut right: f32) -> (f32, f32) {
        if let Some(eq) = self.eq.as_mut() {
            (left, right) = eq.process(left, right);
        }
        if let Some(c) = self.compressor.as_mut() {
            (left, right) = c.process(left, right);
        }
        (left, right) = self.limiter.process(left, right);
        (
            (left as f64 * self.vol_out) as f32,
            (right as f64 * self.vol_out) as f32,
        )
    }
}

// ---------------------------------------------------------------------------
// Voice-track assist (auto-fill / "play next when voice track ends")
// ---------------------------------------------------------------------------

/// Thresholds controlling automatic next-music behaviour under voice
/// tracks (`ThresholdStart`, `ThresholdNext`, `ThresholdEnd`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VoiceAssistConfig {
    /// Level (dB) above which music starts when the VT ends
    /// (`ThresholdStart`).
    pub start_next_db: f64,
    /// Level (dB) below which the current music is kept until the VT
    /// ends (`ThresholdNext`).
    pub auto_fill_db: f64,
    /// Level (dB) under which the assist is considered ended while the
    /// VT is silent (`ThresholdEnd`).
    pub end_db: f64,
}

impl Default for VoiceAssistConfig {
    fn default() -> Self {
        Self {
            start_next_db: -25.0,
            auto_fill_db: -15.0,
            end_db: -28.0,
        }
    }
}

/// Tracks the current voice-track state machine used by assisted play.
#[derive(Debug, Default)]
pub struct VoiceTrackAssist {
    cfg: VoiceAssistConfig,
    pub active: bool,
}

impl VoiceTrackAssist {
    pub fn new(cfg: VoiceAssistConfig) -> Self {
        Self {
            cfg,
            active: false,
        }
    }

    pub fn config(&self) -> VoiceAssistConfig {
        self.cfg
    }

    /// True when the measured voice level indicates the voice track is
    /// effectively over (silent past the end threshold), so the next
    /// item may start.
    pub fn should_start_next(&self, voice_level_db: f64) -> bool {
        voice_level_db < self.cfg.end_db
    }

    /// True when a new music item may be started to fill the gap.
    pub fn should_autofill(&self, current_level_db: f64) -> bool {
        current_level_db < self.cfg.auto_fill_db
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressor_reduces_gain_above_threshold() {
        let rate = 44100.0;
        let mut c = Compressor::new(
            CompressorConfig {
                threshold_db: -20.0,
                ratio: 4.0,
                attack_ms: 1.0,
                release_ms: 20.0,
                makeup_db: 0.0,
                knee_db: 6.0,
            },
            rate,
        );
        // steady 1 kHz sine at -6 dBFS
        let mut max_out = 0.0f32;
        for i in 0..rate as usize {
            let s = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate).sin() as f32 * 0.5;
            let (l, _) = c.process(s, s);
            if i > 2000 {
                max_out = max_out.max(l.abs());
            }
        }
        // 0.5 (-6 dB) input must come out below -20 dBFS
        assert!(max_out < 0.1, "max_out {}", max_out);
        assert!(c.gain_reduction_db() < -6.0);
    }

    #[test]
    fn limiter_stays_below_ceiling() {
        let rate = 44100.0;
        let mut lim = Limiter::new(
            LimiterConfig {
                ceiling_db: -1.0,
                attack_ms: 0.3,
                release_ms: 20.0,
            },
            rate,
        );
        let mut max_out = 0.0f32;
        for i in 0..(rate as usize) {
            let s = (2.0 * std::f64::consts::PI * 500.0 * i as f64 / rate).sin() as f32;
            let (l, r) = lim.process(s, s);
            if i > 1000 {
                max_out = max_out.max(l.abs()).max(r.abs());
            }
        }
        let ceiling = 10f32.powf(-1.0 / 20.0);
        // allow one sample worth of attack overshoot
        assert!(max_out <= ceiling * 1.02, "max_out {} > ceiling", max_out);
    }

    #[test]
    fn high_shelf_boosts_high_energy() {
        let rate = 44100.0;
        let mut sh = HighShelf::new(
            HighShelfConfig {
                frequency: 2000.0,
                tilt_db: 12.0,
            },
            rate,
        );
        // 8 kHz tone should be amplified
        let mut sum = 0.0f64;
        let mut n = 0.0f64;
        for i in 0..rate as usize {
            let s = (2.0 * std::f64::consts::PI * 8000.0 * i as f64 / rate).sin() as f32;
            let y = sh.process(s);
            if i > 4000 {
                sum += (y * y) as f64;
                n += 1.0;
            }
        }
        let rms = sum.sqrt() / n.sqrt();
        let dry_rms = 0.5f64.sqrt();
        assert!(rms > dry_rms * 1.5, "rms {} dry {}", rms, dry_rms);
    }

    #[test]
    fn low_shelf_boosts_low_energy() {
        let rate = 44100.0;
        let mut sh = LowShelf::new(
            LowShelfConfig {
                frequency: 200.0,
                tilt_db: 12.0,
            },
            rate,
        );
        let mut sum = 0.0f64;
        let mut n = 0.0f64;
        for i in 0..rate as usize {
            let s = (2.0 * std::f64::consts::PI * 50.0 * i as f64 / rate).sin() as f32;
            let y = sh.process(s);
            if i > 4000 {
                sum += (y * y) as f64;
                n += 1.0;
            }
        }
        let rms = sum.sqrt() / n.sqrt();
        let dry_rms = 0.5f64.sqrt();
        assert!(rms > dry_rms * 1.5, "rms {} dry {}", rms, dry_rms);
    }

    #[test]
    fn parametric_eq_boosts_band() {
        let rate = 44100.0;
        let mut eq = ParametricEq::new(
            &[0.0, 0.0, 0.0, 0.0, 0.0, 12.0, 0.0, 0.0, 0.0, 0.0],
            rate,
        );
        assert!(eq.is_active());
        let mut sum = 0.0f64;
        let mut n = 0.0f64;
        for i in 0..rate as usize {
            let s = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate).sin() as f32;
            let (l, _) = eq.process(s, s);
            if i > 4000 {
                sum += (l * l) as f64;
                n += 1.0;
            }
        }
        let rms = sum.sqrt() / n.sqrt();
        assert!(rms > 0.5f64.sqrt() * 1.5, "rms {}", rms);
    }

    #[test]
    fn width_zero_is_mono() {
        let w = MidSideWidth::new(WidthConfig { width: 0.0 });
        let (l, r) = w.process(1.0, -0.4);
        assert!((l - r).abs() < 1e-6);
    }

    #[test]
    fn sound_processor_processes_stereo() {
        let mut sp = SoundProcessor::from_settings(
            true,
            -18.0,
            3.0,
            10.0,
            100.0,
            0.0,
            true,
            &[0.0; 10],
            0.0,
            44100.0,
        );
        let (l, r) = sp.process(0.5, -0.5);
        assert!(l.is_finite() && r.is_finite());
    }

    #[test]
    fn voice_assist_thresholds() {
        let va = VoiceTrackAssist::new(VoiceAssistConfig::default());
        assert!(va.should_start_next(-50.0)); // silence -> start next
        assert!(!va.should_start_next(-10.0)); // voice present
        assert!(va.should_autofill(-40.0));
        assert!(!va.should_autofill(-10.0));
    }

    #[test]
    fn soft_clip_reduces_peaks() {
        let sc = SoftClip::new(SoftClipConfig {
            saturation_db: 12.0,
        });
        let y = sc.process(2.0);
        assert!(y.abs() < 2.0 && y.abs() <= 1.05);
    }
}
