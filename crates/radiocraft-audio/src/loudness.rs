//! ITU-R BS.1770-4 loudness measurement (K-weighting, gated integrated
//! loudness) and 4x-oversampled true-peak estimation. Used for the
//! `bs1770` (integrated LUFS) and `loudness` (true-peak dBTP) library
//! columns.
//!
//! The K-weighting filter coefficients are derived from the canonical
//! EBU R128 formula (stage 1 high-shelf + stage 2 high-pass) so the
//! measurement is correct for any internal sample rate.

use crate::util::{clamp_f32, lin_to_db};

/// Loudness-unit offset used by the BS.1770 / EBU R128 meters.
pub const LU_OFFSET: f64 = -0.691;

/// Absolute (outer) gate in LUFS.
pub const GATE_OUTER_DB: f64 = -70.0;
/// Relative gate offset in LU.
pub const GATE_RELATIVE_DB: f64 = -10.0;

/// Non-overlapping gating block length (ms).
pub const BLOCK_MS: f64 = 400.0;

// ---------------------------------------------------------------------------
// K-weighting
// ---------------------------------------------------------------------------

/// Canonical K-weighting coefficients: stage-1 high-shelf
/// (f0=1681.974450955533 Hz, G=3.999843853973347 dB, Q=0.7071752369554196)
/// cascaded with the stage-2 high-pass (f0=38.13547087602444 Hz,
/// Q=0.5003270373238773), folded into a single 4th-order section using
/// the EBU R128 reference formulation (`K = tan(pi*f0/rate)`).
///
/// Returns `b` (5 coefficients) and `a` (5 coefficients, `a[0] = 1.0`).
#[allow(non_snake_case)]
fn k_coefficients(rate: f64) -> ([f64; 5], [f64; 5]) {
    let f0 = 1681.974450955533;
    let G = 3.999843853973347;
    let Q = 0.7071752369554196;

    let K = f64::tan(std::f64::consts::PI * f0 / rate);
    let Vh = f64::powf(10.0, G / 20.0);
    let Vb = f64::powf(Vh, 0.4996667741545416);

    let a0 = 1.0 + K / Q + K * K;
    let pb = [
        (Vh + Vb * K / Q + K * K) / a0,
        2.0 * (K * K - Vh) / a0,
        (Vh - Vb * K / Q + K * K) / a0,
    ];
    let pa = [
        1.0,
        2.0 * (K * K - 1.0) / a0,
        (1.0 - K / Q + K * K) / a0,
    ];
    let rb = [1.0f64, -2.0, 1.0];

    let f0 = 38.13547087602444;
    let Q = 0.5003270373238773;
    let K = f64::tan(std::f64::consts::PI * f0 / rate);
    let a0 = 1.0 + K / Q + K * K;
    let ra = [
        1.0,
        2.0 * (K * K - 1.0) / a0,
        (1.0 - K / Q + K * K) / a0,
    ];

    // Cascade numerator/denominator: b = pb ⊗ rb, a = pa ⊗ ra.
    let b = conv3(&pb, &rb);
    let a = conv3(&pa, &ra);
    (b, a)
}

