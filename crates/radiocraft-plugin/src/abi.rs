//! C ABI — RadioCraft's clean-room equivalent of RadioDJ's `rdjInterface` vtable pattern.
//!
//! Unlike `rdjInterface`, which passes .NET `object` arrays and has no version control,
//! this ABI is explicit:
//!
//! - Every plugin dylib exports `radiocraft_abi_version() -> u32` (bump on breaking
//!   changes; the loader refuses mismatches — docs/03 §5 "Interface version mismatch").
//! - Every plugin dylib exports `radiocraft_plugin_register(*mut *mut PluginVtable) -> i32`.
//! - Wire rules: primitives pass directly (`bool` ↔ `c_int`, `f32`/`f64` by value),
//!   strings are UTF-8 NUL-terminated C strings (out-params are `*mut *mut c_char`),
//!   structured payloads (SongData lists, EventAction, StateInfo, ImportRequest, SQL
//!   parameter lists) travel as JSON strings, NULL inputs mean empty/default, every
//!   host-call shim is panic-fenced (`catch_unwind`) so Rust panics inside host
//!   callbacks never cross the FFI boundary — they return a default instead.
//! - Strings returned by the ABI (`*mut c_char`) are heap-allocated and must be
//!   released with `radiocraft_abi_free_string`.
//!
//! `HostVtable` mirrors the [`Host`] trait 1:1 (which itself mirrors the recovered
//! `IHost` interface, analysis/decompiled-csharp/PluginInterface/rdjInterface/IHost.cs).
//! Every slot is optional: NULL means "host does not provide this" (mirrors the
//! defensive `if (host != null)`/partial-feature semantics of the original host).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::host::{Host, ImportRequest, PluginUiRequest, StateInfo};
use radiocraft_core::model::SongData;

/// Bump when the vtable layout or wire rules change in a breaking way.
pub const ABI_VERSION: u32 = 1;

/// Returned by `radiocraft_plugin_register` on success.
pub const REGISTER_OK: c_int = 0;
/// Returned by host-call shims when the host callback panicked.
pub const ERR_PANICKED: c_int = -1;
/// Returned when a required pointer argument was NULL.
pub const ERR_NULL_ARG: c_int = -2;
/// Returned when a JSON payload could not be decoded.
pub const ERR_BAD_JSON: c_int = -3;

// ---- marshaling helpers ---------------------------------------------------

pub fn to_json_ptr<T: Serialize>(value: &T) -> *mut c_char {
    match serde_json::to_string(value) {
        Ok(json) => match CString::new(json) {
            Ok(s) => s.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

pub fn to_cstr_ptr(value: &str) -> *mut c_char {
    match CString::new(value) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// NULL-safe: NULL → "" (non-NUL chars copied).
pub unsafe fn read_cstr<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
}

/// NULL-safe JSON argument: NULL or invalid → `Default::default()`.
pub fn json_arg<T: DeserializeOwned + Default>(ptr: *const c_char) -> T {
    let raw = unsafe { read_cstr(ptr) };
    if raw.is_empty() {
        return T::default();
    }
    serde_json::from_str(raw).unwrap_or_default()
}

/// Take a JSON pointer produced by our own ABI (heap-allocated) and decode it.
pub(crate) fn consume_json<T: DeserializeOwned + Default>(ptr: *mut c_char) -> T {
    if ptr.is_null() {
        return T::default();
    }
    let value = json_arg::<T>(ptr);
    unsafe { radiocraft_abi_free_string(ptr) };
    value
}

/// Take a heap-allocated C string from our own ABI and return it as `String`.
fn consume_string(ptr: *mut c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let value = unsafe { read_cstr(ptr).to_owned() };
    unsafe { radiocraft_abi_free_string(ptr) };
    value
}

/// Run a host callback with panic fencing. All host shims funnel through here.
fn host_call<R, F: FnOnce() -> R>(f: F) -> Result<R, c_int> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|_| ERR_PANICKED)
}

/// Release a `*mut c_char` returned by any RadioCraft ABI function.
///
/// # Safety
/// `ptr` must be NULL, or previously returned by our ABI (allocated by `CString::into_raw`).
#[no_mangle]
pub unsafe extern "C" fn radiocraft_abi_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(unsafe { CString::from_raw(ptr) });
    }
}

// ---- Fn-pointer wire types (short names keep the struct readable) ---------

pub type FnVoidUnit = unsafe extern "C" fn(*mut c_void);
pub type FnVoidStr = unsafe extern "C" fn(*mut c_void, *const c_char);
pub type FnVoidStrStr = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char);

