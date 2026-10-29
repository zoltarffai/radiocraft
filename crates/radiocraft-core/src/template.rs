//! The `$var$` interpolation engine used for logging, now-playing broadcast,
//! and now-playing aux metadata, plus the file-by-date / playlist-by-date
//! mask expansion rules.
//!
//! Rules implemented from `docs/02-feature-ledger.md` §7:
//! - Variables are referenced as `$var$` and matching is **case-sensitive**.
//! - Unknown variables are left in the output untouched.
//! - `$var_enc$` variants receive percent-encoding (the RadioDJ `_enc` suffix).
//! - `$next1_title$` .. `$next5_title$` refer to the up-to-five upcoming tracks.
//! - Durations are exposed as `$duration$` (`HH:MM:SS`),
//!   `$durationinseconds$`, and `$durationinmillseconds$`.

use std::collections::HashMap;

use chrono::{Local, NaiveDate, NaiveDateTime};

use crate::model::SongData;

/// Interpolation context: a case-sensitive map of variable names to values.
#[derive(Debug, Default, Clone)]
pub struct Context {
    vars: HashMap<String, String>,
    next_titles: Vec<String>,
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a variable (overwrite).
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(name.into(), value.into());
    }

    /// Get a variable value.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// Provide the upcoming tracks backing `$nextN_title$` (max 5 are used).
    pub fn set_next_tracks(&mut self, titles: impl IntoIterator<Item = impl Into<String>>) {
        self.next_titles = titles.into_iter().map(Into::into).take(5).collect();
    }

    /// Populate track-derived variables: artist, title, album, titleonly,
    /// duration (`HH:MM:SS`), durationinseconds, durationinmillseconds,
    /// length, catid, rotid, song id, and the `_enc` mirrors.
    pub fn set_track(&mut self, song: &SongData) {
        let seconds = song.duration.max(0.0) as i64;
        self.set("artist", song.artist.clone());
        self.set("title", song.title.clone());
        self.set("album", song.album.clone());
        let titleonly = song
            .title
            .split('|')
            .next()
            .map(str::to_string)
            .unwrap_or_else(|| song.title.clone());
        self.set("titleonly", titleonly);
        self.set("duration", format_hms(song.duration));
        self.set("durationinseconds", seconds.to_string());
        self.set(
            "durationinmillseconds",
            (seconds * 1000).to_string(),
        );
        self.set("length", seconds.to_string());
        self.set("song.id", song.id.to_string());
        self.set("catid", song.id_subcat.to_string());
        self.set("songtype", song.track_type.to_string());
        self.set("year", song.year.to_string());
        self.set("label", song.label.clone());
    }

    /// Populate the `now-*` clock variables (mirrors of the filebydate mask vars).
    pub fn set_now(&mut self, dt: NaiveDateTime) {
        self.set("now-year", dt.format("%Y").to_string());
        self.set("now-yy", dt.format("%y").to_string());
        self.set("now-month", dt.format("%m").to_string());
        self.set("now-monthname", dt.format("%B").to_string());
        self.set("now-days", dt.format("%d").to_string());
        self.set("now-dayname", dt.format("%A").to_string());
        self.set("now-hour", dt.format("%H").to_string());
        self.set("now-min", dt.format("%M").to_string());
        self.set("now-sec", dt.format("%S").to_string());
    }

    /// Insert a variable (alias of [`Context::set`]).
    pub fn insert(&mut self, name: &str, value: &str) {
        self.vars.insert(name.to_string(), value.to_string());
    }
}

/// The `$var$` template engine.
#[derive(Debug, Default, Clone)]
pub struct TemplateEngine;

impl TemplateEngine {
    pub fn new() -> Self {
        TemplateEngine
    }

    /// Render `template`, substituting known `$var$` references from `ctx`.
    /// Unknown references are left untouched; `_enc` variants percent-encode.
    pub fn render(&self, template: &str, ctx: &Context) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find('$') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            if let Some(end) = after.find('$') {
                let name = &after[..end];
                if let Some(value) = resolve(name, ctx) {
                    out.push_str(&value);
                    rest = &after[end + 1..];
                    continue;
                }
            }
            // Unknown (or unterminated): pass through the '$' literally.
            out.push('$');
            rest = after;
        }
        out.push_str(rest);
        out
    }
}

