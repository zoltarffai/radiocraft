//! RadioCraft plugin boundary: the host-facing `Host` trait, the plugin-side
//! `Plugin` trait, a C ABI bridge shaped after RadioDJ's `rdjInterface`,
//! and a dynamic loader with a plugin registry.

pub mod abi;
pub mod host;
pub mod hostwire;
pub mod loader;
pub mod plugin;
pub mod pluginabi;

pub use abi::{ABI_VERSION, HostBridge, HostVtable};
pub use host::{EventAction, Host, PluginUiRequest, StateChange, StateInfo};
pub use hostwire::HostHandle;
pub use loader::{DispatchError, LoadedPlugin, LoaderError, PluginRegistry};
pub use plugin::{Plugin, PluginInfo};
pub use pluginabi::{NativePlugin, PluginVtable};

// `export_plugin!` is re-exported at crate root via #[macro_export].

/// A simple `Host` implementation recording debug lines, useful for plugin
/// unit/integration tests and host-side testing.
#[cfg(any(test, feature = "test-util"))]
pub mod test_support;
