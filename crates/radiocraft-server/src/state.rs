//! Shared server state: settings stores, the database handle, the playout
//! engine, now-playing bookkeeping, and the event hub every subscriber
//! receives from.
//!
//! Like the old WinForms app, the engine is called synchronously (`&self`
//! everywhere; its pump thread is engine-owned). All push state lives
//! behind a tokio [`broadcast`] channel; one pump task fans engine
//! events and meters out to it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Local};
use radiocraft_audio::device::{DeviceManager, NullSink};
use radiocraft_audio::engine::{EngineConfig, LevelSnapshot, PlayoutEngine};
use radiocraft_audio::mixer::Zone;
use radiocraft_core::model::SongData;
use radiocraft_db::config::DbConfig;
use radiocraft_db::{self as db, Db};
use radiocraft_net::nowplaying::{ExportConfig, NowPlayingExporter};
use radiocraft_settings::{DatabaseSettings, GeneralSettings, PluginSettings, SoundcardSettings};
use serde::Serialize;
use tokio::sync::{broadcast, RwLock};

use crate::frames::{CartNowInfo, Frame, LevelInfo, NowInfo, StatusInfo, ZoneMix, ZoneState};

/// Settings file for the now-playing exporter plugin (same name as the
/// RadioDJ plugin config, so an existing `Plugins/` directory is reused).
pub const METADATA_EXPORT_FILE: &str = "Plugins/Plugin_NowPlaying.xml";

/// The main playout zone (the song follow-on queue player).
pub const MAIN_ZONE: Zone = Zone::Main;

/// All playout zones in wire order.
pub const ALL_ZONES: [Zone; 4] = [MAIN_ZONE, Zone::Aux, Zone::Cart, Zone::Instant];

/// Settings key `volume_out`: 0..100 percent to dBFS for the master trim.
pub fn volume_out_to_db(volume_out: i32) -> f64 {
    let percent = volume_out.clamp(0, 100);
    if percent == 0 {
        f64::NEG_INFINITY
    } else {
        // 0..100 linear percent → dB, so 100% maps to 0 dBFS.
        f64::from(percent).log10() * 20.0 - 40.0
    }
}

/// Engine knobs taken from `settings_general.xml`.
pub fn engine_config(g: &GeneralSettings) -> EngineConfig {
    EngineConfig {
        block_frames: 1024,
        master_db: volume_out_to_db(*g.volume_out()),
        duck_db: f64::from(*g.vol_onsweeper()),
        vt_duck_db: f64::from(*g.vol_vt()),
        overlay_db: f64::from(*g.vol_overlay()),
        default_fade_out_ms: *g.dur_fadeout(),
    }
}

/// `settings_database.xml` mirrored into a `DbConfig`.
pub fn db_config_from(database: &DatabaseSettings) -> DbConfig {
    let mut cfg = DbConfig::new(database.db_server(), database.db_name());
    cfg.username = database.db_username().clone();
    cfg.password = database.db_pass().clone();
    cfg.ssl = if *database.ssl() {
        db::config::SslMode::Required
    } else {
        db::config::SslMode::Disabled
    };
    cfg
}

/// Wire name of a zone (`main`/`aux`/`cart`/`instant`).
pub fn zone_name(zone: Zone) -> &'static str {
    match zone {
        Zone::Main => "main",
        Zone::Aux => "aux",
        Zone::Cart => "cart",
        Zone::Instant => "instant",
    }
}

/// Parse a wire zone name; `None` for unknown names.
pub fn parse_zone(name: &str) -> Option<Zone> {
    match name {
        "main" => Some(Zone::Main),
        "aux" => Some(Zone::Aux),
        "cart" => Some(Zone::Cart),
        "instant" => Some(Zone::Instant),
        _ => None,
    }
}