fn resolve(name: &str, ctx: &Context) -> Option<String> {
    if let Some(v) = ctx.get(name) {
        return Some(v.to_string());
    }
    // `_enc` suffix percent-encodes the resolved value; strip it and resolve
    // the base name (which may itself be a `nextN_title` reference).
    let (base, enc) = match name.strip_suffix("_enc") {
        Some(base) => (base, true),
        None => (name, false),
    };
    if let Some(v) = ctx.get(base) {
        return Some(if enc { percent_encode(v) } else { v.to_string() });
    }
    if let Some(rest) = base.strip_prefix("next") {
        let mut chars = rest.chars();
        let digit = chars.next()?;
        if !matches!(digit, '1'..='5') || chars.next() != Some('_') {
            return None;
        }
        let field = digit.to_digit(10)? as usize;
        let tail = &base["next1_".len()..];
        if tail != "title" {
            return None;
        }
        let idx = field - 1;
        let title = ctx.next_titles.get(idx)?;
        return Some(if enc { percent_encode(title) } else { title.clone() });
    }
    None
}

/// Format a duration in seconds as `HH:MM:SS`.
pub fn format_hms(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Percent-encode a string (UTF-8 bytes), keeping unreserved characters
/// `A-Za-z0-9-._~` plus `/` (RadioDJ keeps path separators in titles).
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Render a file-by-date mask: `{YYYY}`, `{MM}`, ... are replaced from `date`;
/// everything else is literal. `&` means "match everything" (empty suffix).
pub fn render_date_mask(mask: &str, date: NaiveDate) -> String {
    if mask == "&" {
        return String::new();
    }
    let mut out = String::new();
    let mut rest = mask;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            if let Some(piece) = date_token(&after[..close], date) {
                out.push_str(&piece);
                rest = &after[close + 1..];
                continue;
            }
        }
        out.push('{');
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Render a playlist-by-date mask (folder + mask; callers append `.m3u`).
pub fn render_playlist_mask(mask: &str, date: NaiveDate) -> String {
    render_date_mask(mask, date)
}

fn date_token(token: &str, d: NaiveDate) -> Option<String> {
    match token {
        "YYYY" => Some(d.format("%Y").to_string()),
        "YY" => Some(d.format("%y").to_string()),
        "MM" => Some(d.format("%m").to_string()),
        "M" => Some(d.format("%m").to_string().trim_start_matches('0').to_string()),
        "MONTHNAME" => Some(d.format("%B").to_string()),
        "MONTHNAMESHORT" => Some(d.format("%b").to_string()),
        "DD" => Some(d.format("%d").to_string()),
        "D" => Some(d.format("%d").to_string().trim_start_matches('0').to_string()),
        "DAYNAME" => Some(d.format("%A").to_string()),
        "DAYNAMESHORT" => Some(d.format("%a").to_string()),
        _ => None, // unknown token: literal braces restored by caller
    }
}

/// Local-time helper for callers (pure wrapper around `chrono::Local`).
pub fn local_now() -> NaiveDateTime {
    Local::now().naive_local()
}

/// Resolve the effective local time for a song row, mirroring the
/// `IFNULL(date_played, <never-played sentinel>)` fallback.
pub fn effective_play_time(
    date_played: Option<NaiveDateTime>,
    never_played: NaiveDateTime,
) -> NaiveDateTime {
    date_played.unwrap_or(never_played)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackType;
    use chrono::{FixedOffset, TimeZone};

    fn song() -> SongData {
        SongData {
            artist: "Cher".to_string(),
            title: "Believe".to_string(),
            album: "Believe Hits".to_string(),
            duration: 218.0, // 00:03:38
            track_type: TrackType::Music as i32,
            ..Default::default()
        }
    }

    fn ctx_with_track() -> Context {
        let mut ctx = Context::new();
        ctx.set_track(&song());
        ctx.set("station_name", "My Station");
        ctx
    }

    #[test]
    fn unknown_vars_pass_through() {
        let mut ctx = Context::new();
        ctx.set("station_name", "My Station");
        let engine = TemplateEngine::new();
        let out = engine.render("Hello $station_name$, $does_not_exist$!", &ctx);
        assert_eq!(out, "Hello My Station, $does_not_exist$!");
    }

    #[test]
    fn case_sensitive() {
        let ctx = ctx_with_track();
        let engine = TemplateEngine::new();
        // Wrong case is not a match.
        assert_eq!(engine.render("$Title$", &ctx), "$Title$");
        assert_eq!(engine.render("$artist$", &ctx), "Cher");
    }

    #[test]
    fn enc_encoding() {
        let mut ctx = Context::new();
        ctx.set("artist", "AC/DC");
        ctx.set("title", "Citt? & Str?gg");
        let engine = TemplateEngine::new();
        assert_eq!(engine.render("$artist_enc$", &ctx), "AC/DC");
        assert_eq!(
            engine.render("$title_enc$", &ctx),
            "Citt%3F%20%26%20Str%3Fgg"
        );
        // non-ascii through UTF-8
        ctx.set("title2", "\u{00C4}");
        assert_eq!(engine.render("$title2_enc$", &ctx), "%C3%84");
    }

    #[test]
    fn next3_title() {
        let mut ctx = Context::new();
        ctx.set_next_tracks(["One", "Two", "Three"]);
        let engine = TemplateEngine::new();
        assert_eq!(
            engine.render("$next1_title$|$next2_title$|$next3_title$", &ctx),
            "One|Two|Three"
        );
        // beyond 5 -> passthrough
        ctx.set_next_tracks(["a", "b", "c", "d", "e"]);
        assert_eq!(engine.render("$next6_title$", &ctx), "$next6_title$");
        // slots present up to the five seeded tracks
        assert_eq!(engine.render("$next3_title$", &ctx), "c");
        assert_eq!(engine.render("$next5_title$", &ctx), "e");
        // missing slots beyond the seeded list -> passthrough
        ctx.set_next_tracks(["a", "b"]);
        assert_eq!(engine.render("$next4_title$", &ctx), "$next4_title$");
        // _enc on next
        let mut ctx2 = Context::new();
        ctx2.set_next_tracks(["A & B"]);
        assert_eq!(engine.render("$next1_title_enc$", &ctx2), "A%20%26%20B");
    }

    #[test]
    fn duration_formats() {
        let engine = TemplateEngine::new();
        let ctx = ctx_with_track();
        assert_eq!(engine.render("$duration$", &ctx), "00:03:38");
        assert_eq!(engine.render("$durationinseconds$", &ctx), "218");
        assert_eq!(engine.render("$durationinmillseconds$", &ctx), "218000");
        assert_eq!(format_hms(218.0), "00:03:38");
        assert_eq!(format_hms(3661.0), "01:01:01");
        assert_eq!(format_hms(0.0), "00:00:00");
    }

    #[test]
    fn track_variables() {
        let engine = TemplateEngine::new();
        let ctx = ctx_with_track();
        assert_eq!(engine.render("$artist$ - $title$", &ctx), "Cher - Believe");
        assert_eq!(engine.render("$titleonly$", &ctx), "Believe");
    }

    #[test]
    fn date_mask_expansion() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        assert_eq!(render_date_mask("{YYYY}-{MM}-{DD}", d), "2026-10-29");
        assert_eq!(
            render_date_mask("{MONTHNAME}_{D}_{DAYNAMESHORT}", d),
            "October_29_Thu"
        );
        assert_eq!(render_date_mask("&", d), "");
        assert_eq!(render_date_mask("{unknown}", d), "{unknown}");
    }

    #[test]
    fn now_vars() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 10, 29)
            .unwrap()
            .and_hms_opt(13, 45, 9)
            .unwrap();
        let mut ctx = Context::new();
        ctx.set_now(now);
        let engine = TemplateEngine::new();
        assert_eq!(
            engine.render(
                "$now-year$-$now-month$-$now-days$ $now-hour$:$now-min$:$now-sec$",
                &ctx
            ),
            "2026-10-29 13:45:09"
        );
        assert_eq!(
            engine.render("$now-dayname$ $now-monthname$", &ctx),
            "Thursday October"
        );
    }

    #[test]
    fn clock_format() {
        let dt = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 10, 29, 13, 45, 0)
            .unwrap()
            .naive_local();
        let mut ctx = Context::new();
        ctx.set_now(dt);
        let engine = TemplateEngine::new();
        assert_eq!(engine.render("$now-hour$:$now-min$", &ctx), "13:45");
    }
}