/// `void (user, const char*, const char*, const char*)`
pub type FnVoidStrStrStr =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *const c_char);
pub type FnGetStr = unsafe extern "C" fn(*mut c_void, *mut *mut c_char);
pub type FnSetStr = unsafe extern "C" fn(*mut c_void, *const c_char);
pub type FnGetBool = unsafe extern "C" fn(*mut c_void) -> c_int;
pub type FnSetBool = unsafe extern "C" fn(*mut c_void, c_int);
pub type FnGetI32 = unsafe extern "C" fn(*mut c_void) -> c_int;
pub type FnSetI32 = unsafe extern "C" fn(*mut c_void, c_int);
pub type FnSetI64 = unsafe extern "C" fn(*mut c_void, i64);
pub type FnGetF32 = unsafe extern "C" fn(*mut c_void) -> f32;
pub type FnSetF32 = unsafe extern "C" fn(*mut c_void, f32);
pub type FnGetF64 = unsafe extern "C" fn(*mut c_void) -> f64;
pub type FnGetJson = unsafe extern "C" fn(*mut c_void) -> *mut c_char;
pub type FnStrToPtr = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char;
pub type FnStrStrToPtr =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> *mut c_char;
pub type FnStrStrToI32 =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
pub type FnI32ToI32 = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
pub type FnI32ToPtr = unsafe extern "C" fn(*mut c_void, c_int) -> *mut c_char;
pub type FnI64ToI32 = unsafe extern "C" fn(*mut c_void, i64) -> c_int;
pub type FnI32I32 = unsafe extern "C" fn(*mut c_void, c_int, c_int);
pub type FnI64I32 = unsafe extern "C" fn(*mut c_void, i64, c_int) -> c_int;
pub type FnI64I32Void = unsafe extern "C" fn(*mut c_void, i64, c_int);
pub type FnStrI32ToI32 = unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_int;
pub type FnStrI32Void = unsafe extern "C" fn(*mut c_void, *const c_char, c_int);
/// SongData/StateInfo JSON in, void out.
pub type FnJsonVoid = unsafe extern "C" fn(*mut c_void, *const c_char);
/// JSON in, i32 out (`update_history`); JSON in, i64 out (`import_file`).
pub type FnJsonToI32 = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
pub type FnJsonToI64 = unsafe extern "C" fn(*mut c_void, *const c_char) -> i64;
/// JSON-encoded `PluginUiRequest` (`show_ui`).
pub type FnUiReq = unsafe extern "C" fn(*mut c_void, *const c_char);
/// settings: file, key, default, out value.
pub type FnConfigGet =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *const c_char, *mut *mut c_char);

/// The host's services, as a function-pointer table (the `IHost` equivalent).
/// Field names and calling shapes mirror the [`Host`] trait one-for-one.
/// All fields are optional: NULL means "host does not implement this".
/// Every entry is panic-fenced: 0/NULL/false/empty on panic.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct HostVtable {
    pub user_data: *mut c_void,
    // IHost properties
    pub main_autodj: Option<FnGetBool>,
    pub set_main_autodj: Option<FnSetBool>,
    pub main_assisted: Option<FnGetBool>,
    pub set_main_assisted: Option<FnSetBool>,
    pub unmute_input: Option<FnGetBool>,
    pub set_unmute_input: Option<FnSetBool>,
    pub track_loop: Option<FnGetBool>,
    pub set_track_loop: Option<FnSetBool>,
    pub record: Option<FnGetBool>,
    pub set_record: Option<FnSetBool>,
    pub pause_events: Option<FnGetBool>,
    pub set_pause_events: Option<FnSetBool>,
    pub playlist2_full_screen: Option<FnGetBool>,
    pub set_playlist2_full_screen: Option<FnSetBool>,
    pub search_results: Option<FnGetI32>,
    pub set_reload_zones: Option<FnSetBool>,
    pub title_main: Option<FnGetStr>,
    pub set_title_main: Option<FnSetStr>,
    pub get_type_title: Option<FnI32ToPtr>,
    pub get_type_color: Option<FnI32ToI32>,
    pub get_sound_card: Option<FnI32ToPtr>,
    // main player
    pub main_volume: Option<FnGetF32>,
    pub set_main_volume: Option<FnSetF32>,
    pub main_track_volume: Option<FnGetF32>,
    pub set_main_track_volume: Option<FnSetF32>,
    pub main_mute: Option<FnGetBool>,
    pub set_main_mute: Option<FnSetBool>,
    pub set_play_from_intro: Option<FnSetBool>,
    pub set_play_from_loop: Option<FnSetBool>,
    pub set_play_from_outro: Option<FnSetBool>,
    pub set_restart_track: Option<FnSetBool>,
    pub pause: Option<FnGetBool>,
    pub set_pause: Option<FnSetBool>,
    pub set_stop_playing: Option<FnSetBool>,
    pub set_stop_playing_without_fade: Option<FnSetBool>,
    pub set_update_playlist: Option<FnSetBool>,
    pub get_main_playlist: Option<FnGetJson>,
    pub set_main_playlist: Option<FnJsonVoid>,
    pub set_clear_playlist: Option<FnSetBool>,
    pub queue_count: Option<FnGetI32>,
    pub now_playing_handle: Option<FnGetI32>,
    pub remaining_time: Option<FnGetF64>,
    // reload/refresh commands
    pub set_refresh_events: Option<FnSetBool>,
    pub set_reload_categories: Option<FnSetBool>,
    pub set_refresh_gui: Option<FnSetBool>,
    pub set_carts_changed: Option<FnSetBool>,
    pub set_cart_started: Option<FnSetI32>,
    pub set_main_player_started: Option<FnSetI32>,
    // misc
    pub application_path: Option<FnGetStr>,
    pub get_songs: Option<FnStrToPtr>,
    pub state_changed: Option<FnJsonVoid>,
    pub run_event: Option<FnI64ToI32>,
    // playlist commands
    pub play_playlist_track: Option<FnI32I32>,
    pub add_to_playlist_by_id: Option<FnI64I32Void>,
    pub add_to_playlist: Option<FnStrI32Void>,
    pub delete_from_playlist: Option<FnSetI32>,
    pub add_playlist_by_id: Option<FnI64I32>,
    pub add_playlist_by_path: Option<FnStrI32ToI32>,
    pub import_file: Option<FnJsonToI64>,
    pub update_history: Option<FnJsonToI32>,
    // UI
    pub show_ui: Option<FnUiReq>,
    // plugin actions
    pub get_plugin_actions: Option<FnGetJson>,
    pub run_plugin_action: Option<FnVoidStrStr>,
    pub play_cart_by_number: Option<FnSetI32>,
    // logging
    pub debug_add_line: Option<FnVoidStr>,
    // rotations
    pub load_rotation_id: Option<FnI64ToI32>,
    // database
    pub mysql_query: Option<FnStrStrToPtr>,
    pub mysql_insert: Option<FnStrStrToI32>,
    // ini-style settings
    pub save_setting: Option<FnVoidStrStrStr>,
    pub get_setting: Option<FnConfigGet>,
}

