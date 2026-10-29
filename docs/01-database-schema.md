# RadioCraft — 01. Database Schema Specification

**Source artifacts analyzed (complete corpus):**
- `reference/schema/database.sql` — full dump (schema + seed data), MariaDB 12.2.2, database `radiodj3`, exported by HeidiSQL 12.14.0.7165
- `reference/schema/database-nodefaults.sql` — same schema, seed rows removed (see §7 for the exact diff)

RadioDJ v3.0.0.5 uses MySQL/MariaDB database `radiodj3`: **17 tables + 1 stored procedure (`UpdateTracks2`)**. This document specifies every table verbatim (columns, exact MySQL types, defaults, nullability, indexes, PKs), the inferred role of each table, the stored procedure, the `settings` key/value pattern, and portability notes for a Rust/sqlx rebuild.

Key relationships (no enforced FKs anywhere — all relationships are soft, application-level):

```
category ──parentid──> subcategory <── id_subcat ── songs / history / queuelist(implicit)
   genre  <── id_genre ── songs / history / queuelist(implicit)

playlists ──pID──> playlists_list <──sID──> songs, <──swID/vtID──> songs (voice-track/sweper)
rotations ──pID──> rotations_list <──catID/subID/genID──> category/subcategory/genre
carts ──pID──> carts_list <──swID──> songs
events <──catID──> events_categories ; songs.startEvent/endEvent ── -1 = none
requests.songID ──> songs        queuelist.songID ──> songs (denominated snapshot rows)
history.trackID ──> songs (denominated playback audit, independent of songs lifecycle)
songs.path  UNIQUE  ← the natural identity key of the library
```

Convention notes that recur throughout:
- The sentinel `datetime DEFAULT '2002-01-01 00:00:01'` means "never played / no date set" (a deliberate fake epoch). Likewise `'&'` in varchar/text means "not set / all", `0` in int flags means "none/all", `-1` means "no reference".
- `tinyint(1)` columns are effectively booleans; `tinyint(2)` columns are enums.
- Decimal durations use `decimal(11,5)` — time values are seconds with 5-decimal precision.

---

## 1. Table-by-table specification

### 1.1 `carts` — cart-wall pages (containers)

On-screen cart walls ("cart books"); each row is one wall/page of buttons.

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `name` | `varchar(250)` | NOT NULL | — | wall name |

- PK: `PRIMARY KEY (ID)` (auto-increment)
- Engine `InnoDB`, charset `utf8mb4`, collate `utf8mb4_unicode_ci`.
- Seed row: `(1, 'Default')`.

### 1.2 `carts_list` — buttons on a cart wall

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `pID` | `int(10) unsigned` | NOT NULL | — | → `carts.ID` |
| `swID` | `int(10) unsigned` | NOT NULL | — | → `songs.ID` (track the button plays) |
| `swButton` | `int(10) unsigned` | NOT NULL | — | grid position/button index on the wall |
| `color` | `varchar(100)` | NOT NULL | — | button color (string-encoded, e.g. color name/RGB) |

- PK: `PRIMARY KEY (ID)`; KEY `fk_carts_list_pID (pID)` (name implies FK intent, but no constraint is defined).

### 1.3 `category` — top-level music/asset categories

Root of the two-level classification (category → subcategory) used by library, rotations, and history stats.

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `name` | `varchar(100)` | NOT NULL | — | |

- PK: `PRIMARY KEY (ID)`
- Seed rows (11): `1 Music, 2 Sound Effects, 3 Sweepers, 4 Station IDs, 5 Jingles, 6 Promos, 7 Commercials, 8 News, 9 Interviews, 10 Radio Shows, 11 Radio Streams`.

### 1.4 `subcategory` — second-level classification (with sweeper chaining)

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `parentid` | `int(10) unsigned` | NOT NULL | — | → `category.ID` |
| `name` | `varchar(100)` | NOT NULL | — | |
| `sweeper_subID` | `int(10)` | NOT NULL | `0` | subcategory whose tracks act as sweeper separators when tracks of this subcategory play (`0` = none) |

- PK: `PRIMARY KEY (ID)`
- Seed rows (22, IDs 1–22): `90s/80s/Oldies/2000s/Heavy Rotation/Top 40` under Music(1); `Contest Effects`(2); `Daily/Nightly Sweepers`(3); `Artist IDs`(4); `Station Jingles`(5); `Radio Show Promos`(6); `Sponsor/Paid Commercials`(7); `Daily News`(8); `Music/Political Interviews`(9); `My First/Second Show`(10); `Other/Syndicated Shows/Syndicated News`(11).

### 1.5 `genre` — music genres

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `id` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK (**lower-case `id`** — this table uses lowercase PK unlike the others) |
| `name` | `varchar(50)` | NOT NULL | — | |

- PK: `PRIMARY KEY (id)`; UNIQUE `uc_name (name)` — the only table with a uniqueness rule on its name.
- Seed rows: 148 WinAmp-style genre names (IDs 1–148: Acapella, Acid, … Vocal).

### 1.6 `events` — timed calendar/clock events (heartbeat triggers)

The automation "clock": scheduled triggers that fire actions (play a playlist, start AutoDJ, run a web feature, etc.) at time/date/day-of-week criteria.

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `type` | `tinyint(2)` | NOT NULL | — | event type enum (e.g. timed/once/daily/web/recurrent — coded integer) |
| `time` | `varchar(20)` | NOT NULL | — | time-of-day trigger, free-form string (e.g. `HH:mm`, or a future datetime) |
| `name` | `varchar(200)` | NOT NULL | — | event name |
| `date` | `date` | NULL | `'2002-01-01'` | one-shot date (fake epoch = "unset") |
| `day` | `varchar(30)` | NULL | `'&'` | day-of-week mask (`&` = all days) |
| `hours` | `varchar(100)` | NULL | `'&'` | hour mask (`&` = all hours) |
| `data` | `text` | NULL | NULL | action payload (playlist name, command args, URL, etc.) |
| `enabled` | `tinyint(1)` | NULL | NULL | active flag (NULL = not yet set) |
| `catID` | `int(11)` | NOT NULL | `0` | → `events_categories.ID` (signed, unlike other FK-ish columns) |
| `smart` | `tinyint(1)` | NOT NULL | `0` | "smart" event flag (event re-evaluates its own conditions rather than firing blindly) |
| `grouped` | `tinyint(1)` | NOT NULL | `0` | grouping flag (linked/child events of a parent) |