/// The item currently sounding in a zone (RadioDJ's now-playing record).
#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub song_id: i64,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub path: String,
    pub track_type: i32,
    pub duration_secs: f64,
    pub started_at: DateTime<Local>,
    /// Editable cue knobs (0 = "not set"), kept so re-cue/segue is possible.
    pub cue_start: f64,
    pub cue_next: f64,
    pub cue_end: f64,
    pub fade_in_ms: i32,
    pub fade_out_ms: i32,
}

impl NowPlaying {
    /// Length of the live segment (cue out - cue in), which is what the
    /// countdown shows rather than the file's full length.
    pub fn segment_secs(&self) -> f64 {
        (self.cue_end - self.cue_start).max(0.0)
    }

    /// The wire view of the item.
    pub fn info(&self) -> NowInfo {
        NowInfo {
            song_id: Some(self.song_id),
            artist: self.artist.clone(),
            title: self.title.clone(),
            album: self.album.clone(),
            path: self.path.clone(),
            track_type: self.track_type,
            duration_secs: self.duration_secs,
            segment_secs: if self.cue_end > self.cue_start {
                Some(self.segment_secs())
            } else {
                None
            },
            started_at: self.started_at.to_rfc3339(),
        }
    }

    /// The `SongData` payload the now-playing exporter and plugins expect.
    pub fn song_data(&self) -> SongData {
        SongData {
            id: self.song_id,
            artist: self.artist.clone(),
            title: self.title.clone(),
            album: self.album.clone(),
            path: self.path.clone(),
            duration: self.duration_secs,
            track_type: self.track_type,
            ..Default::default()
        }
    }
}

/// Status line state (level: `info`/`warn`/`error`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct Status {
    pub level: String,
    pub message: String,
}

impl Status {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            level: "info".into(),
            message: message.into(),
        }
    }

    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            level: "warn".into(),
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: "error".into(),
            message: message.into(),
        }
    }
}

impl From<Status> for StatusInfo {
    fn from(s: Status) -> Self {
        StatusInfo {
            level: s.level,
            message: s.message,
        }
    }
}

/// The shared application state, held in an `Arc` by axum and the pump.
pub struct App {
    pub started: Instant,
    pub config_dir: PathBuf,
    pub general: std::sync::RwLock<GeneralSettings>,
    pub engine: PlayoutEngine,
    /// Rebuildable on the fly (`export_set` swaps it).
    pub exporter: std::sync::RwLock<Arc<NowPlayingExporter>>,
    /// Live copy of `Plugins/Plugin_NowPlaying.xml` (now-playing export).
    pub(crate) plugin_export: std::sync::RwLock<PluginSettings>,
    /// Master sound-processing config (`Plugins/Plugin_SoundProcessing.xml`).
    pub(crate) soundfx: std::sync::RwLock<radiocraft_audio::ProcessorConfig>,
    /// Built-in cart wall config (`settings_cartwall.xml`).
    pub cartwall: std::sync::RwLock<crate::cartwall::CartwallConfig>,
    /// `None` until the operator connects (`dbConnect`).
    pub db: RwLock<Option<Db>>,
    pub db_cfg: RwLock<DbConfig>,
    /// Short critical section only (never held across `.await`).
    now: std::sync::RwLock<Option<NowPlaying>>,
    /// Cartwall button currently rolled on the cart zone (RadioDJ's
    /// `CartStarted` equivalent); the cart zone never touches `now`.
    cart_now: std::sync::RwLock<Option<CartNow>>,
    /// Instant/cart wall button rolled on the instant zone
    /// (`Plugin_InstantPlayers`); same shape, separate button id space.
    instant_now: std::sync::RwLock<Option<CartNow>>,
    /// Deadline for the temporary program duck applied while a cart or
    /// instant insert plays (RadioDJ's `ReduceVolumeForSeconds`).
    duck_until: std::sync::Mutex<Option<Instant>>,
    /// AutoDJ switch/tuning (mirrors `AutoDJStart` in the old app).
    autodj: AutodjState,
    pub status: RwLock<Status>,
    pub tx: broadcast::Sender<Frame>,
    /// External-cdylib plugin dispatcher; attached once by the server binary
    /// (`PluginManager::start`). Stays empty for in-process test apps.
    pub plugins: std::sync::OnceLock<Arc<crate::plugin_host::PluginManager>>,
}

