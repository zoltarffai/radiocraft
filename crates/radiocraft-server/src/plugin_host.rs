//! Host-side plugin wiring: implements the `radiocraft-plugin` `Host` trait
//! against [`App`], and hosts external cdylib plugins on a dedicated
//! single-threaded dispatcher (mirroring RadioDJ's STA UI-thread model).
//!
//! The [`ServerHost`] bridge lives on the plugin thread; async work (database
//! round-trips) is spawned onto the main tokio runtime and awaited with a
//! blocking oneshot receive — never a `block_on` inside a Tokio worker.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{Column, Row};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{info, warn};

use radiocraft_audio::mixer::Zone;
use radiocraft_core::model::{SongData, TrackType};
use radiocraft_plugin::host::{ImportRequest, PluginUiRequest, StateChange, StateInfo};
use radiocraft_plugin::loader::load_plugin;
use radiocraft_plugin::plugin::Plugin;
use radiocraft_plugin::{Host, PluginRegistry};

use crate::playback::{new_song_from_path, parse_cue_times, request_for_file, CueKnobs};
use crate::state::{App, NowPlaying, MAIN_ZONE};

/// Static process state for the handful of IHost switches the server does
/// not otherwise model (assisted mode, record, event pause, ...). RadioDJ
/// kept these as fields on the main form; one shared copy per process is the
/// faithful stand-in.
#[derive(Debug, Default)]
struct HostFlags {
    assisted: AtomicBool,
    record: AtomicBool,
    pause_events: AtomicBool,
    track_loop: AtomicBool,
    full_screen: AtomicBool,
    search_results: AtomicI32,
}

static HOST_FLAGS: HostFlags = HostFlags {
    assisted: AtomicBool::new(false),
    record: AtomicBool::new(false),
    pause_events: AtomicBool::new(false),
    track_loop: AtomicBool::new(false),
    full_screen: AtomicBool::new(false),
    search_results: AtomicI32::new(0),
};

/// RadioDJ's stock track-type palette (BGR ints as `GetCatColor` returned).
/// Index = ttID.
const TYPE_COLORS: [i32; 15] = [
    0x0000C0, // Music          (red-ish)
    0x008000, // Jingle         (green)
    0x800000, // Sweeper        (maroon)
    0xC0A500, // Voiceover      (olive)
    0x000080, // Commercial     (navy)
    0x808080, // InternetStream (gray)
    0xC0C0C0, // Other          (silver)
    0x0000FF, // VDF            (blue)
    0x800080, // Podcast        (purple)
    0x00FFFF, // Request        (yellow)
    0xFF0000, // News           (cyan)
    0x808000, // PlaylistEvent  (teal)
    0xC080C0, // FileByDate     (pink)
    0xC0C000, // NewestFromFolder
    0xFF00FF, // Teaser         (magenta)
];

fn type_title(tt: i32) -> String {
    let label = match TrackType::from_i32(tt) {
        Some(TrackType::Music) => "Music",
        Some(TrackType::Jingle) => "Jingle",
        Some(TrackType::Sweeper) => "Sweeper",
        Some(TrackType::Voiceover) => "Voiceover",
        Some(TrackType::Commercial) => "Commercial",
        Some(TrackType::InternetStream) => "Internet stream",
        Some(TrackType::Other) => "Other",
        Some(TrackType::VDF) => "VDF",
        Some(TrackType::Podcast) => "Podcast",
        Some(TrackType::Request) => "Request",
        Some(TrackType::News) => "News",
        Some(TrackType::PlaylistEvent) => "Playlist event",
        Some(TrackType::FileByDate) => "File by date",
        Some(TrackType::NewestFromFolder) => "Newest from folder",
        Some(TrackType::Teaser) => "Teaser",
        None => "Unknown",
    };
    label.to_string()
}

/// Rewrite `:name` (or `:name?`) parameter tokens into `?` positionals.
fn rewrite_named_params(sql: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(sql.len());
    let mut names = Vec::new();
    let bytes = sql.as_bytes();
    let mut i = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_single {
            out.push(c);
            if c == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            out.push(c);
            if c == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' => {
                in_single = true;
                out.push(c);
                i += 1;
            }
            '"' => {
                in_double = true;
                out.push(c);
                i += 1;
            }
            '-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                out.push_str("--");
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            '/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                out.push_str("//");
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
            ':' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                    out.push_str("::");
                    i += 2;
                    continue;
                }
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                let name = &sql[i + 1..j];
                if name.is_empty() {
                    out.push(':');
                    i += 1;
                    continue;
                }
                // RadioDJ SQL uses `:name` and `:name?` (optional). A `?`
                // immediately followed by `::` is really a `?:type` cast on
                // the parameter, not the optional marker.
                let mut opt = false;
                if j < bytes.len() && bytes[j] == b'?' {
                    if j + 2 < bytes.len() && bytes[j + 1] == b':' && bytes[j + 2] == b':' {
                        // `:path::text` style: the `?` is the placeholder,
                        // the `::text` cast falls through the main loop.
                        j += 1;
                    } else {
                        opt = true;
                        j += 1;
                    }
                }
                let key = name.to_string();
                out.push('?');
                names.push(if opt { format!("{key}\u{0}") } else { key });
                i = j;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    (out, names)
}

/// Find the value for a named parameter in a RadioDJ-style name/value pair
/// list (`[":artist", "Queen", "title", "Queen", ...]`). Names match
/// case-insensitively with or without a leading `:`. Names marked optional
/// with a `\0` suffix (from `rewrite_named_params`) fall back to empty
/// string when absent.
fn lookup_named_arg(params: &[String], name: &str) -> String {
    let (key, optional) = match name.split_once('\u{0}') {
        Some((k, _)) => (k, true),
        None => (name, false),
    };
    let want = key.trim_start_matches(':').to_lowercase();
    let mut i = 0;
    while i + 1 < params.len() {
        if params[i].trim_start_matches(':').to_lowercase() == want {
            return params[i + 1].clone();
        }
        i += 2;
    }
    if optional {
        String::new()
    } else {
        // No matching pair supplied: bind the name position verbatim so the
        // statement still runs and any mismatch stays visible to the caller.
        params.get(i).cloned().unwrap_or_default()
    }
}

