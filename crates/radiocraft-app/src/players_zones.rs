//! Players/zones panels: one strip per mixer zone (main / aux / cartwall /
//! instant), with level faders, cue fields, "load file" and a waveform
//! rendered straight onto the screen from cached peaks.
//!
//! The playlist-row-to-engine mapping (`CuePlan`) is pure: RadioDJ stores
//! cue and fade values in `playlists.cstart/cnext/cend/fin/fout` and a zero
//! there means "not set, let the engine decide". The mapping below turns
//! those sentinels into `Option`s and feeds `PlayRequest` builder calls.

use std::path::{Path, PathBuf};
use std::time::Duration;

use radiocraft_audio::decoder::{FileSource, Source};
use radiocraft_audio::engine::PlayoutEngine;
use radiocraft_audio::mixer::{ALL_ZONES, Zone};
use radiocraft_audio::player::PlayRequest;
use radiocraft_audio::waveform::{WaveformCache, WaveformData};
use radiocraft_core::template::format_hms;
use radiocraft_db::entities::NewPlaylistItem;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::state::AppState;

/// The linear player always lives on the main zone.
pub fn main_zone() -> Zone {
    Zone::Main
}

/// Window label for a zone (RadioDJ names the strips exactly this way).
pub fn zone_label(zone: Zone) -> &'static str {
    match zone {
        Zone::Main => "Main",
        Zone::Aux => "Aux",
        Zone::Cart => "Cartwall",
        Zone::Instant => "Instant",
    }
}

/// Engine-side knobs extracted from one playlist row.
///
/// `None` everywhere means "the stored value was the zero sentinel": the
/// engine falls back to its defaults (start at 0, no VDF trim, full file,
/// configured fade length).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CuePlan {
    /// `cstart`: absolute position the stream starts at.
    pub start_at: Option<f64>,
    /// `cnext`: VDF target - total sounding time.
    pub vdf_target: Option<f64>,
    /// `cend`: position in the file where the item stops sounding.
    pub stop_at: Option<f64>,
    /// `fin`: fade-in length in milliseconds.
    pub fade_in_ms: Option<i32>,
    /// `fout`: fade-out (and stop) length in milliseconds.
    pub fade_out_ms: Option<i32>,
}

/// Zero in the database means "not set".
fn non_zero_secs(value: Decimal) -> Option<f64> {
    if value.is_zero() {
        None
    } else {
        value.to_f64()
    }
}

fn non_zero_ms(value: Decimal) -> Option<i32> {
    if value.is_zero() {
        None
    } else {
        value.to_i32()
    }
}

/// Map one playlist row's cue columns onto engine knobs.
pub fn cue_plan(item: &NewPlaylistItem) -> CuePlan {
    CuePlan {
        start_at: non_zero_secs(item.cstart),
        vdf_target: non_zero_secs(item.cnext),
        stop_at: non_zero_secs(item.cend),
        fade_in_ms: non_zero_ms(item.fin),
        fade_out_ms: non_zero_ms(item.fout),
    }
}

/// Cue/fade values as the zone panel shows them (0 = sentinel again).
pub fn cue_plan_from_fields(
    start: f64,
    next: f64,
    end: f64,
    fade_in_ms: i32,
    fade_out_ms: i32,
) -> CuePlan {
    CuePlan {
        start_at: if start > 0.0 { Some(start) } else { None },
        vdf_target: if next > 0.0 { Some(next) } else { None },
        stop_at: if end > 0.0 { Some(end) } else { None },
        fade_in_ms: if fade_in_ms > 0 { Some(fade_in_ms) } else { None },
        fade_out_ms: if fade_out_ms > 0 { Some(fade_out_ms) } else { None },
    }
}