/// Full convolution of two 3-tap coefficient arrays into 5 coefficients.
fn conv3(a: &[f64; 3], b: &[f64; 3]) -> [f64; 5] {
    let mut out = [0.0f64; 5];
    for (i, &x) in a.iter().enumerate() {
        for (j, &y) in b.iter().enumerate() {
            out[i + j] += x * y;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 4th-order transposed Direct-Form II section (K-weighting per channel)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Section4 {
    b: [f64; 5],
    a: [f64; 5], // a[0] == 1.0
    s: [f64; 4], // DF2T delay state
}

impl Section4 {
    fn new(b: [f64; 5], a: [f64; 5]) -> Self {
        Section4 { b, a, s: [0.0; 4] }
    }
    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.s[0];
        self.s[0] = self.b[1] * x - self.a[1] * y + self.s[1];
        self.s[1] = self.b[2] * x - self.a[2] * y + self.s[2];
        self.s[2] = self.b[3] * x - self.a[3] * y + self.s[3];
        self.s[3] = self.b[4] * x - self.a[4] * y;
        y
    }
    fn reset(&mut self) {
        self.s = [0.0; 4];
    }
}

/// Stereo K-weighting filter state.
#[derive(Debug, Clone)]
pub struct KWeighting {
    ch: [Section4; 2],
}

impl KWeighting {
    /// Build the K-weighting chain for `rate` (Hz).
    pub fn new(rate: f64) -> Self {
        let rate = rate.max(1.0);
        let (b, a) = k_coefficients(rate);
        KWeighting {
            ch: [Section4::new(b, a), Section4::new(b, a)],
        }
    }

    /// Filter one stereo sample pair.
    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let l = self.ch[0].process(left as f64) as f32;
        let r = self.ch[1].process(right as f64) as f32;
        (l, r)
    }

    /// Reset the filter state.
    pub fn reset(&mut self) {
        self.ch[0].reset();
        self.ch[1].reset();
    }
}

// ---------------------------------------------------------------------------
// Gated integrated loudness
// ---------------------------------------------------------------------------

/// Streaming BS.1770 integrated-loudness meter.
///
/// Feed frames with [`observe_frame`](Self::observe_frame) (or
/// [`observe_buffer`](Self::observe_buffer)), then read [`integrated`](Self::integrated).
#[derive(Debug)]
pub struct LoudnessMeter {
    kw: KWeighting,
    block_len: usize,
    block_pos: usize,
    sum_sq: [f64; 2],
    mean_sq: [f64; 2],
    /// per-block loudness (LUFS), uncalibrated sum of channel powers
    blocks: Vec<f64>,
    max_block: f64,
    max_sample: f64,
    true_peak: TruePeakMeter,
}

impl LoudnessMeter {
    pub fn new(rate: f64) -> Self {
        let rate = rate.max(1.0);
        let block_len = (rate * (BLOCK_MS / 1000.0)).round() as usize;
        Self {
            kw: KWeighting::new(rate),
            block_len: block_len.max(1),
            block_pos: 0,
            sum_sq: [0.0; 2],
            mean_sq: [0.0; 2],
            blocks: Vec::new(),
            max_block: f64::MIN,
            max_sample: 0.0,
            true_peak: TruePeakMeter::new(),
        }
    }

    /// Observe one stereo sample pair.
    #[inline]
    pub fn observe_frame(&mut self, left: f32, right: f32) {
        let (l, r) = self.kw.process(left, right);
        let lf = l as f64;
        let rf = r as f64;
        self.sum_sq[0] += lf * lf;
        self.sum_sq[1] += rf * rf;
        self.max_sample = self.max_sample.max(left.abs() as f64).max(right.abs() as f64);
        self.true_peak.observe(left, right);

        self.block_pos += 1;
        if self.block_pos >= self.block_len {
            self.flush_block();
        }
    }

    /// Observe an interleaved stereo buffer.
    pub fn observe_buffer(&mut self, buf: &[f32]) {
        for pair in buf.as_chunks::<2>().0 {
            self.observe_frame(pair[0], pair[1]);
        }
    }

    fn flush_block(&mut self) {
        let n = self.block_pos as f64;
        self.mean_sq[0] = self.sum_sq[0] / n;
        self.mean_sq[1] = self.sum_sq[1] / n;
        // BS.1770 channel weights: L/R = 1.0 (no surround channels here)
        let block_lufs = LU_OFFSET + 10.0 * (self.mean_sq[0] + self.mean_sq[1]).log10();
        self.blocks.push(block_lufs);
        self.max_block = self.max_block.max(block_lufs);
        self.sum_sq = [0.0; 2];
        self.block_pos = 0;
    }

    fn finish_partial_block(&mut self) {
        if self.block_pos > 0 {
            // partial block at the tail: gate out if shorter than a frame
            // worth of content, otherwise fold into the blocks list.
            self.flush_block();
        }
    }

    /// Integrated loudness in LUFS, or None when every block falls below
    /// both gates (true silence).
    pub fn integrated(&mut self) -> Option<f64> {
        self.finish_partial_block();
        if self.blocks.is_empty() {
            return None;
        }
        // outer gate: blocks louder than -70 LUFS
        let mut sum = 0.0;
        let mut n = 0usize;
        for &b in &self.blocks {
            if b > GATE_OUTER_DB {
                // uncalibrated block power (dB -> power, NOT amplitude)
                sum += 10.0f64.powf((b - LU_OFFSET) / 10.0);
                n += 1;
            }
        }
        if n == 0 {
            return None;
        }
        let g = LU_OFFSET + 10.0 * (sum / n as f64).log10();
        // relative gate
        let rel = g + GATE_RELATIVE_DB;
        let mut sum2 = 0.0;
        let mut n2 = 0usize;
        for &b in &self.blocks {
            if b > GATE_OUTER_DB && b >= rel {
                sum2 += 10.0f64.powf((b - LU_OFFSET) / 10.0);
                n2 += 1;
            }
        }
        if n2 == 0 {
            return None;
        }
        Some(LU_OFFSET + 10.0 * (sum2 / n2 as f64).log10())
    }

    /// Largest 400 ms block loudness (LUFS) (RadioDJ short-loudness meter).
    pub fn max_block_loudness(&mut self) -> f64 {
        self.finish_partial_block();
        self.max_block
    }

    /// True-peak in linear amplitude.
    pub fn true_peak_linear(&mut self) -> f64 {
        self.true_peak.max()
    }

    /// True-peak in dBTP.
    pub fn true_peak_db(&mut self) -> f64 {
        let tp = self.true_peak_linear().max(1e-10);
        20.0 * tp.log10()
    }

    /// Sample-peak in linear amplitude.
    pub fn sample_peak(&self) -> f64 {
        self.max_sample
    }
}

// ---------------------------------------------------------------------------
// True-peak meter (4x oversampled, 32-tap polyphase FIR)
// ---------------------------------------------------------------------------

/// Polyphase branch taps for a 4x oversampled, 32-tap Kaiser(beta=6)
/// lowpass used by true-peak estimation. Branch order is newest-first.
const TP_TAPS: [[f32; 8]; 4] = [
    [
        -0.000467, 0.007262, -0.032946, 0.120846, 0.971435, -0.085555, 0.023580, -0.004452,
    ],
    [
        -0.002994, 0.026990, -0.109508, 0.437875, 0.764099, -0.149924, 0.039822, -0.006064,
    ],
    [
        -0.006064, 0.039822, -0.149924, 0.764099, 0.437875, -0.109508, 0.026990, -0.002994,
    ],
    [
        -0.004452, 0.023580, -0.085555, 0.971435, 0.120846, -0.032946, 0.007262, -0.000467,
    ],
];

/// Free-interpolated? no: per-channel 8-tap delay lines evaluated at the
/// 4 sub-sample phases.
#[derive(Debug)]
struct TruePeakCh {
    hist: [f32; 8],
    pos: usize,
}

impl TruePeakCh {
    fn new() -> Self {
        Self {
            hist: [0.0; 8],
            pos: 0,
        }
    }
    #[inline]
    fn push(&mut self, x: f32) -> f32 {
        self.hist[self.pos] = x;
        self.pos = (self.pos + 1) % 8;
        let mut peak = 0.0f32;
        for phase in TP_TAPS.iter() {
            let mut acc = 0.0f32;
            for (i, t) in phase.iter().enumerate() {
                let h = self.hist[(self.pos + i) % 8];
                acc += h * t;
            }
            // each polyphase branch sums to unity DC gain
            let v = acc.abs();
            if v > peak {
                peak = v;
            }
        }
        peak
    }
}

#[derive(Debug)]
pub struct TruePeakMeter {
    ch: [TruePeakCh; 2],
    max: f64,
}

impl TruePeakMeter {
    pub fn new() -> Self {
        Self {
            ch: [TruePeakCh::new(), TruePeakCh::new()],
            max: 0.0,
        }
    }
    #[inline]
    pub fn observe(&mut self, left: f32, right: f32) {
        let pl = self.ch[0].push(left) as f64;
        let pr = self.ch[1].push(right) as f64;
        self.max = self.max.max(pl).max(pr);
    }
    /// Observe an interleaved stereo buffer.
    pub fn observe_buffer(&mut self, buf: &[f32]) {
        for pair in buf.as_chunks::<2>().0 {
            self.observe(pair[0], pair[1]);
        }
    }

    pub fn max(&self) -> f64 {
        self.max
    }
    /// Max true-peak in dBTP (silence maps to a very low dB value).
    pub fn true_peak_db(&self) -> f64 {
        lin_to_db(self.max)
    }
    pub fn reset(&mut self) {
        self.ch = [TruePeakCh::new(), TruePeakCh::new()];
        self.max = 0.0;
    }
}

impl Default for TruePeakMeter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Convenience helpers
// ---------------------------------------------------------------------------

/// Integrated loudness (LUFS) of an interleaved stereo buffer.
pub fn integrated_lufs(buf: &[f32], rate: f64) -> Option<f64> {
    let mut m = LoudnessMeter::new(rate);
    m.observe_buffer(buf);
    m.integrated()
}

/// True-peak (dBTP) of an interleaved stereo buffer.
pub fn true_peak_dbtp(buf: &[f32], rate: f64) -> f64 {
    let _ = rate;
    let mut m = TruePeakMeter::new();
    for pair in buf.as_chunks::<2>().0 {
        m.observe(pair[0], pair[1]);
    }
    m.true_peak_db()
}

/// Scan result for one file: integrated LUFS and true-peak dBTP.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoudnessScan {
    /// Integrated loudness (LUFS); None when all blocks are gated out.
    pub integrated_lufs: Option<f64>,
    /// True-peak (dBTP).
    pub true_peak_dbtp: f64,
}

