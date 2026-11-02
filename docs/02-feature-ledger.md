# 02 — Feature Ledger

Complete RadioDJ v3.0.0.5 feature inventory, mined from shipped config samples (`settings_*.xml`, `Plugins/Plugin_*.xml`), the changelog (`Docs/ChangesLog.txt`), bundled manuals, and the 13 first-party plugin DLLs. Every entry maps to a RadioCraft implementation unit.

## 1. Main application (window + tray)

| Feature | Evidence (settings keys) | Notes |
|---|---|---|
| Main window layout | WindowWidth/Height/Left/Top, LeftZonesDivider, MainZonesDivider, AlwaysOnTop, Minimize2Tray | resizable 4-zone grid; tray integration |
| Launch behavior | AutoDJStart, AutoPlay, PreventDoubleStart, AllVumeters | start AutoDJ, autoplay first playlist item, single-instance mutex |
| Passive/remote modes | IsPassiveMode, IsRemote, IsRemoteMode, RemoteServer, RemoteUserID | station-mode vs passive (no DB write) vs remote control of another instance |
| Sleep prevention | (3.0.0.0) system sleep prevention during playback | inhibit-sleep on playback |
| Localization | Language + 31 `language-*.xml` files (RTL for ar/fa/he) | string-table localization incl. RTL layout |
| Logging | LogActive, DebugEnable | rotating app log + debug log |
| Shortcuts & buttons | settings_shortcuts.xml, settings_buttons.xml, ShortcutButton1..5, EnableLineInTitle, LineInTitle/LineInArtist | hotkey map + 5 programmable launcher buttons |
| Custom tabs | settings_tabs.xml (zone2/3/4) | user-defined tool windows per zone |
| Info window | ShowInfoWindow | now-playing details popup |

## 2. Playout engine (BASS)

| Feature | Evidence | Notes |
|---|---|---|
| Playback zones | main player + zones 2/3/4 (aux/cart/instant) | separate BASS streams per zone |
| Cue support | CueStart/End/Next/Intro/Outro/LoopIn/LoopOut/HookIn/HookOut, AutoSelectCue | seek-based cue points; cue-aware next-track |
| Auto-fill | ThresholdStart/Next/End | silence-detection (BASS_CONFIG_REC_BUFFER capture) drives gap-free scheduler |
| Fades | dur_fadeout, in_Fade2Input, vol_overlay, vol_onsweeper, vol_vt, audio_mode, InputFadeIn/Out | crossfade engine w/ per-type volumes |
| VDF | Type 7 Variable Duration File + SweeperIntroOverride, SweeperOverrideFit | dynamic filler length |
| Streams | Type 5 internet streams (ICY/SH metadata, BassNet) | UseStreamTitle flag |
| Recording | RecordsFolder, LogActive | record streams to file |
| Waveforms | SaveWaveForm, AllVumeters | waveform cache + VU meters (bass VU) |
| BASS config | BASS_CONFIG_BUFFER / MIXER_BUFFER / REC_BUFFER / VISTA_TRUEPOS | device/init tuning |
| Silence removal | remove_silence | import-time silence trim |
| Loudness | BSTarget, UseBSLoudness, BS1770, BSLoudness, loudness column | ITU-R BS.1770 loudness scan |
| Aux/Cart/Instant players | Plugin_AuxPlayers (PlayersCount, PluginZone, StopFadeDuration), Plugin_InstantPlayers (DefaultCartID, DefaultInstantID, Columns, Rows, Sep*) | jocket/cart launch, hot-swap on air. RadioCraft: `play_cart` (cart zone) + `play_instant` (instant zone + program duck), `cartwall` module for wall/button CRUD + config, `CartWall.svelte` mounted for both zones + `CartwallEditor.svelte` config window; `Plugin_AuxPlayers` contract still open |
| Sound processing | Plugin_SoundProcessing: AGCEnabled, CompressorEnabled (cThreshold/cAttack/cRelease/cRatio/cGain), StereoEnhancerEnabled, eqON/eqValues, fxDampPreset, fxStereo, vol_Out | master FX chain: EQ, compressor, AGC, stereo enh, limiter |

## 3. AutoDJ scheduler

Rotation selection (tables `rotations` + `rotation_criteria`): category/subcategory inclusion-exclusion, genre filter, time-of-day windows, day-of-week bitmask, artist/title/album/track repeat intervals (RepeatArtistInteval, RepeatTitleInteval, RepeatAlbumInteval, RepeatTrackInterval), weight-based selection (weight column, ResetSubcategories, Priority, RecordsFolder priority, DAYS_2_KEEP_HISTORY), play-limit enforcement, `Newest from Folder` (type 13) and `File by Date` (type 12) modes (Files and Playlist By Date.pdf), `LoadRotationID` host API, LastUsedRotationID/LastUsedCategory/Genre/Subcategory/Path, ArtistSeparation, Search result playback.

