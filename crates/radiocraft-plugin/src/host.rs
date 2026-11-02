//! Host-facing API — the clean-room Rust port of `rdjInterface.IHost`.
//!
//! The authoritative contract is the decompiled interface at
//! `analysis/decompiled-csharp/PluginInterface/rdjInterface/IHost.cs`. Every
//! method below maps 1:1 onto a member of that interface, and the original C#
//! signature is recorded in each doc comment. Notable shape conversions:
//!
//! - C# `ref DataSet`/`ref DataGridView` out-params (`MysqlQuery`,
//!   `GetSongs`) become plain return values carrying JSON text (`String` of
//!   the query results; `get_songs` returns the rows as `Vec<SongData>`).
//! - `MysqlInsert(string query)` (parameterized) becomes
//!   `mysql_insert(sql, args) -> i32` (affected rows).
//! - The property get/set idiom becomes plain Rust getters/setters
//!   (`MainVolume { get; set; }` → `main_volume`/`set_main_volume`).
//! - Write-only properties (`StopPlaying { set; }`, `ClearPlaylist { set; }`,
//!   …) become methods named after the property (`set_stop_playing`).
//! - `int[] GetSoundCard(CardTypes)` takes the card type as `i32`
//!   (the `SoundCard.CardTypes` enum value) and returns `Vec<i32>`.
//! - `GetSongs(string sql, ref DataGridView dgv)` keeps SQL placeholders as a
//!   plain `&str`.
//! - UI window requests (`Show*`) are modeled as a serializable
//!   [`PluginUiRequest`] enum the host queues for its GUI layer.
//! - `StateChanged(Features, object)` becomes [`StateInfo`] with a JSON payload.
//! - `ImportFile` overloads collapse into one method taking an
//!   [`ImportRequest`] enum.
//!
//! All methods take `&self` (C# property setters mutate through a shared
//! reference too); host implementations use interior mutability. Every method
//! has a default implementation so hosts only implement what they support and
//! the ABI vtable fills the rest.

use std::path::PathBuf;

use radiocraft_core::model::SongData;
use serde::{Deserialize, Serialize};

/// One action a plugin can run, port of
/// `rdjInterface.Events.EventAction { name, id, args[] }` (docs/03 §3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventAction {
    /// Human-readable action name as shown in the event editor.
    pub name: String,
    /// Stable id used by the event runner (`events.id`-aligned).
    #[serde(default)]
    pub id: i64,
    /// Argument template tokens; concrete values arrive at run time.
    #[serde(default)]
    pub args: Vec<String>,
}

impl EventAction {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), id: 0, args: Vec::new() }
    }
    pub fn with_args(name: impl Into<String>, args: &[&str]) -> Self {
        Self { name: name.into(), id: 0, args: args.iter().map(|s| (*s).to_string()).collect() }
    }

    /// The `Name (Arg1, Arg2)` display form the event editor shows.
    pub fn display_string(&self) -> String {
        if self.args.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.args.join(", "))
        }
    }
}

/// Feature discriminator for host state notifications, stand-in for the
/// `rdjInterface.Features` enum. The exact IL enum values are not recoverable
/// (obfuscated), so variants carry the semantics observed in plugin IL call
/// sites and `Other(i32)` preserves any unrecognized raw value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateChange {
    /// Main player just started playing a track.
    #[default]
    PlayerStarted,
    /// Main player finished the current track.
    PlayerEnded,
    /// Cart/webstream player started (payload: player handle).
    CartStarted,
    /// Cart/webstream player ended (payload: player handle).
    CartEnded,
    /// Currently-playing metadata changed (payload: SongData JSON).
    CurrentlyPlayingChanged,
    /// AutoDJ switched on (payload: bool).
    AutoDjEnabled,
    /// AutoDJ switched off.
    AutoDjDisabled,
    /// Assisted (manual) mode switched on.
    AssistedEnabled,
    /// Assisted (manual) mode switched off.
    ManualEnabled,
    /// Track clock paused.
    Paused,
    /// Track clock resumed.
    Resumed,
    /// The playlist contents changed.
    PlaylistChanged,
    /// Tracks were reloaded from the database.
    TracksReloaded,
    /// Periodic timer tick (SmartEvents-style scan).
    Timer,
    /// Raw value not modeled above.
    Other(i32),
}

/// Payload of the original `StateChanged(Features Feature, object e)` call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StateInfo {
    pub feature: StateChange,
    /// The `object e` argument as JSON (often a SongData or player handle).
    #[serde(default)]
    pub data: serde_json::Value,
}

