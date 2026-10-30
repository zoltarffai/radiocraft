//! Stand-alone configuration windows: the database connection editor,
//! the now-playing (metadata export) editor, the built-in web console,
//! category / rotation managers, the events list, the backup panel and
//! the close-to-tray policy.
//!
//! Every window is a plain egui window owned by [`ConfigUi`]; the main
//! loop toggles them from the menu bar. Like the rest of the app, the
//! windows run database calls with plain `block_on` on the state's
//! runtime, so there is no async plumbing in the GUI.

use std::sync::{Arc, Mutex};

use chrono::Local;
use radiocraft_core::template::Context;
use radiocraft_db::backup;
use radiocraft_db::entities::{self, Category, EventRow, Rotation};
use radiocraft_db::Db;
use radiocraft_net::autost::{CloseAction, ClosePolicy};
use radiocraft_net::nowplaying::{ExportConfig, FileEncoding, NowPlayingSnapshot, WebItem};
use radiocraft_net::webserver::{
    NowPlayingProvider, RequestSink, RequestSubmission, WebServer, WebServerConfig,
};

use crate::state::{db_config_from, song_data_from_row, AppState, SongRow, StatusLine};

/// All configuration windows in one bundle, hanging off `AppState`.
#[derive(Default)]
pub struct ConfigUi {
    pub db: DbWindow,
    pub now_playing: NowPlayingWindow,
    pub web: WebWindow,
    pub categories: CategoryWindow,
    pub rotations: RotationWindow,
    pub events: EventWindow,
    pub backup: BackupWindow,
    pub close: CloseWindow,
}

// ---------------------------------------------------------------------------
// helpers shared by the windows
// ---------------------------------------------------------------------------

/// Show a window; returns whether it is still open. The flag is a plain
/// copy so the draw closure may capture `self` without also borrowing a
/// field of it at the same time.
fn window(
    title: &str,
    id: &str,
    was_open: bool,
    ctx: &egui::Context,
    add: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut still_open = was_open;
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .resizable(true)
        .open(&mut still_open)
        .show(ctx, add);
    still_open
}

