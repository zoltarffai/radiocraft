//! AutoDJ selection pipeline (docs/02 section 3): pick the active rotation at
//! a clock instant, build the candidate query (delegated to [`SchedulerQuery`]),
//! and apply the pure-Rust candidate filters/ordering the db layer must mirror
//! in SQL: replay-interval rejection, play-limit guard, availability window,
//! and weight/least-played ordering. Sweeper, teaser, VDF, file-by-date and
//! newest-from-folder helpers live here too.

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::booking::sentinel;
use crate::rotation::{Rotation, RotationCriteria, SchedulerQuery, SelectionMode, select_rotations};
use crate::template::render_date_mask;

/// One candidate row (the columns the AutoDJ query projects + availability /
/// sweeper metadata the planner needs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackCandidate {
    pub id: i64,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub path: String,
    pub duration: f64,
    pub weight: f64,
    pub song_type: i32,
    pub id_subcat: i64,
    pub id_genre: i64,
    pub language: String,
    pub mood: String,
    pub gender: String,
    pub count_played: i64,
    /// 0 = unlimited.
    pub play_limit: i64,
    pub enabled: bool,
    /// `date_played` (None = the '2002-01-01 00:00:01' sentinel = never played).
    pub date_played: Option<NaiveDateTime>,
    pub artist_played: Option<NaiveDateTime>,
    pub title_played: Option<NaiveDateTime>,
    pub album_played: Option<NaiveDateTime>,
    pub date_added: Option<NaiveDateTime>,
    /// Availability window (None = sentinel / unlimited).
    pub start_date: Option<NaiveDateTime>,
    pub end_date: Option<NaiveDateTime>,
    /// Assigned sweeper song id (`sw_id`; NO_REF = none).
    pub sw_id: i64,
    /// `sw_first`: play the sweeper *before* the track.
    pub sw_first: bool,
    /// `sw_play`: offset (seconds) to start the sweeper *within* the track.
    pub sw_play: f64,
    /// Assigned voice-track (`vt_id`; NO_REF = none).
    pub vt_id: i64,
}

impl Default for TrackCandidate {
    fn default() -> Self {
        TrackCandidate {
            id: 0,
            artist: String::new(),
            title: String::new(),
            album: String::new(),
            path: String::new(),
            duration: 0.0,
            weight: 0.0,
            song_type: 0,
            id_subcat: 0,
            id_genre: 0,
            language: String::new(),
            mood: String::new(),
            gender: String::new(),
            count_played: 0,
            play_limit: 0,
            enabled: true,
            date_played: None,
            artist_played: None,
            title_played: None,
            album_played: None,
            date_added: None,
            start_date: None,
            end_date: None,
            sw_id: sentinel::NO_REF,
            sw_first: false,
            sw_play: 0.0,
            vt_id: sentinel::NO_REF,
        }
    }
}

impl TrackCandidate {
    pub fn new(id: i64, artist: impl Into<String>, title: impl Into<String>) -> Self {
        TrackCandidate {
            id,
            artist: artist.into(),
            title: title.into(),
            ..Default::default()
        }
    }
}

/// A pool of candidates fetched for one criteria query.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CandidatePool {
    pub candidates: Vec<TrackCandidate>,
}

impl CandidatePool {
    pub fn new(candidates: impl Into<Vec<TrackCandidate>>) -> Self {
        CandidatePool {
            candidates: candidates.into(),
        }
    }

