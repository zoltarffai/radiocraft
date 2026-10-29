//! Dynamic plugin loading and the plugin registry.
//!
//! A plugin `.cdylib` must export `radiocraft_abi_version` and
//! `radiocraft_plugin_register` (see `pluginabi::export_plugin!`). The loader
//! validates the ABI version before calling register, and wraps the result in
//! a `LoadedPlugin` so the dynamic library outlives the plugin instance.

use std::path::Path;

use libloading::Library;
use thiserror::Error;

use crate::abi::ABI_VERSION;
use crate::plugin::Plugin;
use crate::pluginabi::{NativePlugin, PluginVtable};

/// Errors while loading or resolving a plugin library.
#[derive(Debug, Error)]
pub enum LoaderError {
    #[error("library loading failed: {0}")]
    LoadError(String),
    #[error("required symbol missing: {0}")]
    MissingSymbol(String),
    #[error("ABI version mismatch: host expects {expected}, plugin reports {found}")]
    AbiMismatch { expected: u32, found: u32 },
}

/// A plugin instance together with the dynamic library that backs it.
///
/// The library is dropped last, so plugin code cannot outlive its own image.
pub struct LoadedPlugin<P: Plugin + ?Sized = dyn Plugin> {
    plugin: Box<P>,
    _library: Library,
}

impl LoadedPlugin {
    /// Access the plugin instance.
    pub fn plugin(&self) -> &dyn Plugin {
        self.plugin.as_ref()
    }

    /// Access the plugin instance mutably.
    pub fn plugin_mut(&mut self) -> &mut dyn Plugin {
        self.plugin.as_mut()
    }
}

/// Load a plugin from a dynamic library path and verify its ABI version.
///
/// # Safety
///
/// The library at `path` must uphold the plugin ABI contract: the vtable
/// returned from `radiocraft_plugin_register` must be `Send + Sync` and all
/// its function pointers must honour the documented calling convention.
pub unsafe fn load_plugin<P: AsRef<Path>>(path: P) -> Result<LoadedPlugin, LoaderError> {
    let library = Library::new(path.as_ref()).map_err(|e| LoaderError::LoadError(e.to_string()))?;

    let version = library
        .get::<unsafe extern "C" fn() -> u32>(b"radiocraft_abi_version\0")
        .map_err(|_| {
            LoaderError::MissingSymbol("radiocraft_abi_version".to_string())
        })?;
    // SAFETY: caller guarantees the library implements the plugin ABI.
    let found = unsafe { version() };
    if found != ABI_VERSION {
        return Err(LoaderError::AbiMismatch {
            expected: ABI_VERSION,
            found,
        });
    }

    let register = library
        .get::<unsafe extern "C" fn(*mut *mut PluginVtable) -> i32>(
            b"radiocraft_plugin_register\0",
        )
        .map_err(|_| LoaderError::MissingSymbol("radiocraft_plugin_register".to_string()))?;

    let mut vtable: *mut PluginVtable = std::ptr::null_mut();
    // SAFETY: ABI verified above; &mut of a local null pointer is a valid out
    // slot per the register contract.
    let code = unsafe { register(&mut vtable) };
    if code != crate::abi::REGISTER_OK {
        return Err(LoaderError::LoadError(format!(
            "radiocraft_plugin_register failed with code {code}"
        )));
    }
    if vtable.is_null() {
        return Err(LoaderError::MissingSymbol(
            "register returned a null vtable".to_string(),
        ));
    }

    // SAFETY: caller guarantees the vtable and its slots are valid and the
    // returned instance satisfies Send + Sync.
    let plugin = unsafe { NativePlugin::from_raw(vtable) };

    Ok(LoadedPlugin {
        plugin: Box::new(plugin),
        _library: library,
    })
}

/// Registry of live plugins with an action-name dispatch index.
#[derive(Default)]
pub struct PluginRegistry {
    plugins: Vec<(u64, Box<dyn Plugin>)>,
    next_id: u64,
}

/// Errors from dispatching a plugin action by name.
#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("no plugin action matches {0}")]
    UnknownAction(String),
    #[error("action is ambiguous: {0} matches multiple plugins")]
    Ambiguous(String),
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plugin instance (e.g. compiled into the binary or a
    /// `LoadedPlugin`'s plugin) and return its id.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.plugins.push((id, plugin));
        id
    }

    /// Load and register a plugin from a dynamic library path.
    ///
    /// # Safety
    ///
    /// See [`load_plugin`].
    pub unsafe fn register_library<P: AsRef<Path>>(&mut self, path: P) -> Result<u64, LoaderError> {
        let loaded = unsafe { load_plugin(path)? };
        Ok(self.register(Box::new(loaded)))
    }

    pub fn unregister(&mut self, id: u64) -> Option<Box<dyn Plugin>> {
        let index = self.plugins.iter().position(|(pid, _)| *pid == id)?;
        Some(self.plugins.remove(index).1)
    }

    pub fn get(&self, id: u64) -> Option<&dyn Plugin> {
        self.plugins.iter().find(|(pid, _)| *pid == id).map(|(_, p)| &**p)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Box<dyn Plugin>> {
        self.plugins
            .iter_mut()
            .find(|(pid, _)| *pid == id)
            .map(|(_, p)| p)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u64, &(dyn Plugin))> {
        self.plugins.iter().map(|(id, p)| (*id, &**p))
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// All actions of all plugins, `(plugin_id, action)` pairs.
    pub fn all_actions(&self) -> Vec<(u64, crate::host::EventAction)> {
        self.plugins
            .iter()
            .flat_map(|(id, p)| p.actions().into_iter().map(move |a| (*id, a)))
            .collect()
    }

    /// Dispatch `action` (matched by exact name or `Name (Arg1)` display
    /// form) across all plugins that offer it.
    pub fn dispatch(&mut self, action: &str, args: &[String]) -> Result<usize, DispatchError> {
        let name = action.split_once(" (").map(|(n, _)| n).unwrap_or(action);
        let matching: Vec<u64> = self
            .plugins
            .iter()
            .filter(|(_, p)| {
                p.actions().iter().any(|a| {
                    a.name == name || a.to_display_string() == action
                })
            })
            .map(|(id, _)| *id)
            .collect();
        if matching.is_empty() {
            return Err(DispatchError::UnknownAction(action.to_string()));
        }
        let mut count = 0;
        for id in matching {
            if let Some(p) = self.get_mut(id) {
                p.run_action(action.to_string(), args);
                count += 1;
            }
        }
        Ok(count)
    }

    /// Fan out a state change notification to all plugins.
    pub fn notify_state_changed(&mut self, state: &crate::host::StateInfo) {
        for (_, p) in &mut self.plugins {
            p.on_state_changed(state);
        }
    }
}
