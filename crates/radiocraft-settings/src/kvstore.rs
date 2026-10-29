//! Generic XML key/value settings store.
//!
//! RadioDJ persists every settings file (`settings_general.xml`,
//! `settings_database.xml`, `settings_soundcards.xml`, `settings_titles.xml`,
//! `Plugins/Plugin_*.xml`, language string tables, ...) with the same
//! .NET `DataSet`-XML layout:
//!
//! ```xml
//! <?xml version="1.0" standalone="yes"?>
//! <NewDataSet>
//!   <Settings>
//!     <Key>Priority</Key>
//!     <Value>2</Value>
//!   </Settings>
//!   <Settings>
//!     <Key>LastUsedPath</Key>
//!     <Value />
//!   </Settings>
//! </NewDataSet>
//! ```
//!
//! [`KvStore`] reads that layout into an ordered list of `(Key, Value)` pairs
//! and writes it back with the same layout (UTF-8, `standalone="yes"`,
//! self-closed empty values), so loading and saving round-trips byte-for-byte
//! for files this crate produces.

use std::path::Path;

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Reader;
use quick_xml::Writer;

use crate::error::Error;

/// An ordered key/value settings document (duplicate keys are last-wins).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KvStore {
    entries: Vec<(String, String)>,
}

impl KvStore {
    pub fn new() -> Self {
        KvStore::default()
    }

    /// Parse the `NewDataSet/Settings/Key/Value` layout from XML text.
    pub fn parse(xml: &str) -> Result<KvStore, Error> {
        let path = std::path::PathBuf::from("<string>");
        let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);

        let mut store = KvStore::new();
        // Parser state for one <Settings> row.
        let mut cur_key: Option<String> = None;
        let mut cur_value: Option<String> = None;
        let mut in_settings = false;

        loop {
            match reader.read_event() {
                Ok(Event::Eof) => break,
                Ok(Event::Start(e)) => {
                    let name = e.local_name();
                    if name.as_ref() == b"Settings" {
                        in_settings = true;
                        cur_key = None;
                        cur_value = None;
                    } else if in_settings && (name.as_ref() == b"Key" || name.as_ref() == b"Value")
                    {
                        // Text child follows; capture the raw content.
                        let want_key = name.as_ref() == b"Key";
                        match reader.read_event() {
                            Ok(Event::Text(t)) => {
                                let v = t.unescape().map_err(|e| {
                                    Error::xml(&path, format!("cannot unescape text: {e}"))
                                })?;
                                if want_key {
                                    cur_key = Some(v.into_owned());
                                } else {
                                    cur_value = Some(v.into_owned());
                                }
                            }
                            Ok(Event::Empty(_)) => {
                                // `<Key/>` / `<Value />`: empty content.
                                if want_key {
                                    cur_key = Some(String::new());
                                } else {
                                    cur_value = Some(String::new());
                                }
                            }
                            Ok(_) => {
                                return Err(Error::xml(
                                    &path,
                                    format!(
                                        "unexpected event inside <{}>",
                                        if want_key { "Key" } else { "Value" }
                                    ),
                                ));
                            }
                            Err(e) => return Err(Error::xml(&path, e.to_string())),
                        }
                    }
                }
                Ok(Event::Empty(e)) if e.local_name().as_ref() == b"Settings" => {
                    // `<Settings />`: empty row, ignored.
                }
                Ok(Event::End(e)) if e.local_name().as_ref() == b"Settings" => {
                    in_settings = false;
                    if let Some(key) = cur_key.take() {
                        let value = cur_value.take().unwrap_or_default();
                        store.set(key, value);
                    }
                }
                Ok(_) => {}
                Err(e) => return Err(Error::xml(&path, e.to_string())),
            }
        }
        Ok(store)
    }

    /// Serialize back to the RadioDJ DataSet layout as bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new(Vec::new());
        // Exactly the declaration RadioDJ's DataSet.WriteXml emits.
        writer
            .write_event(Event::Decl(BytesDecl::new(
                "1.0",
                Some("utf-8"),
                Some("yes"),
            )))
            .expect("writing to Vec<u8> is infallible");
        write_break(&mut writer);
        writer
            .write_event(Event::Start(BytesStart::new("NewDataSet")))
            .expect("writing to Vec<u8> is infallible");
        for (key, value) in &self.entries {
            write_break(&mut writer);
            writer
                .write_event(Event::Start(BytesStart::new("Settings")))
                .expect("writing to Vec<u8> is infallible");
            write_break(&mut writer);
            write_text_element(&mut writer, "Key", key);
            write_break(&mut writer);
            write_text_element(&mut writer, "Value", value);
            write_break(&mut writer);
            writer
                .write_event(Event::End(BytesEnd::new("Settings")))
                .expect("writing to Vec<u8> is infallible");
        }
        write_break(&mut writer);
        writer
            .write_event(Event::End(BytesEnd::new("NewDataSet")))
            .expect("writing to Vec<u8> is infallible");
        writer.into_inner()
    }

    /// Read a settings file from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<KvStore, Error> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        KvStore::parse(&text).map_err(|e| match e {
            Error::Xml { .. } => Error::xml(path, e.to_string()),
            other => other,
        })
    }

    /// Write a settings file to disk.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), crate::error::Error> {
        let path = path.as_ref();
        std::fs::write(path, self.to_bytes()).map_err(|e| Error::io(path, e))
    }

    /// Lookup a key (exact, case-sensitive).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Typed lookup for any [`SettingValue`].
    pub fn get_as<T: SettingValue>(&self, key: &str) -> Option<T> {
        self.get(key).and_then(T::parse_opt)
    }

    /// Set a key: update in place or append, keeping the file's key order.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        if let Some(idx) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries[idx].1 = value;
        } else {
            self.entries.push((key, value));
        }
    }

    /// Remove a key, returning its value when present.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        let idx = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(idx).1)
    }

    /// True when the key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// All pairs in document order.
    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }

    /// All keys in document order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl std::fmt::Display for KvStore {
    /// Serialize back to the RadioDJ DataSet layout.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let xml =
            String::from_utf8(Self::to_bytes(self)).expect("XML writer always produces UTF-8");
        f.write_str(&xml)
    }
}

