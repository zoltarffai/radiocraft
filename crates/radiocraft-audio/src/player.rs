//! Per-zone playout state machine.
//!
//! One [`Player`] drives one playout zone. It owns the current stream plus
//! an optional cued ("open next") stream, applies fade-in / fade-out
//! envelopes, honours cue-point trimming (VDF "trim to length"), loop
//! regions, pause/resume and remaining-time reporting, and produces
//! [`PlayerEvent`]s the engine reacts to (auto-advance, clean-up).

use std::collections::VecDeque;
use std::time::Duration;


use crate::decoder::{LiveTitle, Source};
use crate::error::Result;
use crate::util::clamp_f32;

/// Events emitted by a player after a render block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEvent {
    /// The current stream reached its end (natural end, VDF target or stop
    /// fade finished).
    Finished,
    /// A cued stream was started because the previous stream ended
    /// (linear segue).
    SegueStarted,
}

/// A request to start playback in a zone.
///
/// Fade values use RadioDJ conventions: `>= 0` = explicit fade in ms,
/// `-1` = no fade (engine decides, e.g. the song's own fade values).
pub struct PlayRequest {
    pub source: Box<dyn Source>,
    pub fade_in_ms: i32,
    pub fade_out_ms: i32,
    /// Absolute start position (seconds); e.g. from `start_at_cue_next`.
    pub start_at_secs: Option<f64>,
    /// VDF target length (seconds): playback is trimmed so total
    /// *sounding* time equals this value.
    pub vdf_target_secs: Option<f64>,
    /// Loop region (start_secs, end_secs).
    pub loop_region_secs: Option<(f64, f64)>,
    /// Per-item trim gain (linear), e.g. `vol_overlay` for overlays.
    pub track_gain_lin: f64,
}

impl PlayRequest {
    pub fn new(source: Box<dyn Source>) -> Self {
        Self {
            source,
            fade_in_ms: 0,
            fade_out_ms: -1,
            start_at_secs: None,
            vdf_target_secs: None,
            loop_region_secs: None,
            track_gain_lin: 1.0,
        }
    }

    pub fn with_fades(mut self, fade_in_ms: i32, fade_out_ms: i32) -> Self {
        self.fade_in_ms = fade_in_ms;
        self.fade_out_ms = fade_out_ms;
        self
    }

    pub fn starting_at(mut self, secs: f64) -> Self {
        self.start_at_secs = Some(secs.max(0.0));
        self
    }

    pub fn vdf_target(mut self, secs: f64) -> Self {
        self.vdf_target_secs = Some(secs.max(0.0));
        self
    }

    pub fn looping(mut self, start_secs: f64, end_secs: f64) -> Self {
        self.loop_region_secs = Some((start_secs, end_secs));
        self
    }

    pub fn with_track_gain_lin(mut self, lin: f64) -> Self {
        self.track_gain_lin = lin.max(0.0);
        self
    }
}

/// One active stream: source + envelope + trimming state.
struct Stream {
    source: Box<dyn Source>,
    rate: f64,
    /// Engine-rate frames played so far (post-seek).
    pos: u64,
    duration: Option<u64>,
    gain: f64,
    fade_in_frames: u64,
    fade_out_frames: u64,
    /// Frame index where the fade-out ramp began (None = not fading).
    fade_out_at: Option<u64>,
    /// Hard end position (frames); playback stops when pos >= end_at.
    end_at: Option<u64>,
    /// VDF trim: fade automatically once this many frames were played.
    vdf_end: Option<u64>,
    loop_region: Option<(u64, u64)>,
    unseekable: bool,
    /// Broadcast metadata cell (internet streams); `None` for files.
    live_title: Option<LiveTitle>,
    scratch: Vec<f32>,
    out: Vec<f32>,
    done: bool,
}

impl Stream {
    fn new(req: PlayRequest, rate: f64) -> Result<Self> {
        let duration = req
            .source
            .duration()
            .map(|d| (d.as_secs_f64() * rate) as u64);
        let live_title = req.source.live_title();
        let mut stream = Self {
            source: req.source,
            rate,
            pos: 0,
            duration,
            gain: req.track_gain_lin,
            fade_in_frames: req.fade_in_ms.max(0) as u64 * rate as u64 / 1000,
            fade_out_frames: req.fade_out_ms.max(0) as u64 * rate as u64 / 1000,
            fade_out_at: None,
            end_at: None,
            vdf_end: None,
            loop_region: None,
            unseekable: false,
            live_title,
            scratch: Vec::new(),
            out: Vec::new(),
            done: false,
        };
        if let Some(secs) = req.start_at_secs {
            stream.seek_to(secs)?;
        }
        if let Some(target) = req.vdf_target_secs {
            let target_frames = (target * rate) as u64;
            stream.vdf_end = Some(stream.pos + target_frames);
        }
        if let Some((s, e)) = req.loop_region_secs {
            let (s, e) = ((s * rate) as u64, (e * rate) as u64);
            if e > s {
                stream.loop_region = Some((s, e));
            }
        }
        Ok(stream)
    }

