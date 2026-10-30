//! Optional token auth for LAN access (docs/05: localhost is open, LAN
//! mode requires `RADIOCRAFT_AUTH_TOKEN`).
//!
//! The token is never stored in plaintext: we hash the configured value
//! with SHA-256 and compare hashes (constant-time). Clients send the raw
//! token as the `password` of the `hello` frame.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

/// Auth policy for the whole server process.
#[derive(Debug, Clone, Default)]
pub struct Auth {
    /// True when any `hello` must succeed before commands are accepted.
    pub required: bool,
    /// SHA-256 hex of the expected token (present iff `required`).
    expected_hash: Option<String>,
}

impl Auth {
    /// Build from the environment: `RADIOCRAFT_AUTH_TOKEN` enables auth.
    pub fn from_env() -> Self {
        match std::env::var("RADIOCRAFT_AUTH_TOKEN") {
            Ok(token) if !token.trim().is_empty() => Self {
                required: true,
                expected_hash: Some(hash(token.trim())),
            },
            _ => Self::default(),
        }
    }

    /// Auth from a literal token (tests / config file later).
    pub fn with_token(token: &str) -> Self {
        Self {
            required: true,
            expected_hash: Some(hash(token)),
        }
    }

    /// Check a `hello` payload. The username is ignored (single-operator
    /// app, RadioDJ has no user model); the password must be the token.
    pub fn check_hello(&self, username: Option<&str>, password: Option<&str>) -> bool {
        if !self.required {
            return true;
        }
        let (Some(expected), Some(given)) = (self.expected_hash.as_ref(), password) else {
            return false;
        };
        let _ = username;
        constant_eq_hex(expected, &hash(given))
    }
}

fn hash(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Constant-time comparison of two ASCII hex strings.
fn constant_eq_hex(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_when_no_token() {
        let auth = Auth::default();
        assert!(!auth.required);
        assert!(auth.check_hello(None, None));
    }

    #[test]
    fn token_hello_roundtrip() {
        let auth = Auth::with_token("s3cret");
        assert!(auth.check_hello(Some("op"), Some("s3cret")));
        assert!(!auth.check_hello(None, Some("wrong")));
        assert!(!auth.check_hello(None, None));
    }

    #[test]
    fn hash_is_stable_hex() {
        // sha256("") well-known vector.
        assert_eq!(
            hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
