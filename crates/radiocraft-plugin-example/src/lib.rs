//! Example RadioCraft plugin (cdylib).
//!
//! Doubles as the end-to-end fixture for the plugin ABI: the server's
//! `plugin_dylib_e2e` test loads the built `libradiocraft_plugin_example.so`
//! through `PluginManager` and drives the actions below through the real
//! host shim.
//!
//! Actions:
//! - `Echo (Text)` — writes the argument back through the host debug log
//!   (which lands in the server status line).
//! - `Remember (Key, Value)` / `Recall (Key)` — round-trip the plugin's own
//!   config file (`Plugin_Example.xml`) through the host settings API.
//! - `CountStates` — reports how many host state changes the plugin saw.

use std::cell::RefCell;

use radiocraft_plugin::abi::HostBridge;
use radiocraft_plugin::host::{EventAction, Host, StateInfo};
use radiocraft_plugin::hostwire::HostHandle;
use radiocraft_plugin::plugin::{Plugin, PluginInfo};
use radiocraft_plugin::{export_plugin, host};

const CONFIG_FILE: &str = "Plugin_Example.xml";

#[derive(Default)]
struct ExamplePlugin {
    host: Option<HostBridge>,
    states: RefCell<usize>,
}

impl Plugin for ExamplePlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "Example".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Reference plugin / ABI fixture".into(),
        }
    }

    fn init(&mut self, host_impl: &dyn Host) {
        // Keep a bridge to the host for the rest of our lifetime; the host
        // shim is guaranteed to outlive the plugin instance.
        let handle = HostHandle::new(host_impl);
        self.host = Some(unsafe { HostBridge::new(handle.vtable()) });
    }

    fn actions(&self) -> Vec<EventAction> {
        vec![
            EventAction {
                name: "Echo".into(),
                id: 1,
                args: vec!["Text".into()],
            },
            EventAction {
                name: "Remember".into(),
                id: 2,
                args: vec!["Key".into(), "Value".into()],
            },
            EventAction {
                name: "Recall".into(),
                id: 3,
                args: vec!["Key".into()],
            },
            EventAction {
                name: "CountStates".into(),
                id: 4,
                args: vec![],
            },
        ]
    }

    fn run_action(&mut self, action: &EventAction, args: &[String]) {
        let Some(host_impl) = self.host.as_ref() else {
            return;
        };
        match action.name.as_str() {
            "Echo" => host_impl.debug_add_line(&format!(
                "example echo: {}",
                args.first().cloned().unwrap_or_default()
            )),
            "Remember" => {
                let key = args.first().cloned().unwrap_or_default();
                let value = args.get(1).cloned().unwrap_or_default();
                host_impl.save_setting(CONFIG_FILE, &key, &value);
                host_impl.debug_add_line(&format!("example remembered {key}={value}"));
            }
            "Recall" => {
                let key = args.first().cloned().unwrap_or_default();
                let value = host_impl.get_setting(CONFIG_FILE, &key, "");
                host_impl.debug_add_line(&format!("example recall {key}={value}"));
            }
            "CountStates" => {
                let n = *self.states.borrow();
                host_impl.debug_add_line(&format!("example states={n}"));
            }
            other => host_impl.debug_add_line(&format!("example unknown action {other}")),
        }
    }

    fn on_state_changed(&mut self, _state: &StateInfo) {
        *self.states.borrow_mut() += 1;
    }
}

export_plugin!(ExamplePlugin);

// Silence an unused-import lint only triggered when `host::` paths are
// referenced indirectly by macro expansion in some toolchains.
const _: fn() = || {
    let _ = std::mem::size_of::<host::StateChange>();
};