impl CuePlan {
    /// Apply the cue knobs to a fresh `PlayRequest`.
    ///
    /// Fades go through `with_fades`; a missing side passes `-1`, the
    /// engine's "use my default" marker.
    pub fn apply(&self, req: PlayRequest) -> PlayRequest {
        let mut req = req;
        match (self.fade_in_ms, self.fade_out_ms) {
            (Some(fin), Some(fout)) => req = req.with_fades(fin, fout),
            (Some(fin), None) => req = req.with_fades(fin, -1),
            (None, Some(fout)) => req = req.with_fades(-1, fout),
            (None, None) => {}
        }
        if let Some(secs) = self.start_at {
            req = req.starting_at(secs);
        }
        if let Some(secs) = self.vdf_target {
            req = req.vdf_target(secs);
        }
        req
    }

    /// The stop position to schedule once playback is running
    /// (`cend` -> `PlayoutEngine::stop_at`).
    pub fn stop_at_secs(&self) -> Option<f64> {
        self.stop_at
    }
}

/// One zone's strip: mix controls, cue fields and last-known playback state.
#[derive(Debug, Clone)]
pub struct ZonePanel {
    pub zone: Zone,
    /// Display name of the loaded item (file stem or song title).
    pub track: Option<String>,
    /// On-disk file feeding the waveform display.
    pub track_path: Option<PathBuf>,
    pub playing: bool,
    pub position_secs: f64,
    pub duration_secs: f64,
    /// Zone fader, dB (0 = unity, as the mixer expects).
    pub fader_db: f64,
    pub muted: bool,
    /// Editable cue fields; 0 = "not set".
    pub cue_start: f64,
    pub cue_next: f64,
    pub cue_end: f64,
    pub fade_in_ms: i32,
    pub fade_out_ms: i32,
}

impl ZonePanel {
    pub fn new(zone: Zone) -> Self {
        ZonePanel {
            zone,
            track: None,
            track_path: None,
            playing: false,
            position_secs: 0.0,
            duration_secs: 0.0,
            fader_db: 0.0,
            muted: false,
            cue_start: 0.0,
            cue_next: 0.0,
            cue_end: 0.0,
            fade_in_ms: 0,
            fade_out_ms: 0,
        }
    }

    pub fn cue_plan(&self) -> CuePlan {
        cue_plan_from_fields(
            self.cue_start,
            self.cue_next,
            self.cue_end,
            self.fade_in_ms,
            self.fade_out_ms,
        )
    }

    /// Pull a live cue plan back into the editable fields.
    pub fn set_cue_plan(&mut self, plan: CuePlan) {
        self.cue_start = plan.start_at.unwrap_or(0.0);
        self.cue_next = plan.vdf_target.unwrap_or(0.0);
        self.cue_end = plan.stop_at.unwrap_or(0.0);
        self.fade_in_ms = plan.fade_in_ms.unwrap_or(0);
        self.fade_out_ms = plan.fade_out_ms.unwrap_or(0);
    }

    /// Seconds (start, end) of the segment that will actually sound.
    pub fn sounding_range(&self) -> (f64, f64) {
        let start = self.cue_start.max(0.0);
        let end = if self.cue_end > start {
            self.cue_end
        } else {
            self.duration_secs.max(start)
        };
        (start, end)
    }
}

/// A file action requested by a zone row, collected during `draw` and
/// applied afterwards so panel borrow and dialog code stay apart.
enum ZoneAction {
    Load(Zone),
    Play(Zone),
    Stop(Zone),
}

/// Start one file on `zone` honouring the cue plan.
///
/// Returns the decoded duration on success so the panel can show it.
pub fn start_file(
    state: &mut AppState,
    zone: Zone,
    path: &Path,
    plan: CuePlan,
) -> anyhow::Result<f64> {
    let source = FileSource::open(path)?;
    let duration = source
        .duration()
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let req = plan.apply(PlayRequest::new(Box::new(source)));
    state.engine.play(zone, req, 0, false)?;
    if let Some(secs) = plan.stop_at_secs() {
        state.engine.stop_at(zone, secs, -1)?;
    }
    Ok(duration)
}

