//! Language string tables (`Languages/<Language Name>/*.xml`) — docs/02 §1.
//!
//! Each localized language is a folder whose XML files use the same
//! `NewDataSet/Settings/Key/Value` layout as the settings files. Every file
//! defines a `Culture` key (e.g. `en-US`), and the remaining rows are the
//! UI string table for that form/module. The reference app loads them into a
//! case-insensitive string table keyed by the row key.

use std::collections::HashMap;
use std::path::Path;

use crate::error::Error;
use crate::kvstore::KvStore;

/// One loaded language: its culture tag plus the merged string table of every
/// form file in the language folder.
#[derive(Debug, Clone)]
pub struct LanguageTable {
    culture: String,
    strings: HashMap<String, String>,
}

impl LanguageTable {
    /// Load and merge every `*.xml` in a language folder (e.g.
    /// `Languages/English (United States)`). Files are read in sorted order;
    /// later files do not override earlier duplicate keys.
    pub fn load_dir(dir: impl AsRef<Path>) -> Result<Self, Error> {
        let dir = dir.as_ref();
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| Error::io(dir, e))?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension()?.eq_ignore_ascii_case("xml") {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();
        files.sort();

        let mut table = LanguageTable {
            culture: String::new(),
            strings: HashMap::new(),
        };
        for file in &files {
            let store = KvStore::load(file)?;
            table.merge(store);
        }
        Ok(table)
    }

    /// Load a single language file.
    pub fn load_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let store = KvStore::load(path)?;
        let mut table = LanguageTable {
            culture: String::new(),
            strings: HashMap::new(),
        };
        table.merge(store);
        Ok(table)
    }

    fn merge(&mut self, store: KvStore) {
        for (key, value) in store.entries() {
            if key == "Culture" {
                if self.culture.is_empty() {
                    self.culture = value.clone();
                }
            } else {
                self.strings
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }

    /// The `Culture` tag declared by the files (e.g. `en-US`).
    pub fn culture(&self) -> &str {
        &self.culture
    }

    /// Look up a string by its exact key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.strings.get(key).map(String::as_str)
    }

    /// Translate with fallback: the key itself when it has no entry, matching
    /// RadioDJ's behaviour of showing the resource key when missing.
    pub fn tr<'a>(&'a self, key: &'a str) -> &'a str {
        match self.strings.get(key) {
            Some(v) => v.as_str(),
            None => key,
        }
    }

    /// All string keys (unordered).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.strings.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERAL: &str = "\u{feff}\
<?xml version=\"1.0\" standalone=\"yes\"?>
<NewDataSet>
  <Settings>
    <Key>Culture</Key>
    <Value>en-US</Value>
  </Settings>
  <Settings>
    <Key>Path</Key>
    <Value>Path/URL</Value>
  </Settings>
  <Settings>
    <Key>Artist</Key>
    <Value>Artist</Value>
  </Settings>
</NewDataSet>";

    const MAIN: &str = "\
<?xml version=\"1.0\" standalone=\"yes\"?>
<NewDataSet>
  <Settings>
    <Key>Culture</Key>
    <Value>en-US</Value>
  </Settings>
  <Settings>
    <Key>Play</Key>
    <Value>Play</Value>
  </Settings>
</NewDataSet>";

    #[test]
    fn load_file_skips_bom_and_culture_row() {
        let dir = std::env::temp_dir().join("rc-lang-test-fileread");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("General.xml");
        std::fs::write(&f, GENERAL).unwrap();
        let t = LanguageTable::load_file(&f).unwrap();
        assert_eq!(t.culture(), "en-US");
        assert_eq!(t.get("Path"), Some("Path/URL"));
        assert_eq!(t.get("Artist"), Some("Artist"));
        assert_eq!(t.tr("Play"), "Play"); // fallback = key
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_dir_merges_forms() {
        let dir = std::env::temp_dir().join("rc-lang-test-merge");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("General.xml"), GENERAL).unwrap();
        std::fs::write(dir.join("Main.xml"), MAIN).unwrap();
        let t = LanguageTable::load_dir(&dir).unwrap();
        assert_eq!(t.culture(), "en-US");
        assert_eq!(t.get("Path"), Some("Path/URL"));
        assert_eq!(t.get("Play"), Some("Play"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
