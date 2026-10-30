//! Shared application state: settings stores, the database handle, the
//! playout engine, the now-playing bookkeeping, and the status line that
//! every panel reads.
//!
//! The GUI never holds an owned future. Database calls are plain
//! `block_on(...)` on the runtime owned here, which keeps every panel a
//! synchronous `update`, exactly as the old WinForms code worked. The
//! engine itself is thread-safe (`&self` everywhere), so the GUI may call
//! it directly; the pump thread is owned by the engine and joined by its
//! `Drop`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, Result};
use chrono::{DateTime, Local};
use radiocraft_audio::device::{self, NullSink};
use radiocraft_audio::engine::{EngineConfig, PlayoutEngine};
use radiocraft_core::model::SongData;
use radiocraft_db::{self, Db, DbConfig};
use radiocraft_net::nowplaying::{ExportConfig, NowPlayingExporter, NowPlayingSnapshot};
use radiocraft_plugin::PluginRegistry;
use radiocraft_settings::{DatabaseSettings, GeneralSettings, PluginSettings};
use rust_decimal::prelude::ToPrimitive;
use tokio::runtime::Runtime;

use crate::library::LibraryUi;
use crate::players_zones::PlayersUi;
use crate::playlists::PlaylistsUi;
use crate::windows::ConfigUi;

/// The plugin settings file the now-playing exporter reads (`Plugin_MetadataExport`).
pub const METADATA_EXPORT_FILE: &str = "Plugins/Plugin_NowPlaying.xml";

/// One line of the status bar, the `General.SetStatusText` equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Warn,
    Error,
}

/// A status-bar message plus how loud it should be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub message: String,
    pub kind: StatusKind,
}

impl StatusLine {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: StatusKind::Info,
        }
    }
    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: StatusKind::Warn,
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: StatusKind::Error,
        }
    }
}

impl From<String> for StatusLine {
    fn from(message: String) -> Self {
        StatusLine::info(message)
    }
}

impl From<&str> for StatusLine {
    fn from(message: &str) -> Self {
        StatusLine::info(message)
    }
}

/// A track that is (or was just) live on the main zone.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    pub song_id: i64,
    pub path: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_secs: f64,
    pub track_type: i32,
    pub started_at: DateTime<Local>,
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

    /// Seconds of the segment already heard, from the wall clock.
    pub fn elapsed_from_clock(&self, now: DateTime<Local>) -> f64 {
        let elapsed = (now - self.started_at).num_milliseconds() as f64 / 1000.0;
        elapsed.clamp(0.0, self.segment_secs())
    }
}

/// A row in the library grid.
#[derive(Debug, Clone, PartialEq)]
pub struct SongRow {
    pub id: i64,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_secs: f64,
    pub path: String,
}

/// Rows fetched from the library, plus what produced them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LibraryCache {
    pub fetched_at: Option<DateTime<Local>>,
    pub total: i64,
    pub rows: Vec<SongRow>,
}

/// The track record for the engine and the title exporter.
pub fn song_data_from_row(row: &SongRow, track_type: i32) -> SongData {
    SongData {
        id: row.id,
        artist: row.artist.clone(),
        title: row.title.clone(),
        album: row.album.clone(),
        path: row.path.clone(),
        duration: row.duration_secs,
        track_type,
        ..Default::default()
    }
}

/// Convert one database row into the app's grid row.
pub fn row_from_db_song(song: &db::entities::Song) -> SongRow {
    SongRow {
        id: i64::from(song.ID),
        artist: song.artist.clone(),
        title: song.title.clone(),
        album: song.album.clone(),
        duration_secs: song.duration.to_f64().unwrap_or(0.0),
        path: song.path.clone(),
    }
}