impl StateInfo {
    pub fn new(feature: StateChange) -> Self {
        Self { feature, data: serde_json::Value::Null }
    }
    pub fn with_data(feature: StateChange, data: serde_json::Value) -> Self {
        Self { feature, data }
    }
}

/// A UI window the host is asked to open, modeling the `Show*` members of
/// IHost. The host queues these for its GUI/event-loop layer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginUiRequest {
    /// `void ShowTrackDetails(TrackPlayer trackData, bool RefreshPlaylist, int Position = -1)`
    TrackDetails {
        track: SongData,
        refresh_playlist: bool,
        #[serde(default)]
        position: i32,
    },
    /// `void ShowTrackDetailsWindow(TrackPlayer trackData)`
    TrackDetailsWindow { track: SongData },
    /// `void ShowTrackEditor(TrackPlayer trackData, string PluginName, int Position = -1)`
    TrackEditor {
        track: SongData,
        plugin_name: String,
        #[serde(default)]
        position: i32,
    },
    /// `void ShowCategoriesEditor()`
    #[default]
    CategoriesEditor,
    /// `void ShowTracksWindow(string plugName, int trackType, double maxTrackDuration, int Position = -1)`
    TracksWindow {
        plugin_name: String,
        #[serde(default)]
        track_type: i32,
        #[serde(default)]
        max_track_duration: f64,
        #[serde(default)]
        position: i32,
    },
    /// `void ShowImportFolder()`
    ImportFolder,
}

/// Argument for [`Host::import_file`] — the three `ImportFile` overloads of
/// IHost collapsed into one enum.
// No `PartialEq`: core's `SongData` does not implement it and import payloads carry songs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // the `Track` payload is a SongData by design —
// it is the `ImportFile(TrackPlayer)` overload; boxing would only add churn
pub enum ImportRequest {
    /// `long ImportFile(string FilePath, bool fromURL, long subcatID = 0, long genreID = 0)`
    FilePath {
        path: String,
        #[serde(default)]
        from_url: bool,
        #[serde(default)]
        subcat_id: i64,
        #[serde(default)]
        genre_id: i64,
    },
    /// `long ImportFile(TrackPlayer FileData)`
    Track {
        track: SongData,
    },
    /// `long ImportFile(string[] filePaths, bool fromURL, long subcatID = 0, long genreID = 0)`
    FilePaths {
        paths: Vec<String>,
        #[serde(default)]
        from_url: bool,
        #[serde(default)]
        subcat_id: i64,
        #[serde(default)]
        genre_id: i64,
    },
}

impl Default for ImportRequest {
    fn default() -> Self {
        ImportRequest::FilePath {
            path: String::new(),
            from_url: false,
            subcat_id: 0,
            genre_id: 0,
        }
    }
}

/// The host API surface available to plugins: a Rust-shaped, JSON-transported
/// port of the `rdjInterface.IHost` members (139 lines, authoritative source:
/// `analysis/decompiled-csharp/PluginInterface/rdjInterface/IHost.cs`).
///
/// Every method has a default implementation (no-op / default value), matching
/// the "any subset of host features may be available" semantics of the ABI
/// vtable (NULL slots).
pub trait Host {
    // ---- IHost properties ---------------------------------------------------

    /// `bool MainAutoDJ { get; set; }`
    fn main_autodj(&self) -> bool {
        false
    }
    /// `bool MainAutoDJ { get; set; }` (setter)
    fn set_main_autodj(&self, _value: bool) {}

    /// `bool MainAssisted { get; set; }`
    fn main_assisted(&self) -> bool {
        false
    }
    /// `bool MainAssisted { get; set; }` (setter)
    fn set_main_assisted(&self, _value: bool) {}

    /// `bool UnmuteInput { get; set; }`
    fn unmute_input(&self) -> bool {
        false
    }
    /// `bool UnmuteInput { get; set; }` (setter)
    fn set_unmute_input(&self, _value: bool) {}

    /// `bool TrackLoop { get; set; }`
    fn track_loop(&self) -> bool {
        false
    }
    /// `bool TrackLoop { get; set; }` (setter)
    fn set_track_loop(&self, _value: bool) {}

    /// `bool Record { get; set; }`
    fn record(&self) -> bool {
        false
    }
    /// `bool Record { get; set; }` (setter)
    fn set_record(&self, _value: bool) {}

