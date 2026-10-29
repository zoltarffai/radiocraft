//! Scheduled events (docs/02 section 3 / docs/03 section 3): the `events`
//! table model plus the pure "is this event due at clock `now`?" evaluation.
//! The db crate owns persistence and the action runner owns execution; this
//! module only decides *when* an event fires and describes *what* runs.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};

use crate::booking::sentinel;
use crate::rotation::{weekday_index, DayMask};

/// Trigger kind (`events.type`): when the event fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i8)]
pub enum EventTrigger {
    /// 0: fires on the one-shot calendar `date` (once only).
    Calendar = 0,
    /// 1: fires daily at the clock `time` (`HH:mm`, or `HH:mm:ss`).
    Clock = 1,
    /// 2: fires on a weekly `day` bitmask at clock `time`; `hours` optionally
    /// restricts the hour range (`&` = unrestricted).
    Weekly = 2,
    /// 3: fires every `data` tracks played (track-count interval).
    TrackCount = 3,
}

impl EventTrigger {
    pub fn from_db(value: i8) -> Option<EventTrigger> {
        match value {
            0 => Some(EventTrigger::Calendar),
            1 => Some(EventTrigger::Clock),
            2 => Some(EventTrigger::Weekly),
            3 => Some(EventTrigger::TrackCount),
            _ => None,
        }
    }

    pub fn as_i8(self) -> i8 {
        match self {
            EventTrigger::Calendar => 0,
            EventTrigger::Clock => 1,
            EventTrigger::Weekly => 2,
            EventTrigger::TrackCount => 3,
        }
    }
}

/// One action the event runner executes: plugin-registered `name` + stable
/// `id` and its argument list (docs/03 `EventAction { name, id, args[] }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventAction {
    pub name: String,
    pub id: String,
    pub args: Vec<String>,
}

impl EventAction {
    pub fn new(name: impl Into<String>, id: impl Into<String>, args: Vec<String>) -> Self {
        EventAction {
            name: name.into(),
            id: id.into(),
            args,
        }
    }
}

/// Row shape of the `events` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    /// `events.type` (0..=3).
    pub event_type: i8,
    /// Trigger clock time (`HH:mm` / `HH:mm:ss`); `&` = not set.
    pub time: String,
    pub name: String,
    /// One-shot calendar date (`NO_DATE` sentinel = unset).
    pub date: Option<NaiveDate>,
    /// Weekly day bitmask string (`&` = all days).
    pub day: String,
    /// Weekly hour range (`&` = any hour).
    pub hours: String,
    /// Trigger payload: track-count interval for type 3; action config for others.
    pub data: Option<String>,
    pub enabled: bool,
    pub cat_id: i64,
    /// Participates in the SmartEvents interval scan.
    pub smart: bool,
    /// Actions fire as one grouped batch.
    pub grouped: bool,
    /// Actions loaded from `events_rules`/event config by the db layer.
    pub actions: Vec<EventAction>,
}

impl Event {
    pub fn new(id: i64, event_type: EventTrigger, name: impl Into<String>) -> Self {
        Event {
            id,
            event_type: event_type.as_i8(),
            time: sentinel::NOT_SET.to_string(),
            name: name.into(),
            date: None,
            day: sentinel::NOT_SET.to_string(),
            hours: sentinel::NOT_SET.to_string(),
            data: None,
            enabled: true,
            cat_id: 0,
            smart: false,
            grouped: false,
            actions: Vec::new(),
        }
    }

    pub fn trigger(&self) -> Option<EventTrigger> {
        EventTrigger::from_db(self.event_type)
    }

    /// Parse the clock `time` field (`HH:mm`, `HH:mm:ss`, or `HH:mm:ss.fff`).
    pub fn clock_time(&self) -> Option<chrono::NaiveTime> {
        parse_clock(&self.time)
    }

    /// Track-count interval for type-3 events (`data` holds the integer).
    pub fn track_interval(&self) -> Option<u64> {
        self.data.as_deref().and_then(|s| s.trim().parse().ok()).filter(|n: &u64| *n > 0)
    }
}

/// Why an event became due (also carries which rule matched).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DueReason {
    /// One-shot calendar date equals `now`'s date.
    CalendarDate,
    /// Clock time equals `now`'s minute.
    ClockTime,
    /// Weekly day matched and the clock hour is allowed.
    WeeklyDay,
    /// Track count reached a multiple of the interval.
    TrackInterval,
}

/// Result of evaluating one event at an instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Due {
    pub event_id: i64,
    pub reason: DueReason,
}

/// Parse a clock literal (`HH:mm`, `HH:mm:ss`, `HH:mm:ss.fff`); `&` / empty
/// return None.
pub fn parse_clock(text: &str) -> Option<chrono::NaiveTime> {
    let text = text.trim();
    if text.is_empty() || text == sentinel::NOT_SET {
        return None;
    }
    for fmt in ["%H:%M:%S%.f", "%H:%M:%S", "%H:%M"] {
        if let Ok(t) = chrono::NaiveTime::parse_from_str(text, fmt) {
            return Some(t);
        }
    }
    None
}