    /// True when `c` satisfies every predicate rendered by `q` (client-side
    /// mirror of the SQL WHERE: enabled, play-limit, availability window,
    /// metadata, weight floor, replay cutoffs).
    pub fn accepts(c: &TrackCandidate, q: &SchedulerQuery) -> bool {
        if q.enabled && !c.enabled {
            return false;
        }
        if let Some(t) = q.song_type {
            if c.song_type != t {
                return false;
            }
        }
        if q.respect_play_limit && c.play_limit != 0 && c.count_played >= c.play_limit {
            return false;
        }
        if q.respect_availability_window {
            if let Some(s) = c.start_date {
                if s > q.now {
                    return false;
                }
            }
            if let Some(e) = c.end_date {
                if e < q.now {
                    return false;
                }
            }
        }
        if let Some(w) = q.weight_min {
            if c.weight < w {
                return false;
            }
        }
        for (needle, val) in [
            (q.language.as_ref(), &c.language),
            (q.mood.as_ref(), &c.mood),
            (q.gender.as_ref(), &c.gender),
        ] {
            if let Some(want) = needle {
                if !val.eq_ignore_ascii_case(want) {
                    return false;
                }
            }
        }
        // Replay-avoidance: reject while last-play is *after* the cutoff.
        for (minutes, last) in [
            (q.repeat_track_minutes, c.date_played),
            (q.repeat_artist_minutes, c.artist_played),
            (q.repeat_title_minutes, c.title_played),
            (q.repeat_album_minutes, c.album_played),
        ] {
            if minutes > 0 {
                if let Some(cut) = q.now.checked_sub_signed(chrono::Duration::minutes(minutes)) {
                    if let Some(lp) = last {
                        if lp > cut {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// Filter to the candidates that pass `q`'s predicates.
    pub fn filter(&self, q: &SchedulerQuery) -> Vec<TrackCandidate> {
        self.candidates
            .iter()
            .filter(|c| Self::accepts(c, q))
            .cloned()
            .collect()
    }
}

/// Why a candidate was rejected during planning (mirrors the SQL predicates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    Disabled,
    SongTypeMismatch,
    PlayLimitReached,
    OutsideAvailabilityWindow,
    WeightBelowFloor,
    MetadataMismatch,
    /// Repeat-interval rejection for `date_played`.
    RepeatTrack,
    /// Repeat-interval rejection for `artist_played`.
    RepeatArtist,
    /// Repeat-interval rejection for `title_played`.
    RepeatTitle,
    /// Repeat-interval rejection for `album_played`.
    RepeatAlbum,
    /// `repeat_rule = 0` forbids the same track twice in a row.
    SameTrackAsLast,
}

/// The chosen track plus the scheduled sweeper/voice-track slots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pick {
    pub candidate: TrackCandidate,
    /// Sweeper to play *before* the track (`sw_first = 1`), if any.
    pub sweeper_before: Option<TrackCandidate>,
    /// Sweeper to start *within* the track at `sw_play` seconds, if any.
    pub sweeper_within: Option<TrackCandidate>,
    /// Assigned voice-track slot (VDF / voice tracking), if any.
    pub voice_track: Option<TrackCandidate>,
    /// Offset (seconds into the track) where `sweeper_within` starts.
    pub sweeper_offset_seconds: f64,
}

/// How one criteria row ordered the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderApplied {
    Selection(SelectionMode),
}

/// Full planning decision for one AutoDJ slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchedulerDecision {
    /// Chosen rotation (None = no rotation matches the clock).
    pub rotation_id: Option<i64>,
    pub rotation_name: Option<String>,
    /// Chosen criteria row id (None = no active criteria).
    pub criteria_id: Option<i64>,
    /// Declarative candidate query the db crate should execute.
    pub query: Option<SchedulerQuery>,
    /// Accepted candidates in selection order.
    pub ordered: Vec<TrackCandidate>,
    /// Rejected candidates with the first predicate that rejected them.
    pub rejected: Vec<(i64, RejectReason)>,
    /// Final pick (None = pool exhausted or same-track forbidden).
    pub pick: Option<Pick>,
}

/// Scheduler tuning knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SchedulerConfig {
    /// Enforce `play_limit = 0 OR count_played < play_limit`.
    pub respect_play_limit: bool,
    /// Enforce replay-avoidance intervals (artist/title/album separation).
    pub respect_repeat_intervals: bool,
    /// Enforce `songs.start_date/end_date` availability.
    pub respect_availability: bool,
    /// Pin a song type (e.g. build a sweeper-only pool); None = any.
    pub forced_song_type: Option<i32>,
    /// Max candidates the query should return.
    pub max_candidates: Option<u32>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        SchedulerConfig {
            respect_play_limit: true,
            respect_repeat_intervals: true,
            respect_availability: true,
            forced_song_type: None,
            max_candidates: None,
        }
    }
}

