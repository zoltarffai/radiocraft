//! Schema bootstrap: installs the embedded radiodj3 DDL + seed baseline and
//! tracks the schema version in the `settings` table (`dbVersion` key, same as
//! RadioDJ).
//!
//! The stored procedure `UpdateTracks2` is deliberately **not** installed by
//! default: RadioCraft books plays from Rust (see [`crate::booking`]).
//! [`install_procedure`] is provided for tools that want the SQL procedure
//! (e.g. comparing against a stock RadioDJ database).

use sqlx::Executor;
use sqlx::MySqlPool;

use crate::booking::PROCEDURE_SQL;
use crate::error::{DbError, Result};
use crate::settings_store;

/// The radiodj3 DDL + seed baseline (17 tables), verbatim copy of
/// `reference/schema/database.sql` with the stored-procedure block removed.
pub const SCHEMA_SQL: &str = include_str!("../schema/database.sql");

/// Schema version this crate expects (matches RadioDJ's `dbVersion`).
pub const CURRENT_DB_VERSION: i64 = 2006;

/// `settings` row identifying the schema version (RadioDJ stores this in
/// `settings_database.xml` under the `dbVersion` key).
pub const DB_VERSION_KEY: &str = "dbVersion";

/// `source` column used when persisting settings to the database.
pub const DB_VERSION_SOURCE: &str = "settings_database";

/// What [`ensure_schema`] did with this database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaAction {
    /// The radiodj3 schema was installed from scratch.
    Installed,
    /// The schema was already present and at the expected version.
    AlreadyCurrent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapReport {
    pub action: SchemaAction,
    pub db_version: i64,
    /// Number of SQL statements executed (0 when nothing was applied).
    pub statements: usize,
}

/// Split a MySQL script into executable statements.
///
/// Quote-aware scanner: strips `--` / `#` line comments and `/* */` block
/// comments (including the `/*!…*/` session-preset lines of HeidiSQL dumps,
/// which become empty and are dropped), understands backtick/quote strings,
/// and honours `DELIMITER` directives. Returns the statements without their
/// trailing terminator.
pub fn split_statements(script: &str) -> Vec<String> {
    let mut statements: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut delimiter = String::from(";");
    let chars: Vec<char> = script.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];

        // Line comments (outside strings).
        if c == '#' || (c == '-' && i + 1 < chars.len() && chars[i + 1] == '-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        // Block comments (including /*! executable sections: stripped, the
        // RadioDJ dump's presets are session settings sqlx sets itself).
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            if i < chars.len() && chars[i] == '!' {
                // skip the version marker like 50503
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            while i < chars.len() && !(chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 < chars.len() {
                i += 2; // past */
            }
            continue;
        }

        // Quoted identifiers / strings: copy verbatim, honouring the closing
        // quote and simple backslash escapes inside ' / " strings.
        if c == '`' || c == '\'' || c == '"' {
            let quote = c;
            current.push(c);
            i += 1;
            while i < chars.len() {
                let ch = chars[i];
                current.push(ch);
                i += 1;
                if ch == '\\' && quote != '`' && i < chars.len() {
                    current.push(chars[i]);
                    i += 1;
                    continue;
                }
                if ch == quote {
                    // doubled quote escape ('' or "" or ``)
                    if i < chars.len() && chars[i] == quote {
                        current.push(chars[i]);
                        i += 1;
                        continue;
                    }
                    break;
                }
            }
            continue;
        }

        // DELIMITER directive (line-based; HeidiSQL dumps use this for
        // routines). Recognised whenever a fresh statement begins, including
        // the `DELIMITER ;` reset line.
        if c == 'D'
            && current.trim().is_empty()
            && chars[i..].iter().collect::<String>().to_uppercase().starts_with("DELIMITER ")
        {
            // consume the whole line
            let mut line = String::new();
            while i < chars.len() && chars[i] != '\n' {
                line.push(chars[i]);
                i += 1;
            }
            let rest = line["DELIMITER".len()..].trim();
            // strip any line comment that got glued on, then trailing/leading WS
            let rest = rest.split("--").next().unwrap_or("").trim();
            if rest == ";" || rest == "/;" {
                delimiter = String::from(";");
            } else if !rest.is_empty() {
                delimiter = rest.to_string();
            }
            current.clear();
            continue;
        }

        // Statement terminator?
        if starts_with(&chars[i..], &delimiter) {
            let stmt = current.trim().to_string();
            if !stmt.is_empty() {
                statements.push(stmt);
            }
            current.clear();
            i += delimiter.len();
            continue;
        }

        current.push(c);
        i += 1;
    }

    let tail = current.trim().to_string();
    if !tail.is_empty() {
        statements.push(tail);
    }
    statements
}

fn starts_with(chars: &[char], needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, c)| chars.get(k) == Some(&c))
}

/// True when the radiodj3 base tables are present in the current database.
pub async fn schema_exists(pool: &MySqlPool) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = 'songs'",
    )
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

/// Read `dbVersion` from the `settings` table (None when unset).
pub async fn db_version(pool: &MySqlPool) -> Result<Option<i64>> {
    let raw = settings_store::get_setting(pool, DB_VERSION_SOURCE, DB_VERSION_KEY).await?;
    Ok(raw.and_then(|v| v.trim().parse::<i64>().ok()))
}

