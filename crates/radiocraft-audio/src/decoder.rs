//! Decoding and pluggable audio sources.
//!
//! All sources produce interleaved `f32` PCM at the engine's fixed
//! `SAMPLE_RATE` (44.1 kHz, 2 channels). `FileSource` wraps symphonia and
//! transparently handles sample-format conversion and sample-rate differences
//! via [`util::StreamResampler`].

use std::path::Path;
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use crate::error::{AudioError, Result};
use crate::util::StreamResampler;
use crate::SAMPLE_RATE;

/// A stream of interleaved stereo f32 frames at 44.1 kHz.
pub trait Source: Send {
    /// Read at most `buf.len()/2` frames into `buf` (interleaved stereo).
    /// Returns the number of frames written; `0` means end of stream.
    fn read(&mut self, buf: &mut [f32]) -> usize;

    /// Seek to an absolute position.
    ///
    /// `Ok(false)` = seek performed. `Ok(true)` = this source cannot seek; the
    /// caller must restart it from the beginning instead.
    fn seek(&mut self, seconds: f64) -> Result<bool>;

    /// Total duration when known.
    fn duration(&self) -> Option<Duration> {
        None
    }
}

// ---------------------------------------------------------------------------
// FileSource
// ---------------------------------------------------------------------------

/// Audio file decoded through symphonia (mp3/aac/m4a/flac/ogg/wav/pcm…).
pub struct FileSource {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    duration: Option<Duration>,
    resampler: StreamResampler,
    resampling: bool,
    flushed: bool,
    scratch: Vec<f32>,
    /// Decoded-but-unconsumed interleaved stereo frames at the mix rate.
    pending: std::collections::VecDeque<f32>,
    done: bool,
}

impl FileSource {
    /// Open a media file.
    pub fn open(path: &Path) -> Result<Self> {
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

        let reader = probe.format;
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

        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions { verify: false })
            .map_err(|e| AudioError::Unsupported(format!("{}: {e}", path.display())))?;

        let source_rate = params
            .sample_rate
            .unwrap_or(SAMPLE_RATE as u32);

        let duration = params.n_frames.zip(params.time_base).and_then(|(frames, tb)| {
            if frames == 0 || tb.denom == 0 {
                None
            } else {
                Duration::try_from_secs_f64(
                    frames as f64 * f64::from(tb.numer) / f64::from(tb.denom),
                )
                .ok()
            }
        });

        let resampling = source_rate != SAMPLE_RATE as u32;
        Ok(FileSource {
            reader,
            decoder,
            track_id,
            duration,
            resampler: StreamResampler::new(source_rate as usize, SAMPLE_RATE, 2),
            resampling,
            flushed: false,
            scratch: Vec::new(),
            pending: std::collections::VecDeque::new(),
            done: false,
        })
    }

    /// Decode packets until `pending` gains frames. Returns false at EOF.
    fn pump(&mut self) -> bool {
        if self.done {
            if self.resampling && !self.flushed {
                self.flushed = true;
                self.scratch.clear();
                self.resampler.flush(&mut self.scratch);
                self.pending.extend(self.scratch.iter());
            }
            return !self.pending.is_empty();
        }
        loop {
            let packet = match self.reader.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(_) => {
                    self.done = true;
                    return self.pump();
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    let spec = *buf.spec();
                    let frames = buf.frames();
                    let src_ch = spec.channels.count();
                    let mut samples = SampleBuffer::<f32>::new(frames as u64, spec);
                    samples.copy_interleaved_ref(buf);
                    let src = samples.samples();
                    let stereo: Vec<f32> = if src_ch == 1 {
                        src.iter().flat_map(|s| [*s, *s]).collect()
                    } else {
                        let mut v = vec![0.0; frames * 2];
                        for f in 0..frames {
                            v[f * 2] = src[f * src_ch];
                            v[f * 2 + 1] = src[f * src_ch + 1];
                        }
                        v
                    };
                    if self.resampling {
                        self.scratch.clear();
                        self.resampler.push(&stereo, &mut self.scratch);
                        self.pending.extend(self.scratch.iter());
                    } else {
                        self.pending.extend(stereo.iter());
                    }
                    if !self.pending.is_empty() {
                        return true;
                    }
                    // otherwise keep decoding more packets
                }
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(_) => {
                    self.done = true;
                    return self.pump();
                }
            }
        }
    }
}

