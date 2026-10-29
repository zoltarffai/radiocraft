//! Pure data structures mirroring the `UpdateTracks2` playback-booking
//! transaction (docs/01 section 3 + appendix).
//!
//! The db crate consumes these structs and executes the equivalent sqlx
//! transaction; this module defines the inputs/outputs and documents every
//! invariant the engine must preserve. No I/O happens here.

use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

/// Data-level sentinels used throughout the schema (docs/01 convention notes:
/// these are *literal defaults*, not NULL; the whole app logic keys on them).
pub mod sentinel {
    use chrono::{NaiveDateTime, ParseResult};

    /// `datetime DEFAULT '2002-01-01 00:00:01'`: "never played / no date set"
    /// (a deliberate fake epoch applied to `date_played`, `artist_played`,
    /// `title_played`, `album_played`, `start_date`, `end_date`, `queuelist.ETA`,
    /// `history.date_played`, ...).
    pub const NEVER_PLAYED: &str = "2002-01-01 00:00:01";
    /// `date DEFAULT '2002-01-01'` (events.one-shot date unset).
    pub const NO_DATE: &str = "2002-01-01";
    /// varchar/text "not set / all" marker (`cue_times`, `events.day`, `events.hours`).
    pub const NOT_SET: &str = "&";
    /// `year DEFAULT '1900'` string-year unset marker (`songs.year`, `history.year`).
    pub const YEAR_UNSET: &str = "1900";
    /// int "no reference" marker (`startEvent`, `endEvent`, `queuelist.swID/vtID`).
    pub const NO_REF: i64 = -1;
    /// `lang DEFAULT 'und'` (undetermined language).
    pub const LANG_UND: &str = "und";
    /// `mood`/`gender` DEFAULT 'Not Set'.
    pub const NOT_SET_TAG: &str = "Not Set";
    /// `image DEFAULT 'no_image.jpg'`, `buy_link`/`url1`/`url2` DEFAULT 'http://'.
    pub const NO_IMAGE: &str = "no_image.jpg";
    pub const URL_UNSET: &str = "http://";

    /// Parse a schema datetime literal (`YYYY-MM-DD HH:MM:SS`).
    pub fn parse_dt(text: &str) -> ParseResult<NaiveDateTime> {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
    }

    /// The fake-epoch instant as a chrono value (the literal is fixed by schema,
    /// parsing cannot fail).
    pub fn never_played_dt() -> NaiveDateTime {
        parse_dt(NEVER_PLAYED).expect("valid schema sentinel")
    }
}

/// `limit_action` semantics (docs/01 section 3 step 5/6): what happens when
/// `count_played >= play_limit` at booking time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LimitAction {
    /// 0: do nothing.
    None,
    /// 1: disable the track (`enabled = 0`), then clear `play_limit`/`limit_action`.
    Disable,
    /// 2: **delete** the `songs` row outright (dangling references in
    /// carts_list/playlists_list/queuelist/requests are *not* cascaded — parity).
    Delete,
    /// Negative N: move the track into subcategory `N` (`ABS(limit_action)`),
    /// then clear `play_limit`/`limit_action`.
    MoveToSubcategory(u32),
}

impl LimitAction {
    /// Decode the raw `limit_action` tinyint (negative = subcategory id).
    pub fn from_i32(v: i32) -> Self {
        if v < 0 {
            LimitAction::MoveToSubcategory(v.unsigned_abs())
        } else {
            match v {
                1 => LimitAction::Disable,
                2 => LimitAction::Delete,
                _ => LimitAction::None,
            }
        }
    }
    /// Encode back to the raw tinyint form.
    pub fn as_i32(self) -> i32 {
        match self {
            LimitAction::None => 0,
            LimitAction::Disable => 1,
            LimitAction::Delete => 2,
            // Saturate: subcategory ids beyond i32::MAX collapse to 0 (never real).
            LimitAction::MoveToSubcategory(n) => (n as i32).checked_neg().unwrap_or(0),
        }
    }
}