    fn seek_to(&mut self, secs: f64) -> Result<()> {
        match self.source.seek(secs)? {
            false => {
                self.pos = (secs * self.rate) as u64;
            }
            true => {
                // unseekable: skip frames by draining the source
                self.unseekable = true;
                let mut skip = (secs * self.rate) as u64;
                let mut buf = vec![0.0f32; 4096 * 2];
                while skip > 0 {
                    let want = (skip as usize).min(buf.len() / 2);
                    let got = self.source.read(&mut buf[..want * 2]);
                    if got == 0 {
                        break;
                    }
                    skip -= got as u64;
                    self.pos += got as u64;
                }
            }
        }
        Ok(())
    }

    /// Read `frames` frames into `self.out` (interleaved), applying
    /// envelopes and track gain. Returns frames actually played.
    fn fill(&mut self, frames: usize) -> usize {
        if self.done {
            return 0;
        }
        let start_pos = self.pos;

        // VDF trim: begin the fade-out when the target time is reached.
        if let Some(vdf_end) = self.vdf_end {
            if self.fade_out_at.is_none() && self.fade_out_frames > 0 && start_pos + frames as u64 + 1 >= vdf_end {
                self.fade_out_at = Some(vdf_end.saturating_sub(self.fade_out_frames));
                self.end_at = Some(vdf_end);
            } else if self.fade_out_at.is_none() && start_pos + frames as u64 >= vdf_end {
                self.end_at = Some(vdf_end);
            }
        }

        // Stop at hard end (e.g. stop() fade).
        let allow = match self.end_at {
            Some(e) if start_pos >= e => {
                self.done = true;
                return 0;
            }
            Some(e) => ((e - start_pos) as usize).min(frames),
            None => frames,
        };

        self.scratch.resize(allow * 2, 0.0);
        let got = self.source.read(&mut self.scratch[..allow * 2]);
        if got == 0 {
            self.done = true;
            return 0;
        }
        self.pos += got as u64;

        // Loop region: jump back to loop start on crossing the loop end.
        if let Some((ls, le)) = self.loop_region {
            if self.pos >= le {
                let target = ls as f64 / self.rate;
                match self.source.seek(target) {
                    Ok(false) => {}
                    _ => {
                        self.unseekable = true;
                    }
                }
                self.pos = ls;
            }
        }

        // Envelope
        let fade_in = self.fade_in_frames;
        let fade_out_at = self.fade_out_at.unwrap_or(u64::MAX);
        let fade_out = self.fade_out_frames;
        let gain = self.gain as f32;
        self.out.clear();
        self.out.extend_from_slice(&self.scratch[..got * 2]);
        for (i, frame) in self.out.chunks_mut(2).enumerate() {
            let pos = start_pos + i as u64;
            let mut env = 1.0f32;
            if pos < fade_in {
                env *= pos as f32 / fade_in.max(1) as f32;
            }
            if pos >= fade_out_at && fade_out > 0 {
                let rem = fade_out - (pos - fade_out_at).min(fade_out);
                env *= rem as f32 / fade_out as f32;
            }
            frame[0] = clamp_f32(frame[0] * env * gain, -1.0, 1.0);
            frame[1] = clamp_f32(frame[1] * env * gain, -1.0, 1.0);
        }
        got
    }

    /// Estimated seconds left before this stream ends (-1 = unknown).
    fn remaining_secs(&self) -> f64 {
        let end = match (self.vdf_end, self.end_at, self.duration) {
            (Some(v), _, _) => Some(v),
            (_, Some(e), _) => Some(e),
            (_, _, Some(d)) => Some(d),
            _ => None,
        };
        match end {
            Some(e) if e > self.pos => (e - self.pos) as f64 / self.rate,
            Some(_) => 0.0,
            None => -1.0,
        }
    }
}

/// A playout channel: current stream + optional cued (open-next) stream.
pub struct Player {
    rate: f64,
    current: Option<Stream>,
    /// Cued stream ("open next"); started on segue or manual crossfade.
    pending: Option<Stream>,
    /// Second stream currently overlapping with `current` (crossfade).
    overlap: Option<Stream>,
    paused: bool,
    events: VecDeque<PlayerEvent>,
}

