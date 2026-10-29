//! Rotation (AutoDJ clock-wheel) model and MySQL-agnostic selection query.
//!
//! Mirrors the `rotations` / `rotations_list` pair from docs/01 sections
//! 1.13-1.14 (the RadioDJ v2-era `rotation_criteria` rule set is folded into
//! [`RotationCriteria`] here: category include/exclusion, repeat intervals,
//! weight floor, day-of-week bitmask and daypart time windows).
//!
//! The db crate consumes [`SchedulerQuery::sql_fragments`] and executes the
//! rendered WHERE fragment with the named parameters; this crate performs no
//! SQL execution itself (see docs/01 section 5: cutoff datetimes are computed
//! client-side with chrono, which keeps the fragments portable for a MariaDB
//! *and* SQLite-style rebuild while preserving the
//! `NOW() - INTERVAL n MINute` replay-avoidance semantics of the original).

use chrono::{Datelike, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};

use crate::booking::sentinel;

/// Map a chrono `Weekday` to the schema day-number (0 = Sunday ... 6 =
/// Saturday) used by [`DayMask`] bit positions.
pub fn weekday_index(w: chrono::Weekday) -> u32 {
    match w {
        chrono::Weekday::Sun => 0,
        chrono::Weekday::Mon => 1,
        chrono::Weekday::Tue => 2,
        chrono::Weekday::Wed => 3,
        chrono::Weekday::Thu => 4,
        chrono::Weekday::Fri => 5,
        chrono::Weekday::Sat => 6,
    }
}

/// Day-of-week bitmask (`events.day` / rotation day list).
///
/// Bit `n` (`1 << n`) corresponds to the weekday with `num_uses_sun() == n`,
/// i.e. bit 0 = Sunday ... bit 6 = Saturday. The RadioDJ sentinel `'&'`
/// (all days) is [`DayMask::ALL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct DayMask(pub u32);

impl DayMask {
    /// Every day of the week (the `'&'` sentinel).
    pub const ALL: DayMask = DayMask(0x7F);
    /// No days (empty rotation window).
    pub const NONE: DayMask = DayMask(0);

    /// Mask containing exactly one weekday (`num_uses_sun`: 0 = Sunday ... 6 = Saturday).
    pub fn for_weekday(num_uses_sun: u32) -> Self {
        debug_assert!(num_uses_sun < 7);
        DayMask(1u32 << (num_uses_sun & 7))
    }

    /// True when the mask accepts the given weekday number (0 = Sunday).
    pub fn contains_weekday(&self, num_uses_sun: u32) -> bool {
        self.0 & (1u32 << (num_uses_sun % 7)) != 0
    }

    /// True when the mask accepts `when`'s weekday.
    pub fn matches_date(&self, when: NaiveDateTime) -> bool {
        self.contains_weekday(weekday_index(Datelike::weekday(&when.date())))
    }

    /// Parse the RadioDJ `day` column text: `'&'` = all days, otherwise a
    /// comma-separated list of weekday numbers (0 = Sunday ... 6 = Saturday).
    /// Unknown tokens are ignored (be liberal with historical data).
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        if text == "&" || text.is_empty() {
            return DayMask::ALL;
        }
        let mut mask = 0u32;
        for token in text.split(',') {
            if let Ok(n) = token.trim().parse::<u32>() {
                if n < 7 {
                    mask |= 1 << n;
                }
            }
        }
        DayMask(mask)
    }
}

/// Category/subcategory/genre match polarity (`catType` in `rotation_criteria`;
/// schema counterpart fields in `rotations_list` use `0 = any`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CategoryType {
    /// 0: rows matching the id are *excluded*.
    #[default]
    Exclude = 0,
    /// 1: rows matching the id are *included*.
    Include = 1,
}

impl CategoryType {
    pub fn from_i32(v: i32) -> Self {
        if v == 1 {
            CategoryType::Include
        } else {
            CategoryType::Exclude
        }
    }
    pub fn as_i32(self) -> i32 {
        match self {
            CategoryType::Exclude => 0,
            CategoryType::Include => 1,
        }
    }
}