- PK: `PRIMARY KEY (ID)`
- Indexes: `idx_events_catid (catID)`, `idx_events_smart (smart)`, `idx_events_enabled (enabled) USING BTREE`, `idx_events_date (date, enabled) USING BTREE`.

### 1.7 `events_categories` — grouping for events

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `name` | `varchar(200)` | NOT NULL | — | |

- PK: `PRIMARY KEY (ID)`; seed row: `(1, 'Default')`.

### 1.8 `history` — immutable playback audit log

One row per played occurrence (written by `UpdateTracks2` when `historyDays > 0`). Metadata is **denominated at play time** (copied in), so history survives song deletion. This is the source for daypart/stat reports and the importers of "file by date".

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `trackID` | `int(10) unsigned` | NOT NULL | — | → `songs.ID` (soft) |
| `date_played` | `datetime` | NULL | `'2002-01-01 00:00:01'` | insertion timestamp (NOW()) |
| `song_type` | `tinyint(2)` | NOT NULL | — | type at time of play (`tType` param) |
| `id_subcat` | `int(10) unsigned` | NOT NULL | — | subcategory at time of play |
| `id_genre` | `int(10) unsigned` | NOT NULL | — | genre at time of play |
| `duration` | `decimal(11,5)` | NOT NULL | `0.00000` | |
| `artist` | `varchar(200)` | NOT NULL | — | played artist (`curArtist`, may be rewritten) |
| `original_artist` | `varchar(200)` | NOT NULL | — | |
| `title` | `varchar(200)` | NOT NULL | — | played title |
| `album` | `varchar(200)` | NOT NULL | — | |
| `composer` | `varchar(200)` | NOT NULL | — | |
| `label` | `varchar(250)` | NOT NULL | — | (250, unlike the 200s around it) |
| `year` | `varchar(4)` | NOT NULL | `'1900'` | string year |
| `track_no` | `int(11)` | NOT NULL | `0` | |
| `disc_no` | `int(11)` | NOT NULL | `0` | |
| `publisher` | `varchar(200)` | NOT NULL | — | |
| `copyright` | `varchar(200)` | NOT NULL | — | |
| `isrc` | `varchar(200)` | NOT NULL | — | |
| `listeners` | `int(10) unsigned` | NULL | NULL | listener count at play time (`curListeners`) |

- PK: `PRIMARY KEY (ID)`
- Indexes: `idx_history_date (date_played)`; `idx_history_track_search (trackID, date_played)`; `idx_history_artist_title (artist(100), title(100))` (prefix index); `idx_history_meta (song_type, id_subcat, id_genre)`; **FULLTEXT** `ft_history_search (artist, title, album, composer, publisher)`.
- Pruning: rows older than `historyDays` are deleted in chunks of 100 by the procedure.

### 1.9 `playlists` — named playlist containers

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `name` | `varchar(200)` | NOT NULL | — | |

- PK: `PRIMARY KEY (ID)`; UNIQUE `name (name)` (playlist names are globally unique — events reference them by name text in `events.data`).

### 1.10 `playlists_list` — playlist items with per-item cue/mix settings

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `pID` | `int(10) unsigned` | NOT NULL | — | → `playlists.ID` |
| `sID` | `int(10)` **signed** | NOT NULL | — | → `songs.ID` (signedness differs; likely allows negative/sentinel values) |
| `cstart` | `decimal(11,5)` | NOT NULL | `0.00000` | cue start |
| `cnext` | `decimal(11,5)` | NOT NULL | `0.00000` | cue next/hook |
| `cend` | `decimal(11,5)` | NOT NULL | `0.00000` | cue end |
| `fin` | `decimal(11,5)` | NOT NULL | `0.00000` | fade-in length |
| `fout` | `decimal(11,5)` | NOT NULL | `0.00000` | fade-out length |
| `swID` | `int(10) unsigned` | NOT NULL | — | sweeper song ID (→ `songs.ID`) |
| `swplay` | `decimal(11,5)` | NOT NULL | `0.00000` | time offset within track at which sweeper starts |
| `vtID` | `int(10) unsigned` | NOT NULL | — | voice-track song ID (→ `songs.ID`) |
| `vtplay` | `decimal(11,5)` | NOT NULL | `0.00000` | voice-track start offset |
| `swfirst` | `tinyint(1)` | NOT NULL | `0` | play sweeper before (instead of over) the track |
| `ord` | `int(10) unsigned` | NOT NULL | — | item order in playlist |

- PK: `PRIMARY KEY (ID)`; redundant `KEY ID (ID)`; `idx_playlists_list_pid (pID)`, `idx_playlists_list_sid (sID)`, `idx_playlists_list_swid (swID)`, `idx_playlists_list_vtid (vtID)`.

### 1.11 `queuelist` — live on-air queue / future log (denominated)

