//! Host-facing API — the clean-room Rust port of `rdjInterface.IHost`.
//!
//! Every method below maps 1:1 onto an observed interface member from a real
//! RadioDJ-family app (IL data spec in `analysis/plugin-api-overrides.txt` /
//! `analysis/ihost-methods.txt`). The original C#-ish signature is recorded in
//! each doc comment. Notable shape conversions:
//!
//! - C# `ref DataSet`/`ref DataTable` out-params (`MysqlQuery`) become plain
//!   return values carrying JSON-encoded text (`String` of a DataSet-shaped
//!   object; `MysqlInsert` returns the affected-rows count).
//! - `GetSongs(sqlFilter, ref DataGridView&) -> int32` becomes
//!   `get_songs(sql_filter) -> Vec<SongData>`; the row count falls out of the
//!   vector length.
//! - The 73-property get/set idiom becomes plain Rust getters/setters
//!   (`get_MainVolume` → `main_volume`, `set_MainVolume` → `set_main_volume`).
//! - Command-style write-only properties (`set_StopPlaying`, `set_ClearPlaylist`,
//!   …) become methods named after the property.
//! - UI window requests (`Show*`) are modeled as a serializable
//!   [`PluginUiRequest`] enum the host queues for its GUI layer.
//! - `StateChanged(Features, object)` becomes [`StateInfo`] with a JSON payload.
//!
//! All methods take `&self` (C# property setters mutate through a shared
//! reference too); host implementations use interior mutability.

use std::path::PathBuf;

use radiocraft_core::model::SongData;
use serde::{Deserialize, Serialize};

/// One action a plugin can run, port of
/// `rdjInterface.Events.EventAction { name, id, args[] }` (docs/03 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// Feature discriminator for host state notifications, stand-in for the
/// `rdjInterface.Features` enum. The exact IL enum values are not recoverable
/// (obfuscated), so variants carry the semantics observed in plugin IL call
/// sites and `Other(i32)` preserves any unrecognized raw value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateChange {
    /// Main player just started playing a track.
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginUiRequest {
    /// `void ShowTrackDetails(TrackPlayer trackData, bool RefreshPlaylist, int PlaylistPosition = 0)`
    TrackDetails {
        track: SongData,
        refresh_playlist: bool,
        #[serde(default)]
        position: i32,
    },
    /// `void ShowTrackDetailsWindow(TrackPlayer TrackData)`
    TrackDetailsWindow { track: SongData },
    /// `void ShowTrackEditor(TrackPlayer trackData, string PluginName, int Position = 0)`
    TrackEditor {
        track: SongData,
        plugin_name: String,
        #[serde(default)]
        position: i32,
    },
    /// `void ShowCategoriesEditor()`
    CategoriesEditor,
    /// `void ShowTracksWindow(string plugName, int trackType, double minTrackDuration, int Position = 0)`
    TracksWindow {
        plugin_name: String,
        #[serde(default)]
        track_type: i32,
        #[serde(default)]
        min_track_duration: f64,
        #[serde(default)]
        position: i32,
    },
    /// `void ShowImportFolder()`
    ImportFolder,
}

/// The host API surface available to plugins: a Rust-shaped, JSON-transported
/// port of the 73 IHost members. See module docs for mapping rules. Methods
/// that the original exposes but RadioCraft's GUI/DB layer may not implement
/// yet carry a no-op / default-impl.
pub trait Host {
    // ---- core state: volume/mute/autodj -----------------------------------

    /// `float32 get_MainVolume()`
    fn main_volume(&self) -> f32;
    /// `void set_MainVolume(float32 'value')`
    fn set_main_volume(&self, value: f32);
    /// `bool get_MainMute()`
    fn main_mute(&self) -> bool;
    /// `void set_MainMute(bool 'value')`
    fn set_main_mute(&self, value: bool);
    /// `bool get_MainAutoDJ()`
    fn main_autodj(&self) -> bool;
    /// `void set_MainAutoDJ(bool 'value')`
    fn set_main_autodj(&self, value: bool);
    /// `float32 get_MainTrackVolume()`
    fn main_track_volume(&self) -> f32;
    /// `void set_MainTrackVolume(float32 'value')`
    fn set_main_track_volume(&self, value: f32);
    /// `bool get_Pause()`
    fn pause(&self) -> bool;
    /// `void set_Pause(bool 'value')`
    fn set_pause(&self, value: bool);