/// One round-trip result from the runtime bridge.
#[derive(Debug)]
pub struct SqlReply {
    pub rows: Vec<Value>,
    pub insert_id: u64,
    pub affected: u64,
    pub error: Option<String>,
}

/// Send a plugin state event through the hub as a `plugin` frame.
pub fn fan_plugin_event(
    tx: &broadcast::Sender<crate::frames::Frame>,
    plugin: &str,
    state: Option<StateInfo>,
    ui: Option<PluginUiRequest>,
) {
    let _ = tx.send(crate::frames::Frame::PluginEvent {
        plugin: plugin.to_string(),
        state: state.map(Box::new),
        ui: ui.map(Box::new),
    });
}

/// Control channel for the plugin dispatcher thread.
enum PluginCtl {
    Load {
        path: PathBuf,
        reply: oneshot::Sender<Result<u64, String>>,
    },
    Unload {
        id: u64,
        reply: oneshot::Sender<bool>,
    },
    Invoke {
        action: String,
        args: Vec<String>,
        reply: oneshot::Sender<Result<usize, String>>,
    },
    Notify(StateInfo),
    List {
        reply: oneshot::Sender<Value>,
    },
    Shutdown,
}

/// Metadata snapshot of a loaded plugin (safe to read from any thread).
#[derive(Debug, Clone, Serialize)]
struct PluginEntry {
    id: u64,
    name: String,
    version: String,
    description: String,
    path: Option<String>,
    actions: Vec<String>,
}

/// Owns the plugin dispatcher thread and its registry. Cheap to clone the
/// `Arc`; control calls block on the plugin thread.
pub struct PluginManager {
    ctl: mpsc::UnboundedSender<PluginCtl>,
    entries: std::sync::Mutex<Vec<PluginEntry>>,
    join: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl PluginManager {
    /// Start the dispatcher thread and scan `config_dir/Plugins` for cdylibs.
    pub fn start(app: Arc<App>) -> Arc<Self> {
        let (ctl, rx) = mpsc::unbounded_channel();
        let mgr = Arc::new(Self {
            ctl: ctl.clone(),
            entries: std::sync::Mutex::new(Vec::new()),
            join: std::sync::Mutex::new(None),
        });
        let thread_mgr = mgr.clone();
        let outer_rt = tokio::runtime::Handle::try_current().ok();
        let handle = std::thread::Builder::new()
            .name("rc-plugins".into())
            .spawn(move || plugin_thread(app, thread_mgr, outer_rt, rx))
            .expect("spawn plugin thread");
        *mgr.join.lock().expect("join lock") = Some(handle);
        // The boot scan of config_dir/Plugins runs on the plugin thread
        // itself (see `plugin_thread`), so this never blocks a runtime.
        mgr
    }

    pub fn load(&self, path: PathBuf) -> Result<u64, String> {
        let (tx, rx) = oneshot::channel();
        self.ctl
            .send(PluginCtl::Load { path, reply: tx })
            .map_err(|_| "plugin thread gone".to_string())?;
        let id = rx.blocking_recv().map_err(|_| "plugin thread died".to_string())??;
        Ok(id)
    }

    pub fn unload(&self, id: u64) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.ctl.send(PluginCtl::Unload { id, reply: tx }).is_err() {
            return false;
        }
        rx.blocking_recv().unwrap_or(false)
    }

    pub fn invoke(&self, action: &str, args: &[String]) -> Result<usize, String> {
        let (tx, rx) = oneshot::channel();
        self.ctl
            .send(PluginCtl::Invoke {
                action: action.to_string(),
                args: args.to_vec(),
                reply: tx,
            })
            .map_err(|_| "plugin thread gone".to_string())?;
        rx.blocking_recv().map_err(|_| "plugin thread died".to_string())?
    }

    /// Broadcast a host state change to every plugin (`on_state_changed`).
    pub fn notify(&self, state: StateInfo) {
        let _ = self.ctl.send(PluginCtl::Notify(state));
    }

    /// Snapshot of loaded plugins for the `plugin_list` op. Goes through the
    /// plugin thread so in-flight loads/unloads are reflected consistently.
    pub fn list(&self) -> Value {
        let (reply, rx) = oneshot::channel();
        if self.ctl.send(PluginCtl::List { reply }).is_err() {
            return json!({ "plugins": [] });
        }
        match rx.blocking_recv() {
            Ok(v) => v,
            Err(_) => json!({ "plugins": self.entries.lock().expect("entries lock").clone() }),
        }
    }

    pub fn shutdown(&self) {
        let _ = self.ctl.send(PluginCtl::Shutdown);
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        let _ = self.ctl.send(PluginCtl::Shutdown);
        if let Some(h) = self.join.get_mut().expect("join lock").take() {
            // The thread drains its channel; do not join forever.
            let _ = h;
        }
    }
}

/// The plugin dispatcher thread: owns the registry and every `ServerHost`
/// shim (leaked, so plugin code can call back whenever it likes).
fn plugin_thread(
    app: Arc<App>,
    mgr: Arc<PluginManager>,
    outer_rt: Option<tokio::runtime::Handle>,
    mut rx: mpsc::UnboundedReceiver<PluginCtl>,
) {
    let mut registry = PluginRegistry::new();

    // Load whatever sits in the Plugins folder on boot. This runs on the
    // plugin thread itself: no channel round-trip, and the caller's tokio
    // runtime is never blocked by the scan.
    let dir = app.config_dir.join("Plugins");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            let is_lib = p
                .extension()
                .map(|e| e == "so" || e == "dll" || e == "dylib")
                .unwrap_or(false);
            if is_lib {
                if let Err(e) = load_into_registry(&mut registry, &app, &mgr, &outer_rt, &p) {
                    warn!("plugin load failed: {e}");
                }
            }
        }
    }

    while let Some(msg) = rx.blocking_recv() {
        match msg {
            PluginCtl::Load { path, reply } => {
                let outcome =
                    load_into_registry(&mut registry, &app, &mgr, &outer_rt, &path)
                        .map_err(|e| format!("loading {}: {e}", path.display()));
                let _ = reply.send(outcome);
            }
            PluginCtl::Unload { id, reply } => {
                let removed = registry.unregister(id).is_some();
                mgr.entries.lock().expect("entries lock").retain(|e| e.id != id);
                let _ = reply.send(removed);
            }
            PluginCtl::Invoke { action, args, reply } => {
                let outcome = registry
                    .dispatch(&action, &args)
                    .map_err(|e| format!("{e}"));
                let _ = reply.send(outcome);
            }
            PluginCtl::Notify(state) => {
                registry.notify_state_changed(&state);
            }
            PluginCtl::List { reply } => {
                let mut list = mgr.entries.lock().expect("entries lock").clone();
                list.sort_by_key(|e| e.id);
                let _ = reply.send(json!({ "plugins": list }));
            }
            PluginCtl::Shutdown => break,
        }
    }
    info!("plugin dispatcher thread stopped");
}