    /// `bool PauseEvents { get; set; }`
    fn pause_events(&self) -> bool {
        false
    }
    /// `bool PauseEvents { get; set; }` (setter)
    fn set_pause_events(&self, _value: bool) {}

    /// `bool Playlist2FullScreen { get; set; }`
    fn playlist2_full_screen(&self) -> bool {
        false
    }
    /// `bool Playlist2FullScreen { get; set; }` (setter)
    fn set_playlist2_full_screen(&self, _value: bool) {}

    /// `int SearchResults { get; }`
    fn search_results(&self) -> i32 {
        0
    }

    /// `bool ReloadZones { set; }`
    fn set_reload_zones(&self, _value: bool) {}

    /// `string TitleMain { get; set; }`
    fn title_main(&self) -> String {
        String::new()
    }
    /// `string TitleMain { get; set; }` (setter)
    fn set_title_main(&self, _value: &str) {}

    /// Parameterized property: `string get_GetTypeTitle(int TrackType)`
    fn get_type_title(&self, _track_type: i32) -> String {
        String::new()
    }

    /// Parameterized property: `int get_GetTypeColor(int TrackType)`
    fn get_type_color(&self, _track_type: i32) -> i32 {
        0
    }

    /// Parameterized property: `int[] get_GetSoundCard(SoundCard.CardTypes sc)`
    /// (`sc` is the `SoundCard.CardTypes` enum value as `i32`.)
    fn get_sound_card(&self, _card_type: i32) -> Vec<i32> {
        Vec::new()
    }

    // ---- main player ----------------------------------------------------------

    /// `float MainVolume { get; set; }`
    fn main_volume(&self) -> f32 {
        0.0
    }
    /// `float MainVolume { get; set; }` (setter)
    fn set_main_volume(&self, _value: f32) {}

    /// `float MainTrackVolume { get; set; }`
    fn main_track_volume(&self) -> f32 {
        0.0
    }
    /// `float MainTrackVolume { get; set; }` (setter)
    fn set_main_track_volume(&self, _value: f32) {}

    /// `bool MainMute { get; set; }`
    fn main_mute(&self) -> bool {
        false
    }
    /// `bool MainMute { get; set; }` (setter)
    fn set_main_mute(&self, _value: bool) {}

    /// `bool PlayFromIntro { set; }`
    fn set_play_from_intro(&self, _value: bool) {}

    /// `bool PlayFromLoop { set; }`
    fn set_play_from_loop(&self, _value: bool) {}

    /// `bool PlayFromOutro { set; }`
    fn set_play_from_outro(&self, _value: bool) {}

    /// `bool RestartTrack { set; }`
    fn set_restart_track(&self, _value: bool) {}

    /// `bool Pause { get; set; }`
    fn pause(&self) -> bool {
        false
    }
    /// `bool Pause { get; set; }` (setter)
    fn set_pause(&self, _value: bool) {}

    /// `bool StopPlaying { set; }`
    fn set_stop_playing(&self, _value: bool) {}

    /// `bool StopPlayingWithoutFade { set; }`
    fn set_stop_playing_without_fade(&self, _value: bool) {}

    /// `bool UpdatePlaylist { set; }`
    fn set_update_playlist(&self, _value: bool) {}

    /// `List<TrackPlayer> MainPlaylist { get; set; }`
    fn get_main_playlist(&self) -> Vec<SongData> {
        Vec::new()
    }
    /// `List<TrackPlayer> MainPlaylist { get; set; }` (setter)
    fn set_main_playlist(&self, _playlist: &[SongData]) {}

    /// `bool ClearPlaylist { set; }`
    fn set_clear_playlist(&self, _value: bool) {}

    /// `int QueueCount { get; }`
    fn queue_count(&self) -> i32 {
        0
    }

    /// `int NowPlayingHandle { get; }`
    fn now_playing_handle(&self) -> i32 {
        0
    }

    /// `double RemainingTime { get; }`
    fn remaining_time(&self) -> f64 {
        0.0
    }

    // ---- reload/refresh commands ----------------------------------------------

    /// `bool RefreshEvents { set; }`
    fn set_refresh_events(&self, _value: bool) {}

    /// `bool ReloadCategories { set; }`
    fn set_reload_categories(&self, _value: bool) {}

    /// `bool RefreshGUI { set; }`
    fn set_refresh_gui(&self, _value: bool) {}

    /// `bool CartsChanged { set; }`
    fn set_carts_changed(&self, _value: bool) {}

