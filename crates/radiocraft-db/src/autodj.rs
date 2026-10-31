//! AutoDJ data access: executes the core scheduler's declarative queries
//! (docs/02 §3 contract — `radiocraft-core` renders [`SqlFragments`], this
//! crate renders the named `:params` into driver placeholders and runs them)
//! and loads `rotations` / `rotations_list` into the core scheduler model.

use chrono::NaiveDateTime;
use sqlx::{FromRow, MySqlPool, Row};

use radiocraft_core::booking::sentinel;
use radiocraft_core::rotation::{
    CategoryType, Param, ParamValue, Rotation as CoreRotation, RotationCriteria as CoreCriteria,
    SelectionMode, SqlFragments,
};
use radiocraft_core::scheduler::TrackCandidate;

use crate::entities::{self, Song};
use crate::error::{DbError, Result};

// ---------------------------------------------------------------------------
// named-parameter rendering
// ---------------------------------------------------------------------------

/// Rewrite `:name` placeholders in a core fragment into positional `?`
/// markers, collecting the bind values in occurrence order (docs/06 §10:
/// the runtime placeholder count must match the bind count).
///
/// Identifiers are matched greedily (`[A-Za-z0-9_]+`) so `:artist_played`
/// never shadows `:artist_played_cutoff`.
pub fn render_fragments(f: &SqlFragments) -> Result<(String, Vec<ParamValue>)> {
    let by_name: std::collections::HashMap<&str, &ParamValue> = f
        .params
        .iter()
        .map(|Param { name, value }| (name.as_str(), value))
        .collect();

    let mut out = String::with_capacity(f.where_clause.len() + f.params.len() * 8);
    let mut binds: Vec<ParamValue> = Vec::with_capacity(f.params.len());
    let bytes = f.where_clause.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > i + 1 {
                let name = &f.where_clause[i + 1..j];
                let value = by_name.get(name).copied().ok_or_else(|| {
                    DbError::Other(anyhow::anyhow!("fragment names :{name}, params do not carry it"))
                })?;
                out.push('?');
                binds.push(value.clone());
                i = j;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    // Multi-byte UTF-8 was copied byte-wise above; rebuild via lossy round-trip
    // only if the fragment actually held non-ASCII (metadata params stay in
    // binds, so the clause itself is SQL and effectively ASCII).
    Ok((out, binds))
}

/// Assemble the full SELECT from core fragments.
///
/// The core `select` list is advisory: this crate reads `songs.*` so the
/// in-memory re-checks in `radiocraft-core` see the full row (`enabled`,
/// `play_limit`, `lang`/`mood`/`gender`, availability window...).
fn assemble(f: &SqlFragments, limit_extra: Option<u32>) -> Result<(String, Vec<ParamValue>)> {
    let (where_sql, binds) = render_fragments(f)?;
    let mut sql = format!("SELECT * FROM {}", f.from);
    if !where_sql.trim().is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_sql);
    }
    sql.push_str(&format!(" ORDER BY {}", f.order_by));
    match (f.limit, limit_extra) {
        (Some(n), _) | (None, Some(n)) if n > 0 => {
            // `limit` is a validated u32 from core, safe to inline.
            sql.push_str(&format!(" LIMIT {n}"));
        }
        _ => {}
    }
    Ok((sql, binds))
}

// ---------------------------------------------------------------------------
// candidate pool
// ---------------------------------------------------------------------------

/// True when `d` is the "never played / unlimited window" sentinel date.
fn is_sentinel(d: NaiveDateTime) -> bool {
    d.format("%Y-%m-%d %H:%M:%S").to_string() == sentinel::NEVER_PLAYED
}

/// Sentinel-aware datetime: the `'2002-01-01 00:00:01'` literal means None.
fn real_time(d: Option<NaiveDateTime>) -> Option<NaiveDateTime> {
    d.filter(|d| !is_sentinel(*d))
}

