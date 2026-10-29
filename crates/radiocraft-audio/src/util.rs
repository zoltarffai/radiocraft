//! Small numeric and signal helpers shared across the audio crate.

/// Convert decibels to a linear amplitude factor (20*log domain).
#[inline]
pub fn db_to_lin(db: f64) -> f64 {
    10.0f64.powf(db / 20.0)
}

/// Convert a linear amplitude factor to decibels. Silence maps to -inf.
#[inline]
pub fn lin_to_db(lin: f64) -> f64 {
    if lin <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * lin.log10()
    }
}

/// `lin_to_db` for f32 samples.
#[inline]
pub fn lin_to_db_f32(lin: f32) -> f32 {
    if lin <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * lin.log10()
    }
}

#[inline]
pub fn clamp_f32(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi)
}

#[inline]
pub fn clamp_f64(v: f64, lo: f64, hi: f64) -> f64 {
    v.max(lo).min(hi)
}

/// Streaming, stateful, linear-interpolation resampler for interleaved buffers.
///
/// Quality note: linear interpolation is deliberately cheap; playout audio is
/// converted once per file at load (see `decoder`) and only this helper is
/// used at the device boundary when the host device rate differs from the
/// internal mix rate.
pub struct StreamResampler {
    in_rate: f64,
    out_rate: f64,
    chans: usize,
    carry: Vec<f32>,
    pos: f64,
    consumed: usize,
}

impl StreamResampler {
    pub fn new(in_rate: usize, out_rate: usize, chans: usize) -> Self {
        StreamResampler {
            in_rate: in_rate as f64,
            out_rate: out_rate as f64,
            chans,
            carry: vec![0.0; chans],
            pos: 0.0,
            consumed: 0,
        }
    }

    fn frame_at<'a>(&'a self, chunk: &'a [f32], base: usize, i: usize) -> &'a [f32] {
        if i < base {
            &self.carry
        } else {
            let off = (i - base) * self.chans;
            if off + self.chans <= chunk.len() {
                &chunk[off..off + self.chans]
            } else {
                &self.carry
            }
        }
    }

    /// Resample `input` (interleaved, `chans` channels) appending to `out`.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let in_frames = input.len() / self.chans;
        let base = self.consumed;
        let end = base + in_frames;
        let step = self.in_rate / self.out_rate;
        while self.pos < (end.saturating_sub(1)) as f64 {
            let idx = self.pos.floor() as usize;
            let frac = (self.pos - idx as f64) as f32;
            let a = self.frame_at(input, base, idx);
            let b = self.frame_at(input, base, idx + 1);
            for c in 0..self.chans {
                out.push(a[c] + (b[c] - a[c]) * frac);
            }
            self.pos += step;
        }
        if in_frames > 0 {
            let tail = (in_frames - 1) * self.chans;
            self.carry.copy_from_slice(&input[tail..tail + self.chans]);
        }
        self.consumed = end;
    }

    /// Emit the remaining interpolated frames past the last pushed chunk.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        let step = self.in_rate / self.out_rate;
        while self.pos < (self.consumed.saturating_sub(1)) as f64 {
            let idx = self.pos.floor() as usize;
            let frac = (self.pos - idx as f64) as f32;
            let a = self.frame_at(&[], 0, idx);
            let b = self.frame_at(&[], 0, idx + 1);
            for c in 0..self.chans {
                out.push(a[c] + (b[c] - a[c]) * frac);
            }
            self.pos += step;
        }
        self.reset();
    }

    pub fn reset(&mut self) {
        self.carry.iter_mut().for_each(|s| *s = 0.0);
        self.pos = 0.0;
        self.consumed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_roundtrip() {
        assert!((db_to_lin(-6.0) - 10f64.powf(-6.0 / 20.0)).abs() < 1e-12);
        assert_eq!(lin_to_db(0.0), f64::NEG_INFINITY);
        assert!((lin_to_db(1.0)).abs() < 1e-12);
    }

    #[test]
    fn identity_resample() {
        let mut r = StreamResampler::new(44100, 44100, 2);
        let input: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let mut out = Vec::new();
        r.push(&input, &mut out);
        r.flush(&mut out);
        assert_eq!(out.len(), 100);
        assert!((out[50] - input[50]).abs() < 1e-3);
    }

    #[test]
    fn halving_resample_length() {
        let mut r = StreamResampler::new(96000, 48000, 1);
        let input = vec![1.0f32; 100];
        let mut out = Vec::new();
        r.push(&input, &mut out);
        r.flush(&mut out);
        assert_eq!(out.len(), 50);
        assert!(out.iter().all(|s| (*s - 1.0).abs() < 1e-6));
    }
}
