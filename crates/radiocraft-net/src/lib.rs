//! Radiocraft-net: now playing export, embedded web server, AutoStart helpers.
//!
//! This crate mirrors RadioDJ's outward-facing integrations:
//!
//! * [`nowplaying`] — the MetadataExport plugin (file, web, serial targets).
//! * [`webserver`] — the built-in HTTP console + JSON API + request endpoint.
//! * [`autost`] — the daily AutoBackup timer and close-to-tray policy.
//!
//! It deliberately does not depend on radiocraft-db: hosts plug their own
//! sinks in through [`nowplaying::HttpTransport`]/[`nowplaying::SerialWriter`],
//! [`webserver::RequestSink`] and [`autost::BackupSink`].

pub mod autost;
pub mod nowplaying;
pub mod webserver;

pub use autost::{AutoBackupConfig, AutoBackupScheduler, BackupSink, CloseAction, ClosePolicy};
pub use nowplaying::{
    encoding_code, ExportConfig, ExportDraft, ExportError, FileEncoding, NowPlayingExporter,
    NowPlayingSnapshot, SerialWriter, WebItem,
};
pub use webserver::{
    NowPlayingProvider, RequestSink, RequestSubmission, WebError, WebServer, WebServerConfig,
};
