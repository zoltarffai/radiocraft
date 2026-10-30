//! Entity models (sqlx `FromRow`) and CRUD helpers for every radiodj3 table.
//!
//! Naming/typality follows the schema exactly: `int(10) unsigned` → `u32`,
//! signed `int` → `i32` (they carry the `-1`/`0` sentinels), `tinyint(1)` →
//! `bool` except `songs.limit_action` which is `i8` because negative values
//! encode "relocate to subcategory N", decimals → `rust_decimal`, `year` is a
//! `varchar(4)` string, and RadioDJ sentinel defaults (`'2002-01-01 …'`,
//! `'Not Set'`, `'und'`, `'&'`, `'http://'`, `'no_image.jpg'`) are preserved
//! as data rather than migrated to NULL.

use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::MySqlPool;

use crate::error::Result;

/// Re-export so callers of the booking transaction can name the history row.
pub use crate::booking::BookingResult;

/// Build `?,?,?` for `IN (…)` clauses.
fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

// ---------------------------------------------------------------------------
// songs
// ---------------------------------------------------------------------------

/// Full `songs` row.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Song {
    pub ID: u32,
    pub path: String,
    pub enabled: bool,
    pub date_added: Option<NaiveDateTime>,
    pub date_modified: Option<NaiveDateTime>,
    pub date_played: Option<NaiveDateTime>,
    pub artist_played: Option<NaiveDateTime>,
    pub album_played: Option<NaiveDateTime>,
    pub title_played: Option<NaiveDateTime>,
    pub count_played: i32,
    pub play_limit: i32,
    /// 0=none, 1=disable, 2=delete, negative id_subcat=relocate.
    pub limit_action: i8,
    pub start_date: Option<NaiveDateTime>,
    pub end_date: Option<NaiveDateTime>,
    pub startEvent: i32,
    pub endEvent: i32,
    pub song_type: i8,
    pub id_subcat: u32,
    pub id_genre: u32,
    pub weight: Decimal,
    pub duration: Decimal,
    pub original_duration: Decimal,
    pub cue_times: String,
    pub precise_cue: bool,
    pub fade_type: i8,
    pub start_type: i8,
    pub end_type: i8,
    pub mix_type: i8,
    pub mood: String,
    pub gender: String,
    pub lang: String,
    pub rating: i32,
    pub loudness: Decimal,
    pub bs_loudness: Decimal,
    pub bs1770: Decimal,
    pub overlay: bool,
    pub artist: String,
    pub associated_artists: String,
    pub original_artist: String,
    pub title: String,
    pub alternate_title: String,
    pub album: String,
    pub composer: String,
    pub label: String,
    pub year: String,
    pub track_no: i32,
    pub disc_no: i32,
    pub publisher: String,
    pub copyright: String,
    pub isrc: String,
    pub bpm: Decimal,
    pub comments: Option<String>,
    pub sweepers: Option<String>,
    pub image: String,
    pub buy_link: String,
    pub url1: String,
    pub url2: String,
    pub tdate_played: Option<NaiveDateTime>,
    pub tartist_played: Option<NaiveDateTime>,
    pub ttitle_played: Option<NaiveDateTime>,
    pub talbum_played: Option<NaiveDateTime>,
    pub originalmetadata: bool,
}

/// Columns needed to insert a song; everything else keeps its DDL default.
#[derive(Debug, Clone, Default)]
pub struct NewSong {
    pub path: String,
    pub song_type: i8,
    pub id_subcat: u32,
    pub id_genre: u32,
    pub artist: String,
    pub original_artist: String,
    pub title: String,
    pub album: String,
    pub duration: Decimal,
    pub original_duration: Decimal,
    pub enabled: bool,
    pub weight: Decimal,
}

pub const SQL_SONG_INSERT: &str = "INSERT INTO songs (path, enabled, date_added, date_modified, \
     song_type, id_subcat, id_genre, weight, duration, original_duration, artist, original_artist, \
     title, album) VALUES (?, ?, NOW(), NOW(), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

/// Insert a song and return its new `ID`.
pub async fn insert_song(pool: &MySqlPool, s: &NewSong) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query(SQL_SONG_INSERT)
        .bind(&s.path)
        .bind(s.enabled)
        .bind(s.song_type)
        .bind(s.id_subcat)
        .bind(s.id_genre)
        .bind(s.weight)
        .bind(s.duration)
        .bind(s.original_duration)
        .bind(&s.artist)
        .bind(&s.original_artist)
        .bind(&s.title)
        .bind(&s.album)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()")
        .fetch_one(&mut *conn)
        .await?;
    Ok(id as u32)
}