/// Inputs of `UpdateTracks2(trackID, curArtist, curTitle, tType, curListeners,
/// historyDays, pWeight)` — the song-finished handler (one call per play).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookingRequest {
    /// `songs.ID` of the just-played track.
    pub track_id: i64,
    /// Artist as played — may be rewritten (request lines); it, not the stored
    /// `songs.artist`, is what stamps `artist_played` and lands in history.
    pub cur_artist: String,
    /// Title as played (same rewrite rule as `cur_artist`).
    pub cur_title: String,
    /// Track type (`song_type` registry, docs/03 section 6): the special cases
    /// are 0 (Music) and 9 (Request).
    pub track_type: i32,
    /// Listener count at play time (denormalized into history).
    pub cur_listeners: i64,
    /// `DAYS_2_KEEP_HISTORY` from settings_general.xml: when > 0, append the
    /// play into history and prune rows older than N days (chunked LIMIT 100).
    pub history_days: i64,
    /// Post-play weight decay (settings-driven): when > 0,
    /// `weight = GREATEST(0, weight - p_weight)`.
    pub p_weight: f64,
}

impl BookingRequest {
    /// True when replay-avoidance bookkeeping applies (`tType = 0 OR tType = 9`).
    pub fn stamps_separation(&self) -> bool {
        self.track_type == 0 || self.track_type == 9
    }
    /// True when pending `requests` rows must be closed (`tType = 9`).
    pub fn closes_requests(&self) -> bool {
        self.track_type == 9
    }
}

/// One `history` INSERT row (docs/01 section 1.8: denormalized at play time —
/// survives song deletion). Field names/column list mirror the procedure's
/// INSERT ... SELECT exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// `trackID` -> songs.ID (soft reference).
    pub track_id: i64,
    /// Insertion timestamp (procedure: `NOW()`); sentinel default
    /// '2002-01-01 00:00:01'.
    pub date_played: NaiveDateTime,
    /// `song_type` at time of play (tinyint registry).
    pub song_type: i32,
    /// Subcategory at time of play.
    pub id_subcat: i64,
    /// Genre at time of play.
    pub id_genre: i64,
    /// decimal(11,5) seconds.
    pub duration: f64,
    /// Played artist (`curArtist`, may be rewritten).
    pub artist: String,
    pub original_artist: String,
    /// Played title (`curTitle`).
    pub title: String,
    pub album: String,
    pub composer: String,
    /// varchar(250) — wider than the surrounding 200s (schema quirk, keep it).
    pub label: String,
    /// varchar(4) string year, sentinel `'1900'`.
    pub year: String,
    pub track_no: i32,
    pub disc_no: i32,
    pub publisher: String,
    pub copyright: String,
    pub isrc: String,
    /// Nullable listener count at play time.
    pub listeners: Option<i64>,
}

/// Invariants the booking transaction must apply, in the exact ordering of
/// `UpdateTracks2` (docs/01 section 3). The db crate executes these as one
/// sqlx transaction; this struct is the pure, testable description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookingOutcome {
    /// `count_played` after increment.
    pub new_count_played: i64,
    /// `date_played` stamped on the played track.
    pub date_played: NaiveDateTime,
    /// Set when separation bookkeeping fired: (artist, title, Option<album>)
    /// the db layer stamps on **all enabled songs sharing those values**.
    /// Album is None when the played track's album is blank (procedure skips).
    pub separation_stamps: Option<(String, String, Option<String>)>,
    /// Set when `tType = 9`: close `requests.played = 1` for this song.
    pub close_request: Option<i64>,
    /// Play-limit action that fires post-increment when
    /// `count_played >= play_limit > 0` (current track only — the "OPTIMIZAREA 1"
    /// perf note: never sweep the table).
    pub limit_action_applied: Option<LimitAction>,
    /// Weight after decay; None when no decay applied. Invariant: never below 0
    /// (`GREATEST(0, weight - p_weight)`).
    pub new_weight: Option<f64>,
    /// History row to append (only when `history_days > 0`).
    pub history: Option<HistoryEntry>,
    /// History prune cutoff: delete rows with `date_played < cutoff`
    /// (`DELETE ... LIMIT 100` chunking is a db-layer concern). Parity quirk:
    /// the procedure also prunes when `history_days = 0`, wiping *all* rows.
    pub prune_before: Option<NaiveDateTime>,
}

