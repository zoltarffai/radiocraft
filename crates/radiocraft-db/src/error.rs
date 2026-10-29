//! Error type for the persistence layer.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Debug, Error)]
pub enum DbError {
    /// Any driver error (connection refused, lost connection, protocol, decode).
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Filesystem errors raised by the backup exporter.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The `radiodj3` tables do not exist yet — run bootstrap first.
    #[error("database is not bootstrapped: radiodj3 schema is missing, expected dbVersion {expected}")]
    NotBootstrapped { expected: i64 },

    /// The `settings.dbVersion` value is older than this crate's schema target
    /// and the schema already exists — a migration path is required.
    #[error("database needs migration: found dbVersion {found}, expected {expected}")]
    NeedsMigration { found: i64, expected: i64 },

    /// The database was written by a newer RadioCraft schema version.
    #[error("database dbVersion {found} is newer than this build supports ({expected})")]
    CurrentVersionTooHigh { found: i64, expected: i64 },

    /// Requested backup of a table that is not part of the radiodj3 schema.
    #[error("unknown table: {0}")]
    UnknownTable(String),

    /// Free-form context (schema parsing, malformed values, etc.).
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