pub async fn song_by_id(pool: &MySqlPool, id: u32) -> Result<Option<Song>> {
    Ok(sqlx::query_as::<_, Song>("SELECT * FROM songs WHERE ID = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn song_by_path(pool: &MySqlPool, path: &str) -> Result<Option<Song>> {
    Ok(sqlx::query_as::<_, Song>("SELECT * FROM songs WHERE path = ?")
        .bind(path)
        .fetch_optional(pool)
        .await?)
}

pub async fn list_songs(pool: &MySqlPool, limit: i64, offset: i64) -> Result<Vec<Song>> {
    Ok(sqlx::query_as::<_, Song>("SELECT * FROM songs ORDER BY ID LIMIT ? OFFSET ?")
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?)
}

pub async fn count_songs(pool: &MySqlPool) -> Result<u64> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM songs").fetch_one(pool).await?;
    Ok(n as u64)
}

/// FULLTEXT search over artist/title/album/composer (`ft_search`).
pub async fn search_songs(pool: &MySqlPool, needle: &str, limit: i64) -> Result<Vec<Song>> {
    Ok(sqlx::query_as::<_, Song>(
        "SELECT * FROM songs WHERE MATCH (artist, title, album, composer) AGAINST (? IN BOOLEAN MODE) \
         ORDER BY artist, title LIMIT ?",
    )
    .bind(needle)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Update every editable metadata column (booking timestamps are untouched).
pub async fn update_song(pool: &MySqlPool, s: &Song) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE songs SET path=?, enabled=?, start_date=?, end_date=?, startEvent=?, endEvent=?, \
         song_type=?, id_subcat=?, id_genre=?, weight=?, duration=?, original_duration=?, \
         cue_times=?, precise_cue=?, fade_type=?, start_type=?, end_type=?, mix_type=?, \
         mood=?, gender=?, lang=?, rating=?, loudness=?, bs_loudness=?, bs1770=?, overlay=?, \
         artist=?, associated_artists=?, original_artist=?, title=?, alternate_title=?, album=?, \
         composer=?, label=?, `year`=?, track_no=?, disc_no=?, publisher=?, copyright=?, isrc=?, \
         bpm=?, comments=?, sweepers=?, image=?, buy_link=?, url1=?, url2=?, play_limit=?, \
         limit_action=?, date_modified=NOW() WHERE ID=?",
    )
    .bind(&s.path)
    .bind(s.enabled)
    .bind(s.start_date)
    .bind(s.end_date)
    .bind(s.startEvent)
    .bind(s.endEvent)
    .bind(s.song_type)
    .bind(s.id_subcat)
    .bind(s.id_genre)
    .bind(s.weight)
    .bind(s.duration)
    .bind(s.original_duration)
    .bind(&s.cue_times)
    .bind(s.precise_cue)
    .bind(s.fade_type)
    .bind(s.start_type)
    .bind(s.end_type)
    .bind(s.mix_type)
    .bind(&s.mood)
    .bind(&s.gender)
    .bind(&s.lang)
    .bind(s.rating)
    .bind(s.loudness)
    .bind(s.bs_loudness)
    .bind(s.bs1770)
    .bind(s.overlay)
    .bind(&s.artist)
    .bind(&s.associated_artists)
    .bind(&s.original_artist)
    .bind(&s.title)
    .bind(&s.alternate_title)
    .bind(&s.album)
    .bind(&s.composer)
    .bind(&s.label)
    .bind(&s.year)
    .bind(s.track_no)
    .bind(s.disc_no)
    .bind(&s.publisher)
    .bind(&s.copyright)
    .bind(&s.isrc)
    .bind(s.bpm)
    .bind(&s.comments)
    .bind(&s.sweepers)
    .bind(&s.image)
    .bind(&s.buy_link)
    .bind(&s.url1)
    .bind(&s.url2)
    .bind(s.play_limit)
    .bind(s.limit_action)
    .bind(s.ID)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

pub async fn delete_song(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM songs WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

// --- mass operations -------------------------------------------------------

async fn mass_set_u32(
    pool: &MySqlPool,
    column: &str,
    value: u32,
    ids: &[u32],
) -> Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let sql = format!(
        "UPDATE songs SET {column} = ?, date_modified = NOW() WHERE ID IN ({})",
        placeholders(ids.len())
    );
    let mut q = sqlx::query(&sql).bind(value);
    for id in ids {
        q = q.bind(*id);
    }
    Ok(q.execute(pool).await?.rows_affected())
}

pub async fn mass_set_enabled(pool: &MySqlPool, ids: &[u32], enabled: bool) -> Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let sql = format!(
        "UPDATE songs SET enabled = ?, date_modified = NOW() WHERE ID IN ({})",
        placeholders(ids.len())
    );
    let mut q = sqlx::query(&sql).bind(enabled);
    for id in ids {
        q = q.bind(*id);
    }
    Ok(q.execute(pool).await?.rows_affected())
}

pub async fn mass_set_subcategory(pool: &MySqlPool, ids: &[u32], id_subcat: u32) -> Result<u64> {
    mass_set_u32(pool, "id_subcat", id_subcat, ids).await
}

pub async fn mass_set_genre(pool: &MySqlPool, ids: &[u32], id_genre: u32) -> Result<u64> {
    mass_set_u32(pool, "id_genre", id_genre, ids).await
}

pub async fn mass_set_weight(pool: &MySqlPool, ids: &[u32], weight: Decimal) -> Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let sql = format!(
        "UPDATE songs SET weight = ?, date_modified = NOW() WHERE ID IN ({})",
        placeholders(ids.len())
    );
    let mut q = sqlx::query(&sql).bind(weight);
    for id in ids {
        q = q.bind(*id);
    }
    Ok(q.execute(pool).await?.rows_affected())
}