/// Compute the booking outcome for a request against a snapshot of the
/// current song row. Pure and fully testable; the db crate still needs to run
/// the cross-row UPDATEs described in [`BookingOutcome`].
pub fn plan_booking(
    req: &BookingRequest,
    snapshot: &BookingSnapshot,
    now: NaiveDateTime,
) -> BookingOutcome {
    let new_count = snapshot.count_played + 1;
    let mut limit_action_applied = None;
    if snapshot.play_limit > 0 && new_count >= snapshot.play_limit {
        match LimitAction::from_i32(snapshot.limit_action) {
            // Step 5/6 guard: the combined UPDATE only fires for
            // limit_action = 1 or < 0; the DELETE fires for = 2.
            a @ (LimitAction::Disable | LimitAction::MoveToSubcategory(_)) => {
                limit_action_applied = Some(a);
            }
            LimitAction::Delete => limit_action_applied = Some(LimitAction::Delete),
            LimitAction::None => {}
        }
    }
    let new_weight = if req.p_weight > 0.0 {
        Some((snapshot.weight - req.p_weight).max(0.0))
    } else {
        None
    };
    let separation_stamps = if req.stamps_separation() {
        let album = snapshot.album.trim();
        Some((
            req.cur_artist.clone(),
            req.cur_title.clone(),
            if !album.is_empty() {
                Some(snapshot.album.clone())
            } else {
                None
            },
        ))
    } else {
        None
    };
    let history = if req.history_days > 0 {
        Some(HistoryEntry {
            track_id: req.track_id,
            date_played: now,
            song_type: req.track_type,
            id_subcat: snapshot.id_subcat,
            id_genre: snapshot.id_genre,
            duration: snapshot.duration,
            artist: req.cur_artist.clone(),
            original_artist: snapshot.original_artist.clone(),
            title: req.cur_title.clone(),
            album: snapshot.album.clone(),
            composer: snapshot.composer.clone(),
            label: snapshot.label.clone(),
            year: snapshot.year.clone(),
            track_no: snapshot.track_no,
            disc_no: snapshot.disc_no,
            publisher: snapshot.publisher.clone(),
            copyright: snapshot.copyright.clone(),
            isrc: snapshot.isrc.clone(),
            listeners: Some(req.cur_listeners),
        })
    } else {
        None
    };
    // Parity: the verbatim procedure body prunes even when history_days = 0
    // (cutoff = now -> every row). The nodefaults body prunes on
    // TIMESTAMPDIFF >= historyDays; both collapse to "prune at cutoff".
    let prune_before = if req.history_days >= 0 {
        now.checked_sub_signed(chrono::Duration::days(req.history_days))
    } else {
        None
    };
    BookingOutcome {
        new_count_played: new_count,
        date_played: now,
        separation_stamps,
        close_request: req.closes_requests().then_some(req.track_id),
        limit_action_applied,
        new_weight,
        history,
        prune_before,
    }
}

/// Minimal `songs` snapshot the booking plan needs (the fields the procedure
/// reads from the row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookingSnapshot {
    pub count_played: i64,
    pub play_limit: i64,
    pub limit_action: i32,
    pub weight: f64,
    pub album: String,
    pub original_artist: String,
    pub id_subcat: i64,
    pub id_genre: i64,
    pub duration: f64,
    pub year: String,
    pub track_no: i32,
    pub disc_no: i32,
    pub composer: String,
    pub label: String,
    pub publisher: String,
    pub copyright: String,
    pub isrc: String,
}

impl Default for BookingSnapshot {
    fn default() -> Self {
        BookingSnapshot {
            count_played: 0,
            play_limit: 0,
            limit_action: 0,
            weight: 0.0,
            album: String::new(),
            original_artist: String::new(),
            id_subcat: 0,
            id_genre: 0,
            duration: 0.0,
            year: sentinel::YEAR_UNSET.into(),
            track_no: 0,
            disc_no: 0,
            composer: String::new(),
            label: String::new(),
            publisher: String::new(),
            copyright: String::new(),
            isrc: String::new(),
        }
    }
}

/// UTC "now" helper for callers that don't pin a timezone (docs/01 section 5:
/// pin the DB session time_zone consistently or `date_played` ordering drifts).
pub fn utc_now_naive() -> NaiveDateTime {
    Utc::now().naive_utc()
}