/// The AutoDJ planner: rotation pick -> criteria query -> filter -> order -> pick.
#[derive(Debug, Clone, Default)]
pub struct Scheduler {
    pub config: SchedulerConfig,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Self {
        Scheduler { config }
    }

    /// Plan the next AutoDJ slot at `now`.
    ///
    /// `last_pick_id` is the track just played (the repeat-rule guard: with
    /// `repeat_rule = 0` the exact same track may not be chosen again).
    pub fn plan(
        &self,
        rotations: &[Rotation],
        pool: &CandidatePool,
        now: NaiveDateTime,
        last_pick_id: Option<i64>,
    ) -> SchedulerDecision {
        // Walk rotations by priority, then their active criteria by ord; use
        // the first criteria that yields a pick (parity with the app trying
        // each criteria query in turn until the playlist slot fills).
        // Rejection diagnostics from every attempted criteria are kept so a
        // no-pick decision explains *why* nothing matched.
        let active = select_rotations(rotations, now);
        let mut accumulated: Vec<(i64, RejectReason)> = Vec::new();
        for rotation in active {
            for criteria in rotation.active_criteria(now) {
                let d = self.plan_one(rotation, criteria, pool, now, last_pick_id);
                if d.pick.is_some() {
                    let mut d = d;
                    d.rejected = std::mem::take(&mut accumulated)
                        .into_iter()
                        .chain(d.rejected)
                        .collect();
                    return d;
                }
                accumulated.extend(d.rejected);
            }
        }
        SchedulerDecision {
            rotation_id: None,
            rotation_name: None,
            criteria_id: None,
            query: None,
            ordered: Vec::new(),
            rejected: accumulated,
            pick: None,
        }
    }

    fn plan_one(
        &self,
        rotation: &Rotation,
        criteria: &RotationCriteria,
        pool: &CandidatePool,
        now: NaiveDateTime,
        last_pick_id: Option<i64>,
    ) -> SchedulerDecision {
        let mut q = criteria.query_for(now, self.config.forced_song_type);
        q.respect_play_limit = self.config.respect_play_limit;
        q.respect_availability_window = self.config.respect_availability;
        if !self.config.respect_repeat_intervals {
            q.repeat_track_minutes = 0;
            q.repeat_artist_minutes = 0;
            q.repeat_title_minutes = 0;
            q.repeat_album_minutes = 0;
        }
        q.limit = self.config.max_candidates;

        let mut rejected = Vec::new();
        let mut ordered = Vec::new();
        for c in &pool.candidates {
            match self.reject_reason(c, &q) {
                Some(r) => rejected.push((c.id, r)),
                None => ordered.push(c.clone()),
            }
        }
        Self::sort_pool(&mut ordered, criteria.selection);

        // Repeat-rule guard: forbid picking the exact track just played.
        if criteria.repeat_rule == 0 {
            if let Some(last) = last_pick_id {
                if ordered.iter().any(|c| c.id == last) {
                    ordered.retain(|c| c.id != last);
                    rejected.push((last, RejectReason::SameTrackAsLast));
                }
            }
        }

        let pick = ordered.first().map(|c| Pick {
            candidate: c.clone(),
            sweeper_before: if c.sw_id != sentinel::NO_REF && c.sw_first {
                pool.candidates.iter().find(|s| s.id == c.sw_id).cloned()
            } else {
                None
            },
            sweeper_within: if c.sw_id != sentinel::NO_REF && !c.sw_first {
                pool.candidates.iter().find(|s| s.id == c.sw_id).cloned()
            } else {
                None
            },
            voice_track: if c.vt_id != sentinel::NO_REF {
                pool.candidates.iter().find(|s| s.id == c.vt_id).cloned()
            } else {
                None
            },
            sweeper_offset_seconds: c.sw_play,
        });

        SchedulerDecision {
            rotation_id: Some(rotation.id),
            rotation_name: Some(rotation.name.clone()),
            criteria_id: Some(criteria.id),
            query: Some(q),
            ordered,
            rejected,
            pick,
        }
    }