/// True when the `hours` field (`&` = any; otherwise `start-end` such as
/// `9-17`, or a comma list of hours) admits `hour`.
pub fn hour_allowed(hours: &str, hour: u32) -> bool {
    let hours = hours.trim();
    if hours.is_empty() || hours == sentinel::NOT_SET {
        return true;
    }
    hours.split(',').any(|spec| {
        if let Some((a, b)) = spec.split_once('-') {
            let a: u32 = a.trim().parse().unwrap_or(24);
            let b: u32 = b.trim().parse().unwrap_or(24);
            if a <= b {
                (a..=b).contains(&hour)
            } else {
                // overnight span, e.g. 22-2
                hour >= a || hour <= b
            }
        } else {
            spec.trim().parse::<u32>().is_ok_and(|h| h == hour)
        }
    })
}

/// Is `event` due at the wall-clock instant `now`?
///
/// `last_fired` is the event's previous firing timestamp (`None` = never
/// fired); used to suppress the calendar one-shot after its date passes and
/// to deduplicate clock events that already fired this minute.
pub fn due_now(event: &Event, now: NaiveDateTime, last_fired: Option<NaiveDateTime>) -> Option<Due> {
    if !event.enabled {
        return None;
    }
    match event.trigger()? {
        EventTrigger::Calendar => {
            let day = event.date?;
            if day != now.date() {
                return None;
            }
            // The clock part: fire at (or after) the event's time today.
            let fire_at = day.and_time(event.clock_time().unwrap_or_default());
            if now < fire_at {
                return None;
            }
            if last_fired.is_some_and(|lf| lf >= fire_at) {
                return None; // already fired for this one-shot
            }
            Some(Due { event_id: event.id, reason: DueReason::CalendarDate })
        }
        EventTrigger::Clock => {
            let want = event.clock_time()?;
            if now.time() != want {
                return None;
            }
            if last_fired.is_some_and(|lf| lf.date() == now.date()) {
                return None; // already fired at this minute today
            }
            Some(Due { event_id: event.id, reason: DueReason::ClockTime })
        }
        EventTrigger::Weekly => {
            let mask = DayMask::parse(&event.day);
            if !mask.contains_weekday(weekday_index(Datelike::weekday(&now.date()))) {
                return None;
            }
            if let Some(want) = event.clock_time() {
                if now.time() != want {
                    return None;
                }
            }
            if !hour_allowed(&event.hours, now.hour()) {
                return None;
            }
            if last_fired.is_some_and(|lf| lf.date() == now.date()) {
                return None;
            }
            Some(Due { event_id: event.id, reason: DueReason::WeeklyDay })
        }
        EventTrigger::TrackCount => {
            // handled by `due_for_track_count`; not clock-evaluable.
            None
        }
    }
}

/// Track-count trigger: `count` is the number of tracks played so far; the
/// event is due when `count` is a non-zero multiple of its `data` interval.
pub fn due_for_track_count(event: &Event, count: u64) -> Option<Due> {
    if !event.enabled || event.trigger()? != EventTrigger::TrackCount {
        return None;
    }
    let interval = event.track_interval()?;
    if count > 0 && count.is_multiple_of(interval) {
        Some(Due { event_id: event.id, reason: DueReason::TrackInterval })
    } else {
        None
    }
}