    /// `int CartStarted { set; }`
    fn set_cart_started(&self, _value: i32) {}

    /// `int MainPlayerStarted { set; }`
    fn set_main_player_started(&self, _value: i32) {}

    // ---- misc ------------------------------------------------------------------

    /// `string ApplicationPath { get; }`
    fn application_path(&self) -> PathBuf {
        PathBuf::new()
    }

    /// `int GetSongs(string sql, ref DataGridView dgv)` — the matched rows;
    /// the row count is the vec length.
    fn get_songs(&self, _sql: &str) -> Vec<SongData> {
        Vec::new()
    }

    /// `void StateChanged(Features Feature, object e)` — host-side state
    /// broadcast (plugins may re-broadcast state through this).
    fn state_changed(&self, _state: &StateInfo) {}

    /// `bool RunEvent(long eventID)`
    fn run_event(&self, _event_id: i64) -> bool {
        false
    }

    // ---- playlist commands -------------------------------------------------------

    /// `void PlayPlaylistTrack(int trackNumber, bool WithFade)`
    fn play_playlist_track(&self, _track_number: i32, _with_fade: bool) {}

    /// `void Add2Playlist(long trackID, int playlistPosition = 0)`
    fn add_to_playlist_by_id(&self, _track_id: i64, _position: i32) {}

    /// `void Add2Playlist(TrackPlayer trackData, int playlistPosition = 0)`
    fn add_to_playlist(&self, _track: &SongData, _position: i32) {}

    /// `void DeleteFromPlaylist(int trackNumber)`
    fn delete_from_playlist(&self, _track_number: i32) {}

    /// `bool AddPlaylist(long PlaylistID, int PlaylistPosition)`
    fn add_playlist_by_id(&self, _playlist_id: i64, _position: i32) -> bool {
        false
    }

    /// `bool AddPlaylist(string PlaylistPath, int PlaylistPosition)`
    fn add_playlist_by_path(&self, _path: &str, _position: i32) -> bool {
        false
    }

    /// `long ImportFile(...)` (three overloads, see [`ImportRequest`]).
    fn import_file(&self, _request: &ImportRequest) -> i64 {
        0
    }

    /// `bool UpdateHistory(SongData Track)`
    fn update_history(&self, _track: &SongData) -> bool {
        false
    }

    // ---- UI requests ---------------------------------------------------------------

    /// `void ShowTracksWindow(...)` / `ShowTrackDetails(...)` /
    /// `ShowTrackDetailsWindow(...)` / `ShowTrackEditor(...)` /
    /// `ShowImportFolder()` / `ShowCategoriesEditor()` — all `Show*` members
    /// funneled through one queued request.
    fn show_ui(&self, _request: PluginUiRequest) {}

    // ---- plugin actions -------------------------------------------------------------

    /// `List<Events.EventAction> GetPluginActions()`
    fn get_plugin_actions(&self) -> Vec<EventAction> {
        Vec::new()
    }

    /// `void RunPluginAction(Events.EventAction EventItem, string[] args = null)`
    fn run_plugin_action(&self, _action: &EventAction, _args: &[String]) {}

    /// `void PlayCartByNumber(int cartNumber)`
    fn play_cart_by_number(&self, _cart_number: i32) {}

    // ---- logging ---------------------------------------------------------------------

    /// `void DebugAddLine(string entry)`
    fn debug_add_line(&self, _entry: &str) {}

    // ---- rotations ----------------------------------------------------------------------

    /// `bool LoadRotationID(long rotationID)`
    fn load_rotation_id(&self, _rotation_id: i64) -> bool {
        false
    }

    // ---- database --------------------------------------------------------------------------

    /// `int MysqlQuery(string query, ref DataSet ds)` — results as
    /// JSON-encoded text of the DataSet (NULL/empty on failure).
    fn mysql_query(&self, _sql: &str, _args: &[&str]) -> String {
        String::new()
    }

    /// `int MysqlInsert(string query)` — affected rows.
    fn mysql_insert(&self, _sql: &str, _args: &[&str]) -> i32 {
        0
    }

    // ---- ini-style settings --------------------------------------------------------------------

    /// `bool SaveSetting(string FileName, string Key, string Value)`
    fn save_setting(&self, _file: &str, _key: &str, _value: &str) {}

    /// `string GetSetting(string FileName, string Key, string defaultValue = "0")`
    fn get_setting(&self, _file: &str, _key: &str, _default: &str) -> String {
        String::new()
    }
}
