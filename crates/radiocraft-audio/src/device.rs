//! Output sinks behind a thin trait so the engine can run headless in CI.
//!
//! - [`NullSink`] — drops audio (headless tests / detached engine).
//! - [`WavSink`] — writes an interleaved f32 WAV file through hound
//!   (recording, offline rendering).
//! - [`CpalSink`] — real hardware output through cpal; only constructed on
//!   demand, never in tests.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;

use crate::error::{AudioError, Result};

/// io error with the file path folded into the message.
fn io_err(path: &Path, e: impl std::fmt::Display) -> AudioError {
    AudioError::Io(std::io::Error::other(format!("{}: {e}", path.display())))
}

/// Metadata describing a sink.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SinkInfo {
    pub name: String,
    pub sample_rate: usize,
    pub channels: usize,
    /// True for a live hardware device, false for file / null sinks.
    pub is_device: bool,
}

/// Something that accepts interleaved f32 audio at the engine rate.
pub trait AudioSink: Send {
    /// Write one interleaved stereo block.
    fn write(&mut self, buf: &[f32]) -> Result<()>;
    /// Pause/resume the sink where the backend supports it.
    fn set_paused(&mut self, _paused: bool) {}
    /// Push out any buffered audio.
    fn flush(&mut self) {}
    /// Sink metadata.
    fn info(&self) -> SinkInfo;
}

// ---------------------------------------------------------------------------
// Null sink
// ---------------------------------------------------------------------------

/// Discards everything written to it; useful in tests and when the engine
/// runs without an output device.
#[derive(Debug, Default)]
pub struct NullSink {
    frames_written: u64,
}

impl NullSink {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }
}

impl AudioSink for NullSink {
    fn write(&mut self, buf: &[f32]) -> Result<()> {
        self.frames_written += buf.len() as u64 / 2;
        Ok(())
    }
    fn info(&self) -> SinkInfo {
        SinkInfo {
            name: "null".into(),
            sample_rate: crate::SAMPLE_RATE,
            channels: crate::CHANNELS,
            is_device: false,
        }
    }
}

// ---------------------------------------------------------------------------
// WAV file sink (hound)
// ---------------------------------------------------------------------------

/// Recording / offline-render sink that writes a 16-bit-free f32 WAV file.
pub struct WavSink {
    path: PathBuf,
    writer: hound::WavWriter<BufWriter<File>>,
    frames: u64,
}

impl WavSink {
    /// Create (overwriting) a WAV file at `path`.
    pub fn create(path: &Path) -> Result<Self> {
        let spec = hound::WavSpec {
            channels: crate::CHANNELS as u16,
            sample_rate: crate::SAMPLE_RATE as u32,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let writer =
            hound::WavWriter::create(path, spec).map_err(|e| io_err(path, e))?;
        Ok(Self {
            path: path.to_path_buf(),
            writer,
            frames: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn frames_written(&self) -> u64 {
        self.frames
    }
}

impl AudioSink for WavSink {
    fn write(&mut self, buf: &[f32]) -> Result<()> {
        for &s in buf {
            self.writer
                .write_sample(s)
                .map_err(|e| io_err(&self.path, e))?;
        }
        self.frames += buf.len() as u64 / 2;
        Ok(())
    }
    fn flush(&mut self) {
        let _ = self.writer.flush();
    }
    fn info(&self) -> SinkInfo {
        SinkInfo {
            name: format!("wav:{}", self.path.display()),
            sample_rate: crate::SAMPLE_RATE,
            channels: crate::CHANNELS,
            is_device: false,
        }
    }
}

// ---------------------------------------------------------------------------
// cpal device sink
// ---------------------------------------------------------------------------

/// Simple non-circular transfer buffer between the engine pump thread and
/// the cpal data callback (undruns feed silence).
#[derive(Debug)]
struct DeviceRing {
    buf: Vec<f32>,
    capacity: usize,
}

impl DeviceRing {
    fn new(capacity: usize) -> Self {
        Self { buf: Vec::with_capacity(capacity), capacity }
    }
    fn push(&mut self, data: &[f32]) {
        self.buf.extend_from_slice(data);
        // keep latency bounded: drop oldest samples on overflow
        if self.buf.len() > self.capacity {
            let excess = self.buf.len() - self.capacity;
            self.buf.drain(..excess);
        }
    }
    fn pull(&mut self, out: &mut [f32]) {
        let n = out.len().min(self.buf.len());
        out[..n].copy_from_slice(&self.buf[..n]);
        self.buf.drain(..n);
        for o in &mut out[n..] {
            *o = 0.0;
        }
    }
}

/// cpal-backed output sink. Opens the host's default output device; the
/// audio callback pulls from a shared ring so [`AudioSink::write`] never
/// blocks.
pub struct CpalSink {
    _stream: cpal::Stream,
    ring: Arc<Mutex<DeviceRing>>,
    info: SinkInfo,
}

// cpal's stream handle exposes an opaque pointer that the compiler cannot
// prove is Send, but moving an active output stream between threads is
// sound on every platform backend we support (the handle itself is what
// ALSA/CoreAudio/WASAPI thread-safety contracts around). The engine's pump
// thread owns the sink, so this Send impl is required and safe here.
// SAFETY: no other type in CpalSink (Mutex<DeviceRing>, SinkInfo) is !Send.
unsafe impl Send for CpalSink {}

impl CpalSink {
    /// Open the default output device (never called from tests).
    pub fn open_default() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| AudioError::Unsupported("no output device available".into()))?;
        Self::for_device(&device)
    }

    /// Open a specific cpal device.
    pub fn for_device(device: &cpal::Device) -> Result<Self> {
        let config = device
            .default_output_config()
            .map_err(|e| AudioError::Unsupported(e.to_string()))?;
        let rate = config.sample_rate().0 as usize;
        let channels = config.channels() as usize;
        let ring = Arc::new(Mutex::new(DeviceRing::new(rate * 2 * 2)));

        let err_fn = |e: cpal::StreamError| {
            tracing::warn!("audio stream error: {e}");
        };

        // The engine always renders 44.1 kHz stereo; devices that cannot
        // take f32 are refused here (the engine resamples on demand in
        // the GUI layer).
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let r = Arc::clone(&ring);
                device
                    .build_output_stream(
                        &config.into(),
                        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                            r.lock().pull(data);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AudioError::Unsupported(e.to_string()))?
            }
            cpal::SampleFormat::I16 => {
                let r = Arc::clone(&ring);
                device
                    .build_output_stream(
                        &config.into(),
                        move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                            let mut tmp = vec![0.0f32; data.len()];
                            r.lock().pull(&mut tmp);
                            for (d, s) in data.iter_mut().zip(tmp) {
                                *d = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                            }
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| AudioError::Unsupported(e.to_string()))?
            }
            other => {
                return Err(AudioError::Unsupported(format!(
                    "unsupported device sample format {other:?}"
                )))
            }
        };
        stream
            .play()
            .map_err(|e| AudioError::Unsupported(e.to_string()))?;

        Ok(Self {
            _stream: stream,
            ring,
            info: SinkInfo {
                name: device.name().unwrap_or_else(|_| "output device".into()),
                sample_rate: rate,
                channels,
                is_device: true,
            },
        })
    }
}