/// `play_limit` 0 = unlimited; `limit_action` 0/1/2 or negative subcategory.
pub async fn mass_set_play_limit(
    pool: &MySqlPool,
    ids: &[u32],
    play_limit: i32,
    limit_action: i8,
) -> Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let sql = format!(
        "UPDATE songs SET play_limit = ?, limit_action = ?, date_modified = NOW() WHERE ID IN ({})",
        placeholders(ids.len())
    );
    let mut q = sqlx::query(&sql).bind(play_limit).bind(limit_action);
    for id in ids {
        q = q.bind(*id);
    }
    Ok(q.execute(pool).await?.rows_affected())
}

pub async fn mass_delete(pool: &MySqlPool, ids: &[u32]) -> Result<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let sql = format!("DELETE FROM songs WHERE ID IN ({})", placeholders(ids.len()));
    let mut q = sqlx::query(&sql);
    for id in ids {
        q = q.bind(*id);
    }
    Ok(q.execute(pool).await?.rows_affected())
}

// ---------------------------------------------------------------------------
// category / subcategory / genre
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Category {
    pub ID: u32,
    pub name: String,
}

pub async fn list_categories(pool: &MySqlPool) -> Result<Vec<Category>> {
    Ok(sqlx::query_as::<_, Category>("SELECT ID, name FROM category ORDER BY name")
        .fetch_all(pool)
        .await?)
}