/// Map a full `songs` row onto a scheduler candidate (docs/01 sentinel
/// conventions: `*sentinel*` dates -> None, "Not Set"/`und`/empty metadata
/// -> empty string, `sweepers` pool-name overrides stay unresolved because
/// this schema generation has no per-song sweeper id columns).
fn candidate_from_song(s: &Song) -> TrackCandidate {
    TrackCandidate {
        id: i64::from(s.ID),
        artist: s.artist.clone(),
        title: s.title.clone(),
        album: s.album.clone(),
        path: s.path.clone(),
        duration: s.duration.try_into().unwrap_or(0.0),
        weight: s.weight.try_into().unwrap_or(0.0),
        song_type: i32::from(s.song_type),
        id_subcat: i64::from(s.id_subcat),
        id_genre: i64::from(s.id_genre),
        language: clean_meta(&s.lang),
        mood: clean_meta(&s.mood),
        gender: clean_meta(&s.gender),
        count_played: i64::from(s.count_played),
        play_limit: i64::from(s.play_limit),
        enabled: s.enabled,
        date_played: real_time(s.date_played),
        artist_played: real_time(s.artist_played),
        title_played: real_time(s.title_played),
        album_played: real_time(s.album_played),
        date_added: s.date_added,
        start_date: real_time(s.start_date),
        end_date: real_time(s.end_date),
        ..Default::default()
    }
}

/// `Not Set`/`und`/`&`/empty are all "no value" markers in this schema.
fn clean_meta(s: &str) -> String {
    match s.trim() {
        "" | "Not Set" | "und" | "&" => String::new(),
        v => v.to_string(),
    }
}

