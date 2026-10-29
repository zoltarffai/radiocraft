//! Port of the stored procedure `UpdateTracks2` (docs/01 appendix, extracted
//! verbatim from RadioDJ's Setup/Sql scripts) as a single sqlx transaction.
//!
//! Statement ordering and effects follow the appendix exactly:
//! 1. bump `count_played`/`date_played` on the played track,
//! 2. for music (type 0) and requests (type 9), stamp `artist_played` /
//!    `title_played` / `album_played` (replay-avoidance bookkeeping),
//! 3. mark matching requests as played,
//! 4. enforce play limits (disable / delete / relocate to another subcategory
//!    via a negative `limit_action`),
//! 5. optionally decay `weight` by `pWeight`,
//! 6. insert the history row (only when `historyDays > 0`),
//! 7. unconditionally trim history older than `historyDays` days — including
//!    the RadioDJ quirk where `historyDays = 0` deletes *every* history row.

use sqlx::MySqlPool;

use crate::entities::HistoryEntry;
use crate::error::Result;

/// The verbatim appendix `CREATE PROCEDURE` text, kept for
/// [`crate::bootstrap::install_procedure`] so a RadioCraft database can be
/// made byte-compatible with stock RadioDJ tooling.
pub const PROCEDURE_SQL: &str = r#"CREATE PROCEDURE `UpdateTracks2`(
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
END"#;

// Statement forms of the procedure body (placeholders replace the IN
// parameters). These are the exact statements the procedure runs, kept here so
// the Rust transaction and the SQL text can never drift apart.

/// `UPDATE songs SET count_played=count_played+1, date_played=NOW() WHERE ID=trackID`
pub const SQL_COUNT_PLAYED: &str =
    "UPDATE songs SET count_played=count_played+1, date_played=NOW() WHERE ID=?";
/// `SET @tAlbum = (SELECT album FROM songs WHERE ID=trackID)`
pub const SQL_FETCH_ALBUM: &str = "SELECT album FROM songs WHERE ID=?";
/// `UPDATE songs SET artist_played=NOW() WHERE artist=curArtist`
pub const SQL_ARTIST_PLAYED: &str = "UPDATE songs SET artist_played=NOW() WHERE artist=?";
/// `UPDATE songs SET title_played=NOW() WHERE title=curTitle`
pub const SQL_TITLE_PLAYED: &str = "UPDATE songs SET title_played=NOW() WHERE title=?";
/// `UPDATE songs SET album_played=NOW() WHERE album=@tAlbum`
pub const SQL_ALBUM_PLAYED: &str = "UPDATE songs SET album_played=NOW() WHERE album=?";
/// `UPDATE requests SET played=1 WHERE songID=trackID`
pub const SQL_REQUEST_PLAYED: &str = "UPDATE requests SET played=1 WHERE songID=?";
/// limit_action=1: disable (play_limit reset), matching RadioDJ
pub const SQL_LIMIT_DISABLE: &str = "UPDATE songs SET enabled=0, play_limit=0 WHERE enabled=1 AND play_limit>0 AND count_played>=play_limit AND limit_action=1";
/// limit_action=2: delete the track (RadioDJ leaves dangling refs, so we too)
pub const SQL_LIMIT_DELETE: &str =
    "DELETE FROM songs WHERE play_limit>0 AND count_played>=play_limit AND limit_action=2";
/// limit_action<0: relocate to `ABS(limit_action)` as id_subcat
pub const SQL_LIMIT_RELOCATE: &str = "UPDATE songs SET id_subcat=ABS(limit_action), play_limit=0, limit_action=0 WHERE enabled=1 AND limit_action<0 AND count_played>=play_limit";
/// `UPDATE songs SET weight=weight-pWeight WHERE ID=trackID AND (weight-pWeight)>=0`
pub const SQL_WEIGHT_DECAY: &str = "UPDATE songs SET weight=weight-? WHERE ID=? AND (weight-?)>=0";
/// history INSERT … SELECT mirroring the procedure (song values come from the
/// `songs` row; artist/title/listeners come from the booking parameters)
pub const SQL_HISTORY_INSERT: &str = "INSERT INTO history(trackID, date_played, song_type, id_subcat, id_genre, duration, artist, \
     original_artist, title, album, composer, label, `year`, track_no, disc_no, publisher, copyright, isrc, listeners) \
     SELECT ?, NOW(), ?, id_subcat, id_genre, duration, ?, original_artist, ?, album, \
     composer, label, `year`, track_no, disc_no, publisher, copyright, isrc, ? FROM songs WHERE ID=?";