/// SmartEvents scan (docs/03 `SmartEventsInterval`): evaluate every enabled,
/// smart event against `now` (track-count events skipped; use
/// [`due_for_track_count`] for those).
pub fn scan_smart_events(
    events: &[Event],
    now: NaiveDateTime,
    last_fired: impl Fn(i64) -> Option<NaiveDateTime>,
) -> Vec<Due> {
    events
        .iter()
        .filter(|e| e.enabled && e.smart && e.trigger() != Some(EventTrigger::TrackCount))
        .filter_map(|e| due_now(e, now, last_fired(e.id)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::booking::sentinel::NO_DATE;

    fn at(y: i32, m: u32, d: u32, hh: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(hh, mi, 0)
            .unwrap()
    }

    #[test]
    fn trigger_round_trip() {
        for kind in [EventTrigger::Calendar, EventTrigger::Clock, EventTrigger::Weekly, EventTrigger::TrackCount] {
            assert_eq!(EventTrigger::from_db(kind.as_i8()), Some(kind));
        }
        assert_eq!(EventTrigger::from_db(9), None);
    }

    #[test]
    fn calendar_one_shot_fires_on_date_and_time() {
        let mut e = Event::new(1, EventTrigger::Calendar, "station anniversary");
        e.date = NaiveDate::from_ymd_opt(2026, 10, 29);
        e.time = "6:15".to_string();

        // before the fire time: not due
        assert_eq!(due_now(&e, at(2026, 10, 29, 6, 14), None), None);
        // at the minute: due
        assert_eq!(
            due_now(&e, at(2026, 10, 29, 6, 15), None),
            Some(Due { event_id: 1, reason: DueReason::CalendarDate })
        );
        // already fired: suppressed
        assert_eq!(due_now(&e, at(2026, 10, 29, 6, 15), Some(at(2026, 10, 29, 6, 15))), None);
        // wrong date: not due
        assert_eq!(due_now(&e, at(2026, 10, 30, 6, 15), None), None);
        // unset date sentinel: never due
        e.date = None;
        assert_eq!(due_now(&e, at(2026, 10, 29, 6, 15), None), None);
    }

    #[test]
    fn clock_event_matches_minute_and_dedups() {
        let mut e = Event::new(2, EventTrigger::Clock, "jingles");
        e.time = "07:00:00".to_string();
        assert_eq!(
            due_now(&e, at(2026, 10, 29, 7, 0), None),
            Some(Due { event_id: 2, reason: DueReason::ClockTime })
        );
        assert_eq!(due_now(&e, at(2026, 10, 29, 7, 1), None), None);
        // same-day dedup, next day fires again
        assert_eq!(due_now(&e, at(2026, 10, 29, 7, 0), Some(at(2026, 10, 29, 7, 0))), None);
        assert!(due_now(&e, at(2026, 10, 30, 7, 0), Some(at(2026, 10, 29, 7, 0))).is_some());
    }

    #[test]
    fn weekly_day_and_hours() {
        let mut e = Event::new(3, EventTrigger::Weekly, "morning talk");
        e.time = "9:00".to_string();
        e.day = "1,5".to_string(); // Mon + Fri (Sun=0)

        let mon = at(2026, 10, 26, 9, 0);
        let tue = at(2026, 10, 27, 9, 0);
        assert_eq!(due_now(&e, mon, None).map(|d| d.reason), Some(DueReason::WeeklyDay));
        assert_eq!(due_now(&e, tue, None), None);

        // hour-range restriction
        e.day = "&".to_string();
        e.hours = "10-18".to_string();
        e.time = "10:00".to_string();
        assert_eq!(due_now(&e, mon, None), None); // 9:00 outside 10-18
        assert!(due_now(&e, at(2026, 10, 26, 10, 0), None).is_some());

        // overnight hour span 22-2
        e.hours = "22-2".to_string();
        e.time = "23:30".to_string();
        assert!(due_now(&e, at(2026, 10, 26, 23, 30), None).is_some());
        assert!(hour_allowed("22-2", 1));
        assert!(!hour_allowed("22-2", 12));
    }

    #[test]
    fn track_count_trigger() {
        let mut e = Event::new(4, EventTrigger::TrackCount, "station hourly sweep");
        e.data = Some("10".to_string());
        assert_eq!(due_for_track_count(&e, 9), None);
        assert_eq!(due_for_track_count(&e, 10).map(|d| d.reason), Some(DueReason::TrackInterval));
        assert_eq!(due_for_track_count(&e, 30).map(|d| d.reason), Some(DueReason::TrackInterval));
        assert_eq!(due_for_track_count(&e, 0), None);
        // clock evaluator ignores track-count events
        assert!(due_now(&e, at(2026, 10, 29, 12, 0), None).is_none());
    }

    #[test]
    fn smart_scan_skips_disabled_and_track_count() {
        let mut smart_clock = Event::new(5, EventTrigger::Clock, "a");
        smart_clock.time = "12:00".to_string();
        smart_clock.smart = true;

        let mut disabled = smart_clock.clone();
        disabled.id = 6;
        disabled.enabled = false;

        let mut not_smart = smart_clock.clone();
        not_smart.id = 7;
        not_smart.smart = false;

        let mut smart_tc = Event::new(8, EventTrigger::TrackCount, "tc");
        smart_tc.data = Some("5".to_string());
        smart_tc.smart = true;

        let due = scan_smart_events(
            &[smart_clock.clone(), disabled, not_smart, smart_tc],
            at(2026, 10, 29, 12, 0),
            |_id| None,
        );
        assert_eq!(due, vec![Due { event_id: 5, reason: DueReason::ClockTime }]);
    }

    #[test]
    fn event_action_shape_and_no_date_default() {
        let a = EventAction::new("PlayTracks", "p1", vec!["rot=3".into(), "n=2".into()]);
        assert_eq!(a.args.len(), 2);
        assert_eq!(a.name, "PlayTracks");

        // NO_DATE literal is the schema default ("unset").
        assert_eq!(NO_DATE, "2002-01-01");
        let e = Event::new(9, EventTrigger::Weekly, "x");
        assert_eq!(e.day, "&");
        assert_eq!(e.clock_time(), None); // "&" -> none
    }
}
