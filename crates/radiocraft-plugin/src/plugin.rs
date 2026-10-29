//! Plugin-side API: the Rust port of what a RadioDJ-style plugin must provide.
//!
//! The original contract is the `rdjInterface.IPlugin`-style surface observed
//! in plugin IL: each plugin exposes its metadata, an init hook wired to the
//! [`Host`], an action list (`GetPluginActions` on the host side enumerates the
//! same list), an action runner, and a state-change sink.

use serde::{Deserialize, Serialize};

use crate::host::{EventAction, Host, StateInfo};

/// Static plugin metadata, port of the plugin `PluginInfo`/`plugin.ini` trio
/// (name, version, description) the original event editor displays.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
}

/// A RadioCraft plugin. All callbacks are `&mut self` (single-threaded
/// dispatch, like the original STA plugin host).
pub trait Plugin {
    /// Plugin name/version/description.
    fn info(&self) -> PluginInfo;

    /// Called once after load; `host` is valid for the lifetime of the plugin
    /// (keep a clone of it if needed across callbacks).
    fn init(&mut self, host: &dyn Host);

    /// Actions this plugin exposes to the event runner
    /// (`GetPluginActions` on the host side returns these).
    fn actions(&self) -> Vec<EventAction>;

    /// Run one exposed action (`RunPluginAction(EventAction, string[] args)`).
    fn run_action(&mut self, action: &EventAction, args: &[String]);

    /// Receive state notifications forwarded by the host
    /// (`StateChanged(Features, object)`).
    fn on_state_changed(&mut self, state: &StateInfo);
}