fn write_break(writer: &mut Writer<Vec<u8>>) {
    std::io::Write::write_all(writer.get_mut(), b"\n").expect("writing to Vec<u8> is infallible");
}

fn write_text_element(writer: &mut Writer<Vec<u8>>, name: &str, content: &str) {
    if content.is_empty() {
        // RadioDJ (DataSet.WriteXml) writes empty values self-closed as `<Value />`.
        std::io::Write::write_all(writer.get_mut(), format!("<{name} />").as_bytes())
            .expect("writing to Vec<u8> is infallible");
    } else {
        let start = BytesStart::new(name);
        writer
            .write_event(Event::Start(start))
            .expect("writing to Vec<u8> is infallible");
        let escaped = escape_xml(content);
        writer
            .write_event(Event::Text(BytesText::from_escaped(escaped)))
            .expect("writing to Vec<u8> is infallible");
        writer
            .write_event(Event::End(BytesEnd::new(name)))
            .expect("writing to Vec<u8> is infallible");
    }
}

/// Escape text content for XML (`&`, `<`, `>`).
fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Values that can appear in a settings file (`True`/`False` booleans, plain
/// decimal numbers, raw strings).
pub trait SettingValue: Sized {
    /// Parse leniently: unknown/invalid input yields `None` (callers keep the
    /// documented default in that case).
    fn parse_opt(raw: &str) -> Option<Self>;
    /// Render back to the file representation.
    fn format(&self) -> String;
}

impl SettingValue for String {
    fn parse_opt(raw: &str) -> Option<Self> {
        Some(raw.to_string())
    }
    fn format(&self) -> String {
        self.clone()
    }
}

impl SettingValue for bool {
    fn parse_opt(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        }
    }
    fn format(&self) -> String {
        // RadioDJ (C#) writes `True`/`False` capitalized.
        if *self {
            "True".to_string()
        } else {
            "False".to_string()
        }
    }
}

macro_rules! impl_setting_value_numeric {
    ($($t:ty),*) => {
        $(
            impl SettingValue for $t {
                fn parse_opt(raw: &str) -> Option<Self> {
                    raw.trim().parse::<Self>().ok()
                }
                fn format(&self) -> String {
                    self.to_string()
                }
            }
        )*
    };
}

impl_setting_value_numeric!(i8, i16, i32, i64, u8, u16, u32, u64, f32, f64);