Runtime scratch table: each row is a scheduled future play with a computed `ETA`. Metadata is copied in (survives library edits mid-broadcast, and can hold rows whose `songID=0` for event/placeholder entries).

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `songID` | `int(11)` signed | NOT NULL | `0` | → `songs.ID` (`0` = non-song entry) |
| `ETA` | `datetime` | NULL | `'2002-01-01 00:00:01'` | estimated start time (fake epoch = not yet scheduled) |
| `duration` | `decimal(11,5)` | NOT NULL | `0.00000` | |
| `artist` | `varchar(200)` | NOT NULL | — | |
| `associated_artists` | `varchar(250)` | NOT NULL | `''` | |
| `title` | `varchar(250)` | NOT NULL | — | |
| `album` | `varchar(250)` | NOT NULL | — | |
| `swID` | `int(11)` signed | NOT NULL | `-1` | sweeper song ID, `-1` = none |
| `swPlay` | `decimal(11,5)` | NOT NULL | `0.00000` | (camelCase `swPlay` here vs `swplay` in playlists_list — case differs between tables) |
| `vtID` | `int(11)` signed | NOT NULL | `-1` | voice-track song ID, `-1` = none |
| `vtPlay` | `decimal(11,5)` | NOT NULL | `0.00000` | |

- PK: `PRIMARY KEY (ID)`; indexes: `idx_queuelist_songid (songID)`, `idx_queuelist_eta (ETA)`, `idx_queuelist_swid (swID)`, `idx_queuelist_vtid (vtID)`.

### 1.12 `requests` — listener request queue

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `songID` | `int(10) unsigned` | NOT NULL | — | → `songs.ID` |
| `username` | `varchar(200)` | NOT NULL | `'Anomymous'` | sic — deliberate misspelling of "Anonymous"; must be reproduced exactly for parity |
| `userIP` | `varchar(50)` | NOT NULL | — | |
| `message` | `text` | NULL | NULL | free-text request message |
| `requested` | `datetime` | NOT NULL | — | request timestamp (no default) |
| `played` | `tinyint(1)` | NOT NULL | `0` | set to 1 by `UpdateTracks2` when a request-type play fires |

- PK: `PRIMARY KEY (ID)`; indexes: `idx_requests_songid`, `idx_requests_username`, `idx_requests_requested`.

### 1.13 `rotations` — named rotation (AutoDJ clock) containers

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `name` | `varchar(200)` | NOT NULL | — | |

- PK: `PRIMARY KEY (ID)`; seed row `(1, 'Default')`.

### 1.14 `rotations_list` — rotation rules ("clock wheel" rows) driving AutoDJ/heartbeat

Each row defines what music to select during a daypart window and how to separate it. This is the core AutoDJ heart: on each "next track" tick the engine picks rules (`ord` position within the rotation), matches category/subcategory/genre (+ mood/ggender/language), and chooses a song from `songs` with separation & replay rules.

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `pID` | `int(10) unsigned` | NOT NULL | — | → `rotations.ID` |
| `catID` | `int(10)` signed | NOT NULL | — | → `category.ID` (0 = any) |
| `subID` | `int(10)` signed | NOT NULL | — | → `subcategory.ID` (0 = any) |
| `genID` | `int(10)` signed | NOT NULL | — | → `genre.ID` (0 = any) |
| `mood` | `varchar(250)` | NULL | NULL | mood filter (multi-value string) |
| `gender` | `varchar(250)` | NULL | NULL | singer-gender filter |
| `language` | `varchar(250)` | NULL | NULL | language filter |
| `start_type` | `int(11)` | NOT NULL | `0` | daypart rule start selector (0 = all) |
| `end_type` | `int(11)` | NOT NULL | `0` | daypart rule end selector |
| `selType` | `tinyint(1)` | NOT NULL | — | selection mode (oldest/random/weighted…) |
| `sweeper` | `tinyint(1)` | NOT NULL | — | sweeper selector flag (which sweeper pool/none) |
| `repeatRule` | `tinyint(1)` | NOT NULL | `0` | repetition behaviour |
| `ord` | `tinyint(2)` | NOT NULL | — | position in the wheel (note: `tinyint(2)`, vs `int(10) unsigned` in playlists_list) |
| `data` | `text` | NOT NULL | — | rule payload (daypart times / recurrence spec) |
| `track_separation` | `int(11)` | NOT NULL | `0` | min distance before same track can re-air |
| `artist_separation` | `int(11)` | NOT NULL | `0` | same-artist separation |
| `title_separation` | `int(11)` | NOT NULL | `0` | similar-title separation |
| `album_separation` | `int(11)` | NOT NULL | `0` | same-album separation |
| `advanced` | `text` | NULL | NULL | advanced selection criteria |

- PK: `PRIMARY KEY (ID)`; indexes: `idx_rotations_list_pid`, `idx_rotations_list_catid`, `idx_rotations_list_subid`, `idx_rotations_list_genid`.

### 1.15 `settings` — key/value store (no PK)

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `source` | `varchar(150)` | NOT NULL | — | namespacing/group of the setting |
| `setting` | `varchar(150)` | NOT NULL | — | key name |
| `value` | `varchar(250)` | NOT NULL | — | value as text |

- **No PK, no indexes, no unique constraint** — the table has no `ID` at all; rows are addressed by `(source, setting)` by convention only (updates are `WHERE source=… AND setting=…`; duplicates are possible unless the app enforces it). See §4 for the pattern and note on defaults.

### 1.16 `songs` — the music/asset library (the hub table)

65 columns. Everything on-air points here through `ID` or the soft references below.