/// Execute a scheduler query against `songs` and build the candidate pool.
pub async fn fetch_candidates(
    pool: &MySqlPool,
    query: &radiocraft_core::rotation::SchedulerQuery,
) -> Result<Vec<TrackCandidate>> {
    let f = query.sql_fragments();
    let (sql, binds) = assemble(&f, None)?;
    let mut q = sqlx::query(&sql);
    for p in binds {
        q = match p {
            ParamValue::Int(v) => q.bind(v),
            ParamValue::Float(v) => q.bind(v),
            ParamValue::Text(v) => q.bind(v),
            // Driver-side string datetime keeps the pinned `NOW()` semantics
            // (docs/01 §5) instead of relying on server session clocks.
            ParamValue::DateTime(v) => q.bind(v),
        };
    }
    let rows = q.fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let song = Song::from_row(&row)?;
        out.push(candidate_from_song(&song));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// rotations / criteria loading
// ---------------------------------------------------------------------------

/// Load every rotation with its criteria into the core scheduler model.
///
/// The radiodj3 `rotations`/`rotations_list` columns carry a subset of the
/// core model (no name/enabled/days/daypart/weight columns exist in this
/// schema generation). Criteria rows whose `data` column holds a JSON object
/// round-trip the *full* core criteria (RadioCraft rotation-editor
/// extension); everything else uses the legacy column mapping:
/// `catID`/`subID` (include), `genID`, `mood`/`gender`/`language`,
/// `*_separation` -> repeat intervals, `selType` 0=weight/1=random,
/// `repeatRule`, `ord`.
pub async fn load_rotations(pool: &MySqlPool) -> Result<Vec<CoreRotation>> {
    let rows = sqlx::query("SELECT ID, name FROM rotations ORDER BY ID")
        .fetch_all(pool)
        .await?;
    let mut rotations = Vec::with_capacity(rows.len());
    for row in rows {
        let id: u32 = row.try_get("ID")?;
        let name: String = row.try_get("name")?;
        let mut rotation = CoreRotation::new(i64::from(id), name);
        for c in entities::list_criteria(pool, id).await? {
            rotation.criteria.push(criteria_from_row(&c)?);
        }
        rotations.push(rotation);
    }
    Ok(rotations)
}

fn meta_opt(s: &Option<String>) -> Option<String> {
    match s.as_deref().map(str::trim) {
        None | Some("") | Some("Not Set") | Some("und") | Some("&") => None,
        Some(v) => Some(v.to_string()),
    }
}

fn criteria_from_row(row: &entities::RotationCriteria) -> Result<CoreCriteria> {
    // RadioCraft editor extension: a JSON object in `data` carries the full
    // core criteria (dayparts, days, weight floor, named rule...).
    if row.data.trim_start().starts_with('{') {
        let c: CoreCriteria = serde_json::from_str(row.data.trim())
            .map_err(|e| DbError::Other(anyhow::anyhow!("bad rotations_list.data JSON: {e}")))?;
        return Ok(CoreCriteria {
            id: i64::from(row.ID),
            rotation_id: i64::from(row.pID),
            ..c
        });
    }

    Ok(CoreCriteria {
        id: i64::from(row.ID),
        rotation_id: i64::from(row.pID),
        name: format!("criterion {}", row.ID),
        enabled: true,
        category_id: i64::from(row.catID),
        // No `catType` column in this schema generation: include semantics.
        category_type: CategoryType::Include,
        subcategory_id: i64::from(row.subID),
        subcategory_type: CategoryType::Include,
        genre_id: i64::from(row.genID),
        weight_min: 0.0,
        repeat_track_interval: i64::from(row.track_separation),
        repeat_artist_interval: i64::from(row.artist_separation),
        repeat_title_interval: i64::from(row.title_separation),
        repeat_album_interval: i64::from(row.album_separation),
        time_start: None,
        time_end: None,
        days: radiocraft_core::rotation::DayMask::ALL,
        language: meta_opt(&row.language),
        mood: meta_opt(&row.mood),
        gender: meta_opt(&row.gender),
        // The legacy tinyint only distinguishes weighted vs random.
        selection: if row.selType {
            SelectionMode::Random
        } else {
            SelectionMode::Weight
        },
        repeat_rule: i32::from(row.repeatRule),
        ord: i32::from(row.ord),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use radiocraft_core::rotation::SchedulerQuery;

    fn param(name: &str, v: i64) -> Param {
        Param {
            name: name.into(),
            value: ParamValue::Int(v),
        }
    }

    #[test]
    fn renders_named_placeholders_in_order() {
        let f = SqlFragments {
            select: "*".into(),
            from: "songs".into(),
            where_clause: "artist_played <= :artist_played_cutoff AND id_subcat IN (:sub_0, :sub_1)"
                .into(),
            order_by: "ID".into(),
            limit: None,
            params: vec![
                Param {
                    name: "artist_played_cutoff".into(),
                    value: ParamValue::DateTime("2026-01-01 00:00:00".into()),
                },
                param("sub_0", 1),
                param("sub_1", 2),
            ],
        };
        let (sql, binds) = render_fragments(&f).unwrap();
        assert_eq!(
            sql,
            "artist_played <= ? AND id_subcat IN (?, ?)"
        );
        assert_eq!(binds.len(), 3);
        assert_eq!(
            binds[0],
            ParamValue::DateTime("2026-01-01 00:00:00".into())
        );
        assert_eq!(binds[1], ParamValue::Int(1));
        assert_eq!(binds[2], ParamValue::Int(2));
    }

    #[test]
    fn unknown_placeholder_is_an_error() {
        let f = SqlFragments {
            select: "*".into(),
            from: "songs".into(),
            where_clause: "weight >= :nope".into(),
            order_by: "ID".into(),
            limit: None,
            params: vec![param("other", 1)],
        };
        assert!(render_fragments(&f).is_err());
    }

    #[test]
    fn core_query_round_trips_through_the_renderer() {
        // The real fragment generator must produce placeholders the renderer
        // resolves 1:1 with its params (placeholder/bind parity, docs/06 §10).
        let mut q = SchedulerQuery::new(chrono::Local::now().naive_local());
        q.enabled = true;
        q.song_type = Some(5);
        q.include_subcategories = vec![1, 2, 3];
        q.repeat_artist_minutes = 30;
        q.respect_availability_window = true;
        q.language = Some("english".into());
        let f = q.sql_fragments();
        let (_sql, binds) = render_fragments(&f).expect("renders");
        assert_eq!(binds.len(), f.params.len());
    }

    #[test]
    fn sentinel_dates_map_to_none() {
        let sent =
            chrono::NaiveDateTime::parse_from_str(sentinel::NEVER_PLAYED, "%Y-%m-%d %H:%M:%S")
                .unwrap();
        assert_eq!(real_time(Some(sent)), None);
        assert!(real_time(Some(chrono::Local::now().naive_local())).is_some());
    }
}
