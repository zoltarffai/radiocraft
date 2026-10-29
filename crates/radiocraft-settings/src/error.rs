//! Error type for settings I/O and XML problems.

use std::path::PathBuf;

/// Any failure produced by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// File-system error while loading or saving a settings file.
    #[error("io error for {}: {0}", path.display())]
    Io { path: PathBuf, source: std::io::Error },

    /// Malformed XML (message includes the offending construct).
    #[error("invalid XML in {}: {message}", path.display())]
    Xml { path: PathBuf, message: String },

    /// Operation requires a file-backed document, but the settings object was
    /// built in memory.
    #[error("operation invalid: {0}")]
    InvalidOperation(&'static str),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io { path: path.into(), source }
    }

    pub(crate) fn xml(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Error::Xml { path: path.into(), message: message.into() }
    }
}