/// Load one cdylib into the registry and mirror its metadata for `list()`.
/// Only ever called on the plugin thread, so the registry needs no lock.
fn load_into_registry(
    registry: &mut PluginRegistry,
    app: &Arc<App>,
    mgr: &Arc<PluginManager>,
    outer_rt: &Option<tokio::runtime::Handle>,
    path: &std::path::Path,
) -> Result<u64, String> {
    // Safety: the ABI gate inside `load_plugin` validates the
    // exported symbols before any call; see loader docs.
    let mut loaded = unsafe { load_plugin(path) }.map_err(|e| e.to_string())?;
    let info = loaded.info();
    // Init with the server host shim. The shim is leaked:
    // it must outlive every call the plugin makes from any
    // thread (same policy as the original app, which never
    // released its `IHost*`). `NativePlugin::init` builds the
    // host vtable around this reference itself.
    let shim: &'static ServerHost = Box::leak(Box::new(ServerHost::new(
        app.clone(),
        info.name.clone(),
        outer_rt.clone(),
    )));
    loaded.init(shim);
    let actions = loaded.actions();
    let id = registry.register(Box::new(loaded));
    shim.set_plugin_id(id);
    let entry = PluginEntry {
        id,
        name: info.name.clone(),
        version: info.version,
        description: info.description,
        path: Some(path.display().to_string()),
        actions: actions.iter().map(|a| a.display_string()).collect(),
    };
    info!("plugin `{}` loaded (id {id})", info.name);
    mgr.entries.lock().expect("entries lock").push(entry);
    Ok(id)
}

/// The host shim handed to each plugin on its thread.
pub struct ServerHost {
    app: Arc<App>,
    plugin_id: AtomicU64,
    plugin_name: String,
    /// Main-server tokio runtime (when the manager was started from inside
    /// one); DB/status bridges spawn onto it instead of a private runtime.
    outer_rt: Option<tokio::runtime::Handle>,
    /// Private fallback runtime, created only if no outer runtime exists.
    rt: std::sync::Mutex<Option<Runtime>>,
}

impl ServerHost {
    fn new(
        app: Arc<App>,
        plugin_name: String,
        outer_rt: Option<tokio::runtime::Handle>,
    ) -> Self {
        Self {
            app,
            plugin_id: AtomicU64::new(u64::MAX),
            plugin_name,
            outer_rt,
            rt: std::sync::Mutex::new(None),
        }
    }

    fn set_plugin_id(&self, id: u64) {
        self.plugin_id.store(id, Ordering::Relaxed);
    }

    fn log(&self, msg: &str) {
        info!("[plugin {} ({})] {msg}", self.plugin_name, self.plugin_id.load(Ordering::Relaxed));
    }

    /// The tokio runtime to spawn bridges onto: the captured main-runtime
    /// handle first, then any runtime that happens to be current.
    fn bridge_handle(&self) -> Option<tokio::runtime::Handle> {
        self.outer_rt
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok())
    }

    /// Run SQL against the app database from this (blocking) thread by
    /// spawning the async round-trip onto the main tokio runtime.
    fn sql(&self, sql: &str, params: Vec<String>) -> SqlReply {
        match self.bridge_handle() {
            Some(handle) => {
                let app = self.app.clone();
                let sql_owned = sql.to_string();
                let (reply_tx, reply_rx) = oneshot::channel();
                handle.spawn(async move {
                    let reply = exec_sql_in(&app, &sql_owned, params).await;
                    let _ = reply_tx.send(reply);
                });
                reply_rx.blocking_recv().unwrap_or(SqlReply {
                    rows: Vec::new(),
                    insert_id: 0,
                    affected: 0,
                    error: Some("sql bridge closed".into()),
                })
            }
            None => {
                // No runtime anywhere: drive it on a private current-thread one.
                let mut guard = self.rt.lock().expect("rt lock");
                let rt = guard
                    .get_or_insert_with(|| Builder::new_current_thread().enable_all().build().expect("rt"));
                rt.block_on(exec_sql_in(&self.app, sql, params))
            }
        }
    }

    /// Convenience: SQL that must not fail; logs and returns default.
    fn sql_or(&self, sql: &str, params: Vec<String>) -> SqlReply {
        let reply = self.sql(sql, params);
        if let Some(e) = &reply.error {
            self.log(&format!("sql error: {e} :: {sql}"));
        }
        reply
    }

    fn set_status(&self, level: &str, message: String) {
        let app = self.app.clone();
        let level = level.to_string();
        match self.bridge_handle() {
            Some(handle) => {
                handle.spawn(async move {
                    app.set_status(crate::state::Status { level, message }).await;
                });
            }
            None => drop((level, message)),
        }
    }

    fn current_playlist_id(&self) -> i64 {
        let general = self.app.general.read().expect("general lock");
        general
            .get_setting("CurrentPlaylist")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    fn current_now(&self) -> Option<NowPlaying> {
        self.app.current_now_playing()
    }
}

