//! # radiocraft-audio
//!
//! Clean-room re-implementation of the RadioDJ playout audio engine.
//!
//! The crate is split into small modules that each cover one part of the
//! playout pipeline:
//!
//! - [`decoder`] — transparent file decoding (symphonia) behind a uniform
//!   interleaved-stereo-f32-at-44.1-kHz [`Source`](decoder::Source) trait,
//!   plus in-memory / silent test sources.
//! - [`player`] — per-zone playback state machine: cue points, fades,
//!   pause/resume, VDF trim-to-target, loop regions and crossfades.
//! - [`mixer`] — four playout zones (main / aux / cart / instant) with
//!   faders, mute and program ducking (`vol_onsweeper`, `vol_overlay`,
//!   `vol_vt`) plus master output gain.
//! - [`fx`] — the `Plugin_SoundProcessing` chain (10-band EQ, compressor,
//!   limiter, output trim) and voice-track assist thresholds.
//! - [`loudness`] — ITU-R BS.1770-4 K-weighting / gated integrated loudness
//!   and 4x-oversampled true-peak meters (feeds the `bs1770` and `loudness`
//!   library columns).
//! - [`levels`] — peak / VU meters and a combined level tracker for meters
//!   and voice-track assist.
//! - [`waveform`] — peak-window waveform data with a `.wfx`-style binary
//!   sidecar cache and TTL memory cache.
//! - [`device`] — output sinks behind a thin trait: null sink (tests /
//!   headless), WAV file sink (hound) and a cpal-backed device sink that is
//!   only opened on demand.
//! - [`engine`] — [`PlayoutEngine`](engine::PlayoutEngine) wiring players,
//!   mixer, FX, recorder, levels and waveform cache together with
//!   `parking_lot` interior mutability so the GUI and the audio pump thread
//!   can share one instance.
//!
//! Everything is testable without an audio device: the device layer sits
//! behind [`AudioSink`](device::AudioSink) and the engine renders into plain
//! `f32` buffers that tests can inspect directly.

pub mod cuepoints;
pub mod decoder;
pub mod device;
pub mod engine;
pub mod error;
pub mod fx;
pub mod levels;
pub mod loudness;
pub mod mixer;
pub mod player;
pub mod util;
pub mod waveform;

/// The one internal sample rate every component in this crate runs at.
/// Sources are resampled on decode so the mix bus is always 44.1 kHz.
pub const SAMPLE_RATE: usize = 44_100;

/// Internal mix-bps sample format helper (interleaved stereo f32).
pub const CHANNELS: usize = 2;

// Convenience re-exports — the surface the server/engine links against.
pub use decoder::{
    FileSource, LiveTitle, MemorySource, SilentSource, Source, StreamSource, is_stream_path,
    sine_source,
};
pub use device::{
    AudioSink, CpalSink, DeviceManager, NullSink, SinkInfo, WavSink,
};
pub use engine::{EngineConfig, PlayoutEngine};
pub use error::{AudioError, Result};
pub use fx::{
    Compressor, DampPreset, Limiter, ParametricEq, ProcessorConfig, SoundProcessor,
    VoiceAssistConfig, VoiceTrackAssist,
};
pub use levels::{LevelTracker, PeakMeter, VuMeter};
pub use loudness::{
    integrated_lufs, scan, true_peak_dbtp, KWeighting, LoudnessMeter,
    LoudnessScan, TruePeakMeter,
};
pub use mixer::{ChannelStrip, Mixer, Zone};
pub use player::{PlayRequest, Player, PlayerEvent};
pub use waveform::{WaveformCache, WaveformData};