/// Settings key `volume_out`: 0..100 percent to dBFS for the master trim.
pub fn volume_out_to_db(volume_out: i32) -> f64 {
    let percent = volume_out.clamp(0, 100);
    if percent == 0 {
        f64::NEG_INFINITY
    } else {
        f64::from(percent).log10() * 200.0
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
    cfg.ssl = if database.ssl() {
        db::config::SslMode::VerifyCa
    } else {
        db::config::SslMode::Disabled
    };
    cfg
}

/// Everything the panels share.
pub struct AppState {
    pub rt: Runtime,
    pub config_dir: PathBuf,

    pub general: GeneralSettings,
    pub database: DatabaseSettings,
    pub plugin_export: PluginSettings,

    pub db: Option<Db>,
    pub db_cfg: DbConfig,
    pub engine: PlayoutEngine,
    pub registry: PluginRegistry,
    pub exporter: NowPlayingExporter,

    pub status: StatusLine,
    pub now_playing: Option<NowPlaying>,
    pub library: LibraryCache,
    pub library_ui: LibraryUi,
    pub playlists_ui: PlaylistsUi,
    pub players_ui: PlayersUi,
    pub config_ui: ConfigUi,

    /// When the status line last changed, so transient messages can expire.
    pub status_at: Instant,
}

impl AppState {
    /// Read the settings files from `config_dir` and build the engine.
    ///
    /// A missing file is not an error: the documented defaults are used,
    /// which is how RadioDJ behaves on a fresh install.
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
        let engine_cfg = engine_config(&general);
        let engine = match device::open_default_sink() {
            Ok(sink) => PlayoutEngine::new(engine_cfg, sink),
            Err(e) => {
                tracing::warn!("no audio device ({e}); using a null sink");
                PlayoutEngine::new(engine_cfg, Box::new(NullSink::new()))
            }
        };

        let db_cfg = db_config_from(&database);
        Ok(Self {
            rt: Runtime::new().context("starting the database worker")?,
            config_dir: config_dir.to_path_buf(),
            general,
            database,
            plugin_export,
            db: None,
            db_cfg,
            engine,
            registry: PluginRegistry::new(),
            exporter,
            status: StatusLine::info("Not connected to the database"),
            now_playing: None,
            library: LibraryCache::default(),
            library_ui: LibraryUi::default(),
            playlists_ui: PlaylistsUi::default(),
            players_ui: PlayersUi::default(),
            config_ui: ConfigUi::default(),
            status_at: Instant::now(),
        })
    }

    /// Replace the status line.
    pub fn set_status(&mut self, line: impl Into<StatusLine>) {
        self.status = line.into();
        self.status_at = Instant::now();
    }

    /// Report a failure on the status bar (error colour).
    pub fn report_error(&mut self, what: &str, err: &anyhow::Error) {
        tracing::error!("{what}: {err:#}");
        self.set_status(StatusLine::error(format!("{what}: {err}")));
    }

    /// The database handle, or a status-line error explaining why not.
    pub fn db(&mut self) -> Option<&Db> {
        if self.db.is_none() {
            self.set_status(StatusLine::error(
                "Not connected to the database - set it up in Database settings",
            ));
        }
        self.db.as_ref()
    }

    pub fn station_name(&self) -> &str {
        self.general.station_name()
    }

    /// How many days of play history to keep (`DAYS_2_KEEP_HISTORY`).
    pub fn days_to_keep_history(&self) -> i32 {
        *self.general.days_2_keep_history()
    }

    /// The directory recordings are written to (`RecordsFolder`).
    pub fn records_folder(&self) -> PathBuf {
        let folder = self.general.records_folder();
        if folder.is_empty() {
            self.config_dir.join("Recordings")
        } else {
            PathBuf::from(folder)
        }
    }

    /// `LastUsedPath`, the folder the last file dialog started in.
    pub fn last_used_path(&self) -> PathBuf {
        let p = self.general.last_used_path();
        if p.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(p)
        }
    }

    // ---- database -----------------------------------------------------------

    /// Connect using the stored settings.
    pub fn connect_db(&mut self) {
        let cfg = self.db_cfg.clone();
        match self.rt.block_on(Db::connect(&cfg)) {
            Ok(db) => {
                let (host, database) = (cfg.host.clone(), cfg.database.clone());
                self.db = Some(db);
                self.set_status(format!("Connected to {database} on {host}"));
                self.load_library();
                self.playlists_ui.load_lists(self);
            }
            Err(err) => {
                self.db = None;
                self.report_error("connecting to the database", &err);
            }
        }
    }

    pub fn disconnect_db(&mut self) {
        if let Some(db) = self.db.take() {
            if let Err(err) = self.rt.block_on(db.close()) {
                self.report_error("closing the database connection", &err);
            }
        }
        self.library = LibraryCache::default();
        self.set_status("Disconnected from the database");
    }

    /// Re-run the library query: a needle searches, otherwise a page of rows.
    pub fn load_library(&mut self) {
        let Some(db) = self.db.as_ref() else {
            self.set_status(StatusLine::warn("Connect to the database first"));
            return;
        };
        let needle = self.library_ui.search.trim().to_string();
        let limit = (*self.general.search_result()).max(1) as i64;
        let pool = db.pool();
        let query = self.rt.block_on(async {
            let total = db::entities::count_songs(pool).await?;
            let rows = if needle.is_empty() {
                db::entities::list_songs(pool, limit, 0).await?
            } else {
                db::entities::search_songs(pool, &needle, limit).await?
            };
            anyhow::Ok((total, rows))
        });
        match query {
            Ok((total, songs)) => {
                self.library = LibraryCache {
                    fetched_at: Some(Local::now()),
                    total: i64::from(total),
                    rows: songs.iter().map(row_from_db_song).collect(),
                };
                self.library_ui.selected = None;
                self.set_status(format!(
                    "{} of {} tracks in the library",
                    self.library.rows.len(),
                    self.library.total
                ));
            }
            Err(err) => self.report_error("loading the library", &err),
        }
    }

    // ---- now playing --------------------------------------------------------

    /// A track started: remember it and run the title exporter.
    pub fn on_track_started(&mut self, playing: NowPlaying) {
        self.export_title(&playing);
        self.now_playing = Some(playing);
    }

    /// The transport finished or the operator stopped the item.
    pub fn on_track_finished(&mut self) {
        if self.now_playing.take().is_some() {
            self.set_status("Stopped");
        }
    }

    /// If the live item no longer sounds, treat it as finished.
    pub fn refresh_now_playing(&mut self) {
        if self.now_playing.is_none() {
            return;
        }
        if !self.engine.is_playing(crate::players_zones::main_zone()) {
            self.on_track_finished();
        }
    }

    /// Elapsed/remaining for the header, preferring the engine's own clock.
    pub fn now_playing_clock(&self) -> Option<(f64, f64)> {
        let playing = self.now_playing.as_ref()?;
        let segment = playing.segment_secs();
        let elapsed = match self.engine.position_secs(crate::players_zones::main_zone()) {
            Some(pos) => pos.clamp(0.0, segment),
            None => playing.elapsed_from_clock(Local::now()),
        };
        Some((elapsed, (segment - elapsed).max(0.0)))
    }

    /// Fire the `NowPlaying` exporter for one track (files + web page).
    fn export_title(&mut self, playing: &NowPlaying) {
        let song = SongData {
            id: playing.song_id,
            artist: playing.artist.clone(),
            title: playing.title.clone(),
            album: playing.album.clone(),
            path: playing.path.clone(),
            duration: playing.duration_secs,
            track_type: playing.track_type,
            ..Default::default()
        };
        if !self.exporter.config().should_export(&song) {
            return;
        }
        let snapshot = NowPlayingSnapshot::from_song(self.general.station_name(), &song);
        let context = self.exporter.context_for(&song);
        if let Err(err) = self.exporter.export_file(&snapshot, &context) {
            self.report_error("writing the now-playing file", &err);
        }
        if let Err(err) = self.exporter.export_web(&snapshot, &context) {
            self.report_error("publishing the now-playing page", &err);
        }
    }

    // ---- settings -----------------------------------------------------------

    pub fn save_general(&self) -> Result<()> {
        self.general
            .save(self.config_dir.join(GeneralSettings::FILE))
            .with_context(|| format!("saving {}", GeneralSettings::FILE))
    }

    pub fn save_database(&mut self) -> Result<()> {
        self.db_cfg = db_config_from(&self.database);
        self.database
            .save(self.config_dir.join(DatabaseSettings::FILE))
            .with_context(|| format!("saving {}", DatabaseSettings::FILE))
    }

    /// Persist the now-playing exporter keys back to its plugin file.
    pub fn save_plugin_export(&mut self) -> Result<()> {
        let path = self.config_dir.join(METADATA_EXPORT_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut store = self.plugin_export.clone();
        store
            .save_at(&path)
            .with_context(|| format!("saving {METADATA_EXPORT_FILE}"))?;
        self.exporter = NowPlayingExporter::new(
            ExportConfig::from_settings(&self.plugin_export),
            self.general.station_name(),
        );
        Ok(())
    }
}