| Column | Type | Null | Default | Notes / role |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `path` | `varchar(250)` | NOT NULL | — | **UNIQUE** full filesystem path — natural key of the library |
| `enabled` | `tinyint(1)` | NOT NULL | `0` | in-rotation flag (0 = not eligible for AutoDJ) |
| `date_added` | `datetime` | NULL | `'2002-01-01 00:00:01'` | import time |
| `date_modified` | `datetime` | NULL | `'2002-01-01 00:00:01'` | |
| `date_played` | `datetime` | NULL | `'2002-01-01 00:00:01'` | last play of *this* track (rotated = higher pick priority for "oldest") |
| `artist_played` | `datetime` | NULL | `'2002-01-01 00:00:01'` | last time *any* track by this artist played |
| `album_played` | `datetime` | NULL | `'2002-01-01 00:00:01'` | last time *any* track of this album played |
| `title_played` | `datetime` | NULL | `'2002-01-01 00:00:01'` | last time *any* track with same title played |
| `count_played` | `int(11)` | NOT NULL | `0` | total plays |
| `play_limit` | `int(11)` | NOT NULL | `0` | max plays (0 = unlimited) |
| `limit_action` | `tinyint(1)` | NOT NULL | `0` | action when limit hit: 0 none, 1 disable, 2 delete, negative N = move to subcategory N (see procedure §3) |
| `start_date` | `datetime` | NULL | `'2002-01-01 00:00:01'` | availability window start |
| `end_date` | `datetime` | NULL | `'2002-01-01 00:00:01'` | availability window end |
| `startEvent` | `int(11)` signed | NOT NULL | `-1` | event that enables this track (-1 = none) |
| `endEvent` | `int(11)` signed | NOT NULL | `-1` | event that disables/expires it (-1 = none) |
| `song_type` | `tinyint(2)` | NOT NULL | — | track type enum (see §1.17 note; 0 = Music … 9 = Request) |
| `id_subcat` | `int(10) unsigned` | NOT NULL | — | → `subcategory.ID` |
| `id_genre` | `int(10) unsigned` | NOT NULL | — | → `genre.ID` |
| `weight` | `decimal(5,2)` | NOT NULL | `0.00` | AutoDJ weight (reduced by `pWeight` after play) |
| `duration` | `decimal(11,5)` | NOT NULL | `0.00000` | effective duration (with cues applied) |
| `original_duration` | `decimal(11,5)` | NOT NULL | `0.00000` | raw file duration |
| `cue_times` | `varchar(250)` | NOT NULL | `'&'` | cue points encoded as string (`&` = none/default) |
| `precise_cue` | `tinyint(1)` | NOT NULL | `0` | strict cue cutting flag |
| `fade_type` | `tinyint(1)` | NOT NULL | `0` | automatic fade style |
| `start_type` | `tinyint(1)` | NOT NULL | `0` | start method (e.g. fresh start vs overlap) |
| `end_type` | `tinyint(1)` | NOT NULL | `0` | end method (e.g. music fade vs hard stop) |
| `mix_type` | `tinyint(1)` | NOT NULL | `0` | mix/transition class |
| `mood` | `varchar(150)` | NOT NULL | `'Not Set'` | mood tag (matched by rotations_list.mood) |
| `gender` | `varchar(150)` | NOT NULL | `'Not Set'` | singer gender tag |
| `lang` | `varchar(150)` | NOT NULL | `'und'` | language tag (ISO-style `und` = undetermined) |
| `rating` | `int(11)` | NOT NULL | `0` | metadata rating |
| `loudness` | `decimal(8,3)` | NOT NULL | `0.000` | loudness (Sound Processing plugin) |
| `bs_loudness` | `decimal(8,3)` | NOT NULL | `0.000` | ReplayGain-style BS value |
| `bs1770` | `decimal(8,3)` | NOT NULL | `0.000` | EBU R128 / ITU BS.1770 loudness |
| `overlay` | `tinyint(1)` | NOT NULL | `0` | overlay/bed-players eligible flag |
| `artist` | `varchar(250)` | NOT NULL | — | |
| `associated_artists` | `varchar(250)` | NOT NULL | `''` | featured artists |
| `original_artist` | `varchar(250)` | NOT NULL | — | un-rewritten artist (copied into history) |
| `title` | `varchar(250)` | NOT NULL | — | |
| `alternate_title` | `varchar(250)` | NOT NULL | — | |
| `album` | `varchar(250)` | NOT NULL | — | |
| `composer` | `varchar(250)` | NOT NULL | — | |
| `label` | `varchar(250)` | NOT NULL | — | |
| `year` | `varchar(4)` | NOT NULL | `'1900'` | string year |
| `track_no` | `int(11)` | NOT NULL | `0` | |
| `disc_no` | `int(11)` | NOT NULL | `0` | |
| `publisher` | `varchar(250)` | NOT NULL | — | |
| `copyright` | `varchar(250)` | NOT NULL | — | |
| `isrc` | `varchar(250)` | NOT NULL | — | |
| `bpm` | `decimal(6,1)` | NOT NULL | `0.0` | tempo |
| `comments` | `text` | NULL | NULL | |
| `sweepers` | `varchar(250)` | NULL | NULL | sweeper selection override (pool name) |
| `image` | `varchar(250)` | NOT NULL | `'no_image.jpg'` | artwork filename |
| `buy_link` | `varchar(250)` | NOT NULL | `'http://'` | |
| `url1` | `varchar(250)` | NOT NULL | `'http://'` | |
| `url2` | `varchar(250)` | NOT NULL | `'http://'` | |
| `tdate_played` | `datetime` | NULL | `'2002-01-01 00:00:01'` | "twin"/last play in *test* context? — second set of played-timestamps: `t*` mirrors of `date_played`/`artist_played`/`title_played`/`album_played` (RadioDJ uses these for the secondary "test" schedule/twin-track bookkeeping) |
| `tartist_played` | `datetime` | NULL | `'2002-01-01 00:00:01'` | |
| `ttitle_played` | `datetime` | NULL | `'2002-01-01 00:00:01'` | |
| `talbum_played` | `datetime` | NULL | `'2002-01-01 00:00:01'` | |
| `originalmetadata` | `tinyint(1)` | NOT NULL | `0` | flag: metadata came untouched from file tags |