// ---- plugin-side shims: HostVtable -> typed calls --------------------------

// Kept for ABI completeness: void-no-arg setters (e.g. `SwitchToPlaylist`)
// may use these once their IHost members are modeled.
#[allow(unused_macros)]
macro_rules! bridge_void {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data) });
            }
        }
    };
}
macro_rules! bridge_set_bool {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, v: bool) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, c_int::from(v)) });
            }
        }
    };
}
macro_rules! bridge_get_bool {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable) -> bool {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return false,
            };
            match host.$field {
                Some(f) => {
                    host_call(|| unsafe { f(host.user_data) }).map(|v| v != 0).unwrap_or(false)
                }
                None => false,
            }
        }
    };
}
macro_rules! bridge_set_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, v: i32) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, v) });
            }
        }
    };
}
#[allow(unused_macros)]
macro_rules! bridge_set_i64 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, v: i64) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, v) });
            }
        }
    };
}
macro_rules! bridge_get_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable) -> i32 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0,
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data) }).unwrap_or(0),
                None => 0,
            }
        }
    };
}
macro_rules! bridge_get_f32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable) -> f32 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0.0,
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data) }).unwrap_or(0.0),
                None => 0.0,
            }
        }
    };
}
macro_rules! bridge_set_f32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, v: f32) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, v) });
            }
        }
    };
}
macro_rules! bridge_get_f64 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable) -> f64 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0.0,
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data) }).unwrap_or(0.0),
                None => 0.0,
            }
        }
    };
}
macro_rules! bridge_get_str {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, out: *mut *mut c_char) {
            if out.is_null() {
                return;
            }
            unsafe { *out = std::ptr::null_mut() };
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, out) });
            }
        }
    };
}
macro_rules! bridge_set_str {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, s: *const c_char) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, s) });
            }
        }
    };
}
macro_rules! bridge_json_get {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable) -> *mut c_char {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return std::ptr::null_mut(),
            };
            match host.$field {
                Some(f) => {
                    host_call(|| unsafe { f(host.user_data) }).unwrap_or(std::ptr::null_mut())
                }
                None => std::ptr::null_mut(),
            }
        }
    };
}
macro_rules! bridge_i32_to_json {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, v: c_int) -> *mut c_char {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return std::ptr::null_mut(),
            };
            match host.$field {
                Some(f) => {
                    host_call(|| unsafe { f(host.user_data, v) }).unwrap_or(std::ptr::null_mut())
                }
                None => std::ptr::null_mut(),
            }
        }
    };
}
macro_rules! bridge_i32_to_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, v: c_int) -> i32 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0,
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data, v) }).unwrap_or(0),
                None => 0,
            }
        }
    };
}
macro_rules! bridge_i64_to_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, v: i64) -> i32 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0,
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data, v) }).unwrap_or(0),
                None => 0,
            }
        }
    };
}
macro_rules! bridge_void_i64_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, a: i64, b: c_int) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, a, b) });
            }
        }
    };
}
macro_rules! bridge_void_str_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, s: *const c_char, v: c_int) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, s, v) });
            }
        }
    };
}
macro_rules! bridge_i32_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, a: c_int, b: c_int) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, a, b) });
            }
        }
    };
}
macro_rules! bridge_i64_i32_to_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, a: i64, b: c_int) -> i32 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0,
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data, a, b) }).unwrap_or(0),
                None => 0,
            }
        }
    };
}
macro_rules! bridge_str_i32_to_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(
            host: *mut HostVtable,
            s: *const c_char,
            v: c_int,
        ) -> i32 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0,
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data, s, v) }).unwrap_or(0),
                None => 0,
            }
        }
    };
}
macro_rules! bridge_str_to_ptr {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, s: *const c_char) -> *mut c_char {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return std::ptr::null_mut(),
            };
            match host.$field {
                Some(f) => {
                    host_call(|| unsafe { f(host.user_data, s) }).unwrap_or(std::ptr::null_mut())
                }
                None => std::ptr::null_mut(),
            }
        }
    };
}
macro_rules! bridge_json_void {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, json: *const c_char) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, json) });
            }
        }
    };
}
macro_rules! bridge_json_to_i32 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, json: *const c_char) -> i32 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0,
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data, json) }).unwrap_or(0),
                None => 0,
            }
        }
    };
}
macro_rules! bridge_json_to_i64 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, json: *const c_char) -> i64 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0,
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data, json) }).unwrap_or(0),
                None => 0,
            }
        }
    };
}
macro_rules! bridge_void_str_str {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(
            host: *mut HostVtable,
            a: *const c_char,
            b: *const c_char,
        ) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, a, b) });
            }
        }
    };
}
macro_rules! bridge_void_str_str_str {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(
            host: *mut HostVtable,
            a: *const c_char,
            b: *const c_char,
            c: *const c_char,
        ) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, a, b, c) });
            }
        }
    };
}
macro_rules! bridge_sql_query {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(
            host: *mut HostVtable,
            sql: *const c_char,
            args_json: *const c_char,
        ) -> *mut c_char {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return std::ptr::null_mut(),
            };
            match host.$field {
                Some(f) => host_call(|| unsafe { f(host.user_data, sql, args_json) })
                    .unwrap_or(std::ptr::null_mut()),
                None => std::ptr::null_mut(),
            }
        }
    };
}
macro_rules! bridge_sql_insert {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(
            host: *mut HostVtable,
            sql: *const c_char,
            args_json: *const c_char,
        ) -> i32 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0,
            };
            match host.$field {
                Some(f) => {
                    host_call(|| unsafe { f(host.user_data, sql, args_json) }).unwrap_or(0)
                }
                None => 0,
            }
        }
    };
}
macro_rules! bridge_config_get {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(
            host: *mut HostVtable,
            file: *const c_char,
            key: *const c_char,
            default: *const c_char,
            out: *mut *mut c_char,
        ) {
            if out.is_null() {
                return;
            }
            unsafe { *out = std::ptr::null_mut() };
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let _ = host_call(|| unsafe { f(host.user_data, file, key, default, out) });
            }
        }
    };
}