impl Source for FileSource {
    fn read(&mut self, buf: &mut [f32]) -> usize {
        let want = buf.len() / 2;
        let mut produced = 0;
        while produced < want {
            if self.pending.is_empty() && !self.pump() {
                break;
            }
            let take = (want - produced).min(self.pending.len() / 2);
            for chunk in buf[produced * 2..produced * 2 + take * 2].chunks_exact_mut(2) {
                chunk[0] = self.pending.pop_front().unwrap_or(0.0);
                chunk[1] = self.pending.pop_front().unwrap_or(0.0);
            }
            produced += take;
        }
        produced
    }

    fn seek(&mut self, seconds: f64) -> Result<bool> {
        match self.reader.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from(seconds),
                track_id: Some(self.track_id),
            },
        ) {
            Ok(_seeked) => {
                self.decoder.reset();
                self.resampler.reset();
                self.pending.clear();
                self.scratch.clear();
                self.flushed = false;
                self.done = false;
                Ok(false)
            }
            Err(e) => Err(AudioError::Invalid(format!("seek not supported: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// MemorySource
// ---------------------------------------------------------------------------

/// A source backed by an in-memory interleaved stereo f32 buffer.
pub struct MemorySource {
    data: Vec<f32>,
    pos: usize,
    rate: f64,
}

impl MemorySource {
    /// `data` must be interleaved stereo at `rate`.
    pub fn new(data: Vec<f32>, rate: f64) -> Self {
        MemorySource { data, pos: 0, rate }
    }

    /// Mono samples are duplicated to both channels.
    pub fn from_mono(samples: &[f32], rate: f64) -> Self {
        let mut data = Vec::with_capacity(samples.len() * 2);
        for s in samples {
            data.push(*s);
            data.push(*s);
        }
        MemorySource { data, pos: 0, rate }
    }
}

impl Source for MemorySource {
    fn read(&mut self, buf: &mut [f32]) -> usize {
        let avail = self.data.len() - self.pos;
        let n = avail.min(buf.len());
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        n / 2
    }

    fn seek(&mut self, seconds: f64) -> Result<bool> {
        let frame = (seconds * self.rate)
            .clamp(0.0, (self.data.len() / 2) as f64)
            .max(0.0) as usize;
        self.pos = frame * 2;
        Ok(false)
    }

    fn duration(&self) -> Option<Duration> {
        Duration::try_from_secs_f64(self.data.len() as f64 / 2.0 / self.rate).ok()
    }
}

// ---------------------------------------------------------------------------
// SilentSource
// ---------------------------------------------------------------------------

/// A source that yields silence forever. Tests and placeholders.
pub struct SilentSource {
    rate: f64,
    read: usize,
}

impl SilentSource {
    pub fn new() -> Self {
        SilentSource {
            rate: SAMPLE_RATE as f64,
            read: 0,
        }
    }
}

impl Default for SilentSource {
    fn default() -> Self {
        Self::new()
    }
}

impl Source for SilentSource {
    fn read(&mut self, buf: &mut [f32]) -> usize {
        buf.fill(0.0);
        self.read += buf.len() / 2;
        buf.len() / 2
    }

    fn seek(&mut self, seconds: f64) -> Result<bool> {
        self.read = (seconds * self.rate).max(0.0) as usize;
        Ok(false)
    }
}

/// Test helper: an in-memory stereo sine tone.
pub fn sine_source(freq: f64, seconds: f64, amp: f64, rate: f64) -> MemorySource {
    let n = (seconds * rate) as usize;
    let mut data = Vec::with_capacity(n * 2);
    for i in 0..n {
        let s = (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin() as f32 * amp as f32;
        data.push(s);
        data.push(s);
    }
    MemorySource::new(data, rate)
}