/// Execute SQL against the app database (async context path).
async fn exec_sql_in(app: &Arc<App>, sql: &str, params: Vec<String>) -> SqlReply {
    let db_guard = app.db.read().await;
    let Some(db) = db_guard.as_ref() else {
        return SqlReply {
            rows: Vec::new(),
            insert_id: 0,
            affected: 0,
            error: Some("not connected to a database".into()),
        };
    };
    exec_sql_on(db.pool(), sql, params).await
}

async fn exec_sql_on(
    pool: &sqlx::mysql::MySqlPool,
    sql: &str,
    params: Vec<String>,
) -> SqlReply {
    let mut conn = match pool.acquire().await {
        Ok(c) => c,
        Err(e) => {
            return SqlReply {
                rows: Vec::new(),
                insert_id: 0,
                affected: 0,
                error: Some(format!("acquire: {e}")),
            }
        }
    };
    // Re-write `:name` params into `?` when present (RadioDJ-style SQL).
    let (sql, named) = if sql.contains(':') {
        rewrite_named_params(sql)
    } else {
        (sql.to_string(), Vec::new())
    };
    let uses_named = !named.is_empty();
    let mut q = sqlx::query(&sql);
    if uses_named {
        // RadioDJ-style plugin SQL passes args as name/value pairs:
        // [":artist", "Queen", "artist", "Queen", ":path?", ...].
        for name in &named {
            q = q.bind(lookup_named_arg(&params, name));
        }
    } else {
        for p in &params {
            q = q.bind(p.clone());
        }
    }
    let is_select = sql
        .split_whitespace()
        .next()
        .map(|w| w.eq_ignore_ascii_case("select") || w.eq_ignore_ascii_case("show"))
        .unwrap_or(false);
    if is_select {
        match q.fetch_all(&mut *conn).await {
            Ok(rows) => SqlReply {
                rows: rows.iter().map(row_to_json).collect(),
                insert_id: 0,
                affected: rows.len() as u64,
                error: None,
            },
            Err(e) => SqlReply {
                rows: Vec::new(),
                insert_id: 0,
                affected: 0,
                error: Some(format!("select: {e}")),
            },
        }
    } else {
        match q.execute(&mut *conn).await {
            Ok(res) => {
                let insert_id = sqlx::query_scalar::<_, i64>("SELECT CAST(LAST_INSERT_ID() AS SIGNED)")
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap_or(0) as u64;
                SqlReply {
                    rows: Vec::new(),
                    insert_id,
                    affected: res.rows_affected(),
                    error: None,
                }
            }
            Err(e) => SqlReply {
                rows: Vec::new(),
                insert_id: 0,
                affected: 0,
                error: Some(format!("exec: {e}")),
            },
        }
    }
}

/// Map one sqlx row to a JSON object (best-effort per column type).
fn row_to_json(row: &sqlx::mysql::MySqlRow) -> Value {
    let mut map = serde_json::Map::new();
    for col in row.columns() {
        let name = col.name().to_string();
        let val = if let Ok(v) = row.try_get::<i32, _>(col.name()) {
            json!(v)
        } else if let Ok(v) = row.try_get::<i64, _>(col.name()) {
            json!(v)
        } else if let Ok(v) = row.try_get::<f64, _>(col.name()) {
            json!(v)
        } else if let Ok(v) = row.try_get::<String, _>(col.name()) {
            json!(v)
        } else if let Ok(v) = row.try_get::<Vec<u8>, _>(col.name()) {
            json!(String::from_utf8_lossy(&v).into_owned())
        } else if let Ok(v) = row.try_get::<bool, _>(col.name()) {
            json!(v)
        } else if let Ok(v) =
            row.try_get::<sqlx::types::chrono::NaiveDateTime, _>(col.name())
        {
            json!(v.format("%Y-%m-%d %H:%M:%S").to_string())
        } else if let Ok(v) = row.try_get::<sqlx::types::Decimal, _>(col.name()) {
            json!(v.to_string())
        } else {
            Value::Null
        };
        map.insert(name, val);
    }
    Value::Object(map)
}

impl Host for ServerHost {
    // ---- app switches ---------------------------------------------------------

    fn main_autodj(&self) -> bool {
        self.app.autodj_enabled()
    }

    fn set_main_autodj(&self, value: bool) {
        self.app.set_autodj_enabled(value);
        self.log(&format!("AutoDJ {}", if value { "enabled" } else { "disabled" }));
    }

    fn main_assisted(&self) -> bool {
        HOST_FLAGS.assisted.load(Ordering::Relaxed)
    }

    fn set_main_assisted(&self, value: bool) {
        HOST_FLAGS.assisted.store(value, Ordering::Relaxed);
        fan_plugin_event(
            &self.app.tx,
            &self.plugin_name,
            Some(StateInfo::new(if value {
                StateChange::AssistedEnabled
            } else {
                StateChange::ManualEnabled
            })),
            None,
        );
    }

    fn unmute_input(&self) -> bool {
        !self.app.engine.is_zone_muted(Zone::Instant)
    }

    fn set_unmute_input(&self, value: bool) {
        self.app.engine.set_zone_muted(Zone::Instant, !value);
        let _ = self.app.tx.send(self.app.state_frame());
    }

    fn track_loop(&self) -> bool {
        HOST_FLAGS.track_loop.load(Ordering::Relaxed)
    }

    fn set_track_loop(&self, value: bool) {
        HOST_FLAGS.track_loop.store(value, Ordering::Relaxed);
    }

    fn record(&self) -> bool {
        HOST_FLAGS.record.load(Ordering::Relaxed)
    }

    fn set_record(&self, value: bool) {
        HOST_FLAGS.record.store(value, Ordering::Relaxed);
        self.log(&format!("record switch -> {value}"));
    }

    fn pause_events(&self) -> bool {
        HOST_FLAGS.pause_events.load(Ordering::Relaxed)
    }

    fn set_pause_events(&self, value: bool) {
        HOST_FLAGS.pause_events.store(value, Ordering::Relaxed);
    }

    fn playlist2_full_screen(&self) -> bool {
        HOST_FLAGS.full_screen.load(Ordering::Relaxed)
    }