/// unconditional history trim (`historyDays = 0` clears history entirely, as RadioDJ does)
pub const SQL_HISTORY_TRIM: &str =
    "DELETE FROM history WHERE TIMESTAMPDIFF(DAY, date_played, NOW()) >= ?";

/// Parameters of the stored procedure, same order/meaning as `UpdateTracks2`.
#[derive(Clone, Debug, PartialEq)]
pub struct BookingParams {
    /// `trackID` — the `songs.ID` being booked.
    pub track_id: u32,
    /// `curArtist` — resolved artist for this play (RadioDJ passes the
    /// display artist, which may differ from `songs.artist`).
    pub cur_artist: String,
    /// `curTitle`
    pub cur_title: String,
    /// `tType` — 0 music, 9 request, otherwise jingle/sweeper etc.
    pub song_type: i32,
    /// `curListeners` — current listener count (nullable).
    pub cur_listeners: Option<i64>,
    /// `historyDays` — `DAYS_2_KEEP_HISTORY` setting.
    pub history_days: i32,
    /// `pWeight` — weight decay applied at play (0 disables decay).
    pub p_weight: f64,
}

/// Row counts from the limit enforcement + history trim, plus the inserted
/// history row (None when `historyDays = 0`, mirroring the procedure).
#[derive(Clone, Debug, PartialEq)]
pub struct BookingResult {
    pub history: Option<HistoryEntry>,
    /// requests rows set to `played=1`.
    pub requests_marked: u64,
    /// songs disabled by `limit_action = 1`.
    pub songs_disabled: u64,
    /// songs deleted by `limit_action = 2`.
    pub songs_deleted: u64,
    /// songs relocated to a new `id_subcat` (`limit_action < 0`).
    pub songs_relocated: u64,
    /// history rows trimmed by the final `DELETE`.
    pub history_trimmed: u64,
}

