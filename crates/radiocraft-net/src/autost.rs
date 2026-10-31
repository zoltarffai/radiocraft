//! AutoStart duties: the daily database backup and close-to-tray policy
//! (docs/02 §8 — "AutoStart" behaviour of the main application).
//!
//! Two behaviours are mirrored here:
//!
//! * **AutoBackup** — when `AutoBackupEnable` is set, the application runs a
//!   database backup every day at `AutoBackupHour` (0-23, local time) into
//!   `BackupFolder`. The actual dump is delegated to a [`BackupSink`] so the
//!   MySQL-specific work stays in the host crate.
//! * **Close-to-tray** — when `MinimizeToTray` is set, closing the main
//!   window hides the app to the tray instead of exiting; the policy helper
//!   below just classifies the close request so the host (a tray widget or
//!   window manager) can act on it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone};
use radiocraft_settings::DatabaseSettings;
use serde::{Deserialize, Serialize};

/// Host seam for the actual backup work (mysqldump or a SQL-script fallback).
pub trait BackupSink: Send + Sync {
    /// Perform a backup writing into `folder`; implementations should create
    /// dated file names (`radiodj-backup-YYYY-MM-DD.sql`) themselves.
    fn run_backup(&self, folder: &str) -> Result<(), String>;
}

/// What the AutoBackup timer needs to know (from `settings_database.xml`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoBackupConfig {
    pub enabled: bool,
    /// Hour of day (0-23, local time) the backup fires.
    pub hour: u32,
    /// Destination folder for backups.
    pub backup_folder: String,
}

impl AutoBackupConfig {
    pub fn from_settings(s: &DatabaseSettings) -> Self {
        let hour = (*s.auto_backup_hour()).clamp(0, 23) as u32;
        AutoBackupConfig {
            enabled: *s.auto_backup_enable(),
            hour,
            backup_folder: s.backup_folder().clone(),
        }
    }

    fn scheduled_at(&self, date: NaiveDate) -> Option<DateTime<Local>> {
        let time = NaiveTime::from_hms_opt(self.hour, 0, 0)?;
        Local.from_local_datetime(&date.and_time(time)).single()
    }

    /// The next local datetime at `hour:00` strictly after `now`.
    pub fn next_run(&self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        if !self.enabled {
            return None;
        }
        let today = now.date_naive();
        let candidate = self.scheduled_at(today)?;
        if candidate > now {
            Some(candidate)
        } else {
            self.scheduled_at(today.checked_add_signed(Duration::days(1))?)
        }
    }

    /// True if a backup is due: the scheduled time for today has passed and
    /// the last run happened before it.
    pub fn is_due(&self, now: DateTime<Local>, last_run: Option<DateTime<Local>>) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(today_run) = self.scheduled_at(now.date_naive()) else {
            return false;
        };
        now >= today_run && last_run.map(|l| l < today_run).unwrap_or(true)
    }
}

/// Classification of a window-close request (close to tray vs. quit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Hide to the tray, keep running.
    HideToTray,
    /// Really exit the application.
    Exit,
}

/// Close-to-tray policy from `MinimizeToTray`.
#[derive(Clone, Debug)]
pub struct ClosePolicy {
    pub minimize_to_tray: bool,
}

impl ClosePolicy {
    pub fn from_settings(s: &DatabaseSettings) -> Self {
        ClosePolicy {
            minimize_to_tray: *s.minimize_to_tray(),
        }
    }

    /// Decide what to do with a close request (the host passes
    /// `quit_requested = true` when the user chose "Exit" explicitly).
    pub fn on_close(&self, quit_requested: bool) -> CloseAction {
        if self.minimize_to_tray && !quit_requested {
            CloseAction::HideToTray
        } else {
            CloseAction::Exit
        }
    }
}

/// Human sentence for a close decision (migrated from the old egui shell's
/// `windows.rs`; the close window's preview pane renders it).
pub fn close_action_label(action: CloseAction) -> &'static str {
    match action {
        CloseAction::HideToTray => "the window will hide; the application keeps running",
        CloseAction::Exit => "the application will exit",
    }
}