/// AutoDJ runtime state: the enable switch (seeded from
/// `GeneralSettings.auto_dj_start`), the core scheduler's tuning knobs,
/// the last auto-pick (for the no-consecutive-repeat rule), and an
/// in-flight flag so pump ticks never overlap fills.
#[derive(Debug, Default)]
struct AutodjState {
    enabled: std::sync::atomic::AtomicBool,
    config: std::sync::Mutex<radiocraft_core::scheduler::SchedulerConfig>,
    last_pick: std::sync::Mutex<Option<i64>>,
    filling: std::sync::atomic::AtomicBool,
}

/// The active cartwall button, mirroring RadioDJ cart semantics: one cart
/// rolls at a time on the dedicated cart zone, ducking the main program.
#[derive(Debug, Clone, PartialEq)]
pub struct CartNow {
    /// `carts_list.ID` of the pressed button.
    pub button_id: u32,
    /// `carts.ID` of the cart wall it belongs to.
    pub cart_id: u32,
    /// `songs.ID` of the linked track.
    pub song_id: i64,
    pub title: String,
}

impl CartNow {
    /// The wire view of the active cart button.
    pub fn info(&self) -> CartNowInfo {
        CartNowInfo {
            button_id: self.button_id,
            cart_id: self.cart_id,
            song_id: self.song_id,
            title: self.title.clone(),
        }
    }
}

impl App {
    /// Read the settings files from `config_dir` and build the engine.
    pub fn load(config_dir: &Path) -> Result<Self> {
        let general = match config_dir.join(GeneralSettings::FILE).exists() {
            true => GeneralSettings::load(config_dir.join(GeneralSettings::FILE))
                .with_context(|| GeneralSettings::FILE)?,
            false => GeneralSettings::default(),
        };
        let database = match config_dir.join(DatabaseSettings::FILE).exists() {
            true => DatabaseSettings::load(config_dir.join(DatabaseSettings::FILE))
                .with_context(|| DatabaseSettings::FILE)?,
            false => DatabaseSettings::default(),
        };
        let export_path = config_dir.join(METADATA_EXPORT_FILE);
        let plugin_export = if export_path.exists() {
            PluginSettings::load(&export_path).with_context(|| METADATA_EXPORT_FILE)?
        } else {
            PluginSettings::new()
        };

        let exporter = NowPlayingExporter::new(
            ExportConfig::from_settings(&plugin_export),
            general.station_name(),
        );
        // Honor a saved soundcard choice (`audio_device` in
        // settings_soundcards.xml), falling back to the system default.
        let saved_device = {
            let path = config_dir.join(radiocraft_settings::SoundcardSettings::FILE);
            path.exists()
                .then(|| radiocraft_settings::KvStore::load(&path).ok())
                .flatten()
                .and_then(|store| store.get("audio_device").map(str::to_string))
        };
        let opened = match saved_device.as_deref().filter(|d| !d.is_empty()) {
            Some(name) => DeviceManager::open_named_sink(name)
                .or_else(|e| {
                    tracing::warn!("saved device {name:?} unavailable ({e}); using default");
                    DeviceManager::open_default_sink()
                }),
            None => DeviceManager::open_default_sink(),
        };
        let engine = match opened {
            Ok(sink) => PlayoutEngine::new(engine_config(&general), sink),
            Err(e) => {
                tracing::warn!("no audio device ({e}); using a null sink");
                PlayoutEngine::new(engine_config(&general), Box::new(NullSink::new()))
            }
        };

        let (tx, _) = broadcast::channel(256);
        let autodj = AutodjState {
            enabled: std::sync::atomic::AtomicBool::new(*general.auto_dj_start()),
            ..AutodjState::default()
        };
        let app = Self {
            started: Instant::now(),
            config_dir: config_dir.to_path_buf(),
            general: std::sync::RwLock::new(general),
            engine,
            exporter: std::sync::RwLock::new(Arc::new(exporter)),
            plugin_export: std::sync::RwLock::new(plugin_export),
            soundfx: std::sync::RwLock::new(radiocraft_audio::ProcessorConfig::default()),
            cartwall: std::sync::RwLock::new(crate::cartwall::CartwallConfig::default()),
            db: RwLock::new(None),
            db_cfg: RwLock::new(db_config_from(&database)),
            now: std::sync::RwLock::new(None),
            cart_now: std::sync::RwLock::new(None),
            instant_now: std::sync::RwLock::new(None),
            duck_until: std::sync::Mutex::new(None),
            autodj,
            status: RwLock::new(Status::info("Not connected to the database")),
            tx,
            plugins: std::sync::OnceLock::new(),
        };
        // Install a saved master FX chain before the first playback.
        crate::soundfx::apply_at_boot(&app)
            .context("loading master sound-processing settings")?;
        // Seed the built-in cart wall config (`settings_cartwall.xml`).
        if let Ok(cfg) = crate::cartwall::load_config(&app) {
            *app.cartwall.write().expect("cartwall lock") = cfg;
        }
        Ok(app)
    }