/// The players window: one panel per zone plus the shared waveform cache.
pub struct PlayersUi {
    pub panels: Vec<ZonePanel>,
    pub wave_cache: WaveformCache,
    pub open: bool,
    /// Pixels of waveform shown per zone strip.
    pub wave_width: usize,
}

impl Default for PlayersUi {
    fn default() -> Self {
        PlayersUi {
            panels: ALL_ZONES.iter().map(|z| ZonePanel::new(*z)).collect(),
            wave_cache: WaveformCache::new(Duration::from_secs(300), 32),
            open: false,
            wave_width: 600,
        }
    }
}

/// Waveform peaks resampled to `width` buckets, (low, high) per column.
pub fn waveform_points(data: &WaveformData, width: usize) -> Vec<(f32, f32)> {
    data.render(width)
}

/// Waveform for `path`: cache first, then the sidecar, then a decode.
pub fn waveform_for(
    cache: &mut WaveformCache,
    engine: &PlayoutEngine,
    path: &Path,
) -> Option<WaveformData> {
    if let Some(cached) = cache.peek(path) {
        return Some(cached.clone());
    }
    let data = match WaveformData::load_for(path) {
        Ok(Some(data)) => Some(data),
        _ => engine.open_waveform(path).ok(),
    }?;
    cache.put(path, data.clone());
    Some(data)
}

/// Mirror the engine's live state into every panel (called once per frame).
pub fn sync_panels(panels: &mut [ZonePanel], engine: &PlayoutEngine) {
    for panel in panels.iter_mut() {
        panel.playing = engine.is_playing(panel.zone);
        panel.position_secs = engine.position_secs(panel.zone).unwrap_or(0.0);
    }
}

/// Push one panel's mix controls to the engine.
pub fn apply_mix(panel: &ZonePanel, engine: &PlayoutEngine) {
    engine.set_zone_fader_db(panel.zone, panel.fader_db);
    engine.set_zone_muted(panel.zone, panel.muted);
}

/// File-picker for "load file" on a zone; updates `LastUsedPath` too.
pub fn pick_file(state: &mut AppState) -> Option<PathBuf> {
    let root = state.last_used_path();
    let picked = rfd::FileDialog::new()
        .set_directory(&root)
        .add_filter(
            "Audio",
            &["mp3", "flac", "ogg", "wav", "m4a", "mp4", "aac"],
        )
        .pick_file()?;
    let parent = picked.parent().unwrap_or(&root).to_path_buf();
    state.general.set_last_used_path(parent.display().to_string());
    if let Err(err) = state.save_general() {
        tracing::warn!("saving LastUsedPath: {err:#}");
    }
    Some(picked)
}

impl PlayersUi {
    pub fn panel(&self, zone: Zone) -> &ZonePanel {
        let index = ALL_ZONES.iter().position(|z| *z == zone).unwrap_or(0);
        &self.panels[index]
    }

    pub fn panel_mut(&mut self, zone: Zone) -> &mut ZonePanel {
        let index = ALL_ZONES.iter().position(|z| *z == zone).unwrap_or(0);
        &mut self.panels[index]
    }

    /// Load and play `path` on `zone`, refreshing the panel.
    pub fn load_and_play(&mut self, state: &mut AppState, zone: Zone, path: &Path) {
        let plan = self.panel(zone).cue_plan();
        match start_file(state, zone, path, plan) {
            Ok(duration) => {
                let panel = self.panel_mut(zone);
                panel.track = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .or_else(|| Some(path.display().to_string()));
                panel.track_path = Some(path.to_path_buf());
                panel.duration_secs = duration;
                panel.position_secs = 0.0;
                panel.playing = true;
                let name = panel.track.clone().unwrap_or_default();
                state.set_status(format!("{name} playing on {}", zone_label(zone)));
            }
            Err(err) => state.report_error("starting the file", &err),
        }
    }