    fn set_playlist2_full_screen(&self, value: bool) {
        HOST_FLAGS.full_screen.store(value, Ordering::Relaxed);
    }

    fn search_results(&self) -> i32 {
        HOST_FLAGS.search_results.load(Ordering::Relaxed)
    }

    fn set_reload_zones(&self, _value: bool) {
        let _ = self.app.tx.send(self.app.state_frame());
    }

    fn title_main(&self) -> String {
        let general = self.app.general.read().expect("general lock");
        general.station_name().clone()
    }

    fn set_title_main(&self, value: &str) {
        // The web shell owns the window title; record the request as an
        // extra general key so any consumer (shell chrome) can read it.
        if let Err(e) = self.app.set_general_setting("MainTitle", value) {
            self.log(&format!("set_title_main failed: {e}"));
        }
    }

    fn get_type_title(&self, track_type: i32) -> String {
        type_title(track_type)
    }

    fn get_type_color(&self, track_type: i32) -> i32 {
        TYPE_COLORS
            .get(track_type as usize)
            .copied()
            .unwrap_or(0xC0C0C0)
    }

    /// `GetSoundCard(cardType)` — per-card track/line id lists, stored as
    /// extras in `settings_soundcards.xml` under `card_{n}` (comma list).
    fn get_sound_card(&self, card_type: i32) -> Vec<i32> {
        let path = self
            .app
            .config_dir
            .join(radiocraft_settings::SoundcardSettings::FILE);
        let store = radiocraft_settings::SoundcardSettings::load(&path).ok();
        store
            .as_ref()
            .and_then(|s| s.get_raw(&format!("card_{card_type}")))
            .unwrap_or_default()
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect()
    }

    // ---- volume / transport -----------------------------------------------------

    fn main_volume(&self) -> f32 {
        *self.app.general.read().expect("general lock").volume_out() as f32
    }

    fn set_main_volume(&self, value: f32) {
        let v = value.clamp(0.0, 100.0).round() as i64;
        if let Err(e) = self.app.set_general_setting("volume_out", &v.to_string()) {
            self.log(&format!("set_main_volume failed: {e}"));
        }
    }

    fn main_track_volume(&self) -> f32 {
        self.app.engine.master_db() as f32
    }

    fn set_main_track_volume(&self, value: f32) {
        self.app.engine.set_master_db(value as f64);
        let _ = self.app.tx.send(self.app.state_frame());
    }

    fn main_mute(&self) -> bool {
        self.app.engine.is_zone_muted(MAIN_ZONE)
    }

    fn set_main_mute(&self, value: bool) {
        self.app.engine.set_zone_muted(MAIN_ZONE, value);
        let _ = self.app.tx.send(self.app.state_frame());
    }

    fn set_play_from_intro(&self, value: bool) {
        self.restart_at_segment(value.then_some(Segment::Intro));
    }

    fn set_play_from_loop(&self, value: bool) {
        self.restart_at_segment(value.then_some(Segment::Loop));
    }

    fn set_play_from_outro(&self, value: bool) {
        self.restart_at_segment(value.then_some(Segment::Outro));
    }

    fn set_restart_track(&self, value: bool) {
        if value {
            self.restart_at_segment(None);
        }
    }

    fn pause(&self) -> bool {
        self.app.engine.is_paused(MAIN_ZONE)
    }

    fn set_pause(&self, value: bool) {
        if value {
            self.app.engine.pause(MAIN_ZONE);
        } else {
            self.app.engine.resume(MAIN_ZONE);
        }
    }

    fn set_stop_playing(&self, value: bool) {
        if value {
            // -1 = engine default fade (same convention as the stop op).
            self.app.engine.stop(MAIN_ZONE, -1);
        }
    }

    fn set_stop_playing_without_fade(&self, value: bool) {
        if value {
            // -999 = hard cut (same convention as the stop op).
            self.app.engine.stop(MAIN_ZONE, -999);
        }
    }

    fn set_update_playlist(&self, value: bool) {
        if value {
            fan_plugin_event(
                &self.app.tx,
                &self.plugin_name,
                Some(StateInfo::new(StateChange::PlaylistChanged)),
                None,
            );
        }
    }

    // ---- now playing -------------------------------------------------------------

    fn now_playing_handle(&self) -> i32 {
        self.current_now().map_or(0, |n| n.song_id as i32)
    }

    fn remaining_time(&self) -> f64 {
        self.current_now()
            .map(|n| self.app.now_playing_clock(&n).1)
            .unwrap_or(0.0)
    }

    // ---- playlist -----------------------------------------------------------------

    fn get_main_playlist(&self) -> Vec<SongData> {
        let pid = self.current_playlist_id();
        let reply = self.sql_or(
            "SELECT s.ID, s.artist, s.title, s.album, s.original_duration, s.duration, \
             s.song_type, s.path, s.cue_times FROM songs s JOIN playlists_list p ON p.sID = s.ID \
             WHERE p.pID = ? ORDER BY p.ord",
            vec![pid.to_string()],
        );
        reply.rows.iter().map(row_json_to_songdata).collect()
    }

    fn set_main_playlist(&self, playlist: &[SongData]) {
        let pid = self.current_playlist_id();
        for (i, track) in playlist.iter().enumerate() {
            let ord = (i + 1) as i64; // `ord` is 1-based, as playlist.rs keeps it
            let reply = self.sql_or(
                "UPDATE playlists_list SET sID = ? WHERE pID = ? AND ord = ?",
                vec![track.id.to_string(), pid.to_string(), ord.to_string()],
            );
            if reply.affected == 0 {
                self.sql_or(
                    "INSERT INTO playlists_list (pID, sID, ord) VALUES (?, ?, ?)",
                    vec![pid.to_string(), track.id.to_string(), ord.to_string()],
                );
            }
        }
    }

    fn set_clear_playlist(&self, value: bool) {
        if value {
            let pid = self.current_playlist_id();
            self.sql_or(
                "DELETE FROM playlists_list WHERE pID = ?",
                vec![pid.to_string()],
            );
            fan_plugin_event(
                &self.app.tx,
                &self.plugin_name,
                Some(StateInfo::new(StateChange::PlaylistChanged)),
                None,
            );
        }
    }

