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
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use crate::error::{AudioError, Result};
use crate::util::StreamResampler;
use crate::SAMPLE_RATE;

/// A live title cell shared between a decoding [`StreamSource`] (audio
/// thread) and whoever polls ICY metadata changes (server/UI). The version
/// counter lets pollers ask "anything new since I last looked?" without
/// ever touching the player state.
#[derive(Clone, Default)]
pub struct LiveTitle {
    cell: std::sync::Arc<std::sync::Mutex<(u64, Option<String>)>>,
}

impl LiveTitle {
    /// Publish a new title; bumps the version.
    pub fn update(&self, title: String) {
        let mut cell = self.cell.lock().expect("live title cell");
        cell.0 += 1;
        cell.1 = Some(title);
    }

    /// `(version, title)` when the newest title is newer than `seen`
    /// (`0` = poll from the start), else `None`.
    pub fn changed_since(&self, seen: u64) -> Option<(u64, String)> {
        let cell = self.cell.lock().expect("live title cell");
        if cell.0 > seen {
            cell.1.clone().map(|t| (cell.0, t))
        } else {
            None
        }
    }
}

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

    /// Shared live-title cell for sources that carry broadcast metadata
    /// (internet streams); `None` for plain files.
    fn live_title(&self) -> Option<LiveTitle> {
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
            for chunk in buf[produced * 2..produced * 2 + take * 2]
                .as_chunks_mut::<2>()
                .0
            {
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

    fn duration(&self) -> Option<std::time::Duration> {
        self.duration
    }
}

// ---------------------------------------------------------------------------
// StreamSource (HTTP internet streams)
// ---------------------------------------------------------------------------

/// True for paths that name an HTTP(S) internet stream rather than a
/// local file (RadioDJ stores streams as `songs` rows with `song_type = 5`
/// and the URL in `path`).
pub fn is_stream_path(path: &str) -> bool {
    let p = path.trim();
    p.starts_with("http://") || p.starts_with("https://")
}

/// Map a shoutcast/icecast `Content-Type` to a symphonia probe extension so
/// the raw body can be probed without a container-extension hint.
fn mime_extension(content_type: &str) -> Option<&'static str> {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match ct.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/x-mp3" | "audio/mpegurl" => "mp3",
        "audio/aac" | "audio/aacp" | "audio/x-aac" => "aac",
        "audio/ogg" | "application/ogg" | "audio/opus" => "ogg",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        _ => return None,
    })
}

/// One decoded symphonia pipeline bound to a media source stream.
struct Pipeline {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    source_rate: u32,
    duration: Option<Duration>,
}

fn build_pipeline(
    mss: MediaSourceStream,
    hint_ext: Option<&str>,
    label: &str,
) -> Result<Pipeline> {
    let mut hint = Hint::new();
    if let Some(ext) = hint_ext {
        hint.with_extension(ext);
    }
    let probe = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| AudioError::Decode(format!("{label}: {e}")))?;
    let reader = probe.format;
    let track = reader
        .default_track()
        .ok_or_else(|| AudioError::Unsupported(format!("{label}: no audio track")))?;
    let params = track.codec_params.clone();
    let track_id = track.id;
    if params.codec == CODEC_TYPE_NULL {
        return Err(AudioError::Unsupported(format!("{label}: unknown codec")));
    }
    let decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions { verify: false })
        .map_err(|e| AudioError::Unsupported(format!("{label}: {e}")))?;
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
    Ok(Pipeline {
        reader,
        decoder,
        track_id,
        source_rate: params.sample_rate.unwrap_or(SAMPLE_RATE as u32),
        duration,
    })
}

/// An HTTP internet radio stream (Icecast/Shoutcast MP3/AAC/OGG…) decoded
/// through symphonia, mirroring RadioDJ's BassNet stream path.
///
/// Semantics that make it playout-safe:
/// * Unseekable and endless: `seek` reports "cannot seek", `duration` is
///   always `None`, so no VDF/stop-at plan is ever derived from it.
/// * Automatic reconnect: a dropped or stalled connection re-opens the URL
///   and keeps the playout position (`error_policy: Reconnect`, the default,
///   as long as `open()` keeps succeeding). Only a source that can never be
///   opened at all reports EOF so the engine can fall back to AutoDJ.
/// * Live metadata: `icy-name`/`icy-description` response headers and MP3
///   `StreamTitle` tags surface through [`Source::live_title`], the
///   equivalent of BASS's `BASS_TAG_GetFromURL` in RadioDJ.
/// * TLS interop: rustls handles the modern side of the internet, but legacy
///   Shoutcast DNAS hosts (e.g. voscast) offer only static-RSA TLS 1.2
///   suites, which rustls deliberately does not implement. On a TLS
///   handshake rejection the source re-opens once through OpenSSL
///   (`native-tls`, feature `openssl-fallback`, on by default) — what
///   BASS/SCHANNEL do in RadioDJ — and sticks with it for reconnects.
pub struct StreamSource {
    url: String,
    inner: Pipeline,
    resampler: StreamResampler,
    resampling: bool,
    flushed: bool,
    scratch: Vec<f32>,
    pending: std::collections::VecDeque<f32>,
    dead: bool,
    failed: bool,
    reconnects: u32,
    ever_decoded: bool,
    tls_openssl: bool,
    title: LiveTitle,
    last_meta: Option<String>,
}

