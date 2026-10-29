//! Crate-wide error type.

use std::fmt;

/// Errors produced by the audio layer.
#[derive(Debug)]
pub enum AudioError {
    /// A file could not be opened or read.
    Io(std::io::Error),
    /// A stream/probe/decode failure (message is backend-specific).
    Decode(String),
    /// The target format is unsupported.
    Unsupported(String),
    /// A generic invalid-argument style failure.
    Invalid(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Io(e) => write!(f, "io error: {e}"),
            AudioError::Decode(m) => write!(f, "decode error: {m}"),
            AudioError::Unsupported(m) => write!(f, "unsupported: {m}"),
            AudioError::Invalid(m) => write!(f, "invalid: {m}"),
        }
    }
}

impl std::error::Error for AudioError {}

impl From<std::io::Error> for AudioError {
    fn from(e: std::io::Error) -> Self {
        AudioError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, AudioError>;