impl AudioSink for CpalSink {
    fn write(&mut self, buf: &[f32]) -> Result<()> {
        self.ring.lock().push(buf);
        Ok(())
    }
    fn set_paused(&mut self, paused: bool) {
        if paused {
            let _ = self._stream.pause();
        } else {
            let _ = self._stream.play();
        }
    }
    fn info(&self) -> SinkInfo {
        self.info.clone()
    }
}

// ---------------------------------------------------------------------------
// Device discovery
// ---------------------------------------------------------------------------

/// Entry point for device selection in the GUI.
pub struct DeviceManager;

impl DeviceManager {
    /// Names of available output devices.
    pub fn output_device_names() -> Vec<String> {
        let host = cpal::default_host();
        host.output_devices()
            .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default()
    }

    /// Open the default output device as a sink.
    pub fn open_default_sink() -> Result<Box<dyn AudioSink>> {
        CpalSink::open_default().map(|s| Box::new(s) as Box<dyn AudioSink>)
    }

    /// Open an output device by display name (as reported by
    /// [`DeviceManager::output_device_names`]).
    pub fn open_named_sink(name: &str) -> Result<Box<dyn AudioSink>> {
        let host = cpal::default_host();
        let device = host
            .output_devices()
            .map_err(|e| AudioError::Unsupported(e.to_string()))?
            .find(|d| d.name().ok().as_deref() == Some(name))
            .ok_or_else(|| AudioError::Unsupported(format!("no such output device: {name}")))?;
        CpalSink::for_device(&device).map(|s| Box::new(s) as Box<dyn AudioSink>)
    }

    /// A headless sink for engine construction without a device.
    pub fn null_sink() -> Box<dyn AudioSink> {
        Box::new(NullSink::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sink_counts_frames() {
        let mut s = NullSink::new();
        s.write(&[0.0, 0.0, 0.0, 0.0]).unwrap();
        assert_eq!(s.frames_written(), 2);
        assert!(!s.info().is_device);
    }

    #[test]
    fn wav_sink_writes_file() {
        let dir = std::env::temp_dir().join("rc_wav_sink_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.wav");
        {
            let mut s = WavSink::create(&path).unwrap();
            s.write(&[0.25, -0.25, 0.5, -0.5]).unwrap();
            s.flush();
            assert_eq!(s.frames_written(), 2);
        }
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, crate::SAMPLE_RATE as u32);
        let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(samples, vec![0.25, -0.25, 0.5, -0.5]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ring_overflow_keeps_capacity() {
        let mut r = DeviceRing::new(4);
        r.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let mut out = [0.0f32; 6];
        r.pull(&mut out);
        // old samples dropped: newest 4 are kept, rest silence
        assert_eq!(&out[0..4], &[3.0, 4.0, 5.0, 6.0][..4][0..4]);
        assert_eq!(&out[4..], &[0.0, 0.0]);
    }

    #[test]
    fn ring_underrun_is_silence() {
        let mut r = DeviceRing::new(8);
        r.push(&[1.0, 2.0]);
        let mut out = [7.0f32; 4];
        r.pull(&mut out);
        assert_eq!(out, [1.0, 2.0, 0.0, 0.0]);
    }
}
