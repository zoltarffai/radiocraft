//! `radiocraft-db`: MySQL/MariaDB persistence for the RadioCraft rebuild, targeting the
//! radiodj3 (RadioDJ 7.6) schema documented in `docs/01-database-schema.md`.
//!
//! Layout:
//! - [`config`] — connection settings mirroring `settings_database.xml`.
//! - [`bootstrap`] — embedded DDL + schema bootstrap/migrations + the stored-procedure
//!   install path.
//! - [`booking`] — the `UpdateTracks2` play-booking transaction.
//! - [`entities`] — `FromRow` row models and CRUD helpers.
//! - [`settings_store`] — the RadioDJ `GetSetting`/`SaveSetting` upsert.
//! - [`backup`] — data-only INSERT dumps (AutoBackup parity).
//!
//! All queries are runtime-built (no `query!` macros), so the crate compiles without a
//! live database. `Db` is a thin cloneable facade over the shared
//! [`sqlx::mysql::MySqlPool`]; the free functions in each module are available for
//! callers managing their own pools.

// radiodj3 column names are RadioDJ's PascalCase identifiers (`ID`, `songID`, `swPlay`
// …); row models mirror the schema exactly, so the naming lints are silenced here.
#![allow(non_snake_case, non_camel_case_types)]

pub mod backup;
pub mod bootstrap;
pub mod booking;
pub mod config;
pub mod entities;
pub mod error;
pub mod settings_store;

use std::path::Path;

use sqlx::mysql::{MySqlPool, MySqlPoolOptions};

use crate::bootstrap::BootstrapReport;
use crate::booking::{BookingParams, BookingResult};
use crate::config::DbConfig;
use crate::error::{DbError, Result};

/// Shared handle to the radiodj3 database. Cheap to clone (pool is `Arc`-backed).
#[derive(Clone, Debug)]
pub struct Db {
    pool: MySqlPool,
}

impl Db {
    /// Open the pool (no schema check yet).
    pub async fn connect(cfg: &DbConfig) -> Result<Db> {
        let pool = cfg
            .pool_options()
            .connect_with(cfg.connect_options()?)
            .await
            .map_err(DbError::Sqlx)?;
        Ok(Db { pool })
    }

    /// Open the pool and ensure the schema/seed data and `dbVersion` marker are in place.
    pub async fn connect_bootstrapped(cfg: &DbConfig) -> Result<(Db, BootstrapReport)> {
        let db = Db::connect(cfg).await?;
        let report = db.bootstrap().await?;
        Ok((db, report))
    }

    /// Build a pool from a bare URL (dev/CLI/test use).
    pub async fn connect_url(url: &str) -> Result<Db> {
        let pool = MySqlPoolOptions::new()
            .max_connections(10)
            .connect(url)
            .await
            .map_err(DbError::Sqlx)?;
        Ok(Db { pool })
    }

    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }

    /// Gracefully wait for all connections to close.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    // ---- schema -------------------------------------------------------------

    pub async fn bootstrap(&self) -> Result<BootstrapReport> {
        bootstrap::ensure_schema(&self.pool).await
    }

    pub async fn db_version(&self) -> Result<Option<i64>> {
        bootstrap::db_version(&self.pool).await
    }

    // ---- settings -----------------------------------------------------------

    pub async fn get_setting(&self, source: &str, setting: &str) -> Result<Option<String>> {
        settings_store::get_setting(&self.pool, source, setting).await
    }

    pub async fn get_setting_i64(&self, source: &str, setting: &str) -> Result<Option<i64>> {
        settings_store::get_setting_i64(&self.pool, source, setting).await
    }

    pub async fn save_setting(
        &self,
        source: &str,
        setting: &str,
        value: &str,
    ) -> Result<()> {
        settings_store::save_setting(&self.pool, source, setting, value).await
    }

    pub async fn all_settings(&self, source: &str) -> Result<Vec<(String, String)>> {
        settings_store::all_settings(&self.pool, source).await
    }

    // ---- booking ------------------------------------------------------------

    /// The `UpdateTracks2` port: full play-booking transaction.
    pub async fn book_track(&self, params: &BookingParams) -> Result<BookingResult> {
        booking::book_track(&self.pool, params).await
    }

    // ---- backup -------------------------------------------------------------

    pub async fn export_backup(&self) -> Result<Vec<String>> {
        backup::export_database(&self.pool).await
    }

    pub async fn export_backup_to_file(&self, path: &Path) -> Result<()> {
        backup::export_to_file(&self.pool, path).await
    }
}