    fn add_to_playlist_by_id(&self, track_id: i64, position: i32) {
        let pid = self.current_playlist_id();
        if pid == 0 {
            self.log("add_to_playlist: no current playlist");
            return;
        }
        let ord = if position > 0 {
            position as i64
        } else {
            let reply = self.sql_or(
                "SELECT COALESCE(MAX(ord), 0) AS m FROM playlists_list WHERE pID = ?",
                vec![pid.to_string()],
            );
            reply
                .rows
                .first()
                .and_then(|r| json_get_i64(r, "m"))
                .map_or(1, |m| m + 1)
        };
        self.sql_or(
            "INSERT INTO playlists_list (pID, sID, ord) VALUES (?, ?, ?)",
            vec![pid.to_string(), track_id.to_string(), ord.to_string()],
        );
        fan_plugin_event(
            &self.app.tx,
            &self.plugin_name,
            Some(StateInfo::new(StateChange::PlaylistChanged)),
            None,
        );
    }

    fn add_to_playlist(&self, track: &SongData, position: i32) {
        self.add_to_playlist_by_id(track.id, position);
    }

    fn delete_from_playlist(&self, track_number: i32) {
        let pid = self.current_playlist_id();
        self.sql_or(
            "DELETE FROM playlists_list WHERE pID = ? AND ord = ?",
            vec![pid.to_string(), track_number.to_string()],
        );
        fan_plugin_event(
            &self.app.tx,
            &self.plugin_name,
            Some(StateInfo::new(StateChange::PlaylistChanged)),
            None,
        );
    }

    fn play_playlist_track(&self, track_number: i32, with_fade: bool) {
        let pid = self.current_playlist_id();
        let reply = self.sql_or(
            "SELECT s.path, s.cue_times, s.song_type FROM songs s \
             JOIN playlists_list p ON p.sID = s.ID WHERE p.pID = ? AND p.ord = ?",
            vec![pid.to_string(), track_number.to_string()],
        );
        let Some(row) = reply.rows.first() else {
            return;
        };
        let Some(path) = json_get_str(row, "path") else {
            return;
        };
        let track_type = json_get_i64(row, "song_type").unwrap_or(0) as i32;
        let knobs = parse_cue_times(&json_get_str(row, "cue_times").unwrap_or_default());
        match request_for_file(Path::new(&path), &knobs) {
            Ok((req, _)) => {
                if !with_fade {
                    // "Play without fade": cut whatever is sounding first.
                    self.app.engine.stop(MAIN_ZONE, -999);
                }
                if let Err(e) = self.app.engine.play(MAIN_ZONE, req, track_type, false) {
                    self.log(&format!("play_playlist_track: {e}"));
                }
            }
            Err(e) => self.log(&format!("play_playlist_track: {e}")),
        }
    }

    fn add_playlist_by_id(&self, _playlist_id: i64, _position: i32) -> bool {
        self.log("AddPlaylist(playlistID) is not supported by RadioCraft yet");
        false
    }

    fn add_playlist_by_path(&self, _path: &str, _position: i32) -> bool {
        self.log("AddPlaylist(path) is not supported by RadioCraft yet");
        false
    }

    // ---- queue ----------------------------------------------------------------------

    fn queue_count(&self) -> i32 {
        let reply = self.sql_or("SELECT COUNT(*) AS c FROM queuelist", Vec::new());
        reply
            .rows
            .first()
            .and_then(|r| json_get_i64(r, "c"))
            .unwrap_or(0) as i32
    }

    // ---- reload / refresh -------------------------------------------------------------

    fn set_refresh_events(&self, _value: bool) {
        self.log("RefreshEvents: server reads events on demand; nothing to do");
    }

    fn set_reload_categories(&self, _value: bool) {
        self.log("ReloadCategories: server reads categories on demand; nothing to do");
    }

    fn set_refresh_gui(&self, _value: bool) {
        let _ = self.app.tx.send(self.app.state_frame());
    }

    fn set_carts_changed(&self, _value: bool) {
        self.log("CartsChanged: clients poll carts; nothing to do");
    }

    fn set_cart_started(&self, value: i32) {
        fan_plugin_event(
            &self.app.tx,
            &self.plugin_name,
            Some(StateInfo::with_data(StateChange::CartStarted, json!(value))),
            None,
        );
    }

    fn set_main_player_started(&self, value: i32) {
        fan_plugin_event(
            &self.app.tx,
            &self.plugin_name,
            Some(StateInfo::with_data(
                StateChange::CurrentlyPlayingChanged,
                json!(value),
            )),
            None,
        );
    }

    // ---- misc ----------------------------------------------------------------------------

    fn application_path(&self) -> PathBuf {
        self.app.config_dir.clone()
    }

    fn get_songs(&self, sql: &str) -> Vec<SongData> {
        let reply = self.sql_or(sql, Vec::new());
        reply.rows.iter().map(row_json_to_songdata).collect()
    }

    fn state_changed(&self, state: &StateInfo) {
        fan_plugin_event(&self.app.tx, &self.plugin_name, Some(state.clone()), None);
        // Re-broadcasting into the plugin registry from inside a plugin
        // callback would re-enter the dispatcher; the hub frame carries the
        // event to every other consumer instead.
    }

    fn run_event(&self, event_id: i64) -> bool {
        let _ = event_id;
        self.log("RunEvent: playlist events are not executed by RadioCraft yet");
        false
    }

    // ---- import ------------------------------------------------------------------------------