    /// First predicate that rejects `c` (None = accepted).
    fn reject_reason(&self, c: &TrackCandidate, q: &SchedulerQuery) -> Option<RejectReason> {
        if q.enabled && !c.enabled {
            return Some(RejectReason::Disabled);
        }
        if let Some(t) = q.song_type {
            if c.song_type != t {
                return Some(RejectReason::SongTypeMismatch);
            }
        }
        if q.respect_play_limit && c.play_limit != 0 && c.count_played >= c.play_limit {
            return Some(RejectReason::PlayLimitReached);
        }
        if q.respect_availability_window
            && (c.start_date.is_some_and(|s| s > q.now) || c.end_date.is_some_and(|e| e < q.now))
        {
            return Some(RejectReason::OutsideAvailabilityWindow);
        }
        if let Some(w) = q.weight_min {
            if c.weight < w {
                return Some(RejectReason::WeightBelowFloor);
            }
        }
        for (needle, val) in [
            (q.language.as_ref(), &c.language),
            (q.mood.as_ref(), &c.mood),
            (q.gender.as_ref(), &c.gender),
        ] {
            if let Some(want) = needle {
                if !val.eq_ignore_ascii_case(want) {
                    return Some(RejectReason::MetadataMismatch);
                }
            }
        }
        for (minutes, last, reason) in [
            (q.repeat_track_minutes, c.date_played, RejectReason::RepeatTrack),
            (q.repeat_artist_minutes, c.artist_played, RejectReason::RepeatArtist),
            (q.repeat_title_minutes, c.title_played, RejectReason::RepeatTitle),
            (q.repeat_album_minutes, c.album_played, RejectReason::RepeatAlbum),
        ] {
            if minutes > 0 {
                if let Some(cut) = q.now.checked_sub_signed(chrono::Duration::minutes(minutes)) {
                    if last.is_some_and(|lp| lp > cut) {
                        return Some(reason);
                    }
                }
            }
        }
        None
    }

    /// Sort the accepted pool the same way the SQL ORDER BY does.
    pub fn sort_pool(pool: &mut [TrackCandidate], mode: SelectionMode) {
        let never = sentinel::never_played_dt();
        pool.sort_by(|a, b| match mode {
            SelectionMode::Weight => b
                .weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.count_played.cmp(&b.count_played))
                .then(
                    a.date_played
                        .unwrap_or(never)
                        .cmp(&b.date_played.unwrap_or(never)),
                ),
            SelectionMode::LeastPlayed => a
                .count_played
                .cmp(&b.count_played)
                .then(a.date_played.unwrap_or(never).cmp(&b.date_played.unwrap_or(never))),
            SelectionMode::Oldest => a
                .date_played
                .unwrap_or(never)
                .cmp(&b.date_played.unwrap_or(never)),
            SelectionMode::Newest => b
                .date_added
                .unwrap_or(never)
                .cmp(&a.date_added.unwrap_or(never)),
            // Deterministic shuffle stand-in: stable id order (the app layer
            // re-seeds at runtime; core keeps it pure and reproducible).
            SelectionMode::Random => a.id.cmp(&b.id),
        });
    }
}

/// Sweepers scheduled around a track: `sw_first = 1` plays the sweeper
/// *before* the track; otherwise the sweeper starts at `sw_play` seconds
/// *within* the track (queuelist.ETA semantics).
pub fn plan_sweeper_slot(
    track: &TrackCandidate,
    sweepers: &[TrackCandidate],
) -> Pick {
    let find = |id: i64| sweepers.iter().find(|s| s.id == id).cloned();
    Pick {
        candidate: track.clone(),
        sweeper_before: if track.sw_id != sentinel::NO_REF && track.sw_first {
            find(track.sw_id)
        } else {
            None
        },
        sweeper_within: if track.sw_id != sentinel::NO_REF && !track.sw_first {
            find(track.sw_id)
        } else {
            None
        },
        voice_track: if track.vt_id != sentinel::NO_REF {
            find(track.vt_id)
        } else {
            None
        },
        sweeper_offset_seconds: track.sw_play,
    }
}