impl StreamSource {
    /// Open the stream; blocks while the HTTP response header is fetched.
    pub fn open(url: &str) -> Result<Self> {
        let title = LiveTitle::default();
        let (inner, tls_openssl) = Self::open_pipeline(url, None, &title, false)?;
        let source_rate = inner.source_rate;
        Ok(StreamSource {
            url: url.to_string(),
            inner,
            resampler: StreamResampler::new(source_rate as usize, SAMPLE_RATE, 2),
            resampling: source_rate != SAMPLE_RATE as u32,
            flushed: false,
            scratch: Vec::new(),
            pending: std::collections::VecDeque::new(),
            dead: false,
            failed: false,
            reconnects: 0,
            ever_decoded: false,
            tls_openssl,
            title,
            last_meta: None,
        })
    }

    /// Re-open the URL and rebuild the decode pipeline; the previous stream
    /// is dropped, which closes its socket.
    fn reopen(&mut self) -> bool {
        match Self::open_pipeline(&self.url, self.content_hint(), &self.title, self.tls_openssl) {
            Ok((inner, tls_openssl)) => {
                self.tls_openssl = tls_openssl;
                if inner.source_rate as usize != self.resampler.in_rate() {
                    self.resampler =
                        StreamResampler::new(inner.source_rate as usize, SAMPLE_RATE, 2);
                    self.resampling = inner.source_rate != SAMPLE_RATE as u32;
                }
                self.inner = inner;
                self.flushed = false;
                self.scratch.clear();
                self.pending.clear();
                self.reconnects += 1;
                tracing::info!("stream {} reconnected (#{})", self.url, self.reconnects);
                true
            }
            Err(e) => {
                tracing::warn!("stream {} reconnect failed: {e}", self.url);
                false
            }
        }
    }

    /// Remember the content type so reconnects probe quickly; the URL path
    /// (`…/stream.mp3`) is used as a fallback hint.
    fn content_hint(&self) -> Option<String> {
        first_hint(&self.url)
    }

    /// Issue the GET for the stream body. The rustls agent is the default;
    /// the `openssl` variant links OpenSSL through `native-tls`, which still
    /// negotiates the legacy static-RSA TLS 1.2 suites that Shoutcast DNAS
    /// hosts serve (rustls implements ECDHE suites only).
    fn fetch(
        url: &str,
        openssl: bool,
    ) -> std::result::Result<ureq::Response, Box<ureq::Error>> {
        let builder = ureq::AgentBuilder::new().try_proxy_from_env(true);
        #[cfg(feature = "openssl-fallback")]
        let mut builder = builder;
        #[cfg(feature = "openssl-fallback")]
        if openssl {
            match ureq::native_tls::TlsConnector::builder().build() {
                Ok(connector) => builder = builder.tls_connector(std::sync::Arc::new(connector)),
                Err(e) => tracing::warn!("could not build OpenSSL TLS connector: {e}"),
            }
        }
        #[cfg(not(feature = "openssl-fallback"))]
        let _ = openssl;
        builder
            .build()
            .get(url)
            .set("User-Agent", "Radiocraft/1.0 (stream client)")
            .set("Icy-MetaData", "1")
            .call()
            .map_err(Box::new)
    }

    /// Does this transport error look like a TLS negotiation failure (rather
    /// than DNS / connection refused / a hang)? Only those are worth retrying
    /// through a different TLS stack.
    fn looks_like_tls_failure(err: &ureq::Error) -> bool {
        let msg = err.to_string().to_lowercase();
        msg.contains("tls") || msg.contains("handshake") || msg.contains("alert")
    }

    /// Open the URL and build the decode pipeline. Returns the pipeline and
    /// whether the (possibly fallen-back) TLS stack in use was OpenSSL, so
    /// reconnects skip straight to the stack that works.
    fn open_pipeline(
        url: &str,
        hint_ext: Option<String>,
        title: &LiveTitle,
        prefer_openssl: bool,
    ) -> Result<(Pipeline, bool)> {
        let mut openssl = prefer_openssl;
        let resp = match Self::fetch(url, openssl) {
            Ok(resp) => resp,
            Err(e) => {
                if openssl
                    || !cfg!(feature = "openssl-fallback")
                    || !Self::looks_like_tls_failure(&e)
                {
                    return Err(AudioError::Decode(format!("{url}: {e}")));
                }
                tracing::info!("stream {url}: host rejected rustls, retrying via OpenSSL");
                openssl = true;
                Self::fetch(url, true).map_err(|e2| {
                    AudioError::Decode(format!(
                        "{url}: openssl fallback also failed: {e2} (rustls: {e})"
                    ))
                })?
            }
        };
        let status = resp.status();
        if !(200..300).contains(&status) {
            return Err(AudioError::Decode(format!("{url}: HTTP status {status}")));
        }
        let header = |name: &str| {
            resp.header(name).map(str::to_owned).filter(|v| !v.trim().is_empty())
        };
        if let Some(name) = header("icy-name").or_else(|| header("icy-description")) {
            title.update(name);
        }
        let content_type = header("Content-Type").unwrap_or_default();
        let ext = hint_ext
            .or_else(|| mime_extension(&content_type).map(str::to_owned))
            .or_else(|| first_hint(url));
        let reader: Box<dyn std::io::Read + Send + Sync> = resp.into_reader();
        let mss = MediaSourceStream::new(Box::new(ReadOnlySource::new(reader)), Default::default());
        let mut pipeline = build_pipeline(mss, ext.as_deref(), url)?;
        // A live stream is endless: even if the server's first body chunk
        // lets symphonia estimate a length, never let it seed a VDF/stop-at
        // plan that would end playout early.
        pipeline.duration = None;
        Ok((pipeline, openssl))
    }

