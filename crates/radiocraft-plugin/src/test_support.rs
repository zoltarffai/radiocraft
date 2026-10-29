//! Shared test scaffolding: a simple host that records debug output.

use std::path::PathBuf;

use radiocraft_core::model::SongData;

use crate::host::{EventAction, Host, PluginUiRequest, StateInfo};

/// Minimal host: records `debug_add_line` entries and UI requests, everything
/// else uses trait defaults.
#[derive(Debug, Default)]
pub struct TestHost {
    pub debug_lines: std::cell::RefCell<Vec<String>>,
    pub ui_requests: std::cell::RefCell<Vec<PluginUiRequest>>,
    pub settings: std::cell::RefCell<std::collections::BTreeMap<(String, String), String>>,
    pub playlist: std::cell::RefCell<Vec<SongData>>,
}

impl TestHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take_debug_lines(&self) -> Vec<String> {
        self.debug_lines.borrow_mut().drain(..).collect()
    }
}

impl Host for TestHost {
    fn debug_add_line(&self, entry: &str) {
        self.debug_lines.borrow_mut().push(entry.to_string());
    }

    fn show_ui(&self, request: PluginUiRequest) {
        self.ui_requests.borrow_mut().push(request);
    }

    fn get_setting(&self, sender: &str, key: &str, default: &str) -> String {
        self.settings
            .borrow()
            .get(&(sender.to_string(), key.to_string()))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    fn save_setting(&self, sender: &str, key: &str, value: &str) -> bool {
        self.settings
            .borrow_mut()
            .insert((sender.to_string(), key.to_string()), value.to_string());
        true
    }

    fn main_playlist(&self) -> Vec<SongData> {
        self.playlist.borrow().clone()
    }

    fn set_main_playlist(&self, playlist: &[SongData]) {
        *self.playlist.borrow_mut() = playlist.to_vec();
    }

    fn application_path(&self) -> PathBuf {
        PathBuf::from("/test/radiocraft")
    }

    fn get_plugin_actions(&self) -> Vec<EventAction> {
        Vec::new()
    }
}