impl Player {
    pub fn new(rate: f64) -> Self {
        Self {
            rate,
            current: None,
            pending: None,
            overlap: None,
            paused: false,
            events: VecDeque::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.current.is_none() && self.pending.is_none() && self.overlap.is_none()
    }

    pub fn is_playing(&self) -> bool {
        self.current.is_some() || self.overlap.is_some()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// Start `req` immediately, replacing anything playing.
    pub fn play(&mut self, req: PlayRequest) -> Result<()> {
        let stream = Stream::new(req, self.rate)?;
        self.current = Some(stream);
        self.pending = None;
        self.overlap = None;
        self.paused = false;
        Ok(())
    }

    /// Cue the next item ("open next"): played automatically with a
    /// segue-crossfade when the current stream ends.
    pub fn cue(&mut self, req: PlayRequest) -> Result<()> {
        self.pending = Some(Stream::new(req, self.rate)?);
        Ok(())
    }

    /// Manually start the cued stream now with a `fade_out_ms` fade of the
    /// current stream (immediate crossfade / segue).
    pub fn start_cued_now(&mut self, fade_out_ms: i32) -> Result<()> {
        if self.pending.is_none() {
            return Ok(());
        }
        let fade_frames = fade_out_ms.max(0) as u64 * self.rate as u64 / 1000;
        if let Some(cur) = self.current.as_mut() {
            if fade_frames > 0 {
                cur.fade_out_at = Some(cur.pos);
                cur.fade_out_frames = fade_frames.max(1);
                cur.end_at = Some(cur.pos + cur.fade_out_frames);
            } else {
                cur.done = true;
            }
        }
        let next = self.pending.take().unwrap();
        self.overlap = Some(next);
        self.events.push_back(PlayerEvent::SegueStarted);
        Ok(())
    }

    /// Stop playback. `fade_out_ms >= 0` fades out first; `end_at` marks
    /// the hard stop position.
    pub fn stop(&mut self, fade_out_ms: i32) {
        self.pending = None;
        if fade_out_ms < 0 {
            self.current = None;
            self.overlap = None;
            self.events.push_back(PlayerEvent::Finished);
            return;
        }
        let fade_frames = (fade_out_ms.max(1)) as u64 * self.rate as u64 / 1000;
        let mut finished_now = true;
        if let Some(cur) = self.current.as_mut() {
            if !cur.done {
                cur.fade_out_at = Some(cur.pos);
                cur.fade_out_frames = fade_frames;
                cur.end_at = Some(cur.pos + fade_frames);
                finished_now = false;
            }
        }
        if let Some(ov) = self.overlap.as_mut() {
            if !ov.done {
                ov.fade_out_at = Some(ov.pos);
                ov.fade_out_frames = fade_frames;
                ov.end_at = Some(ov.pos + fade_frames);
                finished_now = false;
            }
        }
        if finished_now {
            self.current = None;
            self.overlap = None;
        }
    }

    /// Stop at an absolute position (e.g. `cue_end`), with a fade-out
    /// leading into it. Frames after `secs` are not played.
    pub fn stop_at_secs(&mut self, secs: f64, fade_out_ms: i32) -> Result<()> {
        let end = (secs.max(0.0) * self.rate) as u64;
        let fade_frames = fade_out_ms.max(0) as u64 * self.rate as u64 / 1000;
        if let Some(cur) = self.current.as_mut() {
            if end > cur.pos {
                if fade_frames > 0 {
                    cur.fade_out_at = Some(end.saturating_sub(fade_frames));
                    cur.fade_out_frames = fade_frames.max(1);
                }
                cur.end_at = Some(end);
            } else {
                cur.done = true;
            }
        }
        if let Some(ov) = self.overlap.as_mut() {
            if end > ov.pos {
                if fade_frames > 0 {
                    ov.fade_out_at = Some(end.saturating_sub(fade_frames));
                    ov.fade_out_frames = fade_frames.max(1);
                }
                ov.end_at = Some(end);
            } else {
                ov.done = true;
            }
        }
        Ok(())
    }

    /// Seek the current stream (seconds from track start is tracked
    /// internally; cue-point style absolute seek).
    pub fn seek(&mut self, secs: f64) -> Result<()> {
        if let Some(cur) = self.current.as_mut() {
            cur.seek_to(secs)?;
        }
        Ok(())
    }

    /// Position of the current stream in seconds (0 when idle).
    pub fn position_secs(&self) -> f64 {
        self.current.as_ref().map(|c| c.pos as f64 / self.rate).unwrap_or(0.0)
    }

    /// Estimated remaining seconds (-1 = open-ended / unknown).
    pub fn remaining_secs(&self) -> f64 {
        self.current
            .as_ref()
            .and_then(|c| {
                let r = c.remaining_secs();
                (r >= 0.0).then_some(r)
            })
            .unwrap_or(-1.0)
    }

    /// Duration of the current stream when known.
    pub fn duration(&self) -> Option<Duration> {
        self.current
            .as_ref()
            .and_then(|c| c.duration)
            .map(|f| Duration::from_secs_f64(f as f64 / self.rate))
    }

    /// Broadcast metadata cell of whatever is (or is about to be) playing
    /// (internet streams); `None` for files.
    pub fn live_title(&self) -> Option<LiveTitle> {
        self.current
            .as_ref()
            .or(self.pending.as_ref())
            .or(self.overlap.as_ref())
            .and_then(|s| s.live_title.clone())
    }

    /// Render one block of `out.len()/2` frames into `out` (overwrites,
    /// mixing current + overlap streams).
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        if self.paused {
            return;
        }
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }

        // Auto-segue: start the cued stream when the current one is close
        // to its end (crossfade length = the cued stream's fade-in).
        if let (Some(cur), true) = (self.current.as_ref(), self.pending.is_some()) {
            if !cur.done {
                let remaining = cur.remaining_secs();
                if remaining >= 0.0 {
                    // nothing further: the segue fires exactly at the end
                    // (fade overlap handled by the overlap stream below)
                }
            }
        }

        let mut any = false;
        if let Some(cur) = self.current.as_mut() {
            if cur.fill(frames) > 0 {
                accumulate(out, &cur.out);
                any = true;
            } else if cur.done {
                let finished = self.current.take();
                if finished.is_some() {
                    self.events.push_back(PlayerEvent::Finished);
                    // linear segue: promote the cued stream on end
                    if let Some(next) = self.pending.take() {
                        self.overlap = Some(next);
                        self.events.push_back(PlayerEvent::SegueStarted);
                    }
                }
            }
        }
        if let Some(ov) = self.overlap.as_mut() {
            if ov.fill(frames) > 0 {
                accumulate(out, &ov.out);
                any = true;
            } else if ov.done {
                self.overlap = None;
            }
        }
        // If current finished and there is no overlap but a cue was missed
        // (e.g. zero-length streams), promote now.
        if self.current.is_none() && self.overlap.is_none() && self.pending.is_some() && !self.events.contains(&PlayerEvent::SegueStarted) {
            let next = self.pending.take().unwrap();
            self.overlap = Some(next);
            self.events.push_back(PlayerEvent::SegueStarted);
        }
        let _ = any;
    }