/// Declares a typed settings struct over a [`KvStore`].
///
/// ```ignore
/// settings_struct! {
///     /// General application settings (`settings_general.xml`).
///     pub struct Example {
///         "Priority" => priority: i32 = (2),
///         "AutoPlay" => auto_play: bool = (false),
///         "StationName" => station_name: String = ("RadioDJ"),
///         "ThresholdStart" => threshold_start: i32 = (-25),
///     }
/// }
/// ```
///
/// Generates private fields plus typed accessors (`priority()`,
/// `set_priority(v)`), `from_kv`/`to_kv` (defaults for missing or unparsable
/// values), `default()` from the declared defaults, unknown-key passthrough
/// (`extras()`), and raw `get_raw`/`set_raw` accessors for the host API
/// (`GetSetting`/`SaveSetting`).
macro_rules! settings_struct {
    (
        $(#[$outer:meta])*
        $vis:vis struct $name:ident {
            $( $(#[$field_attr:meta])? $key:literal => $field:ident : $ty:ty = $default:tt , )*
        }
    ) => {
        #[derive(Debug, Clone, PartialEq)]
        $(#[$outer])*
        $vis struct $name {
            $( $(#[$field_attr])? $field: $ty , )*
            /// Keys not modeled by this struct, preserved for round-trips.
            extras: Vec<(String, String)>,
        }

        impl Default for $name {
            fn default() -> Self {
                Self { $( $field: $default.into() , )* extras: Vec::new() }
            }
        }

        paste::paste! {
            impl $name {
                /// Known keys in file order.
                pub const KEYS: &'static [&'static str] = &[$($key),*];

                $(
                    /// Typed getter.
                    pub fn $field(&self) -> &$ty {
                        &self.$field
                    }

                    /// Typed setter.
                    pub fn [<set_ $field>](&mut self, value: $ty) {
                        self.$field = value;
                    }
                )*

            /// Build from a parsed store, applying declared defaults.
            pub fn from_kv(store: &crate::kvstore::KvStore) -> Self {
                let mut out = Self::default();
                $(
                    if let Some(raw) = store.get($key) {
                        if let Some(v) = <$ty as crate::kvstore::SettingValue>::parse_opt(raw) {
                            out.$field = v;
                        }
                    }
                )*
                out.extras = store
                    .entries()
                    .iter()
                    .filter(|(k, _)| !Self::KEYS.contains(&k.as_str()))
                    .cloned()
                    .collect();
                out
            }

            /// Render back to a store (known keys in declared order, extras after).
            pub fn to_kv(&self) -> crate::kvstore::KvStore {
                let mut store = crate::kvstore::KvStore::new();
                $(
                    store.set($key, crate::kvstore::SettingValue::format(&self.$field));
                )*
                for (k, v) in &self.extras {
                    store.set(k.clone(), v.clone());
                }
                store
            }

            /// Load straight from a settings file.
            pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, crate::error::Error> {
                Ok(Self::from_kv(&crate::kvstore::KvStore::load(path)?))
            }

            /// Save straight to a settings file.
            pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<(), crate::error::Error> {
                self.to_kv().save(path)
            }

            /// Keys this struct does not model (round-trip passthrough).
            pub fn extras(&self) -> &[(String, String)] {
                &self.extras
            }

            /// Add or replace an unmodeled key.
            pub fn set_extra(&mut self, key: impl Into<String>, value: impl Into<String>) {
                let key = key.into();
                let value = value.into();
                if let Some(e) = self.extras.iter_mut().find(|(k, _)| *k == key) {
                    e.1 = value;
                } else {
                    self.extras.push((key, value));
                }
            }

            /// Raw string lookup, mirrors of the host `GetSetting` API.
            pub fn get_raw(&self, key: &str) -> Option<String> {
                match key {
                    $( $key => Some(crate::kvstore::SettingValue::format(&self.$field)), )*
                    _ => self.extras.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
                }
            }

            /// Raw string set, mirrors of the host `SaveSetting` API.
            /// Returns false when `value` cannot be parsed for the key's type.
            pub fn set_raw(&mut self, key: &str, value: &str) -> bool {
                match key {
                    $(
                        $key => {
                            match <$ty as crate::kvstore::SettingValue>::parse_opt(value) {
                                Some(v) => { self.$field = v; true }
                                None => false,
                            }
                        }
                    )*
                    _ => { self.set_extra(key, value); true }
                }
            }
        }
        }
    };
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use super::*;

    settings_struct! {
        /// Test struct covering every declared shape.
        pub struct Sample {
            "Priority" => priority: i32 = (2),
            "AutoPlay" => auto_play: bool = (false),
            "StationName" => station_name: String = ("RadioDJ"),
            "ThresholdStart" => threshold_start: i32 = (-25),
            "ReducePriorityBy" => reduce_priority_by: i32 = (0),
        }
    }

    const FIXTURE: &str = "\
<?xml version=\"1.0\" standalone=\"yes\"?>
<NewDataSet>
  <Settings>
    <Key>Priority</Key>
    <Value>2</Value>
  </Settings>
  <Settings>
    <Key>AutoPlay</Key>
    <Value>False</Value>
  </Settings>
  <Settings>
    <Key>StationName</Key>
    <Value>Radio&#38;DJ &#60;on air&#62;</Value>
  </Settings>
  <Settings>
    <Key>ThresholdStart</Key>
    <Value>-25</Value>
  </Settings>
  <Settings>
    <Key>LastUsedPath</Key>
    <Value />
  </Settings>
</NewDataSet>";

    #[test]
    fn parses_key_value_rows() {
        let store = KvStore::parse(FIXTURE).unwrap();
        assert_eq!(store.get("Priority"), Some("2"));
        assert_eq!(store.get("AutoPlay"), Some("False"));
        assert_eq!(store.get("StationName").unwrap(), "Radio&DJ <on air>");
        assert_eq!(store.get("LastUsedPath"), Some(""));
        assert_eq!(store.get("Missing"), None);
        assert_eq!(store.keys().count(), 5);
    }

    #[test]
    fn kv_round_trip_preserves_order_and_layout() {
        let store = KvStore::parse(FIXTURE).unwrap();
        let text = store.to_string();
        let again = KvStore::parse(&text).unwrap();
        assert_eq!(store, again);
        // Layout markers: declaration, root element, row shape, self-closed empty.
        assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>"));
        assert!(text.contains("<NewDataSet>"));
        assert!(text.contains("<Settings>"));
        assert!(text.contains("<Key>Priority</Key>"));
        assert!(text.contains("<Value />"));
        assert!(!text.contains("<Value></Value>"));
    }

    #[test]
    fn handles_bom_and_empty_document() {
        let store =
            KvStore::parse("\u{feff}<?xml version=\"1.0\" standalone=\"yes\"?>\n<NewDataSet />")
                .unwrap();
        assert!(store.is_empty());
    }

    #[test]
    fn typed_defaults_and_lenient_parsing() {
        // `Priority` is unparsable -> declared default kept.
        let store = KvStore::parse(
            "<NewDataSet><Settings><Key>Priority</Key><Value>abc</Value></Settings>\
             <Settings><Key>ThresholdStart</Key><Value>-25</Value></Settings>\
             <Settings><Key>AutoPlay</Key><Value>True</Value></Settings>\
             <Settings><Key>UnknownKey</Key><Value>x</Value></Settings></NewDataSet>",
        )
        .unwrap();
        let s = Sample::from_kv(&store);
        assert_eq!(*s.priority(), 2);
        assert_eq!(*s.threshold_start(), -25);
        assert!(*s.auto_play());
        assert_eq!(s.extras(), &[("UnknownKey".to_string(), "x".to_string())]);
    }

    #[test]
    fn defaults_match_declared_values() {
        let s = Sample::default();
        assert_eq!(*s.priority(), 2);
        assert!(!s.auto_play());
        assert_eq!(s.station_name(), "RadioDJ");
        assert_eq!(*s.threshold_start(), -25);
    }

    #[test]
    fn typed_struct_round_trip() {
        let store = KvStore::parse(FIXTURE).unwrap();
        let s = Sample::from_kv(&store);
        let text = s.to_kv().to_string();
        let s2 = Sample::from_kv(&KvStore::parse(&text).unwrap());
        assert_eq!(s, s2);
    }

    #[test]
    fn raw_accessors() {
        let mut s = Sample::default();
        assert_eq!(s.get_raw("StationName").as_deref(), Some("RadioDJ"));
        assert!(s.set_raw("Priority", "7"));
        assert_eq!(*s.priority(), 7);
        assert!(!s.set_raw("Priority", "not-num")); // keeps old value, reports failure
        assert_eq!(*s.priority(), 7);
        assert!(s.set_raw("BrandNew", "42"));
        assert_eq!(s.get_raw("BrandNew").as_deref(), Some("42"));
        assert!(s.set_raw("StationName", "X & Y"));
        assert_eq!(s.station_name(), "X & Y");
    }

    #[test]
    fn bool_formats_cstyle() {
        assert_eq!(SettingValue::format(&true), "True");
        assert_eq!(SettingValue::format(&false), "False");
        assert_eq!(bool::parse_opt("true"), Some(true));
        assert_eq!(bool::parse_opt("False"), Some(false));
        assert_eq!(bool::parse_opt("garbage"), None);
    }
}
