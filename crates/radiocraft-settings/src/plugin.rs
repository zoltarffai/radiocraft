//! `Plugins/Plugin_*.xml` — per-plugin configuration files (docs/03 §3).
//!
//! Every plugin settings file uses the same `NewDataSet/Settings/Key/Value`
//! layout as the main settings files. Unlike the typed main settings, plugin
//! keys vary per plugin and are not fixed at compile time (the reference app
//! reads them with `Convert.ToInt32(...)` style helpers), so this module wraps
//! [`KvStore`] with typed lookup helpers instead of a generated struct.

use std::fmt;

use crate::error::Error;
use crate::kvstore::{KvStore, SettingValue};

/// A loaded plugin settings file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginSettings {
    /// Where the file was loaded from (kept so `save` can rewrite in place).
    path: Option<std::path::PathBuf>,
    store: KvStore,
}

impl PluginSettings {
    /// Read `Plugins/Plugin_<name>.xml`.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        Ok(PluginSettings {
            path: Some(path.to_path_buf()),
            store: KvStore::load(path)?,
        })
    }

    /// Parse from XML text without a file.
    pub fn parse(xml: &str) -> Result<Self, Error> {
        Ok(PluginSettings {
            path: None,
            store: KvStore::parse(xml)?,
        })
    }

    /// An empty in-memory plugin store.
    pub fn new() -> Self {
        PluginSettings::default()
    }

    /// Write back to the original path.
    pub fn save(&self) -> Result<(), Error> {
        let path = self.path.as_ref().ok_or(Error::InvalidOperation(
            "plugin settings were not loaded from a file; use save_at",
        ))?;
        self.store.save(path)
    }

    /// Write to an explicit path and remember it as the default save target.
    pub fn save_at(&mut self, path: impl AsRef<std::path::Path>) -> Result<(), Error> {
        let path = path.as_ref();
        self.store.save(path)?;
        self.path = Some(path.to_path_buf());
        Ok(())
    }

    /// The underlying store (read-only access to all rows).
    pub fn store(&self) -> &KvStore {
        &self.store
    }

    /// Mutable access to the underlying store.
    pub fn store_mut(&mut self) -> &mut KvStore {
        &mut self.store
    }

    /// Raw string lookup.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.store.get(key)
    }

    /// Typed lookup with a fallback (mirrors the plugin C# idiom
    /// `int.TryParse(GetSetting(...), out v) ? v : fallback`).
    pub fn get_as<T: SettingValue>(&self, key: &str, default: T) -> T {
        self.store
            .get(key)
            .and_then(T::parse_opt)
            .unwrap_or(default)
    }

    /// Set a value as a raw string.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        self.store.set(key, value);
    }

    /// Set a value with typed rendering (`true` -> `True`).
    pub fn set_typed<T: SettingValue>(&mut self, key: impl Into<String>, value: T) {
        let key = key.into();
        self.store.set(key, value.format());
    }

    /// Remove a key.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.store.remove(key)
    }

    /// True when the key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.store.contains(key)
    }

    /// All keys in document order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.store.keys()
    }
}

impl fmt::Display for PluginSettings {
    /// Serialize to the DataSet XML layout.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
<?xml version=\"1.0\" standalone=\"yes\"?>
<NewDataSet>
  <Settings>
    <Key>TargetPath</Key>
    <Value>C:\\RadioDJv3\\Setup\\NowPlaying.txt</Value>
  </Settings>
  <Settings>
    <Key>FileEncoding</Key>
    <Value>2</Value>
  </Settings>
  <Settings>
    <Key>Export2File</Key>
    <Value>True</Value>
  </Settings>
  <Settings>
    <Key>Export2Web</Key>
    <Value>False</Value>
  </Settings>
</NewDataSet>";

    #[test]
    fn plugin_store_typed_accessors() {
        let p = PluginSettings::parse(FIXTURE).unwrap();
        assert_eq!(p.get("FileEncoding"), Some("2"));
        assert_eq!(p.get_as("FileEncoding", 0i32), 2);
        assert!(!p.get_as("Export2Web", true));
        assert!(p.get_as("Export2File", false));
        assert_eq!(p.get_as("MissingKey", 7i32), 7);
        assert!(p.contains("TargetPath"));
        assert!(!p.contains("Nope"));
    }

    #[test]
    fn plugin_round_trip_and_set() {
        let mut p = PluginSettings::parse(FIXTURE).unwrap();
        p.set_typed("Export2Web", true);
        p.set("TargetPort", "NO SERIAL PORT");
        p.remove("FileEncoding");
        let text = p.to_string();
        let p2 = PluginSettings::parse(&text).unwrap();
        assert_eq!(p, p2);
        assert!(p2.get_as("Export2Web", false));
        assert_eq!(p2.get("TargetPort"), Some("NO SERIAL PORT"));
        assert_eq!(p2.get("FileEncoding"), None);
        assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>"));
    }
}