    /// Attach the plugin dispatcher (called once at server startup; later
    /// calls are ignored).
    pub fn attach_plugins(&self, mgr: Arc<crate::plugin_host::PluginManager>) {
        let _ = self.plugins.set(mgr);
    }

    /// The plugin dispatcher, when one has been attached.
    pub fn plugin_manager(&self) -> Option<&Arc<crate::plugin_host::PluginManager>> {
        self.plugins.get()
    }

    /// Broadcast a host state change to every plugin (no-op for in-process
    /// test apps without a dispatcher). Queued on the plugin thread; never
    /// blocks the caller.
    pub fn notify_plugins(&self, info: radiocraft_plugin::host::StateInfo) {
        if let Some(mgr) = self.plugin_manager() {
            mgr.notify(info);
        }
    }

    /// Where the settings files live.
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Write the current `db_cfg` back to `settings_database.xml`
    /// (`DbServer` is stored as `host:port`).
    pub async fn persist_db_cfg(&self) -> Result<()> {
        let cfg = self.db_cfg.read().await.clone();
        let path = self.config_dir.join(DatabaseSettings::FILE);
        let mut database = if path.exists() {
            DatabaseSettings::load(&path).with_context(|| DatabaseSettings::FILE)?
        } else {
            DatabaseSettings::default()
        };
        if !database.set_raw("DbServer", &format!("{}:{}", cfg.host, cfg.port)) {
            anyhow::bail!("could not set DbServer");
        }
        database.set_raw("DbName", &cfg.database);
        database.set_raw("DbUsername", &cfg.username);
        database.set_raw("DbPass", &cfg.password);
        database.save(&path).with_context(|| DatabaseSettings::FILE)?;
        Ok(())
    }

    /// Rebuild the now-playing exporter from the live plugin settings +
    /// current station name (call after either changes).
    pub fn rebuild_exporter(&self) {
        let plugin = self.plugin_export.read().expect("plugin export lock");
        let station = self.general.read().expect("general lock").station_name().clone();
        let exporter = NowPlayingExporter::new(ExportConfig::from_settings(&plugin), station);
        *self.exporter.write().expect("exporter lock") = Arc::new(exporter);
    }

