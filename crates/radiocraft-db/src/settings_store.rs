//! Key/value settings persistence in the `settings` table — RadioDJ's
//! `GetSetting`/`SaveSetting` pair.
//!
//! The original table has no PK or unique index and RadioDJ updates by the
//! `(source, setting)` pair, so an update-then-insert-if-absent upsert mirrors
//! that behaviour without adding a key to the shipped schema.

use sqlx::MySqlPool;

use crate::error::Result;

/// `SELECT value FROM settings WHERE source = ? AND setting = ? LIMIT 1`
pub const SQL_GET: &str = "SELECT value FROM settings WHERE source = ? AND setting = ? LIMIT 1";
/// update-first leg of the upsert (RadioDJ `SaveSetting`)
pub const SQL_UPDATE: &str =
    "UPDATE settings SET value = ? WHERE source = ? AND setting = ?";
/// insert leg when no row matched
pub const SQL_INSERT: &str =
    "INSERT INTO settings (source, setting, value) VALUES (?, ?, ?)";

/// Read one setting; `Ok(None)` when the key has never been saved. If RadioDJ
/// legacy data contains duplicate pairs, the first stored row wins (same as
/// RadioDJ reading the table with a data reader).
pub async fn get_setting(pool: &MySqlPool, source: &str, setting: &str) -> Result<Option<String>> {
    let value = sqlx::query_scalar::<_, String>(SQL_GET)
        .bind(source)
        .bind(setting)
        .fetch_optional(pool)
        .await?;
    Ok(value)
}

/// Read and parse an integer setting (malformed values count as missing).
pub async fn get_setting_i64(pool: &MySqlPool, source: &str, setting: &str) -> Result<Option<i64>> {
    let raw = get_setting(pool, source, setting).await?;
    Ok(raw.and_then(|v| v.trim().parse::<i64>().ok()))
}

/// Upsert one setting: UPDATE first, INSERT when nothing matched — matching
/// RadioDJ's `SaveSetting` order.
pub async fn save_setting(
    pool: &MySqlPool,
    source: &str,
    setting: &str,
    value: &str,
) -> Result<()> {
    let updated = sqlx::query(SQL_UPDATE)
        .bind(value)
        .bind(source)
        .bind(setting)
        .execute(pool)
        .await?
        .rows_affected();
    if updated == 0 {
        sqlx::query(SQL_INSERT)
            .bind(source)
            .bind(setting)
            .bind(value)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Delete every `settings` row for a `(source, setting)` pair (duplicates and
/// stale entries both go away).
pub async fn delete_setting(pool: &MySqlPool, source: &str, setting: &str) -> Result<u64> {
    let res = sqlx::query("DELETE FROM settings WHERE source = ? AND setting = ?")
        .bind(source)
        .bind(setting)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// All settings stored under one `source` (the file/section name RadioDJ used).
pub async fn all_settings(pool: &MySqlPool, source: &str) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT setting, value FROM settings WHERE source = ? ORDER BY setting",
    )
    .bind(source)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_text_matches_table_columns() {
        // settings(source, setting, value) — no PK, update keyed by both cols.
        assert!(SQL_GET.contains("WHERE source = ? AND setting = ?"));
        assert!(SQL_UPDATE.contains("SET value = ?") && SQL_UPDATE.contains("WHERE source = ? AND setting = ?"));
        assert!(SQL_INSERT.contains("INSERT INTO settings (source, setting, value) VALUES (?, ?, ?)"));
    }
}