    // ---- main playlist ------------------------------------------------------

    /// `List<TrackPlayer> get_MainPlaylist()` — playlist rows as SongData.
    fn main_playlist(&self) -> Vec<SongData>;
    /// `void set_MainPlaylist(List<TrackPlayer> 'value')`
    fn set_main_playlist(&self, playlist: &[SongData]) {
        let _ = playlist;
    }

    // ---- playlist / playout control -----------------------------------------

    /// `void Add2Playlist(int64 trackID, int PlaylistPosition = 0)`
    fn add_to_playlist(&self, track_id: i64, position: i32);
    /// `bool AddPlaylist(int64 PlaylistID, int32 PlaylistPosition)`
    fn add_playlist(&self, playlist_id: i64, position: i32) -> bool;
    /// `void DeleteFromPlaylist(int32 trackNumber)`
    fn delete_from_playlist(&self, track_number: i32);
    /// `void PlayPlaylistTrack(int32 trackNumber, bool WithFade)`
    fn play_playlist_track(&self, track_number: i32, with_fade: bool);
    /// `void PlayCartByNumber(int32 cartNumber)`
    fn play_cart_by_number(&self, cart_number: i32);
    /// `bool LoadRotationID(int64 rotationID)`
    fn load_rotation_id(&self, rotation_id: i64) -> bool;
    /// `bool RunEvent(int64 eventID)`
    fn run_event(&self, event_id: i64) -> bool;

    // ---- misc state get/set --------------------------------------------------

    /// `bool get_MainAssisted()`
    fn main_assisted(&self) -> bool {
        false
    }
    /// `void set_MainAssisted(bool 'value')`
    fn set_main_assisted(&self, _value: bool) {}
    /// `bool get_PauseEvents()`
    fn pause_events(&self) -> bool {
        false
    }
    /// `void set_PauseEvents(bool 'value')`
    fn set_pause_events(&self, _value: bool) {}
    /// `bool get_Playlist2Full()`
    fn playlist2_full_screen(&self) -> bool {
        false
    }
    /// `void set_Playlist2Full(bool 'value')`
    fn set_playlist2_full_screen(&self, _value: bool) {}
    /// `bool get_Record()`
    fn record(&self) -> bool {
        false
    }
    /// `void set_Record(bool 'value')`
    fn set_record(&self, _value: bool) {}
    /// `bool get_TrackLoop()`
    fn track_loop(&self) -> bool {
        false
    }
    /// `void set_TrackLoop(bool 'value')`
    fn set_track_loop(&self, _value: bool) {}
    /// `bool get_UnmuteInput()`
    fn unmute_input(&self) -> bool {
        false
    }
    /// `void set_UnmuteInput(bool 'value')`
    fn set_unmute_input(&self, _value: bool) {}
    /// `int32 get_NowPlayingHandle()`
    fn now_playing_handle(&self) -> i32 {
        -1
    }
    /// `int32 get_QueueCount()`
    fn queue_count(&self) -> i32 {
        0
    }
    /// `float64 get_RemainingTime()`
    fn remaining_time(&self) -> f64 {
        0.0
    }
    /// `int32 get_SearchResults()`
    fn search_results(&self) -> i32 {
        0
    }
    /// `string get_titleMain()`
    fn title_main(&self) -> String {
        String::new()
    }
    /// `void set_titleMain(string 'value')`
    fn set_title_main(&self, _value: &str) {}
    /// `string get_ApplicationPath()`
    fn application_path(&self) -> PathBuf;

    /// `int32[] get_GetSoundCard(SoundCard.CardTypes sc)` — card index list
    /// for the given card type; raw i32 passthrough because the original
    /// `CardTypes` enum members are not recoverable.
    fn sound_cards(&self, card_type: i32) -> Vec<i32>;
    /// `int32 get_GetTypeColor(int32 TrackType)` — OLE color for a song type.
    fn get_type_color(&self, track_type: i32) -> i32;
    /// `string get_GetTypeTitle(int32 TrackType)` — display title per type
    /// (docs/03 §6 track-type registry).
    fn get_type_title(&self, track_type: i32) -> String;