    fn import_file(&self, request: &ImportRequest) -> i64 {
        match request {
            ImportRequest::FilePath {
                path,
                subcat_id,
                genre_id,
                ..
            } => {
                let p = Path::new(path);
                if !p.exists() {
                    self.log(&format!("import_file: no such file: {path}"));
                    return 0;
                }
                match existing_or_import(self, p, *subcat_id, *genre_id) {
                    Ok(id) => id as i64,
                    Err(e) => {
                        self.log(&format!("import_file failed: {e}"));
                        0
                    }
                }
            }
            ImportRequest::Track { track } => {
                let reply = self.sql_or(
                    "INSERT INTO songs (artist, title, album, path, original_duration, \
                     duration, song_type, date_added) VALUES (?, ?, ?, ?, ?, ?, ?, NOW())",
                    vec![
                        track.artist.clone(),
                        track.title.clone(),
                        track.album.clone(),
                        track.path.clone(),
                        track.original_duration.to_string(),
                        track.duration.to_string(),
                        track.track_type.to_string(),
                    ],
                );
                if let Some(e) = &reply.error {
                    self.log(&format!("import_file(track) failed: {e}"));
                    0
                } else {
                    reply.insert_id as i64
                }
            }
            ImportRequest::FilePaths {
                paths,
                subcat_id,
                genre_id,
                ..
            } => {
                let mut added = 0i64;
                for p in paths {
                    let pb = Path::new(p);
                    if pb.exists() {
                        if let Ok(id) = existing_or_import(self, pb, *subcat_id, *genre_id) {
                            if id > 0 {
                                added += 1;
                            }
                        }
                    }
                }
                added
            }
        }
    }

    fn update_history(&self, track: &SongData) -> bool {
        let track = track.clone();
        self.log(&format!("UpdateHistory({})", track.title));
        // Booked through the shared entities helper on the main runtime.
        let app = self.app.clone();
        let fut = async move {
            let db_guard = app.db.read().await;
            if let Some(db) = db_guard.as_ref() {
                let _ = radiocraft_db::entities::add_history_row(
                    db.pool(),
                    track.id as u32,
                    track.track_type,
                    &track.artist,
                    &track.title,
                )
                .await;
            }
        };
        match self.bridge_handle() {
            Some(handle) => {
                handle.spawn(fut);
            }
            None => {
                let mut guard = self.rt.lock().expect("rt lock");
                let rt = guard.get_or_insert_with(|| {
                    Builder::new_current_thread().enable_all().build().expect("rt")
                });
                rt.block_on(fut);
            }
        }
        true
    }

    fn show_ui(&self, request: PluginUiRequest) {
        fan_plugin_event(&self.app.tx, &self.plugin_name, None, Some(request));
    }

    fn play_cart_by_number(&self, cart_number: i32) {
        // Carts are ordinary songs rows hung under `carts_list` buttons;
        // play the button's song on the cart zone from position 0.
        let reply = self.sql_or(
            "SELECT s.ID, s.artist, s.title, s.path, s.cue_times, s.song_type, s.overlay \
             FROM songs s JOIN carts_list c ON c.swID = s.ID WHERE c.swButton = ?",
            vec![cart_number.to_string()],
        );
        let Some(row) = reply.rows.first() else {
            return;
        };
        let Some(path) = json_get_str(row, "path") else {
            return;
        };
        let knobs = parse_cue_times(&json_get_str(row, "cue_times").unwrap_or_default());
        let track_type = json_get_i64(row, "song_type").unwrap_or(0) as i32;
        let overlay = json_get_i64(row, "overlay").unwrap_or(0) != 0;
        match request_for_file(Path::new(&path), &knobs) {
            Ok((req, _)) => {
                // Cart plays start at position 0, same as the cart_play op.
                let req = req.starting_at(0.0);
                if let Err(e) = self.app.engine.play(Zone::Cart, req, track_type, overlay) {
                    self.log(&format!("play_cart_by_number: {e}"));
                }
            }
            Err(e) => self.log(&format!("play_cart_by_number: {e}")),
        }
    }

    // ---- logging ------------------------------------------------------------------------------

    fn debug_add_line(&self, entry: &str) {
        info!("[plugin {}] {entry}", self.plugin_name);
        // Mirror the line into the UI status strip, like the reference app's
        // debug window surfacing plugin chatter.
        self.set_status("info", entry.to_string());
    }

    // ---- sql + settings ---------------------------------------------------------------------------

    fn mysql_query(&self, sql: &str, args: &[&str]) -> String {
        let reply = self.sql_or(sql, args.iter().map(|s| s.to_string()).collect());
        if let Some(e) = &reply.error {
            return json!({ "error": e }).to_string();
        }
        serde_json::to_string(&reply.rows).unwrap_or_else(|_| "[]".into())
    }

    fn mysql_insert(&self, sql: &str, args: &[&str]) -> i32 {
        let reply = self.sql_or(sql, args.iter().map(|s| s.to_string()).collect());
        if reply.error.is_some() {
            -1
        } else if reply.insert_id > 0 {
            reply.insert_id as i32
        } else {
            reply.affected as i32
        }
    }

    fn get_setting(&self, file: &str, key: &str, default: &str) -> String {
        if file.is_empty() || file == radiocraft_settings::GeneralSettings::FILE {
            let general = self.app.general.read().expect("general lock");
            return general
                .get_setting(key)
                .map(|s| s.to_string())
                .unwrap_or_else(|| default.to_string());
        }
        let path = self.app.config_dir.join(file);
        match radiocraft_settings::PluginSettings::load(&path) {
            Ok(store) => store
                .get(key)
                .map(|s| s.to_string())
                .unwrap_or_else(|| default.to_string()),
            Err(_) => default.to_string(),
        }
    }

    fn save_setting(&self, file: &str, key: &str, value: &str) {
        if file.is_empty() || file == radiocraft_settings::GeneralSettings::FILE {
            if let Err(e) = self.app.set_general_setting(key, value) {
                self.log(&format!("save_setting({key}) failed: {e}"));
            }
            return;
        }
        let path = self.app.config_dir.join(file);
        let mut store = radiocraft_settings::PluginSettings::load(&path).unwrap_or_default();
        store.set(key, value);
        if let Err(e) = store.save_at(&path) {
            self.log(&format!("save_setting({file}: {key}) failed: {e}"));
        }
    }
}

