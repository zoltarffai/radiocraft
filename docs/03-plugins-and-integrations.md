# 03 — Plugin API & Integration Contracts

Recovered from RadioDJ.exe IL (`.override [PluginInterface]rdjInterface.*` shims) — file `analysis/plugin-api-overrides.txt` (full signatures), `analysis/ihost-methods.txt`, `analysis/songdata-members.txt`. PluginInterface.dll is ConfuserEx-protected but the implementing shims in the main assembly expose the complete, readable contract. This is the ABI RadioCraft must mirror.

## 1. `rdjInterface.IHost` — host-facing API (73 members)

State get/set: `get/set_MainVolume` (float), `get/set_MainMute` (bool), `get/set_MainAutoDJ` (bool), `get/set_MainPlaylist` (?string/list), `get/set_MainTrackVolume`, `get/set_MainAssisted`, `get/set_Playlist2FullScreen`, `get/set_Pause`, `get_PauseEvents`, `get_ApplicationPath`, `get_NowPlayingHandle`, `get_GetTypeColor(int)`, `get_GetTypeTitle(int)`, `get_GetSoundCard(CardTypes)`.

Playback control: `PlayPlaylistTrack(int trackNumber, bool WithFade)`, `PlayCartByNumber(int cartNumber)`, `Add2Playlist(long trackID, int PlaylistPosition=0)`, `AddPlaylist(long PlaylistID, int position)`, `DeleteFromPlaylist(int trackNumber)`, `LoadRotationID(long rotationID)`, `RunEvent(long eventID)`, `StateChanged(...)`.

Data access: `GetSongs(string sqlFilter, ref DataGridView dgv) -> int`, `MysqlQuery(string query, ref DataSet ds) -> int`, `MysqlInsert(string query) -> int`, `GetSetting(string sender, string Key, string defaultValue="0") -> string`, `SaveSetting(string sender, string Key, string Value) -> bool`.

UI windows: `ShowTrackDetailsWindow`, `ShowTrackDetails`, `ShowTrackEditor`, `ShowCategoriesEditor`, `ShowTracksWindow`, `ShowImportFolder`.

Files: `ImportFile(string[] filePaths, bool fromURL, long subcatID=0, long genreID=0) -> long`.

Plugin actions: `GetPluginActions() -> List<EventAction>`, `RunPluginAction(EventAction, string[] args)`, `DebugAddLine(string entry)`, `UpdateHistory(...)`.

(Complete exact C#-level signatures in `analysis/plugin-api-overrides.txt`; RadioCraft's `radiocraft-plugin` crate defines `HostTrait` + `PluginTrait` with 1:1 semantics; each is bridged over a C ABI for cross-platform native plugins and optionally over JSON-RPC/stdio for scripted plugins.)

## 2. `rdjInterface.SongData` — track record DTO (87 members)

Identity/meta: ID, IDGenre, IDSubcat, TrackNo, Artist, OriginalArtist, AssociatedArtists, Title, Album, AlbumArt, Composer, Label, Publisher, Copyright, Isrc, Year, DiscNo, Language, MixType, Mood, Gender, Rating, Comments, AlternateTitle, BuyLink, URL1, URL2, ISRC, VtID, PlayerID, SwID, Sweepers, SwFirst, SwPlay.

Playout: Duration, OriginalDuration, Elapsed, Remaining, TrackType, CueStart/End/Next/Intro/Outro/LoopIn/LoopOut/HookIn/HookOut, CueTimes, B* mirror set (BCueStart…, BPosition — bass-unit variants), BPM, Loudness, BS1770, BSLoudness, HighPrecision, Weight, PlayLimit, LimitAction, Enabled, IsOverlay, SoundCardOutput, StartDate, EndDate, StartEvent, EndEvent, StartType, EndType, FadeIn, FadeOut, AutoFree, UseStreamTitle, DateAdded, DateModified, Path, IsOverlay.

Semantics: dual cue tables (timespan `Cue*` vs bass-unit `BCue*`); `Clone`/`Binara` fields (CloneBinara). RadioCraft: single Rust `SongData` struct, `f64` cue seconds + optional legacy mapping.

## 3. Event/Action model (`rdjInterface.Events/EventAction`)

EventAction { name, id, args[] } exposed to the event runner; actions enumerated by each plugin via `GetPluginActions`. Event trigger kinds (schema `events.event_type`): 0 calendar date, 1 clock, 2 weekly day, 3 track-count, with categories (`events_categories`), `StartType/EndType` validity windows, SmartEvents interval scan.

## 4. First-party feature plugins = RadioCraft built-in features (DLLs dumped to `il/Plugin_*.il`)

AuxPlayers, InstantPlayers (cart), Playlist (view), Search, TracksManager, SongDetails, History, Utilities (DB utils), SoundProcessing (FX chain), MetadataExport (now-playing export: file/web/serial/stream), FolderSync, plus AutoBackup (in Setup). Config keys per plugin are in docs/02 §10 and the shipped XMLs — RadioCraft implements these as first-class modules; the plugin ABI remains for third-party compatibility.

## 5. Integration surfaces to rebuild

1. **Metadata Export file/web/serial** — `$var$` template engine (docs/02 §7), web POST item templates (`Plugin_MetadataExport_web_items.xml` = per-item HTTP POST templates), serial Opticom protocol (SerialTemplate text file + baud settings).
2. **Web Now Playing** — `NowPlaying.txt` + template files (NowPlaying templates: radio-browser, SuperQuery, Sharpcast, MPD, Azuracast styles); PHP example scripts (`Docs/Metadata Export Help/PHP Script/song_update*.php` — receiver contract: POST of variable template).
3. **HTTP remote/requests server** — RemoteServer/RemoteUserID pairing, web requests (`requests` table).
4. **DB settings mode** — settings stored in `settings` DB table (StoreSettingsToDatabase), XML file format otherwise (`<NewDataSet><Settings><Key>/<Value>` layout).
5. **Reports** — SQL-backed listings, CSV + PDF export.
6. **AutoBackup** — daily scheduled data-only mysqldump-equivalent (INSERT-export preserving schema), hour + folder config, close-to-tray guard.

## 6. Track-type registry (index = `songs.song_type`)

0 Music, 1 Jingle, 2 Sweeper, 3 Voiceover, 4 Commercial, 5 Internet Stream, 6 Other, 7 VDF, 8 Podcast, 9 Request, 10 News, 11 Playlist Event, 12 File by Date, 13 Newest from Folder, 14 Teaser. Per-type title templates + colors (settings_titles.xml). Fade-out disabled for overlays; teaser rows kept in history as regular tracks w/ type flag.
