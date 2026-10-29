//! Data-only SQL dump, mirroring the AutoBackup behaviour documented in
//! `docs/03-plugins-and-integrations.md` §5 item 6: every table's rows are exported as
//! plain `INSERT INTO` statements (no DDL, no `DELETE` preamble). This lets the tool back up
//! and restore without depending on a server-side `mysqldump`, and the resulting files are
//! valid SQL that can be replayed through the bootstrap pipeline.

use sqlx::mysql::MySqlPool;
use sqlx::Row;

use crate::error::{DbError, Result};

/// The tables exported by `export_database`, in dependency-free order. Matches the 17-table
/// radiodj3 schema.
pub const BACKUP_TABLES: &[&str] = &[
    "settings",
    "category",
    "subcategory",
    "genre",
    "rotations",
    "rotations_list",
    "events",
    "events_categories",
    "playlists",
    "playlists_list",
    "carts",
    "carts_list",
    "songs",
    "requests",
    "history",
    "queuelist",
    "users",
];

/// Quote a MySQL identifier with backticks.
fn quote_ident(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// Escape a string literal for MySQL (backslash escaping is the default mode).
fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out
}

/// Render one `INSERT INTO` statement for a row. `NULL` columns are emitted bare.
pub fn render_insert(table: &str, columns: &[String], values: &[Option<String>]) -> String {
    let cols = columns
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let vals = values
        .iter()
        .map(|v| match v {
            None => "NULL".to_string(),
            Some(s) => format!("'{}'", escape_string(s)),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({}) VALUES ({});",
        quote_ident(table),
        cols,
        vals
    )
}

fn check_allowed(table: &str) -> Result<()> {
    if BACKUP_TABLES.contains(&table) {
        Ok(())
    } else {
        Err(DbError::UnknownTable(table.to_string()))
    }
}

/// Export one table's rows as INSERT statements.
pub async fn export_table_data(pool: &MySqlPool, table: &str) -> Result<Vec<String>> {
    check_allowed(table)?;
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = ? ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .map_err(DbError::Sqlx)?;
    let columns: Vec<String> = cols.into_iter().map(|(c,)| c).collect();
    if columns.is_empty() {
        return Err(DbError::UnknownTable(table.to_string()));
    }
    let col_list = columns
        .iter()
        .map(|c| format!("CAST(`{}` AS CHAR)", c.replace('`', "``")))
        .collect::<Vec<_>>()
        .join(", ");
    let select = format!("SELECT {} FROM `{}`", col_list, table.replace('`', "``"));
    let rows = sqlx::query(&select)
        .fetch_all(pool)
        .await
        .map_err(DbError::Sqlx)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let values: Vec<Option<String>> = columns
            .iter()
            .enumerate()
            .map(|(i, _)| row.try_get::<Option<String>, _>(i).ok().flatten())
            .collect();
        out.push(render_insert(table, &columns, &values));
    }
    Ok(out)
}

/// Export the whole database as data-only INSERT statements.
pub async fn export_database(pool: &MySqlPool) -> Result<Vec<String>> {
    let mut out = vec!["-- RadioCraft data-only dump".to_string()];
    for table in BACKUP_TABLES {
        out.push(format!("-- table {}", table));
        out.extend(export_table_data(pool, table).await?);
    }
    Ok(out)
}

/// Export to a file on disk.
pub async fn export_to_file(pool: &MySqlPool, path: &std::path::Path) -> Result<()> {
    let stmts = export_database(pool).await?;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|e| DbError::Other(anyhow::anyhow!(e)))?;
        }
    }
    let body = stmts.join("\n") + "\n";
    tokio::fs::write(path, body)
        .await
        .map_err(|e| DbError::Other(anyhow::anyhow!(e)))?;
    tracing::info!("backup written to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_insert_escapes() {
        let cols = vec!["a".to_string(), "b".to_string()];
        let vals = vec![
            Some("it's".to_string()),
            None,
        ];
        let sql = render_insert("songs", &cols, &vals);
        assert_eq!(sql, "INSERT INTO `songs` (`a`, `b`) VALUES ('it\\'s', NULL);");
    }

    #[test]
    fn rejects_unknown_tables() {
        assert!(check_allowed("songs").is_ok());
        assert!(check_allowed("nowplaying").is_err());
    }
}