/// How a rule orders candidate tracks ("ORDER BY" strategy, `selType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(i32)]
pub enum SelectionMode {
    /// Weighted rotation: highest remaining `weight` wins (ties -> least played).
    #[default]
    Weight = 0,
    /// Least-played first (`count_played` asc).
    LeastPlayed = 1,
    /// Oldest `date_played` first (the classic "rotated music" order).
    Oldest = 2,
    /// Newest `date_added` first (new additions win).
    Newest = 3,
    /// Deterministic pseudo-shuffle (hash of the song id; the app layer may
    /// reseed at runtime).
    Random = 4,
}

impl SelectionMode {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => SelectionMode::LeastPlayed,
            2 => SelectionMode::Oldest,
            3 => SelectionMode::Newest,
            4 => SelectionMode::Random,
            _ => SelectionMode::Weight,
        }
    }
}

/// One rotation rule row ("clock wheel" entry, docs/01 sections 1.14 + the
/// `rotation_criteria` appendix fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RotationCriteria {
    /// `rotations_list.ID`.
    pub id: i64,
    /// `rotations_list.pID` -> `rotations.ID`.
    pub rotation_id: i64,
    /// Rule name (free text).
    pub name: String,
    /// Rule active flag.
    pub enabled: bool,
    /// `catID`/`subID`/`genID` triple (0 = any).
    pub category_id: i64,
    /// 0 = exclude / 1 = include (`catType`).
    pub category_type: CategoryType,
    pub subcategory_id: i64,
    pub subcategory_type: CategoryType,
    pub genre_id: i64,
    /// `songs.weight` floor: tracks with less weight are rejected (0 = none).
    pub weight_min: f64,
    /// Replay-avoidance intervals in minutes (0 = no rule). The original
    /// spelling `RepeatArtistInteval` is historical, keep the corrected name.
    pub repeat_track_interval: i64,
    pub repeat_artist_interval: i64,
    pub repeat_title_interval: i64,
    pub repeat_album_interval: i64,
    /// Daypart window (`NULL`/None = all day). Overnight windows (start > end)
    /// wrap past midnight.
    pub time_start: Option<NaiveTime>,
    pub time_end: Option<NaiveTime>,
    /// Day-of-week bitmask (`&` = all).
    pub days: DayMask,
    /// Optional metadata filters (`rotations_list.mood/gender/language`).
    pub language: Option<String>,
    pub mood: Option<String>,
    pub gender: Option<String>,
    /// Selection strategy (`selType`).
    pub selection: SelectionMode,
    /// `repeatRule`: allow the exact same track twice in a row (0 = forbidden).
    pub repeat_rule: i32,
    /// Position within the wheel (`ord`, tinyint in schema).
    pub ord: i32,
}

impl Default for RotationCriteria {
    fn default() -> Self {
        RotationCriteria {
            id: 0,
            rotation_id: 0,
            name: String::new(),
            enabled: true,
            category_id: 0,
            category_type: CategoryType::Include,
            subcategory_id: 0,
            subcategory_type: CategoryType::Include,
            genre_id: 0,
            weight_min: 0.0,
            repeat_track_interval: 0,
            repeat_artist_interval: 0,
            repeat_title_interval: 0,
            repeat_album_interval: 0,
            time_start: None,
            time_end: None,
            days: DayMask::ALL,
            language: None,
            mood: None,
            gender: None,
            selection: SelectionMode::Weight,
            repeat_rule: 0,
            ord: 0,
        }
    }
}

impl RotationCriteria {
    /// True when this rule is active at `when` (day bitmask + daypart window).
    pub fn matches_time(&self, when: NaiveDateTime) -> bool {
        self.enabled
            && self.days.matches_date(when)
            && time_in_window(self.time_start, self.time_end, when.time())
    }

    /// Render the MySQL-agnostic SELECT fragment set for this rule.
    pub fn query_for(&self, now: NaiveDateTime, song_type: Option<i32>) -> SchedulerQuery {
        let mut q = SchedulerQuery::new(now);
        q.song_type = song_type;
        if self.category_id != 0 {
            if self.category_type == CategoryType::Include {
                q.include_categories.push(self.category_id);
            } else {
                q.exclude_categories.push(self.category_id);
            }
        }
        if self.subcategory_id != 0 {
            if self.subcategory_type == CategoryType::Include {
                q.include_subcategories.push(self.subcategory_id);
            } else {
                q.exclude_subcategories.push(self.subcategory_id);
            }
        }
        q.genre_id = self.genre_id;
        q.language = self.language.clone();
        q.mood = self.mood.clone();
        q.gender = self.gender.clone();
        q.weight_min = (self.weight_min > 0.0).then_some(self.weight_min);
        q.repeat_track_minutes = self.repeat_track_interval;
        q.repeat_artist_minutes = self.repeat_artist_interval;
        q.repeat_title_minutes = self.repeat_title_interval;
        q.repeat_album_minutes = self.repeat_album_interval;
        q.selection = self.selection;
        q
    }
}