bridge_get_bool!(host_main_autodj, main_autodj, "IHost: `bool MainAutoDJ { get; }`");
bridge_set_bool!(host_set_main_autodj, set_main_autodj, "IHost: `MainAutoDJ { set; }`");
bridge_get_bool!(host_main_assisted, main_assisted, "IHost: `bool MainAssisted { get; }`");
bridge_set_bool!(host_set_main_assisted, set_main_assisted, "IHost: `MainAssisted { set; }`");
bridge_get_bool!(host_unmute_input, unmute_input, "IHost: `bool UnmuteInput { get; }`");
bridge_set_bool!(host_set_unmute_input, set_unmute_input, "IHost: `UnmuteInput { set; }`");
bridge_get_bool!(host_track_loop, track_loop, "IHost: `bool TrackLoop { get; }`");
bridge_set_bool!(host_set_track_loop, set_track_loop, "IHost: `TrackLoop { set; }`");
bridge_get_bool!(host_record, record, "IHost: `bool Record { get; }`");
bridge_set_bool!(host_set_record, set_record, "IHost: `Record { set; }`");
bridge_get_bool!(host_pause_events, pause_events, "IHost: `bool PauseEvents { get; }`");
bridge_set_bool!(host_set_pause_events, set_pause_events, "IHost: `PauseEvents { set; }`");
bridge_get_bool!(host_playlist2_full_screen, playlist2_full_screen, "IHost: `bool Playlist2FullScreen { get; }`");
bridge_set_bool!(host_set_playlist2_full_screen, set_playlist2_full_screen, "IHost: `Playlist2FullScreen { set; }`");
bridge_get_i32!(host_search_results, search_results, "IHost: `int SearchResults { get; }`");
bridge_set_bool!(host_set_reload_zones, set_reload_zones, "IHost: `bool ReloadZones { set; }`");
bridge_get_str!(host_title_main, title_main, "IHost: `string TitleMain { get; }`");
bridge_set_str!(host_set_title_main, set_title_main, "IHost: `TitleMain { set; }`");
bridge_i32_to_json!(host_get_type_title, get_type_title, "IHost: `string get_GetTypeTitle(int)` (JSON string)");
bridge_i32_to_i32!(host_get_type_color, get_type_color, "IHost: `int get_GetTypeColor(int)`");
bridge_i32_to_json!(host_get_sound_card, get_sound_card, "IHost: `int[] get_GetSoundCard(CardTypes)` (JSON array)");