    /// Decode packets, re-opening the URL when the connection drops. A live
    /// broadcast normally never ends its body; symphonia reports a closed
    /// connection as a read/decode error, which we treat as "Failed".
    fn pump(&mut self) -> bool {
        loop {
            match self.read_inner() {
                ReadOutcome::Have => return true,
                ReadOutcome::Failed => {
                    if self.open_fails() {
                        return false;
                    }
                }
            }
        }
    }

    fn open_fails(&mut self) -> bool {
        // Never retry a source that never delivered a single sample.
        if !self.ever_decoded && self.reconnects == 0 && self.pending.is_empty() {
            self.failed = true;
            return true;
        }
        if !self.reopen() {
            self.dead = true;
        }
        false
    }

    /// Scan the reader's latest metadata revision for a live title.
    fn current_title(&mut self) -> Option<String> {
        let mut log = self.inner.reader.metadata();
        log.skip_to_latest().and_then(|rev| stream_title_of(rev.tags()))
    }

    fn read_inner(&mut self) -> ReadOutcome {
        loop {
            let packet = match self.inner.reader.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(_) => return ReadOutcome::Failed,
            };
            if packet.track_id() != self.inner.track_id {
                continue;
            }
            if let Some(t) = self.current_title() {
                if Some(&t) != self.last_meta.as_ref() {
                    self.last_meta = Some(t.clone());
                    self.title.update(t);
                }
            }
            match self.inner.decoder.decode(&packet) {
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
                        self.ever_decoded = true;
                        return ReadOutcome::Have;
                    }
                    // otherwise keep decoding more packets
                }
                // Corrupt packets are normal on flaky streams: skip them.
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(_) => return ReadOutcome::Failed,
            }
        }
    }
}

enum ReadOutcome {
    Have,
    Failed,
}

/// Pull a Shoutcast v2 `StreamTitle`-style tag out of the reader's current
/// metadata revision. LAME/icecast servers write these as ID3/`TIT1` extra
/// tags; symphonia surfaces them under a `type_` like `TIT1` or
/// `StreamTitle`, and standard `Title` tags also qualify.
fn stream_title_of(tags: &[symphonia::core::meta::Tag]) -> Option<String> {
    use symphonia::core::meta::Value;
    for t in tags {
        let key = t
            .std_key
            .map(|k| format!("{k:?}"))
            .unwrap_or_else(|| t.key.to_string());
        if !(key.contains("TITLE") || key.contains("Title") || key.contains("StreamTitle")) {
            continue;
        }
        if let Value::String(s) = &t.value {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Extension hint from the URL path (`…/live.mp3` → "mp3").
fn first_hint(url: &str) -> Option<String> {
    let path = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let ext = path.rsplit('/').next()?.rsplit('.').next()?;
    if (2..=4).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(ext.to_ascii_lowercase())
    } else {
        None
    }
}

impl Source for StreamSource {
    fn read(&mut self, buf: &mut [f32]) -> usize {
        while self.pending.is_empty() {
            if self.dead || self.failed || !self.pump() {
                if self.resampling && !self.flushed {
                    self.flushed = true;
                    self.scratch.clear();
                    self.resampler.flush(&mut self.scratch);
                    self.pending.extend(self.scratch.iter());
                    if !self.pending.is_empty() {
                        break;
                    }
                }
                return 0;
            }
        }
        let frames = (buf.len() / 2).min(self.pending.len() / 2);
        for chunk in buf[..frames * 2].chunks_mut(2) {
            chunk[0] = self.pending.pop_front().expect("pending had a value");
            chunk[1] = self.pending.pop_front().expect("pending had a value");
        }
        frames
    }

    /// Live streams cannot seek.
    fn seek(&mut self, _seconds: f64) -> Result<bool> {
        Ok(true)
    }

    fn duration(&self) -> Option<Duration> {
        // Streams are endless; open_pipeline forces this field to None.
        self.inner.duration
    }

    fn live_title(&self) -> Option<LiveTitle> {
        Some(self.title.clone())
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