/// `rotations` container row + its criteria rows, with priority-based
/// selection (docs/02 section 3: rotation `Priority` picks among overlapping
/// rotations for the same daypart).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rotation {
    /// `rotations.ID`.
    pub id: i64,
    /// `rotations.name`.
    pub name: String,
    /// Higher wins when several rotations match the current clock (0 = default).
    pub priority: i32,
    /// Rotation-level daypart window (None = all day; overnight wraps).
    pub time_start: Option<NaiveTime>,
    pub time_end: Option<NaiveTime>,
    /// Rotation-level day bitmask.
    pub days: DayMask,
    /// `rotations_list` rows belonging to this rotation.
    pub criteria: Vec<RotationCriteria>,
}

impl Rotation {
    pub fn new(id: i64, name: impl Into<String>) -> Self {
        Rotation {
            id,
            name: name.into(),
            priority: 0,
            time_start: None,
            time_end: None,
            days: DayMask::ALL,
            criteria: Vec::new(),
        }
    }

    /// True when the rotation window accepts `when`
    /// (day bitmask match + possibly-overnight time window).
    pub fn matches_time(&self, when: NaiveDateTime) -> bool {
        self.days.matches_date(when)
            && time_in_window(self.time_start, self.time_end, when.time())
    }

    /// Criteria active at `when`, ordered by wheel position (`ord`).
    pub fn active_criteria(&self, when: NaiveDateTime) -> Vec<&RotationCriteria> {
        let mut v: Vec<&RotationCriteria> = self
            .criteria
            .iter()
            .filter(|c| c.matches_time(when))
            .collect();
        v.sort_by_key(|c| c.ord);
        v
    }
}

/// Pick the highest-priority rotation(s) matching `now`; priority desc, then
/// id asc for stable ordering. Empty when nothing matches.
pub fn select_rotations(all: &[Rotation], now: NaiveDateTime) -> Vec<&Rotation> {
    let mut v: Vec<&Rotation> = all.iter().filter(|r| r.matches_time(now)).collect();
    v.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
    v
}

/// Time-window membership helper shared by rotations and criteria:
/// both-None or one-sided windows are open-ended; `start > end` = overnight
/// window matching `t >= start || t <= end`.
pub fn time_in_window(
    start: Option<NaiveTime>,
    end: Option<NaiveTime>,
    t: NaiveTime,
) -> bool {
    match (start, end) {
        (Some(s), Some(e)) if s > e => t >= s || t <= e,
        (Some(s), Some(e)) => t >= s && t <= e,
        (Some(s), None) => t >= s,
        (None, Some(e)) => t <= e,
        (None, None) => true,
    }
}

/// A named parameter bound to a rendered SQL fragment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub value: ParamValue,
}

/// Portable scalar parameter values (the db layer maps these to driver types).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    Int(i64),
    Float(f64),
    Text(String),
    /// Formatted `YYYY-MM-DD HH:MM:SS` (mirrors sqlx chrono NaiveDateTime binding).
    DateTime(String),
}

/// Fully rendered SQL fragment set produced by a [`SchedulerQuery`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SqlFragments {
    /// Fixed projection reading every column the core model needs.
    pub select: String,
    pub from: String,
    /// Rendered `WHERE ...` body (no `WHERE` keyword), joined with AND.
    pub where_clause: String,
    /// Rendered `ORDER BY ...` body (no keyword).
    pub order_by: String,
    pub limit: Option<u32>,
    /// Parameters referenced by `:name` placeholders inside the fragments.
    pub params: Vec<Param>,
}