bridge_get_f32!(host_main_volume, main_volume, "IHost: `float MainVolume { get; }`");
bridge_set_f32!(host_set_main_volume, set_main_volume, "IHost: `MainVolume { set; }`");
bridge_get_f32!(host_main_track_volume, main_track_volume, "IHost: `float MainTrackVolume { get; }`");
bridge_set_f32!(host_set_main_track_volume, set_main_track_volume, "IHost: `MainTrackVolume { set; }`");
bridge_get_bool!(host_main_mute, main_mute, "IHost: `bool MainMute { get; }`");
bridge_set_bool!(host_set_main_mute, set_main_mute, "IHost: `MainMute { set; }`");
bridge_set_bool!(host_set_play_from_intro, set_play_from_intro, "IHost: `bool PlayFromIntro { set; }`");
bridge_set_bool!(host_set_play_from_loop, set_play_from_loop, "IHost: `bool PlayFromLoop { set; }`");
bridge_set_bool!(host_set_play_from_outro, set_play_from_outro, "IHost: `bool PlayFromOutro { set; }`");
bridge_set_bool!(host_set_restart_track, set_restart_track, "IHost: `bool RestartTrack { set; }`");
bridge_get_bool!(host_pause, pause, "IHost: `bool Pause { get; }`");
bridge_set_bool!(host_set_pause, set_pause, "IHost: `Pause { set; }`");
bridge_set_bool!(host_set_stop_playing, set_stop_playing, "IHost: `bool StopPlaying { set; }`");
bridge_set_bool!(host_set_stop_playing_without_fade, set_stop_playing_without_fade, "IHost: `StopPlayingWithoutFade { set; }`");
bridge_set_bool!(host_set_update_playlist, set_update_playlist, "IHost: `bool UpdatePlaylist { set; }`");
bridge_json_get!(host_get_main_playlist, get_main_playlist, "IHost: `MainPlaylist { get; }` (JSON array of SongData)");
bridge_json_void!(host_set_main_playlist, set_main_playlist, "IHost: `MainPlaylist { set; }` (JSON array of SongData)");
bridge_set_bool!(host_set_clear_playlist, set_clear_playlist, "IHost: `bool ClearPlaylist { set; }`");
bridge_get_i32!(host_queue_count, queue_count, "IHost: `int QueueCount { get; }`");
bridge_get_i32!(host_now_playing_handle, now_playing_handle, "IHost: `int NowPlayingHandle { get; }`");
bridge_get_f64!(host_remaining_time, remaining_time, "IHost: `double RemainingTime { get; }`");

bridge_set_bool!(host_set_refresh_events, set_refresh_events, "IHost: `bool RefreshEvents { set; }`");
bridge_set_bool!(host_set_reload_categories, set_reload_categories, "IHost: `bool ReloadCategories { set; }`");
bridge_set_bool!(host_set_refresh_gui, set_refresh_gui, "IHost: `bool RefreshGUI { set; }`");
bridge_set_bool!(host_set_carts_changed, set_carts_changed, "IHost: `bool CartsChanged { set; }`");
bridge_set_i32!(host_set_cart_started, set_cart_started, "IHost: `int CartStarted { set; }`");
bridge_set_i32!(host_set_main_player_started, set_main_player_started, "IHost: `int MainPlayerStarted { set; }`");

bridge_get_str!(host_application_path, application_path, "IHost: `string ApplicationPath { get; }`");
bridge_str_to_ptr!(host_get_songs, get_songs, "IHost: `GetSongs(sql, ref dgv)` (JSON array of SongData)");
bridge_json_void!(host_state_changed, state_changed, "IHost: `StateChanged(Features, object)` (JSON StateInfo)");
bridge_i64_to_i32!(host_run_event, run_event, "IHost: `bool RunEvent(long)`");

bridge_i32_i32!(host_play_playlist_track, play_playlist_track, "IHost: `PlayPlaylistTrack(int, bool)`");
bridge_void_i64_i32!(host_add_to_playlist_by_id, add_to_playlist_by_id, "IHost: `Add2Playlist(long, int)`");
bridge_void_str_i32!(host_add_to_playlist, add_to_playlist, "IHost: `Add2Playlist(TrackPlayer, int)` (JSON SongData)");
bridge_set_i32!(host_delete_from_playlist, delete_from_playlist, "IHost: `DeleteFromPlaylist(int)`");
bridge_i64_i32_to_i32!(host_add_playlist_by_id, add_playlist_by_id, "IHost: `AddPlaylist(long, int)`");
bridge_str_i32_to_i32!(host_add_playlist_by_path, add_playlist_by_path, "IHost: `AddPlaylist(string, int)`");
bridge_json_to_i64!(host_import_file, import_file, "IHost: `long ImportFile(...)` (JSON ImportRequest)");
bridge_json_to_i32!(host_update_history, update_history, "IHost: `bool UpdateHistory(SongData)` (JSON SongData)");

bridge_json_void!(host_show_ui, show_ui, "queued UI request (JSON PluginUiRequest)");

bridge_json_get!(host_get_plugin_actions, get_plugin_actions, "IHost: `GetPluginActions()` (JSON array of EventAction)");
bridge_void_str_str!(host_run_plugin_action, run_plugin_action, "IHost: `RunPluginAction(EventAction, string[])` (JSON args)");
bridge_set_i32!(host_play_cart_by_number, play_cart_by_number, "IHost: `PlayCartByNumber(int)`");