Event system (`events` + `events_categories`, SmartEventsInterval): calendar/clock/weekly/track-count triggers; actions include RunRotation, PlayTrack/Cart, SQL Query True/False conditionals, If Playing Track Type Is/Not, Load File by Date, Load Playlist by Date (3.0.0.4 rewrite), web page/script execute, app start; EventsStopButton.

Playlist building: `playlist` + `playlist_events` tables, slots total (Plugin_Playlist SlotsTotal/Style), drag-drop reorder, Playlist by Date masks (pdf), Scheduled playlist restore (RestorePlaylistAt, SavePlaylistAt, LockPlaylist), RepeatAlbumInteval enforcement.

Requests: `requests` table (type 9, StartDate/EndDate validity, played flag booking via UpdateTracks2), request username/message variables, web integration.

## 4. Media library & import

`songs` table full CRUD, file import (drag/folder/URL), file scanning, metadata read/write (ID3v2, with ISO 639-2 language codes), artwork (ArtworkPath, AlbumArt image matching), FilenameFormat renamer, Database Utilities plugin: DeleteInvalidTracks, Update Associated Artists (Populate-only-where-empty), album-artist original_artist repair; FolderSync plugin (Plugin_FolderSyncList.xml: watch folders → auto-import/remove); ImportFile host API (paths, fromURL, subcat/genre assignment).

## 5. Search / browse / history

Plugin_Search: columns order + AdvancedFilters + search filters (ArtistPlayed, TitlePlayed, AlbumPlayed, Weight, CountPlayed, Intro, …), AutoRefresh. Plugin_TracksManager: category/genre browser, mass edit. Plugin_History: history viewer (RowsDisplay, ShowListeners, ListenersFile) backed by `history` table (trim per historyDays).

## 6. Metadata export & now-playing (Plugin_MetadataExport)

Targets: file (TargetPath, FileEncoding 0–2), web POST (Export2Web, TargetData, per-item POST templates `Plugin_MetadataExport_web_items.xml`), network (NetEnable, items xml), serial (Export2Serial + SerialTemplate Orban Opticom + baud/databits/parity/stopbits/dtr/rts/handshake), stream-server export (StreamServerExport). TypesToExport per track type, EnableCustomTitle/CustomTitle, AlternateTemplate + AlternateMetadata{Enable,Duration}, txtFileTemplate. Template engine: `$var$` substitution (case-sensitive, `_enc` URL-encode suffix, `next1_`..`next5_` prefixes) — full variable list in Docs/Stream Variables (see §7).

## 7. Template variable engine (canonical list)

station_name, station_slogan, autotitle, rotation_name, rotation_id, request_username, request_message, path, track_id, artist, original_artist, title, album, composer, label, publisher, copyright, comment, rating, mood, gender, language, isrc, image, year, track_no, disc_no, bpm, duration (HH:MM:SS), durationinSeconds, durationinMilliseconds, track-type, subcat_id, genre_id, buy_link, url1, url2, now-hour/-minute/-second/-year/-month/-day, now-shorttime/-longtime/-shortdate/-longdate, listeners; PRO-only: track_type_name, genre_name, cat_name, subcat_name; `next{1..5}_` prefix variants. Track types 0..14: Music, Jingle, Sweeper, Voiceover, Commercial, Internet Stream, Other, VDF, Podcast, Request, News, Playlist Event, File by Date, Newest from Folder, Teaser.

## 8. Web & remote integrations

- Web Now Playing tab: HTML template files (webnowplaying templates), `NowPlaying.txt` local refresh (Plugin_NowPlaying folder, .tpl templates for radio-browser/SuperQuery/Sharpcast/MPD/Azuracast styles), image export for artwork.
- Web streams / radio-browser registration, `web` feature settings.
- HTTP/TCP servers inside main app (remote control + remote instance mode, RemoteServer/RemoteUserID).
- NowPlaying plugin help folder + templates (Docs, NowPlaying templates).

## 9. Utilities & misc

AutoBackup (AutoBackupEnable/AutoBackupHour/BackupFolder; `Database Setup.exe backup` CLI, mysqldump-less data-only export per 3.0.0.0 changelog), Database Setup utility (validate/create/repair/upgrade SQL scripts), Reports (export incl. PDF per 3.0.0.3), settings backup/restore (StoreSettingsToDatabase option: settings live in `settings` DB table), per-track-type colors/titles (settings_titles.xml Color_*/Title_*), ColorPlayed/ColorDisabled/ColorStyle, full_clock, StationName/Slogan, vol_onsweeper/vol_overlay/vol_vt/volume_out, PreloadTracks, SmartEventsInterval, disable_user_management.

## 10. Plugin platform

See docs/03: IHost (73 methods) + IPlugin contract, per-plugin config keys above, plugin windows embedding (PluginZone), actions exposed via GetPluginActions/RunPluginAction.