    /// Validate and persist a now-playing export draft
    /// (`Plugins/Plugin_NowPlaying.xml`), then rebuild the exporter.
    pub fn save_export_draft(&self, draft: radiocraft_net::nowplaying::ExportDraft) -> Result<()> {
        if draft.export_to_file && draft.target_path.trim().is_empty() {
            anyhow::bail!("file export needs a target path");
        }
        if draft.export_to_web
            && draft.web_urls.iter().all(|u| u.trim().is_empty())
        {
            anyhow::bail!("web export needs at least one URL");
        }
        if draft.export_to_serial && draft.target_port.trim().is_empty() {
            anyhow::bail!("serial export needs a port");
        }
        {
            let mut plugin = self.plugin_export.write().expect("plugin export lock");
            for (key, value) in draft.to_keys() {
                plugin.set(key, value);
            }
            plugin
                .save_at(self.config_dir.join(METADATA_EXPORT_FILE))
                .with_context(|| METADATA_EXPORT_FILE)?;
        }
        self.rebuild_exporter();
        Ok(())
    }

    /// Switch the live output device (`None`/empty = system default).
    /// Opens the new sink first so failures leave playout untouched, then
    /// persists the choice to `settings_soundcards.xml`.
    pub fn set_audio_device(&self, name: Option<&str>) -> Result<radiocraft_audio::device::SinkInfo> {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        let sink = match name {
            Some(n) => radiocraft_audio::device::DeviceManager::open_named_sink(n)
                .with_context(|| format!("opening output device '{n}'"))?,
            None => match radiocraft_audio::device::DeviceManager::open_default_sink() {
                Ok(sink) => sink,
                Err(_) => Box::new(radiocraft_audio::device::NullSink::default()),
            },
        };
        let info = sink.info();
        self.engine.set_sink(sink);

        let path = self.config_dir.join(SoundcardSettings::FILE);
        let mut soundcards = if path.exists() {
            SoundcardSettings::load(&path).with_context(|| SoundcardSettings::FILE)?
        } else {
            SoundcardSettings::default()
        };
        soundcards.set_raw(
            "audio_device",
            name.unwrap_or(info.name.as_str()),
        );
        soundcards
            .save(&path)
            .with_context(|| SoundcardSettings::FILE)?;
        Ok(info)
    }

    /// Push a status line (also echoed on the WS event stream).
    pub async fn set_status(&self, status: impl Into<Status>) {
        let status = status.into();
        tracing::debug!(level = %status.level, message = %status.message, "status");
        *self.status.write().await = status.clone();
        let _ = self.tx.send(Frame::Status(status.into()));
    }

    /// Read the current status line.
    pub async fn status_line(&self) -> Status {
        self.status.read().await.clone()
    }

    /// The database handle, or an error explaining why not (status pushed).
    pub async fn require_db(&self) -> Result<Db> {
        match self.db.read().await.clone() {
            Some(db) => Ok(db),
            None => {
                self.set_status(Status::error(
                    "Not connected to the database - run the dbConnect command first",
                ))
                .await;
                anyhow::bail!("not connected to the database")
            }
        }
    }

    /// Connect using the stored settings, replacing any old handle.
    pub async fn connect_db(&self) -> Result<()> {
        let cfg = self.db_cfg.read().await.clone();
        let (db, _report) = db::Db::connect_bootstrapped(&cfg).await?;
        if let Some(old) = self.db.write().await.replace(db) {
            old.close().await;
        }
        self.set_status(Status::info(format!(
            "Connected to {} on {}",
            cfg.database, cfg.host
        )))
        .await;
        Ok(())
    }

    // ---- now playing --------------------------------------------------------

    /// A track started: remember it and run the title exporter without
    /// blocking the async runtime (`broadcast` fans out file/web/serial).
    pub async fn on_track_started(&self, playing: NowPlaying) {
        let song = playing.song_data();
        let exporter = self.exporter.read().expect("exporter lock").clone();
        if exporter.config.should_export(&song) {
            tokio::task::spawn_blocking(move || {
                if let Err(e) = exporter.broadcast(&song) {
                    tracing::warn!("now-playing export failed: {e}");
                }
            });
        }
        let previous = self.now.write().expect("now lock").replace(playing);
        if let Some(previous) = previous {
            self.book_finished(&previous).await;
        }
        let _ = self.tx.send(self.player_frame());
    }