- PK: `PRIMARY KEY (ID)`
- **UNIQUE `idx_path (path)`** — dedup by path.
- Indexes:
  - `idx_artist_title (artist(100), title(100))` — prefix
  - `idx_album (album(100))` — prefix
  - `idx_rotation_core (enabled, song_type, id_subcat, date_played)` — AutoDJ selection
  - `idx_limits (enabled, play_limit, limit_action, count_played)` — play-limit enforcement
  - `idx_genre_subcat (id_genre, id_subcat)`
  - `idx_auto_dj_rotation (id_subcat, enabled, date_played)` — alternate rotation picker
  - **FULLTEXT `ft_search (artist, title, album, composer)`**

Role: the library. Cart buttons, playlist items, queue rows, requests, history, and rotation picks all reference it. The `*_played` datetime family is the separation mechanism the rotations engine consults (an artist/album/title can't replay within `*_separation` seconds of those timestamps).

### 1.17 Track-type enum (`song_type`, `tType`) — inferred value map

Not defined in SQL, but the `events`/`history`/`songs` columns and `UpdateTracks2`'s `tType IN (0, 9)` special cases pin it down. Order in the reference corpus's `TrackTypes.xml` gives the canonical enum:

| Value | Type | Confirmed by |
|---|---|---|
| 0 | Music | procedure: updates `artist_played`/`title_played`/`album_played` |
| 1 | Jingle | |
| 2 | Sweeper | |
| 3 | Voiceover | |
| 4 | Commercial | |
| 5 | Internet Stream | |
| 6 | Other | |
| 7 | Variable Duration File (VDF) | |
| 8 | Podcast | |
| 9 | Request | procedure: marks `requests.played = 1` |
| 10 | News | |
| 11 | Playlist Event | |
| 12 | File By Date | |
| 13 | Newest From Folder | |
| 14 | Teaser | |

(Inferred from the UI enum ordering; 0 and 9 are hard-confirmed by the procedure. Values ≥15 unverified.)

### 1.18 `users` — app login/permission accounts

| Column | Type | Null | Default | Notes |
|---|---|---|---|---|
| `ID` | `int(10) unsigned` | NOT NULL | AUTO_INCREMENT | PK |
| `user` | `varchar(100)` | NOT NULL | — | login name |
| `auth` | `varchar(250)` | NULL | NULL | stored credential — seed row is a base64 blob, 60 chars decoded (36 bytes ≈ 32-byte hash + salt/IV → AES/SHA-256-based, not plain MD5) |
| `data` | `text` | NOT NULL | — | per-user settings blob (empty string seeded) |

- PK: `PRIMARY KEY (ID)`
- Seed row: `(1, 'admin', '8ZCC4bp0CQxRB0NN5pzdOJG0m6L4b71/TEJ+IJG10u93m0Du94Pl15sCgkViAMv9fY150iwQQREU5TuGqa7pmw==', '')` — default admin account with a pre-hashed password.

---

## 2. App-role overview (how the tables collaborate)

- **Library spine:** `songs` ← the only "source of truth" table; everything else either references it or stores copies of it.
- **Classification:** `category` → `subcategory` (two levels; `subcategory.sweeper_subID` wires automatic sweeper insertion after a subcategory plays). `genre` is orthogonal.
- **Manual playout paths:**
  - *Playlists*: `playlists` + `playlists_list` (pre-built, per-item cue/fade/sweeper/voice-track overrides, `ord` ordering). Event type "play this playlist" references by name.
  - *Cart walls*: `carts` + `carts_list` (live-DJ button grid; `swButton` = grid slot, `color` = UI colour).
- **Automatic playout (AutoDJ / heartbeat):** `rotations` + `rotations_list` define the clock-wheel rules. Every "next item" tick (driven by the `events` clock and/or AutoDJ mode) picks a matching rule by `ord` + daypart `data`, then selects an eligible `songs` row via `idx_rotation_core`/`idx_auto_dj_rotation` applying `date_played`/`artist_played`/`title_played`/`album_played` vs `*_separation` values and `selType`/`weight`. Chosen track lands in `queuelist` with computed `ETA`.
- **Runtime queue:** `queuelist` is the materialized future log (denominated rows, ETA-schedulable, mutable while on air). It is transient working state, not audit.
- **Requests:** `requests` feeds the AutoDJ (played via type 9), and `UpdateTracks2` closes the loop by flipping `played=1`.
- **Scheduling/automation:** `events` + `events_categories` are the heartbeat: timers ("time", day/hour masks, one-shot dates) whose `data` triggers actions; `smart` events evaluate conditions, `grouped` links children; `songs.startEvent/endEvent` let events enable/expire library tracks ("song becomes available when event X fires").
- **Audit/reporting:** `history` is written only by the procedure (plus maintenance tools), denominated so it survives deletion, and pruned by the same procedure (`LIMIT 100` chunk delete keeps pruning cheap for InnoDB).
- **Cross-cutting:** `settings` (app config), `users` (login/permissions).

## 3. Stored procedure `UpdateTracks2`

Complete body (identical in both dumps; `DROP PROCEDURE IF EXISTS` precedes it, and the export wraps it in `DELIMITER //`):

```sql
CREATE PROCEDURE `UpdateTracks2`(
    IN `trackID` INT,
    IN `curArtist` VARCHAR(250) CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci,
    IN `curTitle` VARCHAR(250) CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci,
    IN `tType` INT,
    IN `curListeners` INT,
    IN `historyDays` INT,
    IN `pWeight` DOUBLE
)
BEGIN
    DECLARE vAlbum VARCHAR(250) CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci DEFAULT '';

    SELECT album INTO vAlbum FROM songs WHERE ID = trackID LIMIT 1;

    UPDATE songs
    SET count_played = count_played + 1,
    date_played = NOW()
    WHERE ID = trackID;

    IF tType = 0 OR tType = 9 THEN
        UPDATE songs SET artist_played = NOW() WHERE artist = curArtist AND enabled = 1;
        UPDATE songs SET title_played = NOW() WHERE title = curTitle AND enabled = 1;
        IF vAlbum IS NOT NULL AND TRIM(vAlbum) <> '' THEN
            UPDATE songs SET album_played = NOW() WHERE album = vAlbum AND enabled = 1;
        END IF;
    END IF;

    IF tType = 9 THEN
        UPDATE requests SET played = 1 WHERE songID = trackID AND played = 0;
    END IF;

    -- OPTIMIZAREA 1: Verificăm limitele de play DOAR pentru piesa curentă
    UPDATE songs
    SET id_subcat = CASE WHEN limit_action < 0 THEN ABS(limit_action) ELSE id_subcat END,
    enabled = CASE WHEN limit_action = 1 THEN 0 ELSE enabled END,
    play_limit = 0,
    limit_action = 0
    WHERE ID = trackID
    AND count_played >= play_limit
    AND play_limit > 0
    AND (limit_action = 1 OR limit_action < 0);

    DELETE FROM songs
    WHERE ID = trackID
    AND play_limit > 0
    AND count_played >= play_limit
    AND limit_action = 2;

    IF pWeight > 0 THEN
        UPDATE songs SET weight = GREATEST(0, weight - pWeight) WHERE ID = trackID;
    END IF;

    IF historyDays > 0 THEN
        INSERT INTO history (trackID, date_played, song_type, id_subcat, id_genre, duration, artist, original_artist, title, album, composer, label, `year`, track_no, disc_no, publisher, copyright, isrc, listeners)
        SELECT ID, NOW(), tType, id_subcat, id_genre, duration, curArtist, original_artist, curTitle, album, composer, label, `year`, track_no, disc_no, publisher, copyright, isrc, curListeners
        FROM songs WHERE ID = trackID;
    END IF;

    DELETE FROM history WHERE date_played < DATE_SUB(NOW(), INTERVAL historyDays DAY) LIMIT 100;
END
```

Semantics ("playback bookkeeping transaction" — this is the song-finished handler; the comment is Romanian: "Check play limits ONLY for the current track," an explicit perf optimization vs an older version that swept the whole table):

1. Snapshot the track's album into `vAlbum`.
2. Increment `count_played`, stamp `date_played = NOW()` on the track.
3. If type is **Music (0)** or **Request (9)**: stamp `artist_played` / `title_played` (and `album_played` when album non-blank) on **all enabled songs sharing those values** — this is what enforces artist/title/album separation rules later.
4. If type is **Request (9)**: mark pending `requests` rows played.
5. **Play-limit enforcement** (current track only): if `count_played >= play_limit > 0`:
   - `limit_action = 1` → disable (`enabled = 0`);
   - `limit_action < 0` → move track into subcategory `ABS(limit_action)`;
   - then clear `play_limit`/`limit_action`.
6. If `limit_action = 2` under the same condition → **DELETE** the song row outright.
7. Weight decay: `weight = GREATEST(0, weight - pWeight)` when `pWeight > 0` (rewards weighted rotation: recently played tracks become less likely until re-weighted).
8. If `historyDays > 0`: append the play into `history` (denominated from `songs`, with `curArtist`/`curTitle`/`curListeners` as played-time values).
9. Prune `history` older than `NOW() - historyDays` with `DELETE … LIMIT 100` (bounded incremental delete; the `INTERVAL historyDays DAY` form uses the parameter as a day count — note it runs even when `historyDays = 0`, deleting *all* history rows past a 0-day cutoff, i.e. every row; parity quirk worth preserving).

Porting note: the procedure is an atomic multi-statement side effect — in Rust, reimplement as a transaction (sqlx transaction) or keep as a `CREATE PROCEDURE` in the SQL migrations; `DELETE … LIMIT` without `ORDER BY` is MySQL/MariaDB-specific and non-portable SQL.

## 4. `settings` table: key/value pattern and default keys

**Pattern.** `settings(source, setting, value)` — all three `varchar NOT NULL`, **no primary key, no unique index, no seed rows in either dump**. The app treats `(source, setting)` as the natural key: `source` namespaces a subsystem (e.g. the app's config sections), `setting` the key, `value` a text-encoded scalar (booleans as `True`/`False`, numbers as decimal strings, paths/colour ints as text). Lookups are `WHERE source = ? AND setting = ?`; writes are upsert-by-convention (delete+insert or update-then-insert).

**Default keys.** Neither `database.sql` nor `database-nodefaults.sql` contains any `INSERT INTO settings` — the tables the DDL seeds are `carts`, `category`, `events_categories`, `genre`, `rotations`, `subcategory`, `users` (§1 tables above list the seed rows). Database-level defaults therefore live in the app's XML config files and are *mirrored* into `settings` only when `StoreSettingsToDatabase = True` (see `settings_database.xml`). The reference corpus's settings XML files enumerate the app's full default key set; representative highlights:

| source (XML file) | example keys → inferred meaning |
|---|---|
| `settings_database.xml` | `DbServer`, `DbName`, `DbUsername`, `DbPass`, `lastdb_update`, `DebugEnable`, `AutoBackupHour` (22), `AutoBackupEnable` (False), `MinimizeToTray`, `BackupFolder`, `OnlyOneInstance` (True), `StoreSettingsToDatabase` (False — gates the XML→DB mirroring of every other source), `SSL`, `dbVersion` (2006 — schema migration sentinel checked at startup) |
| `settings_general.xml` | `Priority` (process priority), `Language`, `StationName`, `StationSlogan` (feed `$station_name$`/`$station_slogan$` title templates), `BASS_CONFIG_*` (audio engine buffers), `in_Fade2Input`, `vol_onsweeper`, `volume_out`, `vol_overlay`, `vol_vt`, `dur_fadeout` (3500 ms), `dur_mixoverride`, `SweeperIntroOverride` (60 s), `AutoDJStart`, `AutoPlay`, `remove_silence`, `ThresholdStart/Next/End` (silence-detection dB thresholds −25/−15/−28), `AlwaysOnTop`, `Minimize2Tray`, `ShowInfoWindow`, `ShortcutButton1..5` (tool panel dock buttons: Tracks Manager, Playlist Builder, Sound Processing, Utilities, Playing Info), `RightZonesDivider`, `EnableArtwork`, `ArtworkPath`, `DAYS_2_KEEP_HISTORY` (**90 — the `historyDays` argument to `UpdateTracks2`**) |
| `settings_titles.xml` | `Color_*` / `Title_*` per track type (`Color_Music=-8372160`, `Title_Music=$artist$ - $title$`, `Title_Jingle/Sweeper/Voiceover/Commercial=$station_name$ - $station_slogan$`, `Color_InternetStream`, `Color_Other`, `Color_VDF`, …) — per-type display title templates and wall colours, colors are OLE auto-color ints |
| `settings_tabs.xml`, `settings_buttons.xml`, `settings_shortcuts.xml`, `settings_soundcards.xml`, `soundcardslist.xml` | UI panel layout, dashboard/cart shortcut buttons, keyboard shortcuts, sound-card routing (out/backtime aux channels) |

(Exact key spellings come from these XML files; when `StoreSettingsToDatabase=True`, RadioDJ persists them with the file/section as `source`. The `DAYS_2_KEEP_HISTORY`→`historyDays` and weight-decay `pWeight` parameters to `UpdateTracks2` confirm the settings→procedure wiring.)

Rust port guidance: model as `Settings { source, setting, value }` with a **unique index added** on `(source, setting)` (the original lacks it — only safe for a fresh rebuild; parity behaviour tolerates duplicates because updates are keyed by both columns) plus an in-memory typed config cache; do not seed the table at DDL time, mirroring RadioDJ.

## 5. MySQL/MariaDB specifics to watch in the Rust/sqlx port

- **Server flavor.** The dump targets **MariaDB 12.2.2 on Win64**, not MySQL proper. sqlx's `MyRow`/MariaDB protocol works, but target MariaDB explicitly (some MySQL 8-only syntax, e.g. `utf8mb4_0900_ai_ci`, is absent; collation here is `utf8mb4_unicode_ci`).
- **Charset.** Session bootstrap `SET NAMES utf8mb4` (the `SET NAMES utf8` line is superseded by the `/*!50503 SET NAMES utf8mb4`); every table is `DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`. Preserve `utf8mb4_unicode_ci` (not `general_ci`/`_0900`) for parity — it changes `=` comparisons and `UNIQUE` collisions (e.g. `playlists.name`, `genre.name`, `songs.path`). Column-level `CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci` on procedure params matters because comparisons of `curArtist`/`curTitle` against `songs` columns must not transcode.
- **Int display widths are deprecated** (`int(10) unsigned`, `tinyint(1)`, `tinyint(2)`): MariaDB renders but ignores them (MariaDB 10.17+/MySQL 8 drop them). In Rust model as `u32`/`i32` and `i8`/`bool`; preserve *signedness* exactly — it is load-bearing: most ID/FK columns are `int unsigned` (`u32`), but `playlists_list.sID`, `queuelist.songID/swID/vtID`, `songs.startEvent/endEvent`, `events.catID`, `rotations_list.catID/subID/genID` are **signed** (`i32`) precisely because they store `-1`/`0` sentinels. `int(10) unsigned` PK → `u32` in sqlx (`u32` maps fine; keep AUTO_INCREMENT on `u32`).
- **AUTO_INCREMENT quirks.** Dump runs with `SQL_MODE='NO_AUTO_VALUE_ON_ZERO'` — inserting `0` keeps `0` instead of generating an ID (seed rows rely on explicit IDs anyway). InnoDB counter persistence changed across versions (12 pre-8.4/10.x redo-persist vs older index-max scan); if recreating with explicit small IDs, just trust `AUTO_INCREMENT`. No composite PK anywhere; every table is single-column auto-incr except `settings` (no PK at all).
- **Date/time sentinel handling.** `datetime DEFAULT '2002-01-01 00:00:01'`, `date DEFAULT '2002-01-01'`, plus `year DEFAULT '1900'`, `NOT SET`/'und'/'&'/'http://'/'no_image.jpg'` string sentinels are *data-level* defaults (not NULL meaning unset). In Rust keep them as literal defaults (or expose as `Option<T>` with mapping) rather than migrating to NULL — the whole app logic keys on these strings. `NOW()` inside the procedure is **server-timezone dependent**: the dump forces `TIME_ZONE='+00:00'` for the session and restores it at the end — in the port pin the DB session `time_zone` (or move timestamps client-side with `chrono`) consistently or `date_played` ordering drifts. `sqlx` `datetime` ↔ `chrono::NaiveDateTime` (no tz in column type).
- **Decimal precision**: `decimal(11,5)` durations, `decimal(5,2)` weight, `decimal(8,3)` loudness, `decimal(6,1)` bpm → use `rust_decimal`/`BigDecimal` in sqlx, **not f64**, or weight/loudness comparisons will drift. (`DOUBLE` `pWeight` param is the exception.)
- **Prefix indexes & FULLTEXT**: `KEY (artist(100), title(100))` and `FULLTEXT ft_search`/`ft_history_search` are MySQL/MariaDB-only DDL; sqlx can't validate them against non-MySQL, but a MySQL/MariaDB target keeps them as-is. FULLTEXT `MATCH … AGAINST` syntax (and MariaDB's InnoDB-fulltext stopword behaviour) must be replicated in search SQL; on other engines plan a fallback (LIKE or external index).
- **`DELETE … LIMIT 100` without ORDER BY** (history pruning) is MySQL/MariaDB-specific and nondeterministic; keep or restructure as chunked `WHERE id <= (SELECT MAX…)`.
- **No FKs / no cascades** (`FOREIGN_KEY_CHECKS=0` in dump header): deletion of a `songs` row (`limit_action = 2`, manual delete) leaves dangling references in `carts_list`, `playlists_list`, `queuelist`, `requests` — replication of that (or deliberate cascade) is a design decision for the rebuild, not something to silently add.
- **Stored procedure support**: decide — keep `UpdateTracks2` in the DB (MariaDB keeps its body verbatim; sqlx `CALL UpdateTracks2(…)` works but returns no result set) or reimplement as a Rust transaction (recommended; keeps `historyDays`-0 quirk and `GREATEST`/`DATE_SUB` logic testable).
- **Identical `INSERT` semantics:** `INSERT INTO … VALUES` multi-row seeding in dumps can be reused verbatim for RadioCraft's migration 0 baseline.
- **Case-sensitivity of identifiers**: mixed naming (`id` in `genre`, `swplay` vs `swPlay`, `idx_path` as a *unique* key) — preserve exact spellings/casing of columns; Linux MySQL file-name case settings don't affect column names, but sqlx compiled queries will fail on mismatch.

## 6. Seed baseline summary (RadioCraft migration 0)

Rows inserted by `database.sql` (absent from `database-nodefaults.sql`):

| Table | Seed count | Rows |
|---|---|---|
| `carts` | 1 | `(1,'Default')` |
| `category` | 11 | Music … Radio Streams |
| `events_categories` | 1 | `(1,'Default')` |
| `genre` | 148 | WinAmp genre list (IDs 1–148) |
| `rotations` | 1 | `(1,'Default')` |
| `subcategory` | 22 | IDs 1–22 mapped to categories |
| `users` | 1 | admin + pre-hashed credential (present in **both** dumps) |
| `settings` | **0** | no seed at all (see §4) |

## 7. Diff between `database.sql` and `database-nodefaults.sql`

Structurally identical (same DDL, same indexes, same procedure text). Differences: `database-nodefaults.sql` omits the seed `INSERT` blocks for `carts`, `category`, `events_categories`, `genre`, `rotations`, `subcategory` (but still includes the `users` admin row and the procedure). Use the nodefaults file as the RadioCraft migration baseline + a separate "defaults" data migration reproducing §6.

## Appendix: stored procedure `UpdateTracks2` (extracted verbatim from Setup/Sql)

This is the playback-booking transaction executed at every track start — the core of the playout engine:

```sql
CREATE PROCEDURE `UpdateTracks2`(
IN `trackID` INT, IN `curArtist` VARCHAR(250), IN `curTitle` VARCHAR(250),
IN `tType` INT, IN `curListeners` INT, IN `historyDays` INT, IN `pWeight` DOUBLE)
BEGIN
UPDATE songs SET count_played=count_played+1, date_played=NOW() WHERE ID=trackID;
SET @tAlbum = (SELECT album FROM songs WHERE ID=trackID);
IF tType = 0 OR tType = 9 THEN              -- music/request: replay-avoidance bookkeeping
UPDATE songs SET artist_played=NOW() WHERE artist=curArtist;
UPDATE songs SET title_played=NOW() WHERE title=curTitle;
IF @tAlbum <> '' THEN UPDATE songs SET album_played=NOW() WHERE album=@tAlbum; END IF;
END IF;
IF tType = 9 THEN UPDATE requests SET played=1 WHERE songID=trackID; END IF;
-- play-limit enforcement: 1=disable, 2=delete, negative id_subcat=move to that category
UPDATE songs SET enabled=0, play_limit=0 WHERE enabled=1 AND play_limit>0 AND count_played>=play_limit AND limit_action=1;
DELETE FROM songs WHERE play_limit>0 AND count_played>=play_limit AND limit_action=2;
UPDATE songs SET id_subcat=ABS(limit_action), play_limit=0, limit_action=0 WHERE enabled=1 AND limit_action<0 AND count_played>=play_limit;
IF pWeight>0 THEN UPDATE songs SET weight=weight-pWeight WHERE ID=trackID AND (weight-pWeight)>=0; END IF;
IF historyDays > 0 THEN
INSERT INTO history(trackID, date_played, song_type, id_subcat, id_genre, duration, artist,
  original_artist, title, album, composer, label, `year`, track_no, disc_no, publisher, copyright, isrc, listeners)
SELECT ID, NOW(), tType, id_subcat, id_genre, duration, curArtist, original_artist, curTitle, album,
  composer, label, `year`, track_no, disc_no, publisher, copyright, isrc, curListeners FROM songs WHERE ID=trackID;
END IF;
DELETE FROM history WHERE TIMESTAMPDIFF(DAY, date_played, NOW()) >= historyDays;
END
```

**Engine semantics derived:** replay avoidance is recorded by touching `artist_played`/`title_played`/`album_played` on music/request play; the AutoDJ scheduler must use these timestamps with `RepeatArtistInteval`/`RepeatTitleInteval`/`RepeatAlbumInteval`/`RepeatTrackInterval` (minutes) as negative-age filters; play-limit actions auto-disable/delete/relocate tracks at booking time; weight can decay by `pWeight` at play; history rows are trimmed by `DAYS_2_KEEP_HISTORY`.

Port note: RadioCraft implements this as a **sqlx transaction** (one round-trip), same effect ordering.
