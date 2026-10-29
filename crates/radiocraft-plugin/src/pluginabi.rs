//! Plugin-side ABI: `PluginVtable` + generic shims + host-facing `NativePlugin`
//! and the `export_plugin!` macro used by plugin dylibs.

use std::ffi::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::abi::{consume_json, json_arg, to_cstr_ptr, to_json_ptr, HostVtable, REGISTER_OK};
use crate::host::{EventAction, Host, StateInfo};
use crate::plugin::{Plugin, PluginInfo};

/// Returned by `radiocraft_run_action` when the plugin action panicked.
pub const ERR_ACTION_PANICKED: c_int = -1;

// Fn-pointer types for plugin callbacks.
pub type FnPluginGetStr = unsafe extern "C" fn(*mut c_void, *mut *mut c_char);
pub type FnPluginInit = unsafe extern "C" fn(*mut c_void, *const HostVtable);
pub type FnPluginRunAction = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
pub type FnPluginState = unsafe extern "C" fn(*mut c_void, *const c_char);
pub type FnPluginDestroy = unsafe extern "C" fn(*mut c_void);

/// The plugin's callbacks, as a function-pointer table (the `IPlugin` equivalent).
/// `user_data` is the boxed `Plugin` instance owned by whoever built the vtable.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct PluginVtable {
    pub user_data: *mut c_void,
    /// Writes the JSON-encoded [`PluginInfo`] to `*out`.
    pub get_info: Option<FnPluginGetStr>,
    /// Receives a pointer to the host's [`HostVtable`]; the plugin may copy it.
    pub initialize: Option<FnPluginInit>,
    /// Writes the JSON-encoded `Vec<EventAction>` to `*out`.
    pub get_actions: Option<FnPluginGetStr>,
    /// Decodes `(EventAction, Vec<String>)` JSON args; returns 0 or an error code.
    pub run_action: Option<FnPluginRunAction>,
    /// Decodes a JSON-encoded [`StateInfo`].
    pub on_state_changed: Option<FnPluginState>,
    /// Drops the plugin instance (`user_data`).
    pub destroy: Option<FnPluginDestroy>,
}

fn fence<T, F: FnOnce() -> T>(f: F) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}

// --- generic shims: one per callback, instantiated for each plugin type ------

unsafe extern "C" fn info_shim<T: Plugin>(user: *mut c_void, out: *mut *mut c_char) {
    if out.is_null() || user.is_null() {
        return;
    }
    unsafe { *out = std::ptr::null_mut() };
    let plugin = unsafe { &*(user as *const T) };
    if let Some(json) = fence(|| to_json_ptr(&plugin.info())) {
        unsafe { *out = json };
    }
}

unsafe extern "C" fn actions_shim<T: Plugin>(user: *mut c_void, out: *mut *mut c_char) {
    if out.is_null() || user.is_null() {
        return;
    }
    unsafe { *out = std::ptr::null_mut() };
    let plugin = unsafe { &*(user as *const T) };
    if let Some(json) = fence(|| to_json_ptr(&plugin.actions())) {
        unsafe { *out = json };
    }
}

unsafe extern "C" fn init_shim<T: Plugin>(user: *mut c_void, host_vt: *const HostVtable) {
    if user.is_null() || host_vt.is_null() {
        return;
    }
    let plugin = unsafe { &mut *(user as *mut T) };
    let vt = unsafe { (*host_vt).clone() };
    let bridge = unsafe { crate::abi::HostBridge::new(vt) };
    let _ = fence(|| plugin.init(&bridge));
}

unsafe extern "C" fn run_action_shim<T: Plugin>(
    user: *mut c_void,
    action_json: *const c_char,
    args_json: *const c_char,
) -> c_int {
    if user.is_null() {
        return crate::abi::ERR_PANICKED;
    }
    let action = json_arg::<EventAction>(action_json);
    let args = json_arg::<Vec<String>>(args_json);
    let plugin = unsafe { &mut *(user as *mut T) };
    match fence(move || plugin.run_action(&action, &args)) {
        Some(()) => 0,
        None => ERR_ACTION_PANICKED,
    }
}

unsafe extern "C" fn state_shim<T: Plugin>(user: *mut c_void, state_json: *const c_char) {
    if user.is_null() {
        return;
    }
    let state = json_arg::<StateInfo>(state_json);
    let plugin = unsafe { &mut *(user as *mut T) };
    let _ = fence(move || plugin.on_state_changed(&state));
}

unsafe extern "C" fn destroy_shim<T: Plugin>(user: *mut c_void) {
    if user.is_null() {
        return;
    }
    let _ = fence(|| drop(unsafe { Box::from_raw(user as *mut T) }));
}