    /// The transport finished or the operator stopped the item.
    pub async fn on_track_finished(&self) {
        let taken = self.now.write().expect("now lock").take();
        if let Some(playing) = taken {
            self.book_finished(&playing).await;
            self.set_status(Status::info("Stopped")).await;
        }
    }

    /// Snapshot and clear the rolling item (used at shutdown to book it).
    pub fn take_now_for_booking(&self) -> Option<NowPlaying> {
        self.now.write().expect("now lock").take()
    }

    /// If the live item no longer sounds, treat it as finished.
    pub async fn refresh_now_playing(&self) {
        let stale = {
            let now = self.now.read().expect("now lock");
            now.is_some() && !self.engine.is_playing(MAIN_ZONE)
        };
        if stale {
            self.on_track_finished().await;
        }
    }

    // ---- autodj --------------------------------------------------------------

    /// Whether the AutoDJ fill is enabled (`AutoDJStart`).
    pub fn autodj_enabled(&self) -> bool {
        self.autodj.enabled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Flip the in-memory switch (the persisted key is the caller's job).
    pub fn set_autodj_enabled(&self, enabled: bool) {
        self.autodj
            .enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
        use radiocraft_plugin::host::{StateChange, StateInfo};
        self.notify_plugins(StateInfo::new(if enabled {
            StateChange::AutoDjEnabled
        } else {
            StateChange::AutoDjDisabled
        }));
    }

    /// Current scheduler tuning knobs.
    pub fn autodj_config(&self) -> radiocraft_core::scheduler::SchedulerConfig {
        self.autodj.config.lock().expect("autodj config lock").clone()
    }

    /// Replace the scheduler tuning knobs.
    pub fn set_autodj_config(&self, config: radiocraft_core::scheduler::SchedulerConfig) {
        *self.autodj.config.lock().expect("autodj config lock") = config;
    }

    /// Last song AutoDJ rolled (for the no-consecutive-repeat rule).
    pub fn autodj_last_song(&self) -> Option<i64> {
        *self.autodj.last_pick.lock().expect("autodj last-pick lock")
    }

    /// Remember an AutoDJ pick.
    pub fn note_autodj_pick(&self, song_id: i64) {
        *self.autodj.last_pick.lock().expect("autodj last-pick lock") = Some(song_id);
    }

    /// Claim the single in-flight fill slot; false when one is running.
    pub fn begin_autodj_fill(&self) -> bool {
        !self.autodj.filling.swap(true, std::sync::atomic::Ordering::AcqRel)
    }

    /// Release the fill slot.
    pub fn end_autodj_fill(&self) {
        self.autodj.filling.store(false, std::sync::atomic::Ordering::Release);
    }

    // ---- cart zone -----------------------------------------------------------

    /// The active cartwall button, if any.
    pub fn cart_now(&self) -> Option<CartNow> {
        self.cart_now.read().expect("cart now lock").clone()
    }

    /// Roll a cart button (replacing any previous one) and push the frame.
    pub fn set_cart_now(&self, cart: CartNow) {
        *self.cart_now.write().expect("cart now lock") = Some(cart);
        let _ = self.tx.send(self.player_frame());
    }

    /// Clear the active cart button (stop/replacement) and push the frame.
    pub fn clear_cart_now(&self) {
        if self.cart_now.write().expect("cart now lock").take().is_some() {
            let _ = self.tx.send(self.player_frame());
        }
    }

    /// If the rolled cart no longer sounds (natural end or manual stop),
    /// clear the wall highlight.
    pub fn refresh_cart_now(&self) {
        let stale = {
            let cart = self.cart_now.read().expect("cart now lock");
            cart.is_some() && !self.engine.is_playing(Zone::Cart)
        };
        if stale {
            self.clear_cart_now();
        }
    }

    // ---- instant zone --------------------------------------------------------

    /// The active instant/cart wall button, if any.
    pub fn instant_now(&self) -> Option<CartNow> {
        self.instant_now.read().expect("instant now lock").clone()
    }

    /// Roll an instant button (replacing any previous one) and push the frame.
    pub fn set_instant_now(&self, instant: CartNow) {
        *self.instant_now.write().expect("instant now lock") = Some(instant);
        let _ = self.tx.send(self.player_frame());
    }

    /// Clear the active instant button (stop/replacement) and push the frame.
    pub fn clear_instant_now(&self) {
        if self.instant_now.write().expect("instant now lock").take().is_some() {
            let _ = self.tx.send(self.player_frame());
        }
    }

    /// If the rolled instant insert no longer sounds (natural end or manual
    /// stop), clear the wall highlight.
    pub fn refresh_instant_now(&self) {
        let stale = {
            let cur = self.instant_now.read().expect("instant now lock");
            cur.is_some() && !self.engine.is_playing(Zone::Instant)
        };
        if stale {
            self.clear_instant_now();
        }
    }

    // ---- program ducking ------------------------------------------------------

    /// Duck the main program for `duration_ms` while a cart/instant insert
    /// plays (RadioDJ's `NowPlaying.ReduceVolumeForSeconds(duration, true,
    /// -10f)`): `vol_onsweeper` when it is already a negative level, the
    /// plugin's -10 dB reduction otherwise.
    pub fn apply_program_duck(&self, duration_ms: i64) {
        let base = {
            let g = self.general.read().expect("general lock");
            engine_config(&g).duck_db
        };
        let duck = if base < 0.0 { base } else { -10.0 };
        self.engine.set_duck_db(duck);
        let until = Instant::now()
            + std::time::Duration::from_millis(duration_ms.max(0) as u64);
        *self.duck_until.lock().expect("duck lock") = Some(until);
    }

    /// Restore the configured program level after an insert (manual stops
    /// and early ends call this; natural ends reach `check_duck_expiry`).
    pub fn release_program_duck(&self) {
        if self.duck_until.lock().expect("duck lock").take().is_none() {
            return;
        }
        let base = {
            let g = self.general.read().expect("general lock");
            engine_config(&g).duck_db
        };
        self.engine.set_duck_db(base);
    }

    /// Pump hook: drop the temporary duck once its duration has elapsed.
    pub fn check_duck_expiry(&self) {
        let expired = self
            .duck_until
            .lock()
            .expect("duck lock")
            .is_some_and(|until| until <= Instant::now());
        if expired {
            self.release_program_duck();
        }
    }

    // ---- frames -------------------------------------------------------------

    /// The transport/now-playing snapshot frame.
    pub fn player_frame(&self) -> Frame {
        let now = self
            .now
            .read()
            .expect("now lock")
            .as_ref()
            .map(NowPlaying::info);
        Frame::Player {
            now,
            zones: self.zone_states(),
            cart: self.cart_now().map(|c| c.info()),
            instant: self.instant_now().map(|c| c.info()),
        }
    }

    /// Live position + playing flags for every zone.
    pub fn zone_states(&self) -> Vec<ZoneState> {
        ALL_ZONES
            .into_iter()
            .map(|zone| ZoneState {
                zone: zone_name(zone).to_string(),
                playing: self.engine.is_playing(zone),
                paused: self.engine.is_paused(zone),
                position_secs: self.engine.position_secs(zone),
                remaining_secs: self.engine.remaining_secs(zone),
            })
            .collect()
    }

    /// The mixer snapshot frame (live master gain + zone faders/mutes).
    pub fn state_frame(&self) -> Frame {
        let zones = ALL_ZONES
            .into_iter()
            .map(|zone| ZoneMix {
                zone: zone_name(zone).to_string(),
                fader_db: self.engine.zone_fader_db(zone),
                muted: self.engine.is_zone_muted(zone),
            })
            .collect();
        Frame::State {
            master_db: self.engine.master_db(),
            zones,
        }
    }

    /// Update (and push) one zone's fader or mute.
    pub fn set_zone_mix(&self, zone: Zone, fader_db: Option<f64>, muted: Option<bool>) {
        if let Some(db) = fader_db {
            self.engine.set_zone_fader_db(zone, db);
        }
        if let Some(muted) = muted {
            self.engine.set_zone_muted(zone, muted);
        }
        let _ = self.tx.send(self.state_frame());
    }

    /// The master meters for the 20 Hz stream.
    pub fn meters_frame(&self) -> Frame {
        let LevelSnapshot {
            peak_db,
            peak_hold_db,
            rms_db,
            level_db,
        } = self.engine.levels();
        Frame::Meters {
            master: LevelInfo {
                level_db,
                peak_db,
                peak_hold_db,
                rms_db,
            },
            zones: self.zone_states(),
        }
    }

    /// Subscribe to the event stream (the pump forwards frames to WS writers).
    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.tx.subscribe()
    }

    /// Elapsed/remaining for the header, preferring the engine's own clock.
    /// Read the current now-playing record (None when nothing is loaded).
    pub fn current_now_playing(&self) -> Option<NowPlaying> {
        self.now.read().expect("now lock").clone()
    }

    pub fn now_playing_clock(&self, playing: &NowPlaying) -> (f64, f64) {
        let segment = if playing.cue_end > playing.cue_start {
            playing.segment_secs()
        } else {
            playing.duration_secs
        };
        let elapsed = match self.engine.position_secs(MAIN_ZONE) {
            Some(pos) => pos.clamp(0.0, segment),
            None => {
                let wall = (Local::now() - playing.started_at)
                    .num_milliseconds()
                    .max(0) as f64
                    / 1000.0;
                wall.clamp(0.0, segment)
            }
        };
        (elapsed, (segment - elapsed).max(0.0))
    }
}

/// `Duration` uptime helper for the `/api/state` snapshot.
impl App {
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_out_bounds_map_to_mute() {
        assert_eq!(volume_out_to_db(0), f64::NEG_INFINITY);
        assert_eq!(volume_out_to_db(100), 0.0);
        assert_eq!(volume_out_to_db(-5), f64::NEG_INFINITY);
        assert_eq!(volume_out_to_db(105), 0.0);
    }

