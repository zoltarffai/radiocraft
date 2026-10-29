//! `settings_general.xml` — application and UI configuration (docs/02 §1).

settings_struct! {
    /// Options persisted in `settings_general.xml`.
    ///
    /// Field names follow the RadioDJ key names; the typo `RepeatArtistInteval`
    /// is spelled with the same key string in the XML file.
    pub struct GeneralSettings {
        "Priority" => priority: i32 = 2,
        "Language" => language: String = "English (United States)",
        "StationName" => station_name: String = "RadioDJ",
        "StationSlogan" => station_slogan: String = "Your Station Slogan",

        // Audio engine (BASS) configuration.
        "BASS_CONFIG_BUFFER" => bass_config_buffer: i32 = 450,
        "BASS_CONFIG_REC_BUFFER" => bass_config_rec_buffer: i32 = 1000,
        "BASS_CONFIG_MIXER_BUFFER" => bass_config_mixer_buffer: i32 = 1,
        "BASS_CONFIG_VISTA_TRUEPOS" => bass_config_vista_truepos: bool = true,
        "in_Fade2Input" => in_fade2_input: bool = true,
        "vol_onsweeper" => vol_onsweeper: i32 = 5,
        "volume_out" => volume_out: i32 = 40,
        "vol_overlay" => vol_overlay: i32 = 3,
        "vol_vt" => vol_vt: i32 = 3,
        "dur_fadeout" => dur_fadeout: i32 = 3500,
        "dur_mixoverride" => dur_mixoverride: i32 = 500,
        "SweeperIntroOverride" => sweeper_intro_override: i32 = 60,
        "AutoDJStart" => auto_dj_start: bool = false,
        "AutoPlay" => auto_play: bool = false,
        "remove_silence" => remove_silence: bool = true,
        "ThresholdStart" => threshold_start: i32 = (-25),
        "ThresholdNext" => threshold_next: i32 = (-15),
        "ThresholdEnd" => threshold_end: i32 = (-28),

        // UI.
        "AlwaysOnTop" => always_on_top: bool = false,
        "Minimize2Tray" => minimize2_tray: bool = false,
        "ShowInfoWindow" => show_info_window: bool = false,
        "ShortcutButton1" => shortcut_button1: String = "Tracks Manager",
        "ShortcutButton2" => shortcut_button2: String = "Playlist Builder",
        "ShortcutButton3" => shortcut_button3: String = "Sound Processing",
        "ShortcutButton4" => shortcut_button4: String = "Utilities",
        "ShortcutButton5" => shortcut_button5: String = "Playing Info",
        "RightZonesDivider" => right_zones_divider: i32 = 193,
        "EnableArtwork" => enable_artwork: bool = true,
        "ArtworkPath" => artwork_path: String = "C:\\RadioDJv3\\Images",
        "DAYS_2_KEEP_HISTORY" => days_2_keep_history: i32 = 90,
        "LogActive" => log_active: bool = true,
        "full_clock" => full_clock: bool = true,
        "SearchResult" => search_result: i32 = 250,
        "RotationID" => rotation_id: i32 = 1,
        "DateFormat" => date_format: String = "dddd, d MMMM yyyy",
        "DefaultTrackPriority" => default_track_priority: i32 = 50,
        "RepeatTrackInterval" => repeat_track_interval: i32 = 120,
        /// Spelled with the original (sic) typo in the XML key.
        "RepeatArtistInteval" => repeat_artist_interval: i32 = 60,
        "ReducePriorityBy" => reduce_priority_by: i32 = 0,
        "SweeperOverrideFit" => sweeper_override_fit: bool = false,
        "ColorPlayed" => color_played: i32 = (-8388608),
        "ColorDisabled" => color_disabled: i32 = (-8388608),
        "HighlightArtist" => highlight_artist: bool = true,
        "PreloadTracks" => preload_tracks: i32 = 10,
        "StopFadeDuration" => stop_fade_duration: i32 = 1000,
        "ResetSubcategories" => reset_subcategories: bool = true,
        "SaveWaveForm" => save_wave_form: bool = false,
        "AutoSelectCue" => auto_select_cue: bool = false,
        "MainZonesDivider" => main_zones_divider: i32 = 908,
        "ArtistSeparation" => artist_separation: bool = true,
        "LastUsedPath" => last_used_path: String = "",
        "WindowTop" => window_top: i32 = 0,
        "WindowLeft" => window_left: i32 = 0,
        "WindowWidth" => window_width: i32 = 2576,
        "WindowHeight" => window_height: i32 = 1048,
        "RepeatAlbumInteval" => repeat_album_interval: i32 = 30,
        "IsRemote" => is_remote: bool = false,
        "LastUsedCategory" => last_used_category: i32 = 1,
        "LastUsedSubcategory" => last_used_subcategory: i32 = 4,
        "LastUsedGenre" => last_used_genre: i32 = 189,
        "SmartEventsInterval" => smart_events_interval: i32 = 30,
        "disable_user_management" => disable_user_management: bool = true,
        "RepeatTitleInteval" => repeat_title_interval: i32 = 120,
        "ColorStyle" => color_style: String = "Default",
        "RecordsFolder" => records_folder: String = "C:\\RadioDJv3\\Recordings",
        "AllVumeters" => all_vumeters: bool = false,
        "PreventDoubleStart" => prevent_double_start: bool = true,
        "KeepInPlaylistCount" => keep_in_playlist_count: i32 = 15,
        "RemoteUserID" => remote_user_id: String = "",
        "RemoteServer" => remote_server: String = "127.0.0.1",
        "IsPassiveMode" => is_passive_mode: bool = false,
        "IsRemoteMode" => is_remote_mode: bool = false,
        "EventsStopButton" => events_stop_button: bool = true,
        "Stopped_Artist" => stopped_artist: String = "RadioDJ",
        "Stopped_Title" => stopped_title: String = "Your Station Slogan",

        // Loudness normalisation (BS).
        "UseBSLoudness" => use_bs_loudness: bool = true,
        "BSTarget" => bs_target: i32 = (-12),

        // File naming.
        "FilenameFormat" => filename_format: String = "{artist} - {title}",
    }
}

impl GeneralSettings {
    /// The file this setting set is normally stored in.
    pub const FILE: &'static str = "settings_general.xml";

    /// Resolve a raw host setting (`General.GetSetting` analogue).
    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.get_raw(key)
    }

    /// Store a raw host setting (`General.SaveSetting` analogue). Invalid
    /// values for a known typed key are rejected (returns false).
    pub fn save_setting(&mut self, key: &str, value: &str) -> bool {
        self.set_raw(key, value)
    }
}