/// Build a `PluginVtable` for plugin type `T`, creating its instance with
/// `T::default()`. The vtable's `user_data` owns the instance; call the
/// `destroy` entry (or `NativePlugin`'s Drop) to release it.
pub fn new_plugin_vtable<T: Plugin + Default>() -> PluginVtable {
    let instance = Box::into_raw(Box::new(T::default())) as *mut c_void;
    PluginVtable {
        user_data: instance,
        get_info: Some(info_shim::<T>),
        initialize: Some(init_shim::<T>),
        get_actions: Some(actions_shim::<T>),
        run_action: Some(run_action_shim::<T>),
        on_state_changed: Some(state_shim::<T>),
        destroy: Some(destroy_shim::<T>),
    }
}

/// Host-side wrapper: call a loaded plugin's [`PluginVtable`] as a `Plugin`.
pub struct NativePlugin {
    vt: PluginVtable,
}

impl NativePlugin {
    /// Wrap a vtable received from a plugin.
    ///
    /// # Safety
    /// `vt` must be a vtable produced by the plugin crate (its `user_data` must
    /// point to a live plugin instance with the plugin crate's own `T`, and the
    /// defining dylib must stay loaded for this wrapper's lifetime).
    pub unsafe fn new(vt: PluginVtable) -> Self {
        Self { vt }
    }

    /// Release the plugin instance via the vtable's `destroy` entry.
    pub fn destroy(self) {
        if let Some(f) = self.vt.destroy {
            let _ = catch_unwind(AssertUnwindSafe(|| unsafe { f(self.vt.user_data) }));
        }
        std::mem::forget(self);
    }
}

impl Plugin for NativePlugin {
    fn info(&self) -> PluginInfo {
        match self.vt.get_info {
            Some(f) => {
                let mut p: *mut c_char = std::ptr::null_mut();
                let _ = fence(|| unsafe { f(self.vt.user_data, &mut p) });
                consume_json(p)
            }
            None => PluginInfo::default(),
        }
    }

    fn init(&mut self, host: &dyn Host) {
        if let Some(f) = self.vt.initialize {
            let handle = crate::hostwire::HostHandle::new(host);
            let vt = handle.vtable();
            let _ = fence(|| unsafe { f(self.vt.user_data, &vt) });
        }
    }

    fn actions(&self) -> Vec<EventAction> {
        match self.vt.get_actions {
            Some(f) => {
                let mut p: *mut c_char = std::ptr::null_mut();
                let _ = fence(|| unsafe { f(self.vt.user_data, &mut p) });
                consume_json(p)
            }
            None => Vec::new(),
        }
    }

    fn run_action(&mut self, action: &EventAction, args: &[String]) {
        if let Some(f) = self.vt.run_action {
            let action_json = to_json_ptr(action);
            let args_json = to_json_ptr(&args.to_vec());
            if action_json.is_null() || args_json.is_null() {
                return;
            }
            let _ = fence(|| unsafe { f(self.vt.user_data, action_json, args_json) });
            unsafe { crate::abi::radiocraft_abi_free_string(action_json) };
            unsafe { crate::abi::radiocraft_abi_free_string(args_json) };
        }
    }

    fn on_state_changed(&mut self, state: &StateInfo) {
        if let Some(f) = self.vt.on_state_changed {
            let json = to_json_ptr(state);
            if !json.is_null() {
                let _ = fence(|| unsafe { f(self.vt.user_data, json) });
                unsafe { crate::abi::radiocraft_abi_free_string(json) };
            }
        }
    }
}

impl Drop for NativePlugin {
    fn drop(&mut self) {
        if let Some(f) = self.vt.destroy {
            let _ = catch_unwind(AssertUnwindSafe(|| unsafe { f(self.vt.user_data) }));
        }
    }
}

/// Export the two ABI entry points for a plugin dylib:
///
/// ```ignore
/// export_plugin!(MyPlugin);
/// ```
///
/// Emits `radiocraft_abi_version` and `radiocraft_plugin_register`. The plugin
/// type must implement `Plugin + Default`; the instance is created at register time.
#[macro_export]
macro_rules! export_plugin {
    ($ty:ty) => {
        /// RadioCraft plugin ABI version (see `radiocraft_plugin::abi::ABI_VERSION`).
        #[no_mangle]
        pub extern "C" fn radiocraft_abi_version() -> u32 {
            $crate::abi::ABI_VERSION
        }

        /// Instantiate the plugin and write its vtable to `*out`.
        ///
        /// The vtable is heap-allocated and owned by the plugin dylib; call the
        /// vtable's `destroy` entry to release the instance.
        #[no_mangle]
        pub unsafe extern "C" fn radiocraft_plugin_register(
            out: *mut *mut $crate::pluginabi::PluginVtable,
        ) -> i32 {
            if out.is_null() {
                return $crate::abi::ERR_NULL_ARG;
            }
            unsafe { *out = std::ptr::null_mut() };
            let vt = Box::new($crate::pluginabi::new_plugin_vtable::<$ty>());
            unsafe { *out = Box::into_raw(vt) };
            $crate::abi::REGISTER_OK
        }
    };
}