    /// Drain queued events (engine consumes them after each render).
    pub fn drain_events(&mut self) -> Vec<PlayerEvent> {
        self.events.drain(..).collect()
    }
}

/// Mix `src` into `dst` with saturating add.
fn accumulate(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d = clamp_f32(*d + *s, -1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::{sine_source, SilentSource};

    const RATE: f64 = 44_100.0;

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
    }

    fn block(frames: usize) -> Vec<f32> {
        vec![0.0f32; frames * 2]
    }

    #[test]
    fn plays_sine_and_finishes() {
        let mut p = Player::new(RATE);
        p.play(PlayRequest::new(Box::new(sine_source(1000.0, 0.5, 0.5, RATE))))
            .unwrap();
        let mut out = block(1024);
        p.render(&mut out);
        assert!(rms(&out) > 0.1);
        assert!(p.is_playing());

        // run to the end of the 0.5 s source
        for _ in 0..30 {
            p.render(&mut block(2048));
        }
        let events = p.drain_events();
        assert!(events.contains(&PlayerEvent::Finished));
        assert!(!p.is_playing());
    }

    #[test]
    fn fade_in_ramps_up() {
        let mut p = Player::new(RATE);
        let req = PlayRequest::new(Box::new(sine_source(1000.0, 1.0, 1.0, RATE))).with_fades(500, -1);
        p.play(req).unwrap();

        let mut early = block(2205); // first 50 ms
        p.render(&mut early);
        let mut late = block(22050); // last ~0.5 s of the fade
        p.render(&mut late);
        assert!(rms(&early) < rms(&late));
        assert!(rms(&early) < 0.2);
    }

    #[test]
    fn stop_with_fade_goes_silent() {
        let mut p = Player::new(RATE);
        p.play(PlayRequest::new(Box::new(sine_source(1000.0, 5.0, 1.0, RATE))))
            .unwrap();
        let mut out = block(1024);
        p.render(&mut out);
        assert!(rms(&out) > 0.1);

        p.stop(500);
        let mut buf = block(0);
        for _ in 0..25 {
            buf.clear();
            buf.resize(2048 * 2, 0.0);
            p.render(&mut buf);
        }
        assert!(rms(&buf) < 0.01, "rms {}", rms(&buf));
        assert!(!p.is_playing());
    }

    #[test]
    fn stop_now_is_immediate() {
        let mut p = Player::new(RATE);
        p.play(PlayRequest::new(Box::new(sine_source(1000.0, 5.0, 1.0, RATE))))
            .unwrap();
        p.stop(-1);
        let mut out = block(1024);
        p.render(&mut out);
        assert_eq!(rms(&out), 0.0);
    }

    #[test]
    fn vdf_trims_to_target() {
        // 10 s source but VDF target of 1 s: the stream must end near 1 s.
        let mut p = Player::new(RATE);
        let req = PlayRequest::new(Box::new(sine_source(1000.0, 10.0, 1.0, RATE)))
            .with_fades(0, 200)
            .vdf_target(1.0);
        p.play(req).unwrap();
        let mut total = 0.0f64;
        let mut played_any = false;
        for _ in 0..100 {
            let mut buf = block(2048);
            p.render(&mut buf);
            if rms(&buf) > 0.0 {
                played_any = true;
                total += 2048.0 / RATE;
            } else {
                break;
            }
        }
        assert!(played_any);
        assert!((1.0..1.4).contains(&total), "played {total}s");
        let events = p.drain_events();
        assert!(events.contains(&PlayerEvent::Finished));
    }

    #[test]
    fn pause_resume_holds_position() {
        let mut p = Player::new(RATE);
        p.play(PlayRequest::new(Box::new(SilentSource::new()))).unwrap();
        let mut out = block(1024);
        p.render(&mut out);
        let pos = p.position_secs();
        p.pause();
        for _ in 0..5 {
            p.render(&mut out);
        }
        assert!((p.position_secs() - pos).abs() < 1e-6);
        p.resume();
        p.render(&mut out);
        assert!(p.position_secs() > pos);
    }

    #[test]
    fn cue_segue_starts_next_on_end() {
        let mut p = Player::new(RATE);
        p.play(PlayRequest::new(Box::new(sine_source(440.0, 0.1, 0.5, RATE))))
            .unwrap();
        p.cue(PlayRequest::new(Box::new(sine_source(880.0, 1.0, 0.5, RATE))))
            .unwrap();
        for _ in 0..10 {
            p.render(&mut block(1024));
        }
        let events = p.drain_events();
        assert!(events.contains(&PlayerEvent::Finished));
        assert!(events.contains(&PlayerEvent::SegueStarted));
        let mut out = block(1024);
        p.render(&mut out);
        assert!(rms(&out) > 0.1);
    }

    #[test]
    fn start_cued_now_crossfades() {
        let mut p = Player::new(RATE);
        p.play(PlayRequest::new(Box::new(sine_source(440.0, 5.0, 1.0, RATE))))
            .unwrap();
        p.cue(PlayRequest::new(Box::new(sine_source(880.0, 5.0, 1.0, RATE))).with_fades(10, -1))
            .unwrap();
        p.start_cued_now(300).unwrap();
        let mut loud = block(1024);
        p.render(&mut loud);
        assert!(rms(&loud) > 0.3); // overlap of two streams
        for _ in 0..10 {
            p.render(&mut block(2048));
        }
        // after the fade the first stream is gone; second keeps playing
        let mut out = block(2048);
        p.render(&mut out);
        assert!(rms(&out) > 0.3);
    }

    #[test]
    fn loop_region_loops() {
        let mut p = Player::new(RATE);
        // 0.2 s source looping between 0.05 and 0.15 s
        let req = PlayRequest::new(Box::new(sine_source(1000.0, 0.2, 1.0, RATE))).looping(0.05, 0.15);
        p.play(req).unwrap();
        for _ in 0..20 {
            let mut out = block(1024);
            p.render(&mut out);
        }
        assert!(p.is_playing());
        assert!(p.position_secs() < 0.15);
    }

    #[test]
    fn remaining_and_position() {
        let mut p = Player::new(RATE);
        p.play(PlayRequest::new(Box::new(sine_source(1000.0, 2.0, 1.0, RATE))))
            .unwrap();
        let mut out = block(1024);
        p.render(&mut out);
        let rem = p.remaining_secs();
        assert!(rem > 1.9 && rem < 2.05, "rem {rem}");
        let _ = p.duration();
    }
}