/// Declarative AutoDJ candidate query (docs/01 section 2: the AutoDJ heart).
///
/// The db crate renders [`Self::sql_fragments`] against the `songs` table and
/// executes it; every predicate is parameter-named so the same structure can
/// drive MariaDB prepared statements or a SQLite rebuild.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchedulerQuery {
    /// Server "NOW()" pinned client-side (docs/01 section 5: session
    /// time_zone must stay pinned or `date_played` ordering drifts).
    pub now: NaiveDateTime,
    /// Emit `enabled = 1` (in-rotation flag).
    pub enabled: bool,
    /// Optional `song_type` pin (None = any type).
    pub song_type: Option<i32>,
    /// Category ids to include (via `subcategory.parentid`).
    pub include_categories: Vec<i64>,
    pub exclude_categories: Vec<i64>,
    /// Direct `id_subcat` inclusion/exclusion.
    pub include_subcategories: Vec<i64>,
    pub exclude_subcategories: Vec<i64>,
    /// Genre id (0 = any).
    pub genre_id: i64,
    /// Metadata equality filters (None = any).
    pub language: Option<String>,
    pub mood: Option<String>,
    pub gender: Option<String>,
    /// `songs.weight` floor (None = no floor).
    pub weight_min: Option<f64>,
    /// Replay-avoidance intervals in minutes (0 = no rule) applied to
    /// `date_played`/`artist_played`/`title_played`/`album_played`.
    pub repeat_track_minutes: i64,
    pub repeat_artist_minutes: i64,
    pub repeat_title_minutes: i64,
    pub repeat_album_minutes: i64,
    /// Emit `songs.start_date/end_date` availability guards (sentinel-aware).
    pub respect_availability_window: bool,
    /// Emit `(play_limit = 0 OR count_played < play_limit)` guard.
    pub respect_play_limit: bool,
    /// ORDER BY strategy.
    pub selection: SelectionMode,
    /// Maximum candidates returned.
    pub limit: Option<u32>,
}

impl SchedulerQuery {
    pub fn new(now: NaiveDateTime) -> Self {
        SchedulerQuery {
            now,
            enabled: true,
            song_type: None,
            include_categories: Vec::new(),
            exclude_categories: Vec::new(),
            include_subcategories: Vec::new(),
            exclude_subcategories: Vec::new(),
            genre_id: 0,
            language: None,
            mood: None,
            gender: None,
            weight_min: None,
            repeat_track_minutes: 0,
            repeat_artist_minutes: 0,
            repeat_title_minutes: 0,
            repeat_album_minutes: 0,
            respect_availability_window: true,
            respect_play_limit: true,
            selection: SelectionMode::Weight,
            limit: None,
        }
    }

    /// Cutoff for a replay rule: `now - interval_minutes` (0/no-rule -> None).
    fn cutoff(&self, minutes: i64) -> Option<NaiveDateTime> {
        if minutes <= 0 {
            None
        } else {
            self.now
                .checked_sub_signed(chrono::Duration::minutes(minutes))
        }
    }

