//! Connection configuration, mirroring the `settings_database.xml` keys that
//! RadioDJ uses (`DbServer`, `DbName`, `DbUsername`, `DbPass`, `SSL`).
//!
//! [`DbConfig::connect_url`] builds a MySqlConnector-equivalent connection
//! string so the values can be lifted straight out of the RadioDJ XML
//! settings file (or environment variables for the RadioCraft dev loop).

use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};

use crate::error::{DbError, Result};

/// `DbServer` — host (optionally `host:port`), default `127.0.0.1`.
pub const XML_KEY_SERVER: &str = "DbServer";
/// `DbName` — database name, default `radiodj3`.
pub const XML_KEY_DATABASE: &str = "DbName";
/// `DbUsername` — MySQL user name.
pub const XML_KEY_USERNAME: &str = "DbUsername";
/// `DbPass` — MySQL password (may be empty).
pub const XML_KEY_PASSWORD: &str = "DbPass";
/// `SSL` — MySqlConnector boolean (`True`/`False`).
pub const XML_KEY_SSL: &str = "SSL";

/// Fallback database name used by the RadioDJ dumps.
pub const DEFAULT_DATABASE: &str = "radiodj3";
/// Default MySQL/MariaDB port.
pub const DEFAULT_PORT: u16 = 3306;
/// Charset used by every radiodj3 table (`utf8mb4`).
pub const DEFAULT_CHARSET: &str = "utf8mb4";
/// Connect timeout applied to the pool (MySqlConnector `Connection Timeout`).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How the `SSL` XML value maps onto TLS behaviour. MySqlConnector treats
/// `SSL=False` as "no TLS" and `SSL=True` as "Preferred".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SslMode {
    Disabled,
    Preferred,
    Required,
}

impl SslMode {
    /// Parse an XML setting value: `True`/`False` (RadioDJ) plus the explicit
    /// MySqlConnector mode names for convenience.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "preferred" => Some(SslMode::Preferred),
            "false" | "0" | "no" | "none" | "disabled" => Some(SslMode::Disabled),
            "required" => Some(SslMode::Required),
            _ => None,
        }
    }

    pub fn to_sqlx(self) -> MySqlSslMode {
        match self {
            SslMode::Disabled => MySqlSslMode::Disabled,
            SslMode::Preferred => MySqlSslMode::Preferred,
            SslMode::Required => MySqlSslMode::Required,
        }
    }
}

/// Database connection + pool settings for the radiodj3 schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbConfig {
    /// `DbServer`: host only; [`DbConfig::set_server`] splits `host:port`.
    pub host: String,
    pub port: u16,
    /// `DbName`
    pub database: String,
    /// `DbUsername`
    pub username: String,
    /// `DbPass`
    pub password: String,
    /// `SSL`
    pub ssl: SslMode,
    /// Session charset, always `utf8mb4` for this schema.
    pub charset: String,
    /// Pool `acquire_timeout` (also doubles MySqlConnector's connect timeout).
    pub connect_timeout: Duration,
    pub max_connections: u32,
    pub min_connections: u32,
}

impl Default for DbConfig {
    fn default() -> Self {
        DbConfig {
            host: "127.0.0.1".into(),
            port: DEFAULT_PORT,
            database: DEFAULT_DATABASE.into(),
            username: "root".into(),
            password: String::new(),
            ssl: SslMode::Disabled,
            charset: DEFAULT_CHARSET.into(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            max_connections: 10,
            min_connections: 0,
        }
    }
}

impl DbConfig {
    pub fn new(server: impl AsRef<str>, database: impl Into<String>) -> Self {
        let mut cfg = DbConfig {
            database: database.into(),
            ..DbConfig::default()
        };
        cfg.set_server(server);
        cfg
    }

    /// Assign `DbServer`, accepting `host`, `host:port`, or a bare IPv6 host.
    pub fn set_server(&mut self, server: impl AsRef<str>) {
        let server = server.as_ref().trim().to_string();
        if let Some((host, port)) = split_host_port(&server) {
            self.host = host;
            self.port = port;
        } else {
            self.host = server;
            self.port = DEFAULT_PORT;
        }
    }

    /// Build a config from the XML settings pairs (all five keys are optional;
    /// missing keys keep the defaults). Unrecognised keys are ignored, matching
    /// RadioDJ's tolerant `GetSetting` behaviour.
    pub fn from_xml_pairs<'a>(
        pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> DbConfig {
        let mut cfg = DbConfig::default();
        for (key, value) in pairs {
            match key {
                XML_KEY_SERVER => cfg.set_server(value),
                XML_KEY_DATABASE => cfg.database = value.to_string(),
                XML_KEY_USERNAME => cfg.username = value.to_string(),
                XML_KEY_PASSWORD => cfg.password = value.to_string(),
                XML_KEY_SSL => {
                    if let Some(mode) = SslMode::parse(value) {
                        cfg.ssl = mode;
                    }
                }
                _ => {}
            }
        }
        cfg
    }