pub async fn insert_category(pool: &MySqlPool, name: &str) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query("INSERT INTO category (name) VALUES (?)")
        .bind(name)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn rename_category(pool: &MySqlPool, id: u32, name: &str) -> Result<u64> {
    Ok(sqlx::query("UPDATE category SET name = ? WHERE ID = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn delete_category(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM category WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Subcategory {
    pub ID: u32,
    pub parentid: u32,
    pub name: String,
    pub sweeper_subID: i32,
}

pub async fn list_subcategories(pool: &MySqlPool, parentid: Option<u32>) -> Result<Vec<Subcategory>> {
    let rows = match parentid {
        Some(pid) => {
            sqlx::query_as::<_, Subcategory>(
                "SELECT ID, parentid, name, sweeper_subID FROM subcategory WHERE parentid = ? ORDER BY name",
            )
            .bind(pid)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, Subcategory>(
                "SELECT ID, parentid, name, sweeper_subID FROM subcategory ORDER BY parentid, name",
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

pub async fn insert_subcategory(
    pool: &MySqlPool,
    parentid: u32,
    name: &str,
    sweeper_subID: i32,
) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query("INSERT INTO subcategory (parentid, name, sweeper_subID) VALUES (?, ?, ?)")
        .bind(parentid)
        .bind(name)
        .bind(sweeper_subID)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn rename_subcategory(pool: &MySqlPool, id: u32, name: &str) -> Result<u64> {
    Ok(sqlx::query("UPDATE subcategory SET name = ? WHERE ID = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn delete_subcategory(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM subcategory WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Genre {
    pub id: u32,
    pub name: String,
}

pub async fn list_genres(pool: &MySqlPool) -> Result<Vec<Genre>> {
    Ok(sqlx::query_as::<_, Genre>("SELECT id, name FROM genre ORDER BY name")
        .fetch_all(pool)
        .await?)
}

pub async fn insert_genre(pool: &MySqlPool, name: &str) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query("INSERT INTO genre (name) VALUES (?)")
        .bind(name)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn rename_genre(pool: &MySqlPool, id: u32, name: &str) -> Result<u64> {
    Ok(sqlx::query("UPDATE genre SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn delete_genre(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM genre WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

// ---------------------------------------------------------------------------
// rotations + criteria (rotations / rotations_list)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Rotation {
    pub ID: u32,
    pub name: String,
}

pub async fn list_rotations(pool: &MySqlPool) -> Result<Vec<Rotation>> {
    Ok(sqlx::query_as::<_, Rotation>("SELECT ID, name FROM rotations ORDER BY name")
        .fetch_all(pool)
        .await?)
}

pub async fn insert_rotation(pool: &MySqlPool, name: &str) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query("INSERT INTO rotations (name) VALUES (?)")
        .bind(name)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn rename_rotation(pool: &MySqlPool, id: u32, name: &str) -> Result<u64> {
    Ok(sqlx::query("UPDATE rotations SET name = ? WHERE ID = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn delete_rotation(pool: &MySqlPool, id: u32) -> Result<u64> {
    let mut conn = pool.acquire().await?;
    let rotation_rows = sqlx::query("DELETE FROM rotations WHERE ID = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    // RadioDJ deletes the rotation's criteria rows as a separate statement.
    let criteria_rows = sqlx::query("DELETE FROM rotations_list WHERE pID = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    let _ = (rotation_rows, criteria_rows);
    Ok(rotation_rows)
}

/// One AutoDJ criterion line (`rotations_list`). The `-1`/`0` "any" sentinels
/// live in the signed `catID`/`subID`/`genID` columns.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RotationCriteria {
    pub ID: u32,
    pub pID: u32,
    pub catID: i32,
    pub subID: i32,
    pub genID: i32,
    pub mood: Option<String>,
    pub gender: Option<String>,
    pub language: Option<String>,
    pub start_type: i32,
    pub end_type: i32,
    pub selType: bool,
    pub sweeper: bool,
    pub repeatRule: bool,
    pub ord: i8,
    pub data: String,
    pub track_separation: i32,
    pub artist_separation: i32,
    pub title_separation: i32,
    pub album_separation: i32,
    pub advanced: Option<String>,
}

pub async fn list_criteria(pool: &MySqlPool, rotation_id: u32) -> Result<Vec<RotationCriteria>> {
    Ok(sqlx::query_as::<_, RotationCriteria>(
        "SELECT * FROM rotations_list WHERE pID = ? ORDER BY ord",
    )
    .bind(rotation_id)
    .fetch_all(pool)
    .await?)
}

pub async fn insert_criteria(pool: &MySqlPool, c: &RotationCriteria) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query(
        "INSERT INTO rotations_list (pID, catID, subID, genID, mood, gender, language, \
         start_type, end_type, selType, sweeper, repeatRule, ord, data, \
         track_separation, artist_separation, title_separation, album_separation, advanced) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(c.pID)
    .bind(c.catID)
    .bind(c.subID)
    .bind(c.genID)
    .bind(&c.mood)
    .bind(&c.gender)
    .bind(&c.language)
    .bind(c.start_type)
    .bind(c.end_type)
    .bind(c.selType)
    .bind(c.sweeper)
    .bind(c.repeatRule)
    .bind(c.ord)
    .bind(&c.data)
    .bind(c.track_separation)
    .bind(c.artist_separation)
    .bind(c.title_separation)
    .bind(c.album_separation)
    .bind(&c.advanced)
    .execute(&mut *conn)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn update_criteria(pool: &MySqlPool, c: &RotationCriteria) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE rotations_list SET catID=?, subID=?, genID=?, mood=?, gender=?, language=?, \
         start_type=?, end_type=?, selType=?, sweeper=?, repeatRule=?, ord=?, data=?, \
         track_separation=?, artist_separation=?, title_separation=?, album_separation=?, advanced=? \
         WHERE ID=?",
    )
    .bind(c.catID)
    .bind(c.subID)
    .bind(c.genID)
    .bind(&c.mood)
    .bind(&c.gender)
    .bind(&c.language)
    .bind(c.start_type)
    .bind(c.end_type)
    .bind(c.selType)
    .bind(c.sweeper)
    .bind(c.repeatRule)
    .bind(c.ord)
    .bind(&c.data)
    .bind(c.track_separation)
    .bind(c.artist_separation)
    .bind(c.title_separation)
    .bind(c.album_separation)
    .bind(&c.advanced)
    .bind(c.ID)
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn delete_criteria(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM rotations_list WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

// ---------------------------------------------------------------------------
// playlists + playlist items (playlists / playlists_list)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlaylistHeader {
    pub ID: u32,
    pub name: String,
}

pub async fn list_playlists(pool: &MySqlPool) -> Result<Vec<PlaylistHeader>> {
    Ok(sqlx::query_as::<_, PlaylistHeader>("SELECT ID, name FROM playlists ORDER BY name")
        .fetch_all(pool)
        .await?)
}

pub async fn insert_playlist(pool: &MySqlPool, name: &str) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query("INSERT INTO playlists (name) VALUES (?)")
        .bind(name)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn rename_playlist(pool: &MySqlPool, id: u32, name: &str) -> Result<u64> {
    Ok(sqlx::query("UPDATE playlists SET name = ? WHERE ID = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

/// Delete the playlist header and its items (RadioDJ issues both deletes).
pub async fn delete_playlist(pool: &MySqlPool, id: u32) -> Result<u64> {
    let mut conn = pool.acquire().await?;
    sqlx::query("DELETE FROM playlists WHERE ID = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let res = sqlx::query("DELETE FROM playlists_list WHERE pID = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(res.rows_affected())
}

/// One `playlists_list` row. NOTE: `swplay`/`vtplay` are lowercase here while
/// `queuelist` uses `swPlay`/`vtPlay` — the schema mixes casing on purpose.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlaylistItem {
    pub ID: u32,
    pub pID: u32,
    /// signed: song ID, but carries the 0/sentinel semantics.
    pub sID: i32,
    pub cstart: Decimal,
    pub cnext: Decimal,
    pub cend: Decimal,
    pub fin: Decimal,
    pub fout: Decimal,
    pub swID: u32,
    pub swplay: Decimal,
    pub vtID: u32,
    pub vtplay: Decimal,
    pub swfirst: bool,
    pub ord: u32,
}

#[derive(Debug, Clone, Default)]
pub struct NewPlaylistItem {
    pub pID: u32,
    pub sID: i32,
    pub cstart: Decimal,
    pub cnext: Decimal,
    pub cend: Decimal,
    pub fin: Decimal,
    pub fout: Decimal,
    pub swID: u32,
    pub swplay: Decimal,
    pub vtID: u32,
    pub vtplay: Decimal,
    pub swfirst: bool,
    pub ord: u32,
}

pub async fn list_playlist_items(pool: &MySqlPool, playlist_id: u32) -> Result<Vec<PlaylistItem>> {
    Ok(sqlx::query_as::<_, PlaylistItem>(
        "SELECT * FROM playlists_list WHERE pID = ? ORDER BY ord",
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await?)
}

/// A `playlists_list` row plus the library song columns (artist/title/path)
/// when `sID` points at a real song; negative/sentinel sIDs join to NULL.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlaylistItemRow {
    pub ID: u32,
    pub pID: u32,
    pub sID: i32,
    pub cstart: Decimal,
    pub cnext: Decimal,
    pub cend: Decimal,
    pub fin: Decimal,
    pub fout: Decimal,
    pub swID: u32,
    pub swplay: Decimal,
    pub vtID: u32,
    pub vtplay: Decimal,
    pub swfirst: bool,
    pub ord: u32,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub path: Option<String>,
}

pub async fn list_playlist_items_with_songs(
    pool: &MySqlPool,
    playlist_id: u32,
) -> Result<Vec<PlaylistItemRow>> {
    Ok(sqlx::query_as::<_, PlaylistItemRow>(
        "SELECT pl.ID, pl.pID, pl.sID, pl.cstart, pl.cnext, pl.cend, pl.fin, pl.fout, \
         pl.swID, pl.swplay, pl.vtID, pl.vtplay, pl.swfirst, pl.ord, \
         s.artist, s.title, s.path \
         FROM playlists_list pl LEFT JOIN songs s ON s.ID = pl.sID \
         WHERE pl.pID = ? ORDER BY pl.ord",
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await?)
}

pub async fn insert_playlist_item(pool: &MySqlPool, i: &NewPlaylistItem) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query(
        "INSERT INTO playlists_list (pID, sID, cstart, cnext, cend, fin, fout, swID, swplay, \
         vtID, vtplay, swfirst, ord) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(i.pID)
    .bind(i.sID)
    .bind(i.cstart)
    .bind(i.cnext)
    .bind(i.cend)
    .bind(i.fin)
    .bind(i.fout)
    .bind(i.swID)
    .bind(i.swplay)
    .bind(i.vtID)
    .bind(i.vtplay)
    .bind(i.swfirst)
    .bind(i.ord)
    .execute(&mut *conn)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn delete_playlist_item(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM playlists_list WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn clear_playlist_items(pool: &MySqlPool, playlist_id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM playlists_list WHERE pID = ?")
        .bind(playlist_id)
        .execute(pool)
        .await?
        .rows_affected())
}

// ---------------------------------------------------------------------------
// events + event categories
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EventRow {
    pub ID: u32,
    #[sqlx(rename = "type")]
    pub event_type: i8,
    pub time: String,
    pub name: String,
    pub date: Option<NaiveDate>,
    pub day: Option<String>,
    pub hours: Option<String>,
    pub data: Option<String>,
    pub enabled: Option<bool>,
    pub catID: i32,
    pub smart: bool,
    pub grouped: bool,
}

pub async fn list_events(pool: &MySqlPool, only_enabled: bool) -> Result<Vec<EventRow>> {
    let sql = if only_enabled {
        "SELECT * FROM events WHERE enabled = 1 ORDER BY `date`, `time`"
    } else {
        "SELECT * FROM events ORDER BY `date`, `time`"
    };
    Ok(sqlx::query_as::<_, EventRow>(sql).fetch_all(pool).await?)
}

pub async fn event_by_id(pool: &MySqlPool, id: u32) -> Result<Option<EventRow>> {
    Ok(sqlx::query_as::<_, EventRow>("SELECT * FROM events WHERE ID = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

pub async fn insert_event(pool: &MySqlPool, e: &EventRow) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query(
        "INSERT INTO events (`type`, `time`, `name`, `date`, `day`, `hours`, data, enabled, \
         catID, smart, grouped) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(e.event_type)
    .bind(&e.time)
    .bind(&e.name)
    .bind(e.date)
    .bind(&e.day)
    .bind(&e.hours)
    .bind(&e.data)
    .bind(e.enabled)
    .bind(e.catID)
    .bind(e.smart)
    .bind(e.grouped)
    .execute(&mut *conn)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn update_event(pool: &MySqlPool, e: &EventRow) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE events SET `type`=?, `time`=?, `name`=?, `date`=?, `day`=?, `hours`=?, data=?, \
         enabled=?, catID=?, smart=?, grouped=? WHERE ID=?",
    )
    .bind(e.event_type)
    .bind(&e.time)
    .bind(&e.name)
    .bind(e.date)
    .bind(&e.day)
    .bind(&e.hours)
    .bind(&e.data)
    .bind(e.enabled)
    .bind(e.catID)
    .bind(e.smart)
    .bind(e.grouped)
    .bind(e.ID)
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn delete_event(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM events WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EventCategory {
    pub ID: u32,
    pub name: String,
}

pub async fn list_event_categories(pool: &MySqlPool) -> Result<Vec<EventCategory>> {
    Ok(sqlx::query_as::<_, EventCategory>("SELECT ID, name FROM events_categories ORDER BY name")
        .fetch_all(pool)
        .await?)
}

pub async fn insert_event_category(pool: &MySqlPool, name: &str) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query("INSERT INTO events_categories (name) VALUES (?)")
        .bind(name)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn rename_event_category(pool: &MySqlPool, id: u32, name: &str) -> Result<u64> {
    Ok(sqlx::query("UPDATE events_categories SET name = ? WHERE ID = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn delete_event_category(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM events_categories WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

// ---------------------------------------------------------------------------
// requests
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Request {
    pub ID: u32,
    pub songID: u32,
    pub username: String,
    pub userIP: String,
    pub message: Option<String>,
    pub requested: NaiveDateTime,
    pub played: bool,
}

/// Add a request; `requested = NOW()` like RadioDJ.
pub async fn add_request(
    pool: &MySqlPool,
    songID: u32,
    username: &str,
    userIP: &str,
    message: Option<&str>,
) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query("INSERT INTO requests (songID, username, userIP, message, requested) VALUES (?, ?, ?, ?, NOW())")
        .bind(songID)
        .bind(username)
        .bind(userIP)
        .bind(message)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn list_requests(pool: &MySqlPool, include_played: bool, limit: i64) -> Result<Vec<Request>> {
    let sql = if include_played {
        "SELECT * FROM requests ORDER BY requested DESC LIMIT ?"
    } else {
        "SELECT * FROM requests WHERE played = 0 ORDER BY requested ASC LIMIT ?"
    };
    Ok(sqlx::query_as::<_, Request>(sql).bind(limit).fetch_all(pool).await?)
}

/// Explicit mark-played outside the booking transaction.
pub async fn mark_request_played(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("UPDATE requests SET played = 1 WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn delete_request(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM requests WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn pending_request_for_song(pool: &MySqlPool, songID: u32) -> Result<Option<Request>> {
    Ok(sqlx::query_as::<_, Request>(
        "SELECT * FROM requests WHERE songID = ? AND played = 0 ORDER BY requested ASC LIMIT 1",
    )
    .bind(songID)
    .fetch_optional(pool)
    .await?)
}

// ---------------------------------------------------------------------------
// history
// ---------------------------------------------------------------------------

/// One `history` row as read back from the table (SELECT side, carries the
/// generated `ID`). The write side is `radiocraft_core::booking::HistoryEntry`
/// — the planner's INSERT projection without the ID — which is
/// deliberately a separate type: sqlx `FromRow` stays out of core.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct HistoryEntry {
    pub ID: u32,
    pub trackID: u32,
    pub date_played: Option<NaiveDateTime>,
    pub song_type: i8,
    pub id_subcat: u32,
    pub id_genre: u32,
    pub duration: Decimal,
    pub artist: String,
    pub original_artist: String,
    pub title: String,
    pub album: String,
    pub composer: String,
    pub label: String,
    pub year: String,
    pub track_no: i32,
    pub disc_no: i32,
    pub publisher: String,
    pub copyright: String,
    pub isrc: String,
    pub listeners: Option<u32>,
}

const HISTORY_COLUMNS: &str = "ID, trackID, date_played, song_type, id_subcat, id_genre, duration, \
     artist, original_artist, title, album, composer, label, `year`, track_no, disc_no, \
     publisher, copyright, isrc, listeners";

pub async fn list_history(pool: &MySqlPool, limit: i64, offset: i64) -> Result<Vec<HistoryEntry>> {
    Ok(sqlx::query_as::<_, HistoryEntry>(&format!(
        "SELECT {HISTORY_COLUMNS} FROM history ORDER BY date_played DESC, ID DESC LIMIT ? OFFSET ?"
    ))
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?)
}

pub async fn history_for_track(
    pool: &MySqlPool,
    trackID: u32,
    limit: i64,
) -> Result<Vec<HistoryEntry>> {
    Ok(sqlx::query_as::<_, HistoryEntry>(&format!(
        "SELECT {HISTORY_COLUMNS} FROM history WHERE trackID = ? ORDER BY date_played DESC, ID DESC LIMIT ?"
    ))
    .bind(trackID)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Mirror of the booking trim: remove history older than `days`
/// (`days = 0` wipes the table, the RadioDJ quirk).
pub async fn trim_history(pool: &MySqlPool, days: i32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM history WHERE TIMESTAMPDIFF(DAY, date_played, NOW()) >= ?")
        .bind(days)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn clear_history(pool: &MySqlPool) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM history").execute(pool).await?.rows_affected())
}

// ---------------------------------------------------------------------------
// current/now-playing state (derived: no `nowplaying` table exists)
// ---------------------------------------------------------------------------

/// Last booked play = newest history row.
pub async fn last_played(pool: &MySqlPool) -> Result<Option<HistoryEntry>> {
    Ok(sqlx::query_as::<_, HistoryEntry>(&format!(
        "SELECT {HISTORY_COLUMNS} FROM history ORDER BY date_played DESC, ID DESC LIMIT 1"
    ))
    .fetch_optional(pool)
    .await?)
}

/// One `queuelist` row (upcoming scheduled items). `swPlay`/`vtPlay` keep the
/// camelCase spelling of this table (unlike `playlists_list.swplay`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct QueueItem {
    pub ID: u32,
    pub songID: i32,
    pub ETA: Option<NaiveDateTime>,
    pub duration: Decimal,
    pub artist: String,
    pub associated_artists: String,
    pub title: String,
    pub album: String,
    pub swID: i32,
    pub swPlay: Decimal,
    pub vtID: i32,
    pub vtPlay: Decimal,
}

#[derive(Debug, Clone, Default)]
pub struct NewQueueItem {
    pub songID: i32,
    pub ETA: Option<NaiveDateTime>,
    pub duration: Decimal,
    pub artist: String,
    pub associated_artists: String,
    pub title: String,
    pub album: String,
    pub swID: i32,
    pub swPlay: Decimal,
    pub vtID: i32,
    pub vtPlay: Decimal,
}

pub async fn enqueue(pool: &MySqlPool, q: &NewQueueItem) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query(
        "INSERT INTO queuelist (songID, ETA, duration, artist, associated_artists, title, album, \
         swID, swPlay, vtID, vtPlay) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(q.songID)
    .bind(q.ETA)
    .bind(q.duration)
    .bind(&q.artist)
    .bind(&q.associated_artists)
    .bind(&q.title)
    .bind(&q.album)
    .bind(q.swID)
    .bind(q.swPlay)
    .bind(q.vtID)
    .bind(q.vtPlay)
    .execute(&mut *conn)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn list_queue(pool: &MySqlPool, limit: i64) -> Result<Vec<QueueItem>> {
    Ok(sqlx::query_as::<_, QueueItem>("SELECT * FROM queuelist ORDER BY ETA, ID LIMIT ?")
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

pub async fn delete_queue_item(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM queuelist WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn clear_queue(pool: &MySqlPool) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM queuelist").execute(pool).await?.rows_affected())
}

// ---------------------------------------------------------------------------
// carts (carts / carts_list)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Cart {
    pub ID: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CartButton {
    pub ID: u32,
    pub pID: u32,
    pub swID: u32,
    pub swButton: u32,
    pub color: String,
}

pub async fn list_carts(pool: &MySqlPool) -> Result<Vec<Cart>> {
    Ok(sqlx::query_as::<_, Cart>("SELECT ID, name FROM carts ORDER BY name")
        .fetch_all(pool)
        .await?)
}

pub async fn insert_cart(pool: &MySqlPool, name: &str) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query("INSERT INTO carts (name) VALUES (?)")
        .bind(name)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn delete_cart(pool: &MySqlPool, id: u32) -> Result<u64> {
    let mut conn = pool.acquire().await?;
    let cart_rows = sqlx::query("DELETE FROM carts WHERE ID = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM carts_list WHERE pID = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(cart_rows)
}

pub async fn list_cart_buttons(pool: &MySqlPool, cart_id: u32) -> Result<Vec<CartButton>> {
    Ok(sqlx::query_as::<_, CartButton>(
        "SELECT ID, pID, swID, swButton, color FROM carts_list WHERE pID = ? ORDER BY swButton",
    )
    .bind(cart_id)
    .fetch_all(pool)
    .await?)
}

/// A `carts_list` button joined with its library song, for cartwall display.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CartButtonRow {
    pub ID: u32,
    pub pID: u32,
    pub swID: u32,
    pub swButton: u32,
    pub color: String,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub path: Option<String>,
    pub duration: Option<Decimal>,
}

pub async fn list_cart_buttons_with_songs(
    pool: &MySqlPool,
    cart_id: u32,
) -> Result<Vec<CartButtonRow>> {
    Ok(sqlx::query_as::<_, CartButtonRow>(
        "SELECT c.ID, c.pID, c.swID, c.swButton, c.color, \
         s.artist, s.title, s.path, s.duration \
         FROM carts_list c LEFT JOIN songs s ON s.ID = c.swID \
         WHERE c.pID = ? ORDER BY c.swButton",
    )
    .bind(cart_id)
    .fetch_all(pool)
    .await?)
}

pub async fn insert_cart_button(
    pool: &MySqlPool,
    pID: u32,
    swID: u32,
    swButton: u32,
    color: &str,
) -> Result<u32> {
    let mut conn = pool.acquire().await?;
    sqlx::query("INSERT INTO carts_list (pID, swID, swButton, color) VALUES (?, ?, ?, ?)")
        .bind(pID)
        .bind(swID)
        .bind(swButton)
        .bind(color)
        .execute(&mut *conn)
        .await?;
    let id: i64 = sqlx::query_scalar("SELECT LAST_INSERT_ID()").fetch_one(&mut *conn).await?;
    Ok(id as u32)
}

pub async fn delete_cart_button(pool: &MySqlPool, id: u32) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM carts_list WHERE ID = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected())
}

// ---------------------------------------------------------------------------
// users
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct UserRow {
    pub ID: u32,
    pub user: String,
    pub auth: Option<String>,
    pub data: String,
}

pub async fn list_users(pool: &MySqlPool) -> Result<Vec<UserRow>> {
    Ok(sqlx::query_as::<_, UserRow>("SELECT ID, user, auth, data FROM users ORDER BY user")
        .fetch_all(pool)
        .await?)
}

pub async fn user_by_name(pool: &MySqlPool, user: &str) -> Result<Option<UserRow>> {
    Ok(sqlx::query_as::<_, UserRow>("SELECT ID, user, auth, data FROM users WHERE user = ?")
        .bind(user)
        .fetch_optional(pool)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_builds_in_clause() {
        assert_eq!(placeholders(0), "");
        assert_eq!(placeholders(3), "?,?,?");
    }

    #[test]
    fn song_insert_column_count_matches() {
        assert_eq!(SQL_SONG_INSERT.matches('?').count(), 12);
    }
}