bridge_set_str!(host_debug_add_line, debug_add_line, "IHost: `DebugAddLine(string)`");

bridge_i64_to_i32!(host_load_rotation_id, load_rotation_id, "IHost: `bool LoadRotationID(long)`");

bridge_sql_query!(host_mysql_query, mysql_query, "IHost: `MysqlQuery(query, ref DataSet)` (JSON rows)");
bridge_sql_insert!(host_mysql_insert, mysql_insert, "IHost: `int MysqlInsert(query)`");

bridge_void_str_str_str!(host_save_setting, save_setting, "IHost: `bool SaveSetting(file, key, value)`");
bridge_config_get!(host_get_setting, get_setting, "IHost: `string GetSetting(file, key, default)`");

/// Plugin-side view of the host: wraps a raw `HostVtable` received in `init`
/// and exposes safe typed calls (same surface as the [`Host`] trait).
pub struct HostBridge {
    vt: HostVtable,
}

impl HostBridge {
    /// Wrap a host-provided vtable.
    ///
    /// # Safety
    /// `vt` must be a valid `HostVtable` (as produced by [`crate::hostwire::HostHandle`])
    /// whose `user_data` stays live for the whole lifetime of the bridge.
    pub unsafe fn new(vt: HostVtable) -> Self {
        Self { vt }
    }

    /// Wrap a partially-filled vtable (NULL entries fall back to no-op/defaults).
    pub fn from_partial(vt: HostVtable) -> Self {
        Self { vt }
    }

    /// Common `&HostVtable` as a mutable pointer for the shims below.
    fn vt_ptr(&self) -> *mut HostVtable {
        &self.vt as *const HostVtable as *mut HostVtable
    }
}

/// Build an owned heap C-string for `s` (None for empty marshaling failures).
macro_rules! carg {
    ($s:expr) => {
        to_cstr_ptr($s)
    };
}