    /// Render the SELECT fragments with named parameters.
    pub fn sql_fragments(&self) -> SqlFragments {
        let mut conds: Vec<String> = Vec::new();
        let mut params: Vec<Param> = Vec::new();

        if self.enabled {
            conds.push("enabled = 1".to_string());
        }
        if let Some(t) = self.song_type {
            conds.push("song_type = :song_type".to_string());
            params.push(Param {
                name: "song_type".into(),
                value: ParamValue::Int(t as i64),
            });
        }
        if !self.include_subcategories.is_empty() {
            conds.push(format!(
                "id_subcat IN ({})",
                id_placeholders("subcat_in", self.include_subcategories.len())
            ));
            for (i, id) in self.include_subcategories.iter().enumerate() {
                params.push(Param {
                    name: format!("subcat_in_{i}"),
                    value: ParamValue::Int(*id),
                });
            }
        }
        if !self.exclude_subcategories.is_empty() {
            conds.push(format!(
                "id_subcat NOT IN ({})",
                id_placeholders("subcat_out", self.exclude_subcategories.len())
            ));
            for (i, id) in self.exclude_subcategories.iter().enumerate() {
                params.push(Param {
                    name: format!("subcat_out_{i}"),
                    value: ParamValue::Int(*id),
                });
            }
        }
        // Category membership is soft (no FK): resolve via subcategory.parentid.
        if !self.include_categories.is_empty() {
            conds.push(format!(
                "id_subcat IN (SELECT ID FROM subcategory WHERE parentid IN ({}))",
                id_placeholders("cat_in", self.include_categories.len())
            ));
            for (i, id) in self.include_categories.iter().enumerate() {
                params.push(Param {
                    name: format!("cat_in_{i}"),
                    value: ParamValue::Int(*id),
                });
            }
        }
        if !self.exclude_categories.is_empty() {
            conds.push(format!(
                "id_subcat NOT IN (SELECT ID FROM subcategory WHERE parentid IN ({}))",
                id_placeholders("cat_out", self.exclude_categories.len())
            ));
            for (i, id) in self.exclude_categories.iter().enumerate() {
                params.push(Param {
                    name: format!("cat_out_{i}"),
                    value: ParamValue::Int(*id),
                });
            }
        }
        if self.genre_id != 0 {
            conds.push("id_genre = :genre".to_string());
            params.push(Param {
                name: "genre".into(),
                value: ParamValue::Int(self.genre_id),
            });
        }
        for (col, val) in [
            ("lang", &self.language),
            ("mood", &self.mood),
            ("gender", &self.gender),
        ] {
            if let Some(v) = val {
                conds.push(format!("{col} = :{col}"));
                params.push(Param {
                    name: col.into(),
                    value: ParamValue::Text(v.clone()),
                });
            }
        }
        if let Some(w) = self.weight_min {
            conds.push("weight >= :weight_min".to_string());
            params.push(Param {
                name: "weight_min".into(),
                value: ParamValue::Float(w),
            });
        }
        // Replay avoidance: sentinel rows ('2002-01-01 00:00:01' = never
        // played) are older than any cutoff, so they pass automatically.
        for (col, minutes) in [
            ("date_played", self.repeat_track_minutes),
            ("artist_played", self.repeat_artist_minutes),
            ("title_played", self.repeat_title_minutes),
            ("album_played", self.repeat_album_minutes),
        ] {
            if let Some(cut) = self.cutoff(minutes) {
                conds.push(format!("{col} <= :{col}_cutoff"));
                params.push(Param {
                    name: format!("{col}_cutoff"),
                    value: ParamValue::DateTime(cut.format("%Y-%m-%d %H:%M:%S").to_string()),
                });
            }
        }
        if self.respect_play_limit {
            conds.push("(play_limit = 0 OR count_played < play_limit)".to_string());
        }
        if self.respect_availability_window {
            conds.push(
                "(start_date = :start_sentinel OR start_date <= :now_start) AND \
                 (end_date = :end_sentinel OR end_date >= :now_end)"
                    .to_string(),
            );
            let now_s = self.now.format("%Y-%m-%d %H:%M:%S").to_string();
            params.push(Param {
                name: "start_sentinel".into(),
                value: ParamValue::DateTime(sentinel::NEVER_PLAYED.into()),
            });
            params.push(Param {
                name: "end_sentinel".into(),
                value: ParamValue::DateTime(sentinel::NEVER_PLAYED.into()),
            });
            params.push(Param {
                name: "now_start".into(),
                value: ParamValue::DateTime(now_s.clone()),
            });
            params.push(Param {
                name: "now_end".into(),
                value: ParamValue::DateTime(now_s),
            });
        }

        let order_by = match self.selection {
            SelectionMode::Weight => "weight DESC, count_played ASC, date_played ASC".to_string(),
            SelectionMode::LeastPlayed => "count_played ASC, date_played ASC".to_string(),
            SelectionMode::Oldest => "date_played ASC".to_string(),
            SelectionMode::Newest => "date_added DESC".to_string(),
            // Deterministic server-side order; the core `Random` mode hashes
            // ids in-memory instead (keep SQL parity with `ORDER BY ID`).
            SelectionMode::Random => "ID".to_string(),
        };

        SqlFragments {
            select: "ID, artist, title, path, duration, weight, song_type, id_genre, id_subcat, \
                     date_played, artist_played, title_played, album_played, count_played"
                .to_string(),
            from: "songs".to_string(),
            where_clause: conds.join(" AND "),
            order_by,
            limit: self.limit,
            params,
        }
    }
}

fn id_placeholders(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!(":{prefix}_{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}