    /// Stop one zone with the engine's default fade.
    pub fn stop_zone(&mut self, state: &mut AppState, zone: Zone) {
        state.engine.stop(zone, -1);
        self.panel_mut(zone).playing = false;
        state.set_status(format!("{} stopped", zone_label(zone)));
    }

    /// Stop every zone (the Stop button).
    pub fn stop_all(&mut self, state: &mut AppState) {
        state.engine.stop_all(-1);
        for panel in self.panels.iter_mut() {
            panel.playing = false;
        }
        state.set_status("All zones stopped");
    }

    /// The egui players window.
    pub fn draw(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if !self.open {
            return;
        }
        sync_panels(&mut self.panels, &state.engine);
        let mut actions: Vec<ZoneAction> = Vec::new();
        let mut mix_dirty = false;
        let width = self.wave_width;

        egui::Window::new("Players and zones")
            .open(&mut self.open)
            .resizable(true)
            .show(ctx, |ui| {
                for index in 0..self.panels.len() {
                    let zone = self.panels[index].zone;
                    let panel = self.panels[index].clone();
                    let (wave, points) = match panel.track_path.as_ref() {
                        Some(path) => {
                            let data = waveform_for(&mut self.wave_cache, &state.engine, path);
                            let pts = data
                                .as_ref()
                                .map(|d| waveform_points(d, width.max(1)))
                                .unwrap_or_default();
                            (path.display().to_string(), pts)
                        }
                        None => (String::new(), Vec::new()),
                    };
                    let _ = wave;

                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.strong(zone_label(zone));
                            let badge = if panel.playing { "playing" } else { "idle" };
                            ui.label(format!(
                                "{badge} {} / {}",
                                format_hms(panel.position_secs),
                                format_hms(panel.duration_secs)
                            ));
                            if let Some(track) = &panel.track {
                                ui.label(track.as_str());
                            }
                        });
                        if !points.is_empty() {
                            let (rect, _response) =
                                ui.allocate_exact_size(egui::vec2(width as f32, 60.0), egui::Sense::hover());
                            let mid = rect.center().y;
                            let scale = rect.height() / 2.0;
                            for (i, (low, high)) in points.iter().enumerate() {
                                let x = rect.left() + (rect.width() * i as f32) / width as f32;
                                let line = egui::Shape::line(
                                    vec![
                                        egui::pos2(x, mid - high * scale),
                                        egui::pos2(x, mid - low * scale),
                                    ],
                                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(70, 130, 230)),
                                );
                                ui.painter().add(line);
                            }
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Load file").clicked() {
                                actions.push(ZoneAction::Load(zone));
                            }
                            if ui.button("Play").clicked() {
                                actions.push(ZoneAction::Play(zone));
                            }
                            if ui.button("Stop").clicked() {
                                actions.push(ZoneAction::Stop(zone));
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Start");
                            if ui
                                .add(egui::DragValue::new(&mut self.panels[index].cue_start).speed(0.1))
                                .changed()
                            {
                                mix_dirty = true;
                            }
                            ui.label("Next");
                            if ui
                                .add(egui::DragValue::new(&mut self.panels[index].cue_next).speed(0.1))
                                .changed()
                            {
                                mix_dirty = true;
                            }
                            ui.label("End");
                            if ui
                                .add(egui::DragValue::new(&mut self.panels[index].cue_end).speed(0.1))
                                .changed()
                            {
                                mix_dirty = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Fade in ms");
                            ui.add(egui::DragValue::new(&mut self.panels[index].fade_in_ms));
                            ui.label("Fade out ms");
                            ui.add(egui::DragValue::new(&mut self.panels[index].fade_out_ms));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Zone fader dB");
                            if ui
                                .add(egui::Slider::new(&mut self.panels[index].fader_db, -60.0..=0.0))
                                .changed()
                            {
                                mix_dirty = true;
                            }
                            if ui
                                .checkbox(&mut self.panels[index].muted, "Mute")
                                .changed()
                            {
                                mix_dirty = true;
                            }
                        });
                    });
                    ui.separator();
                }
            });

        for action in actions {
            match action {
                ZoneAction::Load(zone) => {
                    if let Some(path) = pick_file(state) {
                        self.load_and_play(state, zone, &path);
                    }
                }
                ZoneAction::Play(zone) => {
                    let path = self.panel(zone).track_path.clone();
                    if let Some(path) = path {
                        self.load_and_play(state, zone, &path);
                    } else {
                        state.set_status("No file loaded on this zone");
                    }
                }
                ZoneAction::Stop(zone) => self.stop_zone(state, zone),
            }
        }
        if mix_dirty {
            for panel in self.panels.iter() {
                apply_mix(panel, &state.engine);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiocraft_audio::decoder::MemorySource;

    fn item(cstart: &str, cnext: &str, cend: &str, fin: &str, fout: &str) -> NewPlaylistItem {
    NewPlaylistItem {
        pID: 1,
        sID: 7,
        cstart: cstart.parse().unwrap(),
        cnext: cnext.parse().unwrap(),
        cend: cend.parse().unwrap(),
        fin: fin.parse().unwrap(),
        fout: fout.parse().unwrap(),
        ..Default::default()
    }
    }

    #[test]
    fn zeros_are_the_not_set_sentinel() {
        let plan = cue_plan(&item("0", "0", "0", "0", "0"));
        assert_eq!(
            plan,
            CuePlan {
                start_at: None,
                vdf_target: None,
                stop_at: None,
                fade_in_ms: None,
                fade_out_ms: None,
            }
        );
    }

    #[test]
    fn cue_columns_map_onto_engine_knobs() {
        let plan = cue_plan(&item("1.5", "42.25", "90", "250", "1500"));
        assert_eq!(plan.start_at, Some(1.5));
        assert_eq!(plan.vdf_target, Some(42.25));
        assert_eq!(plan.stop_at_secs(), Some(90.0));
        assert_eq!(plan.fade_in_ms, Some(250));
        assert_eq!(plan.fade_out_ms, Some(1500));
    }

    #[test]
    fn field_based_plan_treats_zero_the_same_way() {
        let plan = cue_plan_from_fields(0.0, 0.0, 12.5, 0, 800);
        assert_eq!(plan.start_at, None);
        assert_eq!(plan.vdf_target, None);
        assert_eq!(plan.stop_at, Some(12.5));
        assert_eq!(plan.fade_in_ms, None);
        assert_eq!(plan.fade_out_ms, Some(800));
    }

    #[test]
    fn apply_marks_missing_fades_with_the_default_marker() {
        let source = Box::new(MemorySource::from_mono(&[0.0; 8], 44_100.0));
        let plan = CuePlan {
            start_at: Some(2.0),
            vdf_target: Some(30.0),
            stop_at: None,
            fade_in_ms: Some(400),
            fade_out_ms: None,
        };
        let req = plan.apply(PlayRequest::new(source));
        assert_eq!(req.fade_in_ms, 400);
        assert_eq!(req.fade_out_ms, -1);
        assert_eq!(req.start_at_secs, Some(2.0));
        assert_eq!(req.vdf_target_secs, Some(30.0));
    }

    #[test]
    fn apply_without_cues_leeps_the_request_untouched() {
        let source = Box::new(MemorySource::from_mono(&[0.0; 8], 44_100.0));
        let req = CuePlan::default().apply(PlayRequest::new(source));
        assert_eq!(req.fade_in_ms, 0);
        assert_eq!(req.fade_out_ms, -1);
        assert_eq!(req.start_at_secs, None);
        assert_eq!(req.vdf_target_secs, None);
    }
}