impl ServerHost {
    /// Shared implementation of `PlayFromIntro/Loop/Outro/RestartTrack`:
    /// re-cue the currently sounding main-zone file at the chosen segment.
    fn restart_at_segment(&self, segment: Option<Segment>) {
        let Some(now) = self.current_now() else {
            return;
        };
        let path = Path::new(&now.path);
        if !path.exists() {
            return;
        }
        let knobs = CueKnobs {
            start: now.cue_start,
            next: now.cue_next,
            end: now.cue_end,
            loop_in: 0.0,
            loop_out: 0.0,
            fade_in_ms: now.fade_in_ms,
            fade_out_ms: now.fade_out_ms,
        };
        let offset = match segment {
            None | Some(Segment::Intro) => knobs.start,
            Some(Segment::Loop) if knobs.loop_out > knobs.loop_in => knobs.loop_in,
            Some(Segment::Loop) => knobs.start,
            Some(Segment::Outro) if knobs.next > 0.0 => knobs.next,
            Some(Segment::Outro) => knobs.start,
        };
        match request_for_file(path, &knobs) {
            Ok((req, _)) => {
                let req = req.starting_at(offset.max(0.0));
                if let Err(e) =
                    self.app
                        .engine
                        .play(MAIN_ZONE, req, now.track_type, false)
                {
                    self.log(&format!("restart track: {e}"));
                }
            }
            Err(e) => self.log(&format!("restart track: {e}")),
        }
    }
}

#[derive(Clone, Copy)]
enum Segment {
    Intro,
    Loop,
    Outro,
}

/// Reuse an existing library row for the path, else insert a new one.
/// Returns the song id (0 = already existed).
fn existing_or_import(
    host: &ServerHost,
    path: &Path,
    subcat_id: i64,
    genre_id: i64,
) -> Result<u32> {
    let raw = path.display().to_string();
    let exists = host.sql_or(
        "SELECT ID FROM songs WHERE path = ? LIMIT 1",
        vec![raw.clone()],
    );
    if let Some(row) = exists.rows.first() {
        if let Some(id) = json_get_i64(row, "ID") {
            return Ok(id as u32);
        }
    }
    let song = new_song_from_path(path)
        .with_context(|| format!("reading metadata from {}", path.display()))?;
    let reply = host.sql_or(
        "INSERT INTO songs (artist, title, path, duration, original_duration, song_type, \
         id_subcat, id_genre, date_added) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NOW())",
        vec![
            song.artist.clone(),
            song.title.clone(),
            song.path.clone(),
            song.duration.to_string(),
            song.original_duration.to_string(),
            song.song_type.to_string(),
            subcat_id.to_string(),
            genre_id.to_string(),
        ],
    );
    if let Some(e) = &reply.error {
        return Err(anyhow!(e.clone()));
    }
    Ok(reply.insert_id as u32)
}

// ---- JSON row helpers ---------------------------------------------------------

fn json_get_i64(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| match x {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

fn json_get_f64(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| match x {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().replace(',', ".").parse().ok(),
        _ => None,
    })
}

fn json_get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| match x {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// Map a songs-table JSON row (from `row_to_json`) to a `SongData`.
pub fn row_json_to_songdata(row: &Value) -> SongData {
    // Songs-table rows carry the columns every plugin path reads; anything
    // else keeps its default.
    SongData {
        id: json_get_i64(row, "ID").unwrap_or(0),
        artist: json_get_str(row, "artist").unwrap_or_default(),
        title: json_get_str(row, "title").unwrap_or_default(),
        album: json_get_str(row, "album_name")
            .or_else(|| json_get_str(row, "album"))
            .unwrap_or_default(),
        path: json_get_str(row, "path").unwrap_or_default(),
        track_type: json_get_i64(row, "track_type")
            .or_else(|| json_get_i64(row, "song_type"))
            .unwrap_or(0) as i32,
        original_duration: json_get_f64(row, "length")
            .or_else(|| json_get_f64(row, "original_duration"))
            .unwrap_or(0.0),
        duration: json_get_f64(row, "duration").unwrap_or(0.0),
        cue_times: json_get_str(row, "cue_times").unwrap_or_else(|| "&".into()),
        cue_start: json_get_f64(row, "cstart").unwrap_or(0.0),
        cue_end: json_get_f64(row, "cend").unwrap_or(0.0),
        cue_next: json_get_f64(row, "cnext").unwrap_or(0.0),
        ..SongData::default()
    }
}

#[cfg(test)]
mod sql_param_tests {
    use super::*;

    #[test]
    fn named_params_rewritten() {
        let (sql, names) = rewrite_named_params(
            "INSERT INTO t (a,b) VALUES (:artist?, :title, :path::text)",
        );
        // `:artist?` (optional) and `:title` become placeholders; the `?:text`
        // cast on `:path` stays attached to that one placeholder and does not
        // make it optional. The `\0` suffix marks an optional token.
        assert_eq!(sql, "INSERT INTO t (a,b) VALUES (?, ?, ?::text)");
        assert_eq!(names, vec!["artist\0".to_string(), "title".to_string(), "path".to_string()]);

        let (sql2, names2) = rewrite_named_params("SELECT :a, a::int FROM t");
        assert_eq!(sql2, "SELECT ?, a::int FROM t");
        assert_eq!(names2, vec!["a".to_string()]);
    }

    #[test]
    fn named_args_lookups() {
        let params = vec![
            ":artist".to_string(),
            "Queen".to_string(),
            "PATH".to_string(),
            "/music/bohemian.mp3".to_string(),
        ];
        assert_eq!(lookup_named_arg(&params, "artist"), "Queen");
        assert_eq!(lookup_named_arg(&params, "ARTIST"), "Queen");
        assert_eq!(lookup_named_arg(&params, ":path"), "/music/bohemian.mp3");
        // Missing optional name -> empty; missing required -> best effort.
        assert_eq!(lookup_named_arg(&params, "title\0"), "");
        assert_eq!(lookup_named_arg(&params, "title"), "");
    }

    #[test]
    fn comments_and_quotes_skipped() {
        let (sql, names) = rewrite_named_params(
            "SELECT ':not_param' AS lit, :real, -- :also_not\n:tail",
        );
        assert_eq!(sql, "SELECT ':not_param' AS lit, ?, -- :also_not\n?");
        assert_eq!(names, vec!["real".to_string(), "tail".to_string()]);
    }
}