/// Run a database call with the connection borrowed out of `state.db`.
/// The pool is temporarily moved out of the `Option` so the closure can
/// receive both `&mut AppState` (for its runtime) and `&Db` without the
/// borrow checker seeing an alias. The handle is always put back.
fn with_db<T>(
    state: &mut AppState,
    what: &str,
    work: impl FnOnce(&mut AppState, &Db) -> anyhow::Result<T>,
) -> Option<T> {
    let db = match state.db.take() {
        Some(db) => db,
        None => {
            state.set_status(StatusLine::warn(format!(
                "Cannot {what}: connect from the database window first"
            )));
            return None;
        }
    };
    let result = work(state, &db);
    state.db = Some(db);
    match result {
        Ok(v) => Some(v),
        Err(err) => {
            state.report_error(what, &err);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Database connection window
// ---------------------------------------------------------------------------

/// The `settings_database.xml` editor plus connect/test/disconnect.
#[derive(Debug, Default)]
pub struct DbWindow {
    pub open: bool,
    /// Last test-result line (empty until tested).
    pub last_result: String,
}

impl DbWindow {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) {
        self.open = window("Database", "db_window", self.open, ctx, |ui| {
            self.draw(ui, state);
        });
    }

    fn draw(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        // RadioDJ carries the port inside DbServer ("host:port"), so one
        // plain editor covers both.
        let mut server = state.database.db_server().clone();
        ui.horizontal(|ui| {
            ui.label("Server");
            if ui
                .add(egui::TextEdit::singleline(&mut server).desired_width(240.0))
                .changed()
            {
                state.database.set_db_server(server.clone());
            }
        });
        let mut name = state.database.db_name().clone();
        ui.horizontal(|ui| {
            ui.label("Database");
            if ui.add(egui::TextEdit::singleline(&mut name).desired_width(180.0)).changed() {
                state.database.set_db_name(name.clone());
            }
            ui.label("User");
            let mut user = state.database.db_username().clone();
            if ui.add(egui::TextEdit::singleline(&mut user).desired_width(120.0)).changed() {
                state.database.set_db_username(user);
            }
        });
        let mut pass = state.database.db_pass().clone();
        ui.horizontal(|ui| {
            ui.label("Password");
            if ui
                .add(egui::TextEdit::singleline(&mut pass).password(true).desired_width(180.0))
                .changed()
            {
                state.database.set_db_pass(pass);
            }
            let mut ssl = *state.database.ssl();
            if ui.checkbox(&mut ssl, "SSL").changed() {
                state.database.set_ssl(ssl);
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                if let Err(err) = state.save_database() {
                    state.report_error("saving database settings", &err);
                }
            }
            if ui.button("Test").clicked() {
                self.test_connection(state);
            }
            if ui.button("Connect").clicked() {
                if let Err(err) = state.save_database() {
                    state.report_error("saving database settings", &err);
                } else {
                    state.connect_db();
                }
            }
            if ui.button("Disconnect").clicked() {
                state.disconnect_db();
            }
        });
        if !self.last_result.is_empty() {
            ui.label(&self.last_result);
        }
    }

    /// Open a throwaway connection to prove the settings work.
    pub fn test_connection(&mut self, state: &mut AppState) {
        let cfg = db_config_from(&state.database);
        match state.rt.block_on(Db::connect(&cfg)) {
            Ok(db) => {
                state.rt.block_on(db.close());
                self.last_result = format!(
                    "Connection to {} on {} succeeded",
                    cfg.database, cfg.host
                );
            }
            Err(err) => {
                self.last_result = format!("Connection failed: {err:#}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Now-playing (metadata export) window
// ---------------------------------------------------------------------------

/// Everything the now-playing plugin persists (docs/02 §6). Mirrors
/// `ExportConfig` in string form so the panel can edit freely before
/// committing the keys.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportDraft {
    pub export_to_file: bool,
    pub target_path: String,
    /// 0 = ANSI, 1 = UTF-8, 2 = UTF-16 (RadioDJ `FileEncoding`).
    pub encoding: i32,
    pub txt_file_template: String,

    pub export_to_web: bool,
    pub web_urls: Vec<String>,
    pub web_methods: Vec<String>,
    pub target_data: String,

    pub export_to_serial: bool,
    pub serial_template: String,
    pub target_port: String,
    pub serial_baud: i32,
    pub serial_parity: i32,
    pub serial_stop: i32,
    pub serial_data: i32,
    pub serial_dtr: bool,
    pub serial_rts: bool,

    pub enable_custom_title: bool,
    pub custom_title: String,
    /// `0;1;5;` style track-type list kept as raw text.
    pub types_text: String,
}

impl Default for ExportDraft {
    fn default() -> Self {
        Self {
            export_to_file: false,
            target_path: String::new(),
            encoding: 1,
            txt_file_template: String::new(),
            export_to_web: false,
            web_urls: Vec::new(),
            web_methods: Vec::new(),
            target_data: "-".to_string(),
            export_to_serial: false,
            serial_template: String::new(),
            target_port: String::new(),
            serial_baud: 1,
            serial_parity: 0,
            serial_stop: 1,
            serial_data: 8,
            serial_dtr: false,
            serial_rts: false,
            enable_custom_title: false,
            custom_title: String::new(),
            types_text: String::new(),
        }
    }
}

impl ExportDraft {
    pub fn from_config(cfg: &ExportConfig) -> Self {
        ExportDraft {
            export_to_file: cfg.export_to_file,
            target_path: cfg.target_path.clone(),
            encoding: encoding_code(cfg.file_encoding),
            txt_file_template: cfg.txt_file_template.clone(),
            export_to_web: cfg.export_to_web,
            web_urls: cfg.web_items.iter().map(|i| i.url.clone()).collect(),
            web_methods: cfg.web_items.iter().map(|i| i.method.clone()).collect(),
            target_data: cfg.target_data.clone(),
            export_to_serial: cfg.export_to_serial,
            serial_template: cfg.serial_template.clone(),
            target_port: cfg.target_port.clone(),
            serial_baud: cfg.serial_baud,
            serial_parity: cfg.serial_parity,
            serial_stop: cfg.serial_stop,
            serial_data: cfg.serial_data,
            serial_dtr: cfg.serial_dtr,
            serial_rts: cfg.serial_rts,
            enable_custom_title: cfg.enable_custom_title,
            custom_title: cfg.custom_title.clone(),
            types_text: cfg
                .types_to_export
                .iter()
                .map(|t| *t as i32)
                .map(|i| i.to_string())
                .collect::<Vec<String>>()
                .join(";"),
        }
    }

    /// The plugin key/values this draft stands for.
    pub fn to_keys(&self) -> Vec<(&'static str, String)> {
        let items: Vec<WebItem> = self
            .web_urls
            .iter()
            .zip(self.web_methods.iter())
            .filter(|(url, _)| !url.trim().is_empty())
            .map(|(url, method)| WebItem {
                url: url.trim().to_string(),
                method: if method.eq_ignore_ascii_case("get") {
                    "get".to_string()
                } else {
                    "post".to_string()
                },
            })
            .collect();
        let web_items = if items.is_empty() {
            String::new()
        } else {
            let mut xml = String::from("<items>");
            for item in &items {
                xml.push_str(&format!(
                    "<item url=\"{}\" method=\"{}\" />",
                    item.url.replace('"', "&quot;"),
                    item.method
                ));
            }
            xml.push_str("</items>");
            xml
        };
        vec![
            ("Export2File", self.export_to_file.to_string()),
            ("TargetPath", self.target_path.clone()),
            ("FileEncoding", self.encoding.to_string()),
            ("txtFileTemplate", self.txt_file_template.clone()),
            ("Export2Web", self.export_to_web.to_string()),
            ("WebItems", web_items),
            ("TargetData", self.target_data.clone()),
            ("Export2Serial", self.export_to_serial.to_string()),
            ("SerialTemplate", self.serial_template.clone()),
            ("TargetPort", self.target_port.clone()),
            ("SerialBaudRate", self.serial_baud.to_string()),
            ("SerialParity", self.serial_parity.to_string()),
            ("SerialStopBits", self.serial_stop.to_string()),
            ("SerialDataBits", self.serial_data.to_string()),
            ("Serialdtr", self.serial_dtr.to_string()),
            ("Serialrts", self.serial_rts.to_string()),
            ("EnableCustomTitle", self.enable_custom_title.to_string()),
            ("CustomTitle", self.custom_title.clone()),
            ("TypesToExport", self.types_text.clone()),
        ]
    }
}

/// RadioDJ numeric code for a file encoding.
pub fn encoding_code(e: FileEncoding) -> i32 {
    match e {
        FileEncoding::Ansi => 0,
        FileEncoding::Utf8 => 1,
        FileEncoding::Utf16 => 2,
    }
}

/// The `Plugins/Plugin_NowPlaying.xml` editor.
#[derive(Debug, Default)]
pub struct NowPlayingWindow {
    pub open: bool,
    /// Working copy; reloaded from the settings when the window opens.
    pub draft: Option<ExportDraft>,
}

impl NowPlayingWindow {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if self.open && self.draft.is_none() {
            self.draft = Some(ExportDraft::from_config(&state.exporter.config));
        }
        if !self.open {
            return;
        }
        self.open = window(
            "Now Playing",
            "now_playing_window",
            self.open,
            ctx,
            |ui| {
                let draft = self
                    .draft
                    .get_or_insert_with(|| ExportDraft::from_config(&state.exporter.config));
                draw_export_draft(ui, draft);
                ui.separator();
                if ui.button("Save").clicked() {
                    for (key, value) in draft.to_keys() {
                        state.plugin_export.set(key, value);
                    }
                    if let Err(err) = state.save_plugin_export() {
                        state.report_error("saving the now-playing settings", &err);
                    } else {
                        state.set_status("Now-playing settings saved");
                        self.draft = None;
                    }
                }
                if ui.button("Reload").clicked() {
                    self.draft = Some(ExportDraft::from_config(&state.exporter.config));
                }
            },
        );
        if !self.open {
            self.draft = None;
        }
    }
}

fn draw_export_draft(ui: &mut egui::Ui, d: &mut ExportDraft) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.group(|ui| {
                ui.strong("File");
                ui.checkbox(&mut d.export_to_file, "Export to file");
                ui.horizontal(|ui| {
                    ui.label("Target path");
                    ui.add(egui::TextEdit::singleline(&mut d.target_path).desired_width(260.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Encoding");
                    ui.selectable_value(&mut d.encoding, 0, "ANSI");
                    ui.selectable_value(&mut d.encoding, 1, "UTF-8");
                    ui.selectable_value(&mut d.encoding, 2, "Unicode");
                });
                ui.label("Template");
                ui.add(
                    egui::TextEdit::multiline(&mut d.txt_file_template)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
            });
            ui.group(|ui| {
                ui.strong("Web");
                ui.checkbox(&mut d.export_to_web, "Export to web");
                let mut remove: Option<usize> = None;
                for (i, (url, method)) in d
                    .web_urls
                    .iter_mut()
                    .zip(d.web_methods.iter_mut())
                    .enumerate()
                {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(url).desired_width(320.0));
                        ui.selectable_value(method, "post".to_string(), "POST");
                        ui.selectable_value(method, "get".to_string(), "GET");
                        if ui.button("x").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    d.web_urls.remove(i);
                    d.web_methods.remove(i);
                }
                if ui.button("Add web target").clicked() {
                    d.web_urls.push(String::new());
                    d.web_methods.push("post".to_string());
                }
                ui.label("Target data");
                ui.add(egui::TextEdit::singleline(&mut d.target_data).desired_width(420.0));
            });
            ui.group(|ui| {
                ui.strong("Serial");
                ui.checkbox(&mut d.export_to_serial, "Export to serial display");
                ui.horizontal(|ui| {
                    ui.label("Port");
                    ui.add(egui::TextEdit::singleline(&mut d.target_port).desired_width(120.0));
                    ui.label("Baud");
                    ui.add(egui::DragValue::new(&mut d.serial_baud).range(0..=9));
                    ui.label("Parity");
                    ui.add(egui::DragValue::new(&mut d.serial_parity).range(0..=2));
                    ui.label("Stop");
                    ui.add(egui::DragValue::new(&mut d.serial_stop).range(0..=3));
                    ui.label("Data");
                    ui.add(egui::DragValue::new(&mut d.serial_data).range(5..=8));
                    ui.checkbox(&mut d.serial_dtr, "dtr");
                    ui.checkbox(&mut d.serial_rts, "rts");
                });
                ui.label("Template");
                ui.add(
                    egui::TextEdit::multiline(&mut d.serial_template)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
            });
            ui.group(|ui| {
                ui.strong("Titles / types");
                ui.checkbox(&mut d.enable_custom_title, "Custom title");
                ui.add(egui::TextEdit::singleline(&mut d.custom_title).desired_width(320.0));
                ui.horizontal(|ui| {
                    ui.label("Types to export (e.g. 0;1;5;)");
                    ui.add(egui::TextEdit::singleline(&mut d.types_text).desired_width(160.0));
                });
            });
        });
}

// ---------------------------------------------------------------------------
// Web console window (the built-in now-playing / request server)
// ---------------------------------------------------------------------------

/// Now-playing source for the built-in web server: a slot refreshed by the
/// app loop from the main-zone track.
#[derive(Debug, Default)]
pub struct SnapshotSource {
    inner: Mutex<Option<(NowPlayingSnapshot, Context)>>,
}

impl NowPlayingProvider for SnapshotSource {
    fn snapshot(&self) -> Option<NowPlayingSnapshot> {
        self.inner.lock().ok().and_then(|slot| slot.clone()).map(|(s, _)| s)
    }

    fn context(&self) -> Context {
        self.inner
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .map(|(_, ctx)| ctx)
            .unwrap_or_default()
    }
}

impl SnapshotSource {
    /// Update the current-track slot.
    pub fn set_now_playing(&self, snapshot: NowPlayingSnapshot, context: Context) {
        if let Ok(mut slot) = self.inner.lock() {
            *slot = Some((snapshot, context));
        }
    }

    pub fn clear(&self) {
        if let Ok(mut slot) = self.inner.lock() {
            *slot = None;
        }
    }
}

/// Request queue for the web console (`POST /api/request`).
#[derive(Debug, Default)]
pub struct QueueSink {
    inner: Mutex<Vec<RequestSubmission>>,
}

impl RequestSink for QueueSink {
    fn submit(&self, request: RequestSubmission) -> Result<(), String> {
        if let Ok(mut q) = self.inner.lock() {
            q.push(request);
            Ok(())
        } else {
            Err("request queue is gone".to_string())
        }
    }
}

impl QueueSink {
    /// Take every queued request out (the app pushes them onto its assist
    /// list; the panel shows the count).
    pub fn drain(&self) -> Vec<RequestSubmission> {
        self.inner.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The web console configuration: bind address, template folder and the
/// start/stop buttons.
#[derive(Default)]
pub struct WebWindow {
    pub open: bool,
    pub bind: String,
    pub templates_dir: String,
    /// Bound address once running (read-only display).
    pub bound: String,
    pub last_error: String,
    server: Option<WebServer>,
    source: Option<Arc<SnapshotSource>>,
    sink: Arc<QueueSink>,
}

impl WebWindow {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if self.open && self.bind.is_empty() {
            self.bind = "127.0.0.1:8000".to_string();
            self.templates_dir = state
                .config_dir
                .join("Templates")
                .to_string_lossy()
                .into_owned();
        }
        if !self.open {
            return;
        }
        self.open = window("Web Console", "web_window", self.open, ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Bind");
                if self.server.is_some() {
                    ui.label(self.bound.clone());
                } else {
                    ui.add(egui::TextEdit::singleline(&mut self.bind).desired_width(160.0));
                }
                ui.label("Templates");
                ui.add(
                    egui::TextEdit::singleline(&mut self.templates_dir).desired_width(260.0),
                );
            });
            ui.horizontal(|ui| {
                let running = self.server.is_some();
                ui.colored_label(
                    if running { egui::Color32::DARK_GREEN } else { egui::Color32::GRAY },
                    if running { "Running" } else { "Stopped" },
                );
                if ui.add_enabled(!running, egui::Button::new("Start")).clicked() {
                    self.start();
                }
                if ui.add_enabled(running, egui::Button::new("Stop")).clicked() {
                    self.stop();
                }
                let queued = self.sink.len();
                if queued > 0 && ui.button(format!("Take {queued} request(s)")).clicked() {
                    for req in self.sink.drain() {
                        tracing::info!(
                            "web request: {} ({}) by {}",
                            req.path,
                            req.title,
                            req.user
                        );
                    }
                }
            });
            if !self.last_error.is_empty() {
                ui.colored_label(egui::Color32::DARK_RED, &self.last_error);
            }
        });
    }

    /// Start serving; a failure is shown in the panel, not fatal.
    pub fn start(&mut self) {
        let source = Arc::new(SnapshotSource::default());
        let cfg = WebServerConfig::new(self.bind.trim(), self.templates_dir.trim());
        match WebServer::start(cfg, Arc::clone(&source) as Arc<dyn NowPlayingProvider>, Arc::clone(&self.sink) as Arc<dyn RequestSink>) {
            Ok((server, bound)) => {
                self.bound = bound;
                self.source = Some(source);
                self.server = Some(server);
                self.last_error = String::new();
            }
            Err(err) => {
                self.last_error = format!("Web server failed: {err}");
            }
        }
    }

    /// Stop serving and join the worker.
    pub fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.join();
        }
        self.source = None;
        self.bound = String::new();
    }

    /// Keep the server's now-playing slot aligned with the main zone.
    pub fn sync_now_playing(&mut self, state: &AppState) {
        if let Some(source) = self.source.as_ref() {
            match state.now_playing.as_ref() {
                Some(np) => {
                    let row = SongRow {
                        id: np.song_id,
                        artist: np.artist.clone(),
                        title: np.title.clone(),
                        album: np.album.clone(),
                        path: np.path.clone(),
                        duration_secs: np.duration_secs,
                    };
                    let song = song_data_from_row(&row, np.track_type);
                    source.set_now_playing(
                        NowPlayingSnapshot::from_song(state.station_name(), &song),
                        state.exporter.context_for(&song),
                    );
                }
                None => source.clear(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Category window
// ---------------------------------------------------------------------------

/// Song categories (`category` table) manager.
#[derive(Debug, Default)]
pub struct CategoryWindow {
    pub open: bool,
    pub rows: Vec<Category>,
    pub selected: Option<usize>,
    pub new_name: String,
    pub rename: String,
}

impl CategoryWindow {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if !self.open {
            return;
        }
        if self.rows.is_empty() && state.db.is_some() {
            self.reload(state);
        }
        self.open = window("Categories", "category_window", self.open, ctx, |ui| {
            self.draw(ui, state);
        });
    }

    pub fn reload(&mut self, state: &mut AppState) {
        let out = with_db(state, "listing categories", |state, db| {
            state.rt.block_on(entities::list_categories(db.pool())).map_err(anyhow::Error::from)
        });
        if let Some(rows) = out {
            self.rows = rows;
            self.selected = None;
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                for (i, row) in self.rows.iter().enumerate() {
                    if ui
                        .selectable_label(self.selected == Some(i), format!("{}  {}", row.ID, row.name))
                        .clicked()
                    {
                        self.selected = Some(i);
                        self.rename = row.name.clone();
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.label("New");
            ui.add(egui::TextEdit::singleline(&mut self.new_name).desired_width(160.0));
            if ui.button("Add").clicked() {
                let name = self.new_name.trim().to_string();
                if !name.is_empty() {
                    let out = with_db(state, "adding a category", move |state, db| {
                        state.rt.block_on(entities::insert_category(db.pool(), &name)).map_err(anyhow::Error::from)
                    });
                    if out.is_some() {
                        self.new_name = String::new();
                        self.reload(state);
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Rename");
            ui.add(egui::TextEdit::singleline(&mut self.rename).desired_width(160.0));
            let enabled = self.selected.is_some();
            if ui.add_enabled(enabled, egui::Button::new("Apply")).clicked() {
                if let (Some(i), Some(row)) = (self.selected, self.rows.get(self.selected.unwrap()))
                {
                    let id = row.ID;
                    let name = self.rename.trim().to_string();
                    let out = with_db(state, "renaming a category", move |state, db| {
                        state.rt.block_on(entities::rename_category(db.pool(), id, &name)).map_err(anyhow::Error::from)
                    });
                    if out.is_some() {
                        let _ = i;
                        self.reload(state);
                    }
                }
            }
            if ui.add_enabled(enabled, egui::Button::new("Delete")).clicked() {
                if let Some(row) = self.rows.get(self.selected.unwrap()) {
                    let id = row.ID;
                    let out = with_db(state, "deleting a category", move |state, db| {
                        state.rt.block_on(entities::delete_category(db.pool(), id)).map_err(anyhow::Error::from)
                    });
                    if out.is_some() {
                        self.reload(state);
                    }
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Rotation window
// ---------------------------------------------------------------------------

/// Rotations (`rotations` table) manager. Also used read-only by the AutoDJ
/// button in the main window.
#[derive(Debug, Default)]
pub struct RotationWindow {
    pub open: bool,
    pub rows: Vec<Rotation>,
    pub selected: Option<usize>,
    pub new_name: String,
    pub rename: String,
}

impl RotationWindow {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if !self.open {
            return;
        }
        if self.rows.is_empty() && state.db.is_some() {
            self.reload(state);
        }
        self.open = window("Rotations", "rotation_window", self.open, ctx, |ui| {
            self.draw(ui, state);
        });
    }

    pub fn reload(&mut self, state: &mut AppState) {
        let out = with_db(state, "listing rotations", |state, db| {
            state.rt.block_on(entities::list_rotations(db.pool())).map_err(anyhow::Error::from)
        });
        if let Some(rows) = out {
            self.rows = rows;
            self.selected = None;
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                for (i, row) in self.rows.iter().enumerate() {
                    if ui
                        .selectable_label(self.selected == Some(i), format!("{}  {}", row.ID, row.name))
                        .clicked()
                    {
                        self.selected = Some(i);
                        self.rename = row.name.clone();
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.label("New");
            ui.add(egui::TextEdit::singleline(&mut self.new_name).desired_width(160.0));
            if ui.button("Add").clicked() {
                let name = self.new_name.trim().to_string();
                if !name.is_empty() {
                    let out = with_db(state, "adding a rotation", move |state, db| {
                        state.rt.block_on(entities::insert_rotation(db.pool(), &name)).map_err(anyhow::Error::from)
                    });
                    if out.is_some() {
                        self.new_name = String::new();
                        self.reload(state);
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Rename");
            ui.add(egui::TextEdit::singleline(&mut self.rename).desired_width(160.0));
            let sel = self.selected;
            if ui.add_enabled(sel.is_some(), egui::Button::new("Apply")).clicked() {
                if let Some(row) = sel.and_then(|i| self.rows.get(i)) {
                    let id = row.ID;
                    let name = self.rename.trim().to_string();
                    let out = with_db(state, "renaming a rotation", move |state, db| {
                        state.rt.block_on(entities::rename_rotation(db.pool(), id, &name)).map_err(anyhow::Error::from)
                    });
                    if out.is_some() {
                        self.reload(state);
                    }
                }
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new("Delete")).clicked() {
                if let Some(row) = sel.and_then(|i| self.rows.get(i)) {
                    let id = row.ID;
                    let out = with_db(state, "deleting a rotation", move |state, db| {
                        state.rt.block_on(entities::delete_rotation(db.pool(), id)).map_err(anyhow::Error::from)
                    });
                    if out.is_some() {
                        self.reload(state);
                    }
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Events window
// ---------------------------------------------------------------------------

/// The `events` table list (read list + enable toggle; full editing lives in
/// the scheduler windows in the real app — this mirrors the list view).
#[derive(Debug, Default)]
pub struct EventWindow {
    pub open: bool,
    pub rows: Vec<EventRow>,
    pub selected: Option<usize>,
    pub only_enabled: bool,
}

impl EventWindow {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if !self.open {
            return;
        }
        self.open = window("Events", "event_window", self.open, ctx, |ui| {
            self.draw(ui, state);
        });
    }

    pub fn reload(&mut self, state: &mut AppState) {
        let only = self.only_enabled;
        let out = with_db(state, "listing events", move |state, db| {
            state.rt.block_on(entities::list_events(db.pool(), only)).map_err(anyhow::Error::from)
        });
        if let Some(rows) = out {
            self.rows = rows;
            self.selected = None;
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        ui.horizontal(|ui| {
            if ui.button("Reload").clicked() {
                self.reload(state);
            }
            if ui.checkbox(&mut self.only_enabled, "Enabled only").changed() {
                self.reload(state);
            }
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let count = self.rows.len();
                for i in 0..count {
                    let row = self.rows[i].clone();
                    let label = format!(
                        "{}  t{}  {}  {}",
                        row.ID, row.event_type, row.time, row.name
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(self.selected == Some(i), label)
                            .clicked()
                        {
                            self.selected = Some(i);
                        }
                        let mut enabled = row.enabled.unwrap_or(false);
                        if ui.checkbox(&mut enabled, "").changed() {
                            let mut updated = row.clone();
                            updated.enabled = Some(enabled);
                            let send_row = updated.clone();
                            let out = with_db(state, "updating an event", move |state, db| {
                                state.rt.block_on(entities::update_event(db.pool(), &send_row)).map_err(anyhow::Error::from)
                            });
                            if out.is_some() {
                                self.rows[i] = updated;
                            }
                        }
                    });
                }
            });
    }
}

// ---------------------------------------------------------------------------
// Backup window
// ---------------------------------------------------------------------------

/// Manual + automatic backup (`radiocraft-db::backup`, `AutoBackup*` keys).
#[derive(Debug, Default)]
pub struct BackupWindow {
    pub open: bool,
    pub last_line: String,
}

impl BackupWindow {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if !self.open {
            return;
        }
        self.open = window("Backup", "backup_window", self.open, ctx, |ui| {
            self.draw(ui, state);
        });
    }

    fn draw(&mut self, ui: &mut egui::Ui, state: &mut AppState) {
        let mut enabled = *state.database.auto_backup_enable();
        if ui.checkbox(&mut enabled, "Automatic backup").changed() {
            state.database.set_auto_backup_enable(enabled);
            save_database_settings(state);
        }
        let mut hour = *state.database.auto_backup_hour();
        ui.horizontal(|ui| {
            ui.label("Hour");
            if ui
                .add(egui::DragValue::new(&mut hour).range(0..=23))
                .changed()
            {
                state.database.set_auto_backup_hour(hour);
                save_database_settings(state);
            }
            ui.label("Next backup:", );
            let next = radiocraft_net::autost::next_backup_date(
                &radiocraft_net::autost::AutoBackupConfig::from_settings(&state.database),
                Local::now(),
            );
            ui.label(
                next.map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "-".to_string()),
            );
        });
        let mut folder = state.database.backup_folder().clone();
        ui.horizontal(|ui| {
            ui.label("Folder");
            if ui
                .add(egui::TextEdit::singleline(&mut folder).desired_width(320.0))
                .changed()
            {
                state.database.set_backup_folder(folder.clone());
                save_database_settings(state);
            }
        });
        ui.separator();
        if ui.button("Back up now").clicked() {
            self.run_now(state);
        }
        if !self.last_line.is_empty() {
            ui.label(&self.last_line);
        }
    }

    /// Write a data-only SQL dump into the backup folder.
    pub fn run_now(&mut self, state: &mut AppState) {
        let folder = state.database.backup_folder().clone();
        let file = format!("radiocraft-backup-{}.sql", Local::now().format("%Y-%m-%d"));
        let out = with_db(state, "running the backup", move |state, db| {
            let path = std::path::Path::new(&folder).join(&file);
            state.rt.block_on(backup::export_to_file(db.pool(), &path))?;
            Ok(path)
        });
        if let Some(path) = out {
            self.last_line = format!("Backup written to {}", path.display());
            state.set_status(self.last_line.clone());
        }
    }
}

fn save_database_settings(state: &mut AppState) {
    if let Err(err) = state.save_database() {
        state.report_error("saving database settings", &err);
    }
}

// ---------------------------------------------------------------------------
// Close-policy window
// ---------------------------------------------------------------------------

/// Human sentence for a close decision.
pub fn close_action_label(action: CloseAction) -> &'static str {
    match action {
        CloseAction::HideToTray => "the window will hide; the application keeps running",
        CloseAction::Exit => "the application will exit",
    }
}

/// The close-to-tray preference plus a live preview of what closing does.
#[derive(Debug, Default)]
pub struct CloseWindow {
    pub open: bool,
    /// Check this to preview "real exit" instead of hide.
    pub preview_quit: bool,
}

impl CloseWindow {
    pub fn show(&mut self, ctx: &egui::Context, state: &mut AppState) {
        if !self.open {
            return;
        }
        self.open = window("Close Behavior", "close_window", self.open, ctx, |ui| {
            let mut minimize = *state.database.minimize_to_tray();
            if ui.checkbox(&mut minimize, "Close to tray instead of exiting").changed() {
                state.database.set_minimize_to_tray(minimize);
                save_database_settings(state);
            }
            ui.checkbox(&mut self.preview_quit, "Preview: quit requested (File - Exit)");
            let action = ClosePolicy::from_settings(&state.database).on_close(self.preview_quit);
            ui.label(close_action_label(action));
            ui.label(
                "Without the tray feature the hidden window stays hidden; \
                 quit with File - Exit.",
            );
        });
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use radiocraft_settings::DatabaseSettings;

    fn settings(minimize: bool) -> DatabaseSettings {
        let mut s = DatabaseSettings::default();
        s.set_minimize_to_tray(minimize);
        s
    }

    #[test]
    fn close_policy_hides_when_tray_minimise_set() {
        let policy = ClosePolicy::from_settings(&settings(true));
        assert_eq!(policy.on_close(false), CloseAction::HideToTray);
        assert_eq!(policy.on_close(true), CloseAction::Exit);
    }

    #[test]
    fn close_policy_exits_when_minimise_off() {
        let policy = ClosePolicy::from_settings(&settings(false));
        assert_eq!(policy.on_close(false), CloseAction::Exit);
        assert_eq!(policy.on_close(true), CloseAction::Exit);
    }

    #[test]
    fn close_action_label_matches_variants() {
        assert!(close_action_label(CloseAction::HideToTray).contains("hide"));
        assert!(close_action_label(CloseAction::Exit).contains("exit"));
    }

    #[test]
    fn encoding_codes_round_trip() {
        assert_eq!(encoding_code(FileEncoding::Ansi), 0);
        assert_eq!(encoding_code(FileEncoding::Utf8), 1);
        assert_eq!(encoding_code(FileEncoding::Utf16), 2);
    }

    #[test]
    fn export_draft_keys_round_trip_web_items() {
        let mut draft = ExportDraft::default();
        draft.export_to_file = true;
        draft.target_path = "C:\\nowplaying.txt".to_string();
        draft.encoding = 2;
        draft.web_urls = vec![
            "http://host/a".to_string(),
            "http://host/b".to_string(),
            "  ".to_string(),
        ];
        draft.web_methods = vec!["post".to_string(), "GET".to_string(), "post".to_string()];
        let keys = draft.to_keys();
        let get = |k: &str| {
            keys.iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get("Export2File"), "true");
        assert_eq!(get("FileEncoding"), "2");
        let xml = get("WebItems");
        // The blank url is dropped, and the GET method is normalized.
        assert_eq!(
            xml,
            "<items><item url=\"http://host/a\" method=\"post\" />\
             <item url=\"http://host/b\" method=\"get\" /></items>"
        );
    }

    #[test]
    fn export_draft_from_config_pulls_types() {
        let cfg = ExportConfig {
            export_to_file: true,
            target_path: "t".to_string(),
            file_encoding: FileEncoding::Utf8,
            txt_file_template: String::new(),
            export_to_web: false,
            web_items: Vec::new(),
            target_data: "-".to_string(),
            export_to_serial: false,
            serial_template: String::new(),
            target_port: String::new(),
            serial_baud: 4,
            serial_parity: 0,
            serial_stop: 1,
            serial_data: 8,
            serial_handshake: 0,
            serial_dtr: false,
            serial_rts: false,
            enable_custom_title: false,
            custom_title: String::new(),
            types_to_export: vec![
                radiocraft_core::model::TrackType::Music,
                radiocraft_core::model::TrackType::Jingle,
            ],
        };
        let draft = ExportDraft::from_config(&cfg);
        assert_eq!(draft.types_text, "0;1");
        assert_eq!(draft.serial_baud, 4);
    }
}