    #[test]
    fn volume_out_mid_is_negative_gain() {
        let db = volume_out_to_db(50);
        assert!(db < 0.0 && db > -100.0, "{db}");
    }

    #[test]
    fn zones_round_trip() {
        for name in ["main", "aux", "cart", "instant"] {
            let zone = parse_zone(name).expect("known zone");
            assert_eq!(zone_name(zone), name);
        }
        assert!(parse_zone("other").is_none());
    }

    #[test]
    fn segment_from_cue() {
        let mut playing = NowPlaying {
            song_id: 1,
            artist: String::new(),
            title: String::new(),
            album: String::new(),
            path: String::new(),
            track_type: 0,
            duration_secs: 10.0,
            started_at: Local::now(),
            cue_start: 0.0,
            cue_next: 0.0,
            cue_end: 0.0,
            fade_in_ms: 0,
            fade_out_ms: 0,
        };
        assert_eq!(playing.segment_secs(), 0.0);
        assert!(playing.info().segment_secs.is_none());
        playing.cue_start = 2.0;
        playing.cue_end = 8.0;
        assert_eq!(playing.segment_secs(), 6.0);
        assert_eq!(playing.info().segment_secs, Some(6.0));
    }

    #[test]
    fn status_levels() {
        assert_eq!(Status::info("x").level, "info");
        assert_eq!(Status::warn("x").level, "warn");
        assert_eq!(Status::error("x").level, "error");
    }
}