    /// MySqlConnector-style connection string. Query parameters mirror
    /// MySqlConnector keywords (`connect-timeout` = Connection Timeout,
    /// `ssl-mode` = SslMode); the sqlx pool applies the timeout separately via
    /// [`DbConfig::pool_options`].
    pub fn connect_url(&self) -> String {
        let mut url = String::from("mysql://");
        url.push_str(&percent_encode(&self.username));
        url.push(':');
        url.push_str(&percent_encode(&self.password));
        url.push('@');
        if self.host.contains('/') || self.host.contains(':') {
            // IPv6 literal or socket path: keep as-is (sqlx parses both).
            url.push_str(&self.host);
        } else {
            url.push_str(&percent_encode(&self.host));
        }
        url.push(':');
        url.push_str(&self.port.to_string());
        url.push('/');
        url.push_str(&percent_encode(&self.database));
        url.push_str(&format!(
            "?charset={}&connect-timeout={}&ssl-mode={}",
            self.charset,
            self.connect_timeout.as_secs(),
            match self.ssl {
                SslMode::Disabled => "disabled",
                SslMode::Preferred => "preferred",
                SslMode::Required => "required",
            }
        ));
        url
    }

    /// sqlx connect options for this configuration.
    pub fn connect_options(&self) -> Result<MySqlConnectOptions> {
        let opts = MySqlConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .username(&self.username)
            .password(&self.password)
            .database(&self.database)
            .charset(&self.charset)
            .ssl_mode(self.ssl.to_sqlx());
        Ok(opts)
    }

    /// Pool builder with the connect timeout applied as `acquire_timeout`.
    pub fn pool_options(&self) -> sqlx::pool::PoolOptions<sqlx::MySql> {
        sqlx::pool::PoolOptions::<sqlx::MySql>::new()
            .max_connections(self.max_connections)
            .min_connections(self.min_connections)
            .acquire_timeout(self.connect_timeout)
    }

    /// Convenience: parse [`DbConfig::connect_url`] into options (external
    /// tooling parity check; production paths use [`DbConfig::connect_options`]).
    pub fn connect_options_from_url(&self) -> Result<MySqlConnectOptions> {
        MySqlConnectOptions::from_str(&self.connect_url())
            .map_err(|e| DbError::Other(anyhow::anyhow!("invalid connect URL: {e}")))
    }
}

/// Split `host:port` (ignoring a bare IPv6 address without brackets).
fn split_host_port(s: &str) -> Option<(String, u16)> {
    let (host, rest) = s.rsplit_once(':')?;
    if host.contains(':') {
        // IPv6 literal such as `::1` or `fe80::1` — not host:port.
        return None;
    }
    let port: u16 = rest.parse().ok()?;
    Some((host.to_string(), port))
}

/// Percent-encode the reserved characters in URL userinfo/database components.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => out.push(ch),
            // Reserved sets: everything sqlx/URL parsers would choke on.
            ' ' | '"' | '<' | '>' | '#' | '%' | '?' | '/' | ':' | '@' | '[' | ']' | '&' | '='
            | ';' | ',' | '$' | '+' | '!' => {
                out.push('%');
                for byte in ch.encode_utf8(&mut [0u8; 4]).as_bytes() {
                    out.push_str(&format!("{byte:02X}"));
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_pairs() -> Vec<(&'static str, &'static str)> {
        // Values straight from reference/settings/settings_database.xml.
        vec![
            (XML_KEY_SERVER, "127.0.0.1"),
            (XML_KEY_DATABASE, "radiodj3"),
            (XML_KEY_USERNAME, "root"),
            (XML_KEY_PASSWORD, "changeme"),
            (XML_KEY_SSL, "False"),
            ("dbVersion", "2006"),
            ("AutoBackupHour", "22"),
        ]
    }

    #[test]
    fn xml_pairs_map_to_defaults() {
        let cfg = DbConfig::from_xml_pairs(reference_pairs());
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(cfg.database, "radiodj3");
        assert_eq!(cfg.username, "root");
        assert_eq!(cfg.password, "changeme");
        assert_eq!(cfg.ssl, SslMode::Disabled);
    }

    #[test]
    fn ssl_true_maps_to_preferred() {
        let cfg = DbConfig::from_xml_pairs([(XML_KEY_SSL, "True")]);
        assert_eq!(cfg.ssl, SslMode::Preferred);
    }

    #[test]
    fn server_with_port() {
        let cfg = DbConfig::from_xml_pairs([(XML_KEY_SERVER, "db.lan:3307")]);
        assert_eq!(cfg.host, "db.lan");
        assert_eq!(cfg.port, 3307);
    }

    #[test]
    fn ipv6_server_not_split_as_host_port() {
        let cfg = DbConfig::new("::1", "radiodj3");
        assert_eq!(cfg.host, "::1");
        assert_eq!(cfg.port, DEFAULT_PORT);
    }

    #[test]
    fn connect_url_matches_connector_layout() {
        let cfg = DbConfig::from_xml_pairs(reference_pairs());
        assert_eq!(
            cfg.connect_url(),
            "mysql://root:changeme@127.0.0.1:3306/radiodj3?charset=utf8mb4&connect-timeout=15&ssl-mode=disabled"
        );
    }

    #[test]
    fn connect_url_encodes_credentials() {
        let mut cfg = DbConfig::new("localhost", "radiodj3");
        cfg.username = "radio dj".into();
        cfg.password = "p@ss:word/1".into();
        cfg.ssl = SslMode::Required;
        let url = cfg.connect_url();
        assert!(
            url.starts_with("mysql://radio%20dj:p%40ss%3Aword%2F1@localhost:3306/radiodj3?"),
            "{url}"
        );
        assert!(url.ends_with("ssl-mode=required"));
    }

    #[test]
    fn options_from_struct() {
        let cfg = DbConfig::from_xml_pairs(reference_pairs());
        let opts = cfg.connect_options().unwrap();
        let rendered = format!("{opts:?}");
        assert!(rendered.contains("127.0.0.1"), "{rendered}");
        assert!(rendered.contains("radiodj3"), "{rendered}");
    }
}