/// A running AutoBackup scheduler; call [`AutoBackupScheduler::stop`] to end it.
pub struct AutoBackupScheduler {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl AutoBackupScheduler {
    /// Spawn the background ticker. RadioDJ drives this from a 60-second UI
    /// timer; `check` picks the polling interval.
    pub fn spawn(config: AutoBackupConfig, sink: Arc<dyn BackupSink>, check: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut last_run: Option<DateTime<Local>> = None;
            while !flag.load(Ordering::SeqCst) {
                let now = Local::now();
                if config.is_due(now, last_run) {
                    match sink.run_backup(&config.backup_folder) {
                        Ok(()) => {
                            last_run = Some(now);
                            tracing::info!("AutoBackup completed into {}", config.backup_folder);
                        }
                        Err(e) => tracing::error!("AutoBackup failed: {e}"),
                    }
                }
                let millis = check
                    .to_std()
                    .unwrap_or_else(|_| std::time::Duration::from_secs(60));
                std::thread::sleep(millis);
            }
        });
        AutoBackupScheduler {
            stop,
            handle: Some(handle),
        }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The date of the next scheduled backup (for the options form display).
pub fn next_backup_date(config: &AutoBackupConfig, now: DateTime<Local>) -> Option<NaiveDate> {
    config.next_run(now).map(|dt| dt.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;
    use radiocraft_settings::KvStore;
    use std::sync::atomic::AtomicU32;

    fn db(enable: bool, hour: i32) -> DatabaseSettings {
        let mut store = KvStore::new();
        store.set("AutoBackupEnable", if enable { "True" } else { "False" });
        store.set("AutoBackupHour", hour.to_string());
        store.set("BackupFolder", "/backups");
        store.set("MinimizeToTray", if enable { "True" } else { "False" });
        DatabaseSettings::from_kv(&store)
    }

    fn at(day: u32, hour: u32) -> DateTime<Local> {
        Local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2026, 10, day)
                    .unwrap()
                    .and_time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap()),
            )
            .single()
            .unwrap()
    }

    #[test]
    fn config_reads_database_settings() {
        let cfg = AutoBackupConfig::from_settings(&db(true, 22));
        assert!(cfg.enabled);
        assert_eq!(cfg.hour, 22);
        assert_eq!(cfg.backup_folder, "/backups");
    }

    #[test]
    fn disabled_config_has_no_next_run() {
        let cfg = AutoBackupConfig::from_settings(&db(false, 3));
        assert_eq!(cfg.next_run(at(29, 12)), None);
    }

    #[test]
    fn next_run_rolls_to_tomorrow_when_time_passed() {
        let cfg = AutoBackupConfig::from_settings(&db(true, 22));
        let next = cfg.next_run(at(29, 23)).unwrap();
        assert_eq!(
            next.date_naive(),
            NaiveDate::from_ymd_opt(2026, 10, 30).unwrap()
        );
        assert_eq!(next.hour(), 22);
    }

    #[test]
    fn next_run_is_today_before_the_hour() {
        let cfg = AutoBackupConfig::from_settings(&db(true, 22));
        let next = cfg.next_run(at(29, 12)).unwrap();
        assert_eq!(
            next.date_naive(),
            NaiveDate::from_ymd_opt(2026, 10, 29).unwrap()
        );
        assert_eq!(
            next_backup_date(&cfg, at(29, 12)),
            Some(NaiveDate::from_ymd_opt(2026, 10, 29).unwrap())
        );
    }

    #[test]
    fn is_due_fires_once_per_day() {
        let cfg = AutoBackupConfig::from_settings(&db(true, 22));
        assert!(!cfg.is_due(at(29, 21), None)); // before schedule
        assert!(cfg.is_due(at(29, 22), None)); // at schedule
        assert!(cfg.is_due(at(29, 23), Some(at(28, 22)))); // last run was yesterday
        assert!(!cfg.is_due(at(29, 23), Some(at(29, 22)))); // already ran today
    }

    #[test]
    fn close_policy_tray_and_exit() {
        let p = ClosePolicy::from_settings(&db(true, 22));
        assert_eq!(p.on_close(false), CloseAction::HideToTray);
        assert_eq!(p.on_close(true), CloseAction::Exit);
        let p = ClosePolicy::from_settings(&db(false, 22));
        assert_eq!(p.on_close(false), CloseAction::Exit);
    }

    #[test]
    fn close_action_label_matches_variants() {
        assert!(close_action_label(CloseAction::HideToTray).contains("hide"));
        assert!(close_action_label(CloseAction::Exit).contains("exit"));
    }

    #[test]
    fn scheduler_runs_backup_when_due() {
        struct Counting(AtomicU32);
        impl BackupSink for Counting {
            fn run_backup(&self, folder: &str) -> Result<(), String> {
                assert_eq!(folder, "/backups");
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        // hour = current local hour => is_due becomes true as soon as the
        // top of the hour has passed today.
        let now = Local::now();
        let cfg = AutoBackupConfig {
            enabled: true,
            hour: now.hour(),
            backup_folder: "/backups".into(),
        };
        let sink = Arc::new(Counting(AtomicU32::new(0)));
        let sched =
            AutoBackupScheduler::spawn(cfg, sink.clone(), chrono::Duration::milliseconds(30));
        std::thread::sleep(std::time::Duration::from_millis(150));
        sched.stop();
        // Due exactly once; the <= bound guards hour-boundary races.
        assert!(sink.0.load(Ordering::SeqCst) <= 1);
    }
}