    // ---- command-style write-only properties (original `set_*` members) -----

    /// `void set_CartStarted(int32 'value')`
    fn set_cart_started(&self, _value: i32) {}
    /// `void set_CartsChanged(bool 'value')`
    fn set_carts_changed(&self, _value: bool) {}
    /// `void set_ClearPlaylist(bool 'value')`
    fn set_clear_playlist(&self, _value: bool) {}
    /// `void set_MainPlayerStarted(int32 'value')`
    fn set_main_player_started(&self, _value: i32) {}
    /// `void set_PlayFromIntro(bool 'value')`
    fn set_play_from_intro(&self, _value: bool) {}
    /// `void set_PlayFromLoop(bool 'value')`
    fn set_play_from_loop(&self, _value: bool) {}
    /// `void set_PlayFromOutro(bool 'value')`
    fn set_play_from_outro(&self, _value: bool) {}
    /// `void set_RefreshEvents(bool 'value')`
    fn set_refresh_events(&self, _value: bool) {}
    /// `void set_RefreshGUI(bool 'value')`
    fn set_refresh_gui(&self, _value: bool) {}
    /// `void set_ReloadCategories(bool 'value')`
    fn set_reload_categories(&self, _value: bool) {}
    /// `void set_ReloadZones(bool 'value')`
    fn set_reload_zones(&self, _value: bool) {}
    /// `void set_RestartTrack(bool 'value')`
    fn set_restart_track(&self, _value: bool) {}
    /// `void set_StopPlaying(bool 'value')`
    fn set_stop_playing(&self, _value: bool) {}
    /// `void set_StopPlayingWithoutFade(bool 'value')`
    fn set_stop_playing_without_fade(&self, _value: bool) {}
    /// `void set_UpdatePlaylist(bool 'value')`
    fn set_update_playlist(&self, _value: bool) {}

    // ---- data access ----------------------------------------------------------

    /// `string GetSetting(string sender, string Key, string defaultValue = "0")`
    fn get_setting(&self, sender: &str, key: &str, default: &str) -> String;
    /// `bool SaveSetting(string sender, string Key, string Value)`
    fn save_setting(&self, sender: &str, key: &str, value: &str) -> bool;
    /// `int32 MysqlQuery(string query, ref DataSet ds)` — returns the JSON
    /// encoding of the DataSet (tables → arrays of row objects) the caller
    /// would have received through the `ref DataSet` out-param.
    fn mysql_query(&self, query: &str) -> String;
    /// `int32 MysqlInsert(string query)` — affected rows / autoincrement id.
    fn mysql_insert(&self, query: &str) -> i32;
    /// `int32 GetSongs(string sqlFilter, ref DataGridView dgv)` — matching
    /// rows as SongData (row count = Vec length).
    fn get_songs(&self, sql_filter: &str) -> Vec<SongData>;
    /// `int64 ImportFile(string[] filePaths, bool fromURL, long subcatID = 0, long genreID = 0)`
    /// — returns the new song id (0 on failure).
    fn import_file(
        &self,
        file_paths: &[PathBuf],
        from_url: bool,
        subcat_id: i64,
        genre_id: i64,
    ) -> i64;
    /// `bool UpdateHistory(SongData Track)`
    fn update_history(&self, track: &SongData) -> bool;

    // ---- UI -------------------------------------------------------------------

    /// Queues one of the `Show*Window`/editor window requests for the host GUI.
    fn show_ui(&self, request: PluginUiRequest);

    // ---- plugin actions / diagnostics ------------------------------------------

    /// `List<EventAction> GetPluginActions()` — actions offered by host-side
    /// feature plugins reachable through the host (mirrors plugin.actions()).
    fn get_plugin_actions(&self) -> Vec<EventAction>;
    /// `void RunPluginAction(EventAction EventItem, string[] args = null)`
    fn run_plugin_action(&self, action: &EventAction, args: &[String]);
    /// `void DebugAddLine(string entry)` — append to host debug log.
    fn debug_add_line(&self, entry: &str);
    /// `void StateChanged(Features Feature, object e)` — plugin announces a
    /// state change to the host for redistribution.
    fn state_changed(&self, state: &StateInfo);
}