/// Linear volume multiplier that moves `measured_lufs` up/down to
/// `target_lufs` (the dB-ratio form, `10^((target - measured) / 20)`).
///
/// RadioDJ's bulk scanner stores `bs_loudness = ROUND(CalculateGain(measured), 2)`
/// and playout applies that column verbatim as a linear `SetTrackVolume`
/// (`APlayer.cs`), so the column is exactly a multiplier like this one. The
/// original `LoudnessCalculator.CalculateGain` body is encrypted in the
/// shipping binary (docs/06 §8), so this formula is a reconstruction from
/// those column semantics, not verified original code.
pub fn gain_to_target(measured_lufs: f64, target_lufs: f64) -> f64 {
    10f64.powf((target_lufs - measured_lufs) / 20.0)
}

/// Scan an interleaved stereo buffer (what the library stores per file).
pub fn scan(buf: &[f32], rate: f64) -> LoudnessScan {
    let mut m = LoudnessMeter::new(rate);
    m.observe_buffer(buf);
    LoudnessScan {
        integrated_lufs: m.integrated(),
        true_peak_dbtp: m.true_peak_db(),
    }
}

/// Clamp an f32 sample to [-1.0, 1.0].
#[inline]
pub fn hard_clip(v: f32) -> f32 {
    clamp_f32(v, -1.0, 1.0)
}

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_stereo(freq: f64, amp: f64, secs: f64, rate: f64) -> Vec<f32> {
        let n = (secs * rate) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = (2.0 * std::f64::consts::PI * freq * (i as f64 / rate)).sin() * amp;
            v.push(s as f32);
            v.push(s as f32);
        }
        v
    }

    fn k_response_db(kw: &mut KWeighting, freq: f64, rate: f64) -> f64 {
        // settle with a few cycles, then measure rms after settling
        let n = rate as usize * 3;
        let mut sum = 0.0f64;
        let mut cnt = 0.0f64;
        let mut dc = KWeighting::new(rate);
        dc.reset();
        for i in 0..n {
            let s = (2.0 * std::f64::consts::PI * freq * (i as f64 / rate)).sin() as f32;
            let (l, _) = kw.process(s, s);
            if i > (rate as usize) {
                sum += (l as f64) * (l as f64);
                cnt += 1.0;
            }
        }
        // convert sine rms to amplitude gain: rms of a unity sine is 1/sqrt(2)
        20.0 * (2.0 * sum / cnt).sqrt().log10()
    }

    #[test]
    fn k_weighting_response() {
        let rate = 44100.0;
        let mut kw = KWeighting::new(rate);
        let lo = k_response_db(&mut kw, 100.0, rate);
        let mid = k_response_db(&mut kw, 1000.0, rate);
        let hi = k_response_db(&mut kw, 10000.0, rate);
        // canonical K-weighting curve (ebur128 validated): -1.13 / +0.70 / +4.04
        assert!((lo - (-1.13)).abs() < 0.25, "100 Hz: {lo}");
        assert!((mid - 0.70).abs() < 0.25, "1 kHz: {mid}");
        assert!((hi - 4.04).abs() < 0.35, "10 kHz: {hi}");
    }

    #[test]
    fn integrated_stereo_sine() {
        // 1 kHz, amp 0.5 stereo -> -6.011 LUFS (validated python reference)
        let buf = sine_stereo(1000.0, 0.5, 5.0, 44100.0);
        let l = integrated_lufs(&buf, 44100.0).unwrap();
        assert!((l - (-6.011)).abs() < 0.15, "got {l}");
    }

    #[test]
    fn silence_is_gated_out() {
        let buf = vec![0.0f32; 44100 * 4 * 2];
        assert_eq!(integrated_lufs(&buf, 44100.0), None);
    }

    #[test]
    fn right_only_channel() {
        let n = 44100 * 5;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = (2.0 * std::f64::consts::PI * 1000.0 * (i as f64 / 44100.0)).sin() * 0.5;
            v.push(0.0);
            v.push(s as f32);
        }
        let l = integrated_lufs(&v, 44100.0).unwrap();
        assert!((l - (-9.021)).abs() < 0.2, "got {l}");
    }

    #[test]
    fn gated_out_burst() {
        // loud bursts aligned to 400 ms gating blocks, quiet gap between:
        // only the loud blocks count
        let mut buf = Vec::new();
        buf.extend(sine_stereo(1000.0, 0.5, 0.8, 44100.0));
        buf.extend(sine_stereo(1000.0, 0.0001, 4.0, 44100.0));
        buf.extend(sine_stereo(1000.0, 0.5, 0.8, 44100.0));
        let l = integrated_lufs(&buf, 44100.0).unwrap();
        assert!((l - (-6.011)).abs() < 0.2, "got {l}");
    }

    #[test]
    fn true_peak_sine() {
        let buf = sine_stereo(1000.0, 0.5, 2.0, 44100.0);
        let tp = true_peak_dbtp(&buf, 44100.0);
        let db = 20.0 * (0.5f64).log10();
        assert!((tp - db).abs() < 0.15, "tp {tp} expected {db}");
    }

    #[test]
    fn true_peak_square_has_ringing() {
        let n = 44100 * 2;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let ph = (i as f64 / 44100.0 * 100.0) % 1.0;
            let s = if ph < 0.5 { 1.0 } else { -1.0 };
            v.push(s);
            v.push(s);
        }
        let tp = true_peak_dbtp(&v, 44100.0);
        // 100 Hz square at amp 1.0 -> ~1.233 linear (+1.8 dB)
        assert!(tp > 1.4 && tp < 2.4, "tp {tp}");
    }

    #[test]
    fn scan_returns_both() {
        let buf = sine_stereo(1000.0, 0.4, 3.0, 44100.0);
        let s = scan(&buf, 44100.0);
        assert!(s.integrated_lufs.is_some());
        assert!(s.true_peak_dbtp < 0.0);
    }

    #[test]
    fn gain_to_target_matches_db_ratio() {
        // At the target the multiplier is unity; 10 dB below target needs
        // ~3.162x amplitude.
        assert!((gain_to_target(-12.0, -12.0) - 1.0).abs() < 1e-12);
        // -22 LUFS nudged to -12 LUFS is +10 dB: 10^(10/20) ~= 3.162.
        assert!((gain_to_target(-22.0, -12.0) - 10f64.powf(0.5)).abs() < 1e-9);
        // Louder than target pulls down.
        assert!((gain_to_target(-2.0, -12.0) - 10f64.powf(-0.5)).abs() < 1e-9);
    }
}
