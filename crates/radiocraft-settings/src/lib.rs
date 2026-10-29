//! `radiocraft-settings` — XML settings persistence mirroring RadioDJ's
//! `settings_*.xml` / `Plugins/Plugin_*.xml` / language file layout.
//!
//! All RadioDJ configuration files share one .NET `DataSet` XML shape, handled
//! by [`kvstore::KvStore`]; typed structs over that store cover the known keys
//! with the documented defaults:
//!
//! * [`general::GeneralSettings`] — `settings_general.xml`
//! * [`database::DatabaseSettings`] — `settings_database.xml`
//! * [`soundcards::SoundcardSettings`] — `settings_soundcards.xml`
//! * [`titles::TitlesSettings`] — `settings_titles.xml`
//! * [`plugin::PluginSettings`] — per-plugin configuration
//! * [`language::LanguageTable`] — localized string tables

#[macro_use]
mod kvstore;

pub mod database;
pub mod error;
pub mod general;
pub mod language;
pub mod plugin;
pub mod soundcards;
pub mod titles;

pub use database::DatabaseSettings;
pub use error::Error;
pub use general::GeneralSettings;
pub use kvstore::{KvStore, SettingValue};
pub use language::LanguageTable;
pub use plugin::PluginSettings;
pub use soundcards::SoundcardSettings;
pub use titles::TitlesSettings;