/// Write `dbVersion` into the `settings` table.
pub async fn set_db_version(pool: &MySqlPool, version: i64) -> Result<()> {
    settings_store::save_setting(pool, DB_VERSION_SOURCE, DB_VERSION_KEY, &version.to_string()).await
}

/// Execute the embedded schema script statement-by-statement.
pub async fn apply_schema(pool: &MySqlPool) -> Result<usize> {
    let statements = split_statements(SCHEMA_SQL);
    let mut conn = pool.acquire().await?;
    for stmt in &statements {
        // The dump's DROP/CREATE pairs are idempotent (IF EXISTS / IF NOT
        // EXISTS); run each statement on a single connection in sequence.
        conn.execute(stmt.as_str()).await?;
    }
    Ok(statements.len())
}

/// Install the legacy `UpdateTracks2` stored procedure. RadioCraft itself does
/// not need it (it books plays via [`crate::booking`]), but this keeps the
/// schema byte-identical with a stock RadioDJ install for tooling parity.
pub async fn install_procedure(pool: &MySqlPool) -> Result<()> {
    // sqlx sends the whole string to the server; no client-side DELIMITER
    // handling is required because the body is passed as one statement.
    sqlx::query(PROCEDURE_SQL).execute(pool).await?;
    Ok(())
}

/// Idempotent entry point used by the app at startup:
/// - no schema → apply the baseline and stamp [`CURRENT_DB_VERSION`];
/// - schema at the expected version → no-op;
/// - older/newer version → [`DbError::NeedsMigration`] /
///   [`DbError::CurrentVersionTooHigh`].
pub async fn ensure_schema(pool: &MySqlPool) -> Result<BootstrapReport> {
    if !schema_exists(pool).await? {
        let statements = apply_schema(pool).await?;
        set_db_version(pool, CURRENT_DB_VERSION).await?;
        return Ok(BootstrapReport {
            action: SchemaAction::Installed,
            db_version: CURRENT_DB_VERSION,
            statements,
        });
    }

    // Tables exist; settings may predate this tooling (a plain RadioDJ DB has
    // no dbVersion row until newer RadioDJ writes it). Treat "exists but no
    // version row" as needing an explicit stamp by the caller's migration flow.
    match db_version(pool).await? {
        None => Err(DbError::NotBootstrapped {
            expected: CURRENT_DB_VERSION,
        }),
        Some(v) if v < CURRENT_DB_VERSION => Err(DbError::NeedsMigration {
            found: v,
            expected: CURRENT_DB_VERSION,
        }),
        Some(v) if v > CURRENT_DB_VERSION => Err(DbError::CurrentVersionTooHigh {
            found: v,
            expected: CURRENT_DB_VERSION,
        }),
        Some(v) => Ok(BootstrapReport {
            action: SchemaAction::AlreadyCurrent,
            db_version: v,
            statements: 0,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_copy_has_no_procedure_or_delimiter() {
        let upper = SCHEMA_SQL.to_uppercase();
        assert!(!upper.contains("CREATE PROCEDURE"));
        assert!(!upper.contains("DROP PROCEDURE"));
        assert!(!upper.contains("DELIMITER"));
    }

    #[test]
    fn split_yields_expected_statement_count() {
        // 17 DROP TABLE + 17 CREATE TABLE + 7 seed INSERTs (carts, category,
        // events_categories, genre, rotations, subcategory, users).
        let stmts = split_statements(SCHEMA_SQL);
        assert_eq!(stmts.len(), 41, "counts: {stmts:?}");
        for s in &stmts {
            assert!(!s.is_empty());
            assert!(
                !s.lines().any(|l| l.trim_start().starts_with("-- ")),
                "comment leaked into statement: {s}"
            );
        }
        let drops = stmts.iter().filter(|s| s.starts_with("DROP TABLE")).count();
        let creates = stmts
            .iter()
            .filter(|s| s.starts_with("CREATE TABLE"))
            .count();
        let inserts = stmts.iter().filter(|s| s.starts_with("INSERT INTO")).count();
        assert_eq!(drops, 17);
        assert_eq!(creates, 17);
        assert_eq!(inserts, 7);
    }

    #[test]
    fn split_keeps_quoted_semicolons_intact() {
        let sql = "INSERT INTO t (a) VALUES ('a;b'); SELECT `x;y` FROM t;";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("'a;b'"));
        assert!(stmts[1].contains("`x;y`"));
    }

    #[test]
    fn split_strips_comments_and_presets() {
        let sql = "/*!40101 SET NAMES utf8 */;\n-- a comment\n# hash\nCREATE TABLE t (a INT);";
        let stmts = split_statements(sql);
        assert_eq!(stmts, vec!["CREATE TABLE t (a INT)"]);
    }

    #[test]
    fn split_honours_delimiter_directive() {
        let sql = "\
DELIMITER //
CREATE PROCEDURE p() BEGIN
  SELECT 1;
END //
DELIMITER ;
SELECT 1;";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].starts_with("CREATE PROCEDURE"));
        assert!(stmts[0].contains("SELECT 1;"));
        assert_eq!(stmts[1], "SELECT 1");
    }
}