/// Teaser support: the up-to-five upcoming titles that back the
/// `next1_`..`next5_` template variables (docs/02 section 7).
pub fn teaser_titles(next: &[TrackCandidate]) -> Vec<String> {
    next.iter().take(5).map(|c| c.title.clone()).collect()
}

/// File-by-date mode (docs/02 section 3 / track type 12): expand the
/// folder + comments-field mask into the concrete file name for `date`.
/// The mask includes the extension (e.g. `{YYYY}-{MM}.mp3`).
pub fn expand_file_by_date(folder: &str, mask: &str, date: chrono::NaiveDate) -> String {
    let rendered = render_date_mask(mask, date);
    let sep = if folder.ends_with('/') || folder.is_empty() {
        ""
    } else {
        "/"
    };
    format!("{folder}{sep}{rendered}")
}

/// Newest-from-folder mode (track type 13): the newest `date_added` among
/// pool entries whose `path` starts with `folder` (missing `date_added`
/// sorts oldest, never chosen first when others have dates).
pub fn newest_from_folder<'a>(
    pool: &'a [TrackCandidate],
    folder: &str,
) -> Option<&'a TrackCandidate> {
    pool.iter()
        .filter(|c| c.path.starts_with(folder))
        .max_by_key(|c| c.date_added.unwrap_or(sentinel::never_played_dt()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rotation::{DayMask, Rotation, time_in_window};
    use chrono::NaiveDate;

    fn at(y: i32, m: u32, d: u32, hh: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(hh, mi, 0)
            .unwrap()
    }

    // --- required test: rotation time-window match ---------------------------

    #[test]
    fn rotation_time_window_match() {
        // Plain morning window.
        assert!(time_in_window(
            Some(at_t(7, 0)),
            Some(at_t(11, 59)),
            at_t(9, 30)
        ));
        assert!(!time_in_window(
            Some(at_t(7, 0)),
            Some(at_t(11, 59)),
            at_t(12, 0)
        ));
        // Overnight window wraps past midnight: 22:00..02:00.
        assert!(time_in_window(
            Some(at_t(22, 0)),
            Some(at_t(2, 0)),
            at_t(23, 30)
        ));
        assert!(time_in_window(
            Some(at_t(22, 0)),
            Some(at_t(2, 0)),
            at_t(1, 0)
        ));
        assert!(!time_in_window(
            Some(at_t(22, 0)),
            Some(at_t(2, 0)),
            at_t(12, 0)
        ));
        // Open-ended windows.
        assert!(time_in_window(None, None, at_t(0, 0)));
        assert!(time_in_window(Some(at_t(6, 0)), None, at_t(23, 0)));
        assert!(!time_in_window(Some(at_t(6, 0)), None, at_t(5, 59)));

        // Rotation-level match (daypart + day bitmask).
        let mut r = Rotation::new(1, "morning");
        r.time_start = Some(at_t(7, 0));
        r.time_end = Some(at_t(11, 0));
        r.days = DayMask::ALL;
        assert!(r.matches_time(at(2026, 10, 29, 9, 0))); // Thursday
        assert!(!r.matches_time(at(2026, 10, 29, 14, 0)));
    }

    fn at_t(h: u32, m: u32) -> chrono::NaiveTime {
        chrono::NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    // --- required test: day bitmask ----------------------------------------

    #[test]
    fn day_bitmask() {
        // '&' = all days.
        assert_eq!(DayMask::parse("&"), DayMask::ALL);
        let mask = DayMask::parse("1,5"); // Monday + Friday (num_uses_sun).
        assert!(mask.contains_weekday(1));
        assert!(mask.contains_weekday(5));
        assert!(!mask.contains_weekday(0)); // Sunday
        // 2026-10-29 is a Thursday (weekday index 4 with Sun=0).
        assert!(!mask.matches_date(at(2026, 10, 29, 0, 0)));
        let thursday = DayMask::for_weekday(4);
        assert!(thursday.matches_date(at(2026, 10, 29, 12, 0)));
        assert!(!thursday.matches_date(at(2026, 10, 30, 12, 0)));
    }

    // --- required test: repeat-interval rejection ---------------------------

    #[test]
    fn repeat_interval_rejection() {
        let now = at(2026, 10, 29, 12, 0);
        let mut crit = RotationCriteria {
            id: 7,
            rotation_id: 1,
            ..Default::default()
        };
        crit.repeat_artist_interval = 60; // no artist repeat for 60 min
        let q = crit.query_for(now, None);

        let mut recent = TrackCandidate::new(10, "Cher", "Believe");
        recent.artist_played = Some(at(2026, 10, 29, 11, 30)); // 30 min ago
        assert_eq!(
            Scheduler::default().reject_reason(&recent, &q),
            Some(RejectReason::RepeatArtist)
        );

        // Never-played (sentinel/None) rows pass the rule.
        let fresh = TrackCandidate::new(11, "Abba", "Dancing Queen");
        assert_eq!(Scheduler::default().reject_reason(&fresh, &q), None);

        // Outside the interval: played 2h ago with a 60-min rule -> OK.
        let mut old = TrackCandidate::new(12, "Cher", "Gypa");
        old.artist_played = Some(at(2026, 10, 29, 10, 0));
        assert_eq!(Scheduler::default().reject_reason(&old, &q), None);

        // Track-level rejection carries through the plan.
        let mut crit2 = RotationCriteria {
            id: 8,
            rotation_id: 1,
            ..Default::default()
        };
        crit2.repeat_track_interval = 90;
        let q2 = crit2.query_for(now, None);
        let mut c = TrackCandidate::new(13, "X", "Y");
        c.date_played = Some(now - chrono::Duration::minutes(30));
        assert_eq!(
            Scheduler::default().reject_reason(&c, &q2),
            Some(RejectReason::RepeatTrack)
        );
    }

    // --- required test: weight ordering ------------------------------------

    #[test]
    fn weight_ordering() {
        let mut low = TrackCandidate::new(1, "a", "low");
        low.weight = 1.0;
        let mut mid = TrackCandidate::new(2, "b", "mid");
        mid.weight = 3.0;
        let mut high = TrackCandidate::new(3, "c", "high");
        high.weight = 5.0;
        let mut pool = vec![low.clone(), high.clone(), mid.clone()];
        Scheduler::sort_pool(&mut pool, SelectionMode::Weight);
        assert_eq!(
            pool.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );

        // Tie-break: equal weight -> least played first.
        let mut a = TrackCandidate::new(4, "t", "tie");
        a.weight = 5.0;
        a.count_played = 9;
        let mut b = TrackCandidate::new(5, "t", "tie2");
        b.weight = 5.0;
        b.count_played = 1;
        let mut pool = vec![a, b];
        Scheduler::sort_pool(&mut pool, SelectionMode::Weight);
        assert_eq!(pool[0].id, 5);

        // Least-played mode.
        let mut pool = vec![
            {
                let mut x = TrackCandidate::new(6, "x", "more");
                x.count_played = 10;
                x
            },
            {
                let mut x = TrackCandidate::new(7, "x", "less");
                x.count_played = 1;
                x
            },
        ];
        Scheduler::sort_pool(&mut pool, SelectionMode::LeastPlayed);
        assert_eq!(pool[0].id, 7);
    }

    // --- plan end-to-end ----------------------------------------------------

    #[test]
    fn plan_picks_highest_weight_within_rotation() {
        let now = at(2026, 10, 29, 10, 0); // Thursday
        let mut r = Rotation::new(1, "day");
        let mut crit = RotationCriteria {
            id: 1,
            rotation_id: 1,
            ..Default::default()
        };
        crit.selection = SelectionMode::Weight;
        r.criteria.push(crit);

        let mut pool = CandidatePool::default();
        let mut low = TrackCandidate::new(1, "l", "lo");
        low.weight = 1.0;
        let mut high = TrackCandidate::new(2, "h", "hi");
        high.weight = 9.0;
        pool.candidates.extend([low, high]);

        let sched = Scheduler::default();
        let d = sched.plan(&[r], &pool, now, None);
        assert_eq!(d.rotation_id, Some(1));
        assert_eq!(d.pick.as_ref().unwrap().candidate.id, 2);
    }

    #[test]
    fn plan_play_limit_and_repeat_rule_guards() {
        let now = at(2026, 10, 29, 10, 0);
        let mut r = Rotation::new(1, "day");
        let crit = RotationCriteria {
            id: 1,
            rotation_id: 1,
            ..Default::default()
        };
        r.criteria.push(crit);

        let mut capped = TrackCandidate::new(1, "c", "cap");
        capped.play_limit = 2;
        capped.count_played = 2;
        capped.weight = 100.0;
        let other = TrackCandidate::new(2, "o", "ok");
        let pool = CandidatePool::new(vec![capped, other]);

        let d = Scheduler::default().plan(&[r.clone()], &pool, now, None);
        assert_eq!(d.pick.as_ref().unwrap().candidate.id, 2);
        assert!(d.rejected.contains(&(1, RejectReason::PlayLimitReached)));

        // repeat_rule = 0 forbids picking the track just played.
        let d2 = Scheduler::default().plan(&[r], &pool, now, Some(2));
        assert!(d2.pick.is_none());
        assert!(d2.rejected.contains(&(2, RejectReason::SameTrackAsLast)));
    }

    #[test]
    fn sweeper_and_teaser_helpers() {
        let mut track = TrackCandidate::new(10, "t", "track");
        track.sw_id = 11;
        track.sw_first = false;
        track.sw_play = 20.0;
        track.vt_id = 12;
        let sweeper = TrackCandidate::new(11, "s", "sw");
        let voice = TrackCandidate::new(12, "v", "voice");
        let p = plan_sweeper_slot(&track, &[sweeper.clone(), voice.clone()]);
        assert!(p.sweeper_before.is_none());
        assert_eq!(p.sweeper_within.as_ref().unwrap().id, 11);
        assert_eq!(p.voice_track.as_ref().unwrap().id, 12);
        assert_eq!(p.sweeper_offset_seconds, 20.0);

        // sw_first = sweeper plays before the track.
        track.sw_first = true;
        let p2 = plan_sweeper_slot(&track, &[sweeper, voice]);
        assert_eq!(p2.sweeper_before.as_ref().unwrap().id, 11);
        assert!(p2.sweeper_within.is_none());

        // teaser titles cap at 5.
        let next: Vec<TrackCandidate> =
            (0..7).map(|i| TrackCandidate::new(i, "a", format!("t{i}"))).collect();
        assert_eq!(teaser_titles(&next).len(), 5);
    }

    #[test]
    fn file_by_date_and_newest_from_folder() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        assert_eq!(
            expand_file_by_date("/news", "{YYYY}-{MM}.mp3", d),
            "/news/2026-10.mp3"
        );

        let mut older = TrackCandidate::new(1, "a", "old");
        older.path = "/promo/a.mp3".to_string();
        older.date_added = Some(at(2026, 1, 1, 0, 0));
        let mut newer = TrackCandidate::new(2, "a", "new");
        newer.path = "/promo/b.mp3".to_string();
        newer.date_added = Some(at(2026, 9, 1, 0, 0));
        let mut outside = TrackCandidate::new(3, "a", "elsewhere");
        outside.path = "/other/c.mp3".to_string();
        let pool = vec![older, newer, outside];
        assert_eq!(newest_from_folder(&pool, "/promo").unwrap().id, 2);
        assert!(newest_from_folder(&pool, "/none").is_none());
    }
}