impl Host for HostBridge {
    fn main_autodj(&self) -> bool {
        unsafe { host_main_autodj(self.vt_ptr()) }
    }
    fn set_main_autodj(&self, value: bool) {
        unsafe { host_set_main_autodj(self.vt_ptr(), value) }
    }
    fn main_assisted(&self) -> bool {
        unsafe { host_main_assisted(self.vt_ptr()) }
    }
    fn set_main_assisted(&self, value: bool) {
        unsafe { host_set_main_assisted(self.vt_ptr(), value) }
    }
    fn unmute_input(&self) -> bool {
        unsafe { host_unmute_input(self.vt_ptr()) }
    }
    fn set_unmute_input(&self, value: bool) {
        unsafe { host_set_unmute_input(self.vt_ptr(), value) }
    }
    fn track_loop(&self) -> bool {
        unsafe { host_track_loop(self.vt_ptr()) }
    }
    fn set_track_loop(&self, value: bool) {
        unsafe { host_set_track_loop(self.vt_ptr(), value) }
    }
    fn record(&self) -> bool {
        unsafe { host_record(self.vt_ptr()) }
    }
    fn set_record(&self, value: bool) {
        unsafe { host_set_record(self.vt_ptr(), value) }
    }
    fn pause_events(&self) -> bool {
        unsafe { host_pause_events(self.vt_ptr()) }
    }
    fn set_pause_events(&self, value: bool) {
        unsafe { host_set_pause_events(self.vt_ptr(), value) }
    }
    fn playlist2_full_screen(&self) -> bool {
        unsafe { host_playlist2_full_screen(self.vt_ptr()) }
    }
    fn set_playlist2_full_screen(&self, value: bool) {
        unsafe { host_set_playlist2_full_screen(self.vt_ptr(), value) }
    }
    fn search_results(&self) -> i32 {
        unsafe { host_search_results(self.vt_ptr()) }
    }
    fn set_reload_zones(&self, value: bool) {
        unsafe { host_set_reload_zones(self.vt_ptr(), value) }
    }
    fn title_main(&self) -> String {
        let mut out: *mut c_char = std::ptr::null_mut();
        unsafe { host_title_main(self.vt_ptr(), &mut out) };
        consume_string(out)
    }
    fn set_title_main(&self, value: &str) {
        let s = carg!(value);
        unsafe { host_set_title_main(self.vt_ptr(), s) };
        unsafe { radiocraft_abi_free_string(s) };
    }
    fn get_type_title(&self, track_type: i32) -> String {
        let out = unsafe { host_get_type_title(self.vt_ptr(), track_type) };
        consume_json::<String>(out)
    }
    fn get_type_color(&self, track_type: i32) -> i32 {
        unsafe { host_get_type_color(self.vt_ptr(), track_type) }
    }
    fn get_sound_card(&self, card_type: i32) -> Vec<i32> {
        let out = unsafe { host_get_sound_card(self.vt_ptr(), card_type) };
        consume_json(out)
    }
    fn main_volume(&self) -> f32 {
        unsafe { host_main_volume(self.vt_ptr()) }
    }
    fn set_main_volume(&self, value: f32) {
        unsafe { host_set_main_volume(self.vt_ptr(), value) }
    }
    fn main_track_volume(&self) -> f32 {
        unsafe { host_main_track_volume(self.vt_ptr()) }
    }
    fn set_main_track_volume(&self, value: f32) {
        unsafe { host_set_main_track_volume(self.vt_ptr(), value) }
    }
    fn main_mute(&self) -> bool {
        unsafe { host_main_mute(self.vt_ptr()) }
    }
    fn set_main_mute(&self, value: bool) {
        unsafe { host_set_main_mute(self.vt_ptr(), value) }
    }
    fn set_play_from_intro(&self, value: bool) {
        unsafe { host_set_play_from_intro(self.vt_ptr(), value) }
    }
    fn set_play_from_loop(&self, value: bool) {
        unsafe { host_set_play_from_loop(self.vt_ptr(), value) }
    }
    fn set_play_from_outro(&self, value: bool) {
        unsafe { host_set_play_from_outro(self.vt_ptr(), value) }
    }
    fn set_restart_track(&self, value: bool) {
        unsafe { host_set_restart_track(self.vt_ptr(), value) }
    }
    fn pause(&self) -> bool {
        unsafe { host_pause(self.vt_ptr()) }
    }
    fn set_pause(&self, value: bool) {
        unsafe { host_set_pause(self.vt_ptr(), value) }
    }
    fn set_stop_playing(&self, value: bool) {
        unsafe { host_set_stop_playing(self.vt_ptr(), value) }
    }
    fn set_stop_playing_without_fade(&self, value: bool) {
        unsafe { host_set_stop_playing_without_fade(self.vt_ptr(), value) }
    }
    fn set_update_playlist(&self, value: bool) {
        unsafe { host_set_update_playlist(self.vt_ptr(), value) }
    }
    fn get_main_playlist(&self) -> Vec<SongData> {
        let out = unsafe { host_get_main_playlist(self.vt_ptr()) };
        consume_json(out)
    }
    fn set_main_playlist(&self, playlist: &[SongData]) {
        let json = to_json_ptr(&playlist.to_vec());
        unsafe { host_set_main_playlist(self.vt_ptr(), json) };
        unsafe { radiocraft_abi_free_string(json) };
    }
    fn set_clear_playlist(&self, value: bool) {
        unsafe { host_set_clear_playlist(self.vt_ptr(), value) }
    }
    fn queue_count(&self) -> i32 {
        unsafe { host_queue_count(self.vt_ptr()) }
    }
    fn now_playing_handle(&self) -> i32 {
        unsafe { host_now_playing_handle(self.vt_ptr()) }
    }
    fn remaining_time(&self) -> f64 {
        unsafe { host_remaining_time(self.vt_ptr()) }
    }
    fn set_refresh_events(&self, value: bool) {
        unsafe { host_set_refresh_events(self.vt_ptr(), value) }
    }
    fn set_reload_categories(&self, value: bool) {
        unsafe { host_set_reload_categories(self.vt_ptr(), value) }
    }
    fn set_refresh_gui(&self, value: bool) {
        unsafe { host_set_refresh_gui(self.vt_ptr(), value) }
    }
    fn set_carts_changed(&self, value: bool) {
        unsafe { host_set_carts_changed(self.vt_ptr(), value) }
    }
    fn set_cart_started(&self, value: i32) {
        unsafe { host_set_cart_started(self.vt_ptr(), value) }
    }
    fn set_main_player_started(&self, value: i32) {
        unsafe { host_set_main_player_started(self.vt_ptr(), value) }
    }
    fn application_path(&self) -> std::path::PathBuf {
        let mut out: *mut c_char = std::ptr::null_mut();
        unsafe { host_application_path(self.vt_ptr(), &mut out) };
        std::path::PathBuf::from(consume_string(out))
    }
    fn get_songs(&self, sql: &str) -> Vec<SongData> {
        let sql_c = carg!(sql);
        let out = unsafe { host_get_songs(self.vt_ptr(), sql_c) };
        unsafe { radiocraft_abi_free_string(sql_c) };
        consume_json(out)
    }
    fn state_changed(&self, state: &StateInfo) {
        let json = to_json_ptr(state);
        unsafe { host_state_changed(self.vt_ptr(), json) };
        unsafe { radiocraft_abi_free_string(json) };
    }
    fn run_event(&self, event_id: i64) -> bool {
        unsafe { host_run_event(self.vt_ptr(), event_id) != 0 }
    }
    fn play_playlist_track(&self, track_number: i32, with_fade: bool) {
        unsafe { host_play_playlist_track(self.vt_ptr(), track_number, c_int::from(with_fade)) }
    }
    fn add_to_playlist_by_id(&self, track_id: i64, position: i32) {
        unsafe { host_add_to_playlist_by_id(self.vt_ptr(), track_id, position) };
    }
    fn add_to_playlist(&self, track: &SongData, position: i32) {
        let json = to_json_ptr(track);
        unsafe { host_add_to_playlist(self.vt_ptr(), json, position) };
        unsafe { radiocraft_abi_free_string(json) };
    }
    fn delete_from_playlist(&self, track_number: i32) {
        unsafe { host_delete_from_playlist(self.vt_ptr(), track_number) }
    }
    fn add_playlist_by_id(&self, playlist_id: i64, position: i32) -> bool {
        unsafe { host_add_playlist_by_id(self.vt_ptr(), playlist_id, position) != 0 }
    }
    fn add_playlist_by_path(&self, path: &str, position: i32) -> bool {
        let p = carg!(path);
        let ok = unsafe { host_add_playlist_by_path(self.vt_ptr(), p, position) };
        unsafe { radiocraft_abi_free_string(p) };
        ok != 0
    }
    fn import_file(&self, request: &ImportRequest) -> i64 {
        let json = to_json_ptr(request);
        let id = unsafe { host_import_file(self.vt_ptr(), json) };
        unsafe { radiocraft_abi_free_string(json) };
        id
    }
    fn update_history(&self, track: &SongData) -> bool {
        let json = to_json_ptr(track);
        let ok = unsafe { host_update_history(self.vt_ptr(), json) };
        unsafe { radiocraft_abi_free_string(json) };
        ok != 0
    }
    fn show_ui(&self, request: PluginUiRequest) {
        let json = to_json_ptr(&request);
        unsafe { host_show_ui(self.vt_ptr(), json) };
        unsafe { radiocraft_abi_free_string(json) };
    }
    fn get_plugin_actions(&self) -> Vec<crate::host::EventAction> {
        let out = unsafe { host_get_plugin_actions(self.vt_ptr()) };
        consume_json(out)
    }
    fn run_plugin_action(&self, action: &crate::host::EventAction, args: &[String]) {
        let a = to_json_ptr(action);
        let v = to_json_ptr(&args.to_vec());
        unsafe { host_run_plugin_action(self.vt_ptr(), a, v) };
        unsafe { radiocraft_abi_free_string(a) };
        unsafe { radiocraft_abi_free_string(v) };
    }
    fn play_cart_by_number(&self, cart_number: i32) {
        unsafe { host_play_cart_by_number(self.vt_ptr(), cart_number) }
    }
    fn debug_add_line(&self, entry: &str) {
        let s = carg!(entry);
        unsafe { host_debug_add_line(self.vt_ptr(), s) };
        unsafe { radiocraft_abi_free_string(s) };
    }
    fn load_rotation_id(&self, rotation_id: i64) -> bool {
        unsafe { host_load_rotation_id(self.vt_ptr(), rotation_id) != 0 }
    }
    fn mysql_query(&self, sql: &str, args: &[&str]) -> String {
        let sql_c = carg!(sql);
        let args_c = to_json_ptr(&args.to_vec());
        let out = unsafe { host_mysql_query(self.vt_ptr(), sql_c, args_c) };
        unsafe { radiocraft_abi_free_string(sql_c) };
        unsafe { radiocraft_abi_free_string(args_c) };
        consume_string(out)
    }
    fn mysql_insert(&self, sql: &str, args: &[&str]) -> i32 {
        let sql_c = carg!(sql);
        let args_c = to_json_ptr(&args.to_vec());
        let v = unsafe { host_mysql_insert(self.vt_ptr(), sql_c, args_c) };
        unsafe { radiocraft_abi_free_string(sql_c) };
        unsafe { radiocraft_abi_free_string(args_c) };
        v
    }
    fn save_setting(&self, file: &str, key: &str, value: &str) {
        let f = carg!(file);
        let k = carg!(key);
        let v = carg!(value);
        unsafe { host_save_setting(self.vt_ptr(), f, k, v) };
        unsafe { radiocraft_abi_free_string(f) };
        unsafe { radiocraft_abi_free_string(k) };
        unsafe { radiocraft_abi_free_string(v) };
    }
    fn get_setting(&self, file: &str, key: &str, default: &str) -> String {
        let f = carg!(file);
        let k = carg!(key);
        let d = carg!(default);
        let mut out: *mut c_char = std::ptr::null_mut();
        unsafe { host_get_setting(self.vt_ptr(), f, k, d, &mut out) };
        unsafe { radiocraft_abi_free_string(f) };
        unsafe { radiocraft_abi_free_string(k) };
        unsafe { radiocraft_abi_free_string(d) };
        consume_string(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vtable_defaults_are_empty() {
        let vt = HostVtable::default();
        assert!(vt.user_data.is_null());
        assert!(vt.main_autodj.is_none());
        assert!(vt.show_ui.is_none());
    }

    #[test]
    fn bridge_tolerates_null_entries() {
        let bridge = HostBridge::from_partial(HostVtable::default());
        bridge.set_main_autodj(true);
        bridge.add_to_playlist(&SongData::default(), 0);
        bridge.save_setting("f", "k", "v");
        assert!(!bridge.main_autodj());
        assert_eq!(bridge.get_setting("f", "k", "0"), "");
        assert_eq!(bridge.mysql_query("SELECT 1", &[]), "");
        assert_eq!(bridge.main_volume(), 0.0);
        assert!(bridge.get_main_playlist().is_empty());
    }

    #[test]
    fn free_string_accepts_null() {
        unsafe { radiocraft_abi_free_string(std::ptr::null_mut()) };
    }
}