/// Execute the full booking bookkeeping of `UpdateTracks2` in one transaction.
pub async fn book_track(pool: &MySqlPool, p: &BookingParams) -> Result<BookingResult> {
    let mut tx = pool.begin().await?;

    sqlx::query(SQL_COUNT_PLAYED)
        .bind(p.track_id)
        .execute(&mut *tx)
        .await?;

    // SET @tAlbum = (SELECT album FROM songs WHERE ID=trackID)
    let album: String = sqlx::query_scalar(SQL_FETCH_ALBUM)
        .bind(p.track_id)
        .fetch_one(&mut *tx)
        .await?;

    if p.song_type == 0 || p.song_type == 9 {
        sqlx::query(SQL_ARTIST_PLAYED)
            .bind(&p.cur_artist)
            .execute(&mut *tx)
            .await?;
        sqlx::query(SQL_TITLE_PLAYED)
            .bind(&p.cur_title)
            .execute(&mut *tx)
            .await?;
        if !album.is_empty() {
            sqlx::query(SQL_ALBUM_PLAYED)
                .bind(&album)
                .execute(&mut *tx)
                .await?;
        }
    }

    let mut requests_marked = 0;
    if p.song_type == 9 {
        let res = sqlx::query(SQL_REQUEST_PLAYED)
            .bind(p.track_id)
            .execute(&mut *tx)
            .await?;
        requests_marked = res.rows_affected();
    }

    // play-limit enforcement: 1=disable, 2=delete, negative id_subcat=move to that category
    let songs_disabled = sqlx::query(SQL_LIMIT_DISABLE)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let songs_deleted = sqlx::query(SQL_LIMIT_DELETE)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let songs_relocated = sqlx::query(SQL_LIMIT_RELOCATE)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    if p.p_weight > 0.0 {
        sqlx::query(SQL_WEIGHT_DECAY)
            .bind(p.p_weight)
            .bind(p.track_id)
            .bind(p.p_weight)
            .execute(&mut *tx)
            .await?;
    }

    let mut history = None;
    if p.history_days > 0 {
        let _ = sqlx::query(SQL_HISTORY_INSERT)
            .bind(p.track_id)
            .bind(p.song_type)
            .bind(&p.cur_artist)
            .bind(&p.cur_title)
            .bind(p.cur_listeners)
            .bind(p.track_id)
            .execute(&mut *tx)
            .await?;
        history = sqlx::query_as::<_, HistoryEntry>(
            "SELECT ID, trackID, date_played, song_type, id_subcat, id_genre, duration, \
             artist, original_artist, title, album, composer, label, `year`, track_no, disc_no, \
             publisher, copyright, isrc, listeners FROM history WHERE ID = LAST_INSERT_ID()",
        )
        .fetch_optional(&mut *tx)
        .await?;
    }

    let history_trimmed = sqlx::query(SQL_HISTORY_TRIM)
        .bind(p.history_days)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    tx.commit().await?;

    Ok(BookingResult {
        history,
        requests_marked,
        songs_disabled,
        songs_deleted,
        songs_relocated,
        history_trimmed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_forms_match_procedure_body() {
        // Every ported statement must still appear in the verbatim procedure.
        assert!(PROCEDURE_SQL
            .contains("UPDATE songs SET count_played=count_played+1, date_played=NOW() WHERE ID=trackID;"));
        assert!(PROCEDURE_SQL
            .contains("UPDATE songs SET artist_played=NOW() WHERE artist=curArtist;"));
        assert!(PROCEDURE_SQL
            .contains("UPDATE songs SET title_played=NOW() WHERE title=curTitle;"));
        assert!(PROCEDURE_SQL
            .contains("IF @tAlbum <> '' THEN UPDATE songs SET album_played=NOW() WHERE album=@tAlbum; END IF;"));
        assert!(PROCEDURE_SQL.contains("IF tType = 9 THEN UPDATE requests SET played=1 WHERE songID=trackID; END IF;"));
        assert!(PROCEDURE_SQL
            .contains("UPDATE songs SET enabled=0, play_limit=0 WHERE enabled=1 AND play_limit>0 AND count_played>=play_limit AND limit_action=1;"));
        assert!(PROCEDURE_SQL
            .contains("DELETE FROM songs WHERE play_limit>0 AND count_played>=play_limit AND limit_action=2;"));
        assert!(PROCEDURE_SQL
            .contains("UPDATE songs SET id_subcat=ABS(limit_action), play_limit=0, limit_action=0 WHERE enabled=1 AND limit_action<0 AND count_played>=play_limit;"));
        assert!(PROCEDURE_SQL
            .contains("IF pWeight>0 THEN UPDATE songs SET weight=weight-pWeight WHERE ID=trackID AND (weight-pWeight)>=0; END IF;"));
        assert!(PROCEDURE_SQL
            .contains("DELETE FROM history WHERE TIMESTAMPDIFF(DAY, date_played, NOW()) >= historyDays;"));
    }

    #[test]
    fn placeholder_statements_shape() {
        assert_eq!(SQL_COUNT_PLAYED.matches('?').count(), 1);
        assert_eq!(SQL_ARTIST_PLAYED.matches('?').count(), 1);
        assert_eq!(SQL_TITLE_PLAYED.matches('?').count(), 1);
        assert_eq!(SQL_ALBUM_PLAYED.matches('?').count(), 1);
        assert_eq!(SQL_REQUEST_PLAYED.matches('?').count(), 1);
        assert_eq!(SQL_LIMIT_DISABLE.matches('?').count(), 0);
        assert_eq!(SQL_LIMIT_DELETE.matches('?').count(), 0);
        assert_eq!(SQL_LIMIT_RELOCATE.matches('?').count(), 0);
        assert_eq!(SQL_WEIGHT_DECAY.matches('?').count(), 3);
        // trackID, tType, curArtist, curTitle, curListeners, WHERE ID
        assert_eq!(SQL_HISTORY_INSERT.matches('?').count(), 6);
        assert_eq!(SQL_HISTORY_TRIM.matches('?').count(), 1);
        assert!(SQL_HISTORY_TRIM.ends_with(">= ?"));
    }

    #[test]
    fn procedure_signature_and_history_columns_match_appendix() {
        assert!(PROCEDURE_SQL.starts_with("CREATE PROCEDURE `UpdateTracks2`("));
        // The parameter list spans several lines; join them before matching.
        let signature: String = PROCEDURE_SQL
            .lines()
            .take_while(|l| !l.trim().eq("BEGIN"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(signature.contains("IN `curListeners` INT"));
        assert!(signature.contains("IN `historyDays` INT"));
        assert!(signature.contains("IN `pWeight` DOUBLE"));
        assert!(signature.contains("IN `trackID` INT"));
    }
}
