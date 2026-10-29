//! C ABI — RadioCraft's clean-room equivalent of RadioDJ's `rdjInterface` vtable pattern.
//!
//! Unlike `rdjInterface`, which passes .NET `object` arrays and has no version control,
//! this ABI is explicit:
//!
//! - Every plugin dylib exports `radiocraft_abi_version() -> u32` (bump on breaking
//!   changes; the loader refuses mismatches — docs/03 §5 "Interface version mismatch").
//! - Every plugin dylib exports `radiocraft_plugin_register(*mut *mut PluginVtable) -> i32`.
//! - Wire rules: primitives pass directly (`bool` ↔ `i32`), strings are UTF-8 NUL-terminated
//!   C strings (caller-allocates, callee-fills pattern for out params), structured payloads
//!   (EventAction, StateInfo, DB rows, song lists) travel as JSON strings, NULL inputs mean
//!   empty/default, every host-call shim is panic-fenced (`catch_unwind`) so Rust panics
//!   inside host callbacks never cross the FFI boundary — they return an error code instead.
//! - Strings returned by the ABI (`*mut c_char`) are heap-allocated and must be released
//!   with `radiocraft_abi_free_string`.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::host::{Host, PluginUiRequest, StateInfo};
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

pub(crate) fn to_json_ptr<T: Serialize>(value: &T) -> *mut c_char {
    match serde_json::to_string(value) {
        Ok(json) => match CString::new(json) {
            Ok(s) => s.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

pub(crate) fn to_cstr_ptr(value: &str) -> *mut c_char {
    match CString::new(value) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// NULL-safe: NULL → "" (non-NUL chars copied).
pub(crate) unsafe fn read_cstr<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("")
}

/// NULL-safe JSON argument: NULL or invalid → `Default::default()`.
fn json_arg<T: DeserializeOwned + Default>(ptr: *const c_char) -> T {
    let raw = unsafe { read_cstr(ptr) };
    if raw.is_empty() {
        return T::default();
    }
    serde_json::from_str(raw).unwrap_or_default()
}

/// Take a JSON pointer produced by our own ABI (heap-allocated) and decode it.
fn consume_json<T: DeserializeOwned + Default>(ptr: *mut c_char) -> T {
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

// ---- HostVtable: host services handed TO the plugin ------------------------

// Fn-pointer wire types (kept short so the struct below stays readable).
pub type FnVoidUnit = unsafe extern "C" fn(*mut c_void);
pub type FnVoidStr = unsafe extern "C" fn(*mut c_void, *const c_char);
pub type FnVoidStrStr = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char);
pub type FnGetStr = unsafe extern "C" fn(*mut c_void, *mut *mut c_char);
pub type FnSetStr = unsafe extern "C" fn(*mut c_void, *const c_char);
pub type FnGetI32 = unsafe extern "C" fn(*mut c_void) -> c_int;
pub type FnSetI32 = unsafe extern "C" fn(*mut c_void, c_int);
pub type FnGetI64 = unsafe extern "C" fn(*mut c_void) -> i64;
pub type FnSetI64 = unsafe extern "C" fn(*mut c_void, i64);
pub type FnGetF32 = unsafe extern "C" fn(*mut c_void) -> f32;
pub type FnSetF32 = unsafe extern "C" fn(*mut c_void, f32);
pub type FnGetJson = unsafe extern "C" fn(*mut c_void) -> *mut c_char;
pub type FnJsonToStr = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char;
pub type FnJsonToI32 = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
pub type FnUiReq = unsafe extern "C" fn(*mut c_void, *const c_char);
pub type FnConfigGet = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *mut *mut c_char);

/// The host's services, as a function-pointer table (the `IHost` equivalent).
/// All fields are optional: NULL means "host does not implement this".
/// Every entry is panic-fenced: 0/NULL/false on panic.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct HostVtable {
    pub user_data: *mut c_void,
    // Audio player
    pub player_start: Option<FnVoidUnit>,
    pub player_pause: Option<FnVoidUnit>,
    pub player_stop: Option<FnVoidUnit>,
    pub player_resume: Option<FnVoidUnit>,
    pub player_get_is_paused: Option<FnGetI32>,
    pub player_set_is_paused: Option<FnSetI32>,
    pub player_is_stopped: Option<FnGetI32>,
    pub player_get_track: Option<FnGetStr>,
    pub player_set_track: Option<FnSetStr>,
    pub player_set_seek: Option<FnSetI32>,
    pub player_get_seek: Option<FnGetI32>,
    pub player_set_volume: Option<FnSetI32>,
    pub player_get_volume: Option<FnGetI32>,
    // Rotals
    pub rotals_get_rot: Option<FnGetStr>,
    pub rotals_set_rot: Option<FnSetStr>,
    // Tracks
    pub tracks_add: Option<FnVoidStr>,
    pub tracks_clear: Option<FnVoidUnit>,
    pub tracks_set_auto_rot: Option<FnSetI32>,
    // Core
    pub core_enable_shutdown: Option<FnSetStr>,
    pub core_disable_shutdown: Option<FnVoidUnit>,
    pub core_cancel_shutdown: Option<FnVoidUnit>,
    // Web radio
    pub web_is_enabled: Option<FnGetI32>,
    pub web_start: Option<FnVoidUnit>,
    pub web_stop: Option<FnVoidUnit>,
    pub web_restart: Option<FnVoidUnit>,
    pub web_add_stream: Option<FnVoidStr>,
    pub web_remove_stream: Option<FnVoidStr>,
    // Logging
    pub debug_add_line: Option<FnVoidStr>,
    // Interop / plugin window
    pub interop_hide_main_form: Option<FnVoidUnit>,
    pub interop_unhide_main_form: Option<FnVoidUnit>,
    pub interop_is_hidden: Option<FnGetI32>,
    pub interop_show_plugin_window: Option<FnUiReq>,
    pub interop_hide_plugin_window: Option<FnVoidStr>,
    pub interop_open_connection_string: Option<FnSetStr>,
    // Config
    pub config_get_setting: Option<FnConfigGet>,
    pub config_set_setting: Option<FnVoidStrStr>,
    // DB
    pub mysql_query: Option<FnJsonToStr>,
    pub mysql_insert: Option<FnJsonToI32>,
    pub get_songs: Option<FnJsonToStr>,
    // Media
    pub media_open: Option<FnVoidStr>,
    pub media_close: Option<FnVoidUnit>,
    pub media_get_playing: Option<FnGetI32>,
    pub media_set_volume: Option<FnSetI32>,
    pub media_set_mute: Option<FnSetI32>,
    pub media_get_mute: Option<FnGetI32>,
    pub media_set_pan: Option<FnSetI32>,
    pub media_get_pan: Option<FnGetI32>,
    pub media_get_length: Option<FnGetI64>,
    pub media_get_position: Option<FnGetI64>,
    pub media_set_position: Option<FnSetI64>,
    pub media_play: Option<FnVoidUnit>,
    pub media_pause: Option<FnVoidUnit>,
    pub media_stop: Option<FnVoidUnit>,
    // CAPI
    pub capi_set_message: Option<FnVoidStr>,
    // Engine
    pub engine_get_state: Option<FnGetJson>,
    pub engine_set_transition: Option<FnSetI32>,
    pub engine_get_transition: Option<FnGetI32>,
    pub engine_set_paused: Option<FnSetI32>,
    pub engine_get_paused: Option<FnGetI32>,
    // Webcaster
    pub webcaster_get_is_connected: Option<FnGetI32>,
    pub webcaster_get_dnc_count: Option<FnGetI64>,
    pub webcaster_get_stream_uptime: Option<FnGetI64>,
    pub webcaster_get_global_uptime: Option<FnGetI64>,
    // Webcaster listener list
    pub webcaster_listener_get_count: Option<FnGetI32>,
    pub webcaster_listener_get_listener: Option<FnJsonToStr>,
    pub webcaster_listener_get_listeners: Option<FnGetJson>,
    // Misc
    pub get_startup: Option<FnVoidStr>,
    pub set_startup: Option<FnVoidStrStr>,
    pub get_version: Option<FnGetStr>,
    pub restart: Option<FnVoidUnit>,
    // Vuscope
    pub vuscope_add_to_vuscope: Option<FnSetI32>,
    pub vuscope_remove_from_vuscope: Option<FnSetI32>,
    pub vuscope_set_peak_left: Option<FnSetF32>,
    pub vuscope_set_peak_right: Option<FnSetF32>,
    pub vuscope_set_playing: Option<FnSetI32>,
    pub vuscope_set_scrolling: Option<FnSetI32>,
    pub vuscope_set_scrolling2: Option<FnSetI32>,
    pub vuscope_set_clip: Option<FnVoidStr>,
    pub vuscope_set_level_left: Option<FnSetF32>,
    pub vuscope_set_level_right: Option<FnSetF32>,
    // Sound cards
    pub sound_cards_get_count: Option<FnGetI32>,
    pub sound_cards_get_get_sound_card: Option<FnJsonToStr>,
}

// --- Host-call shims (host implements; the plugin crate calls through them) ---

macro_rules! shim_void0 {
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
macro_rules! shim_void_str {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, s: *const c_char) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let s = unsafe { read_cstr(s) };
                if let Ok(c) = CString::new(s) {
                    let _ = host_call(|| unsafe { f(host.user_data, c.as_ptr()) });
                }
            }
        }
    };
}
macro_rules! shim_void_str_str {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, a: *const c_char, b: *const c_char) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let (a, b) = (unsafe { read_cstr(a) }, unsafe { read_cstr(b) });
                if let (Ok(a), Ok(b)) = (CString::new(a), CString::new(b)) {
                    let _ = host_call(|| unsafe { f(host.user_data, a.as_ptr(), b.as_ptr()) });
                }
            }
        }
    };
}
macro_rules! shim_set_str {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, s: *const c_char) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let s = unsafe { read_cstr(s) };
                if let Ok(c) = CString::new(s) {
                    let _ = host_call(|| unsafe { f(host.user_data, c.as_ptr()) });
                }
            }
        }
    };
}
macro_rules! shim_get_str {
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
                let mut p: *mut c_char = std::ptr::null_mut();
                if host_call(|| unsafe { f(host.user_data, &mut p) }).is_ok() && !p.is_null() {
                    unsafe { *out = p };
                }
            }
        }
    };
}
macro_rules! shim_get_bool {
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
macro_rules! shim_set_i32 {
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
macro_rules! shim_get_i32 {
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
macro_rules! shim_set_i64 {
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
macro_rules! shim_get_i64 {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable) -> i64 {
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
macro_rules! shim_set_f32 {
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
macro_rules! shim_get_json {
    ($name:ident, $field:ident, $doc:expr, $ty:ty) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable) -> $ty {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return <$ty>::default(),
            };
            match host.$field {
                Some(f) => {
                    let p = host_call(|| unsafe { f(host.user_data) }).unwrap_or(std::ptr::null_mut());
                    consume_json::<$ty>(p)
                }
                None => <$ty>::default(),
            }
        }
    };
}
macro_rules! shim_json_to_str {
    ($name:ident, $field:ident, $doc:expr, $in:ty, $out:ty) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, arg: $in) -> $out {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return <$out>::default(),
            };
            match host.$field {
                Some(f) => {
                    let input = to_json_ptr(&arg);
                    if input.is_null() {
                        return <$out>::default();
                    }
                    let p = host_call(|| unsafe { f(host.user_data, input) })
                        .unwrap_or(std::ptr::null_mut());
                    consume_json::<$out>(p)
                }
                None => <$out>::default(),
            }
        }
    };
}
macro_rules! shim_json_to_i32 {
    ($name:ident, $field:ident, $doc:expr, $in:ty) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, arg: $in) -> i32 {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return 0,
            };
            match host.$field {
                Some(f) => {
                    let input = to_json_ptr(&arg);
                    if input.is_null() {
                        return 0;
                    }
                    host_call(|| unsafe { f(host.user_data, input) }).unwrap_or(0)
                }
                None => 0,
            }
        }
    };
}
macro_rules! shim_config_get {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(
            host: *mut HostVtable,
            catalog: *const c_char,
            setting: *const c_char,
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
                let c = CString::new(unsafe { read_cstr(catalog) }).unwrap_or_default();
                let s = CString::new(unsafe { read_cstr(setting) }).unwrap_or_default();
                let mut p: *mut c_char = std::ptr::null_mut();
                if host_call(|| unsafe { f(host.user_data, c.as_ptr(), s.as_ptr(), &mut p) }).is_ok()
                    && !p.is_null()
                {
                    unsafe { *out = p };
                }
            }
        }
    };
}
macro_rules! shim_ui_req {
    ($name:ident, $field:ident, $doc:expr) => {
        #[doc = $doc]
        pub unsafe extern "C" fn $name(host: *mut HostVtable, req: &PluginUiRequest) {
            let host = match unsafe { host.as_ref() } {
                Some(h) => h,
                None => return,
            };
            if let Some(f) = host.$field {
                let json = to_json_ptr(req);
                if !json.is_null() {
                    let _ = host_call(|| unsafe { f(host.user_data, json) });
                    unsafe { radiocraft_abi_free_string(json) };
                }
            }
        }
    };
}

shim_void0!(host_player_start, player_start, "IHost: `void player_start()` — starts the audio player.");
shim_void0!(host_player_pause, player_pause, "IHost: `void player_pause()` — pauses playback.");
shim_void0!(host_player_stop, player_stop, "IHost: `void player_stop()` — stops playback.");
shim_void0!(host_player_resume, player_resume, "IHost: `void player_resume()` — resumes playback.");
shim_get_bool!(host_player_get_is_paused, player_get_is_paused, "IHost: `bool get_IsPlayerPaused { get; }`");
shim_set_i32!(host_player_set_is_paused, player_set_is_paused, "IHost: `void set_IsPlayerPaused { set; }`");
shim_get_bool!(host_player_is_stopped, player_is_stopped, "IHost: `bool get_IsPlayerStopped { get; }`");
shim_get_str!(host_player_get_track, player_get_track, "IHost: `string get_CurrentTrackAsFile { get; }`");
shim_set_str!(host_player_set_track, player_set_track, "IHost: `void set_CurrentTrackAsFile { set; }`");
shim_set_i32!(host_player_set_seek, player_set_seek, "IHost: `void set_TrackStartAt { set; }` (seek position in ms)");
shim_get_i32!(host_player_get_seek, player_get_seek, "IHost: `int get_TrackStartAt { get; }` (seek position in ms)");
shim_set_i32!(host_player_set_volume, player_set_volume, "IHost: `void set_TrackVolume { set; }`");
shim_get_i32!(host_player_get_volume, player_get_volume, "IHost: `int get_TrackVolume { get; }`");

shim_get_str!(host_rotals_get_rot, rotals_get_rot, "IHost: `string get_Rot { get; }` — current rotation name.");
shim_set_str!(host_rotals_set_rot, rotals_set_rot, "IHost: `void set_Rot { set; }` — set the rotation.");

shim_void_str!(host_tracks_add, tracks_add, "IHost: `void addToPlaylist(string file)` — file is a JSON-encoded SongData row.");
shim_void0!(host_tracks_clear, tracks_clear, "IHost: `void clearPlaylist()`");
shim_set_i32!(host_tracks_set_auto_rot, tracks_set_auto_rot, "IHost: `void set_isAutoRot { set; }`");

shim_set_str!(host_core_enable_shutdown, core_enable_shutdown, "IHost: `void enableShutdown(string msg)` — Windows shutdown.");
shim_void0!(host_core_disable_shutdown, core_disable_shutdown, "IHost: `void disableShutdown()`");
shim_void0!(host_core_cancel_shutdown, core_cancel_shutdown, "IHost: `void cancelShutdown()`");

shim_get_bool!(host_web_is_enabled, web_is_enabled, "IHost: `bool get_isWebcastEnabled { get; }`");
shim_void0!(host_web_start, web_start, "IHost: `void startWebShow()`");
shim_void0!(host_web_stop, web_stop, "IHost: `void stopWebShow()`");
shim_void0!(host_web_restart, web_restart, "IHost: `void restartWebShow()`");
shim_void_str!(host_web_add_stream, web_add_stream, "IHost: `void addStreamToWebcast(string file)`");
shim_void_str!(host_web_remove_stream, web_remove_stream, "IHost: `void removeStreamFromWebcast(string file)`");

shim_void_str!(host_debug_add_line, debug_add_line, "IHost: `void addLineToDebug(string msg)`");

shim_void0!(host_interop_hide_main_form, interop_hide_main_form, "IHost: `void hideMainForm()`");
shim_void0!(host_interop_unhide_main_form, interop_unhide_main_form, "IHost: `void unhideMainForm()`");
shim_get_bool!(host_interop_is_hidden, interop_is_hidden, "IHost: `bool get_isMainHidden { get; }`");
shim_ui_req!(host_interop_show_plugin_window, interop_show_plugin_window, "IHost: `void showPluginWindow(...)` — UI request as JSON (PluginUiRequest).");
shim_void_str!(host_interop_hide_plugin_window, interop_hide_plugin_window, "IHost: `void hidePluginWindow(string windowName)`");
shim_set_str!(host_interop_open_connection_string, interop_open_connection_string, "IHost: `void OpenConnectionString(string connStr)`");

shim_config_get!(host_config_get_setting, config_get_setting, "IHost: `string getGeneralSetting(string catalog, string setting)` — host writes a heap string to `out`.");
shim_void_str_str!(host_config_set_setting, config_set_setting, "IHost: `void saveGeneralSetting(string catalog, string value)`");

shim_json_to_str!(host_mysql_query, mysql_query, "IHost: `string mysql_query(string sql, string args)` — DB results as JSON.", (&str, &str), String);
shim_json_to_i32!(host_mysql_insert, mysql_insert, "IHost: `int mysql_insert(string sql, string args)` — affected rows.", (&str, &str));
shim_json_to_str!(host_get_songs, get_songs, "IHost: `void getSongs(string sql, ref DataGridView dgv)` — rows as JSON list of SongData.", &str, Vec<SongData>);

shim_void_str!(host_media_open, media_open, "IHost: `void media_Open(string file)`");
shim_void0!(host_media_close, media_close, "IHost: `void media_Close()`");
shim_get_bool!(host_media_get_playing, media_get_playing, "IHost: `bool get_mediaPlaying { get; }`");
shim_set_i32!(host_media_set_volume, media_set_volume, "IHost: `void set_mediaVolume { set; }`");
shim_set_i32!(host_media_set_mute, media_set_mute, "IHost: `void set_mediaMute { set; }`");
shim_get_bool!(host_media_get_mute, media_get_mute, "IHost: `bool get_mediaMute { get; }`");
shim_set_i32!(host_media_set_pan, media_set_pan, "IHost: `void set_mediaPan { set; }`");
shim_get_i32!(host_media_get_pan, media_get_pan, "IHost: `int get_mediaPan { get; }`");
shim_get_i64!(host_media_get_length, media_get_length, "IHost: `long get_mediaLength { get; }`");
shim_get_i64!(host_media_get_position, media_get_position, "IHost: `long get_mediaPosition { get; }`");
shim_set_i64!(host_media_set_position, media_set_position, "IHost: `void set_mediaPosition { set; }`");
shim_void0!(host_media_play, media_play, "IHost: `void media_Play()`");
shim_void0!(host_media_pause, media_pause, "IHost: `void media_Pause()`");
shim_void0!(host_media_stop, media_stop, "IHost: `void media_Stop()`");

shim_void_str!(host_capi_set_message, capi_set_message, "IHost: `void capi_setMessage(string message)`");

shim_get_json!(host_engine_get_state, engine_get_state, "IHost: `StateInfo get_State { get; }` — serialized StateInfo.", StateInfo);
shim_set_i32!(host_engine_set_transition, engine_set_transition, "IHost: `void set_isTransition { set; }`");
shim_get_i32!(host_engine_get_transition, engine_get_transition, "IHost: `bool get_isTransition { get; }`");
shim_set_i32!(host_engine_set_paused, engine_set_paused, "IHost: `void set_isEnginePaused { set; }`");
shim_get_bool!(host_engine_get_paused, engine_get_paused, "IHost: `bool get_isEnginePaused { get; }`");

shim_get_bool!(host_webcaster_get_is_connected, webcaster_get_is_connected, "IHost: `bool get_isWcConnected { get; }`");
shim_get_i64!(host_webcaster_get_dnc_count, webcaster_get_dnc_count, "IHost: `long get_dncCount { get; }`");
shim_get_i64!(host_webcaster_get_stream_uptime, webcaster_get_stream_uptime, "IHost: `long get_streamUptime { get; }`");
shim_get_i64!(host_webcaster_get_global_uptime, webcaster_get_global_uptime, "IHost: `long get_globalUptime { get; }`");

shim_get_i32!(host_webcaster_listener_get_count, webcaster_listener_get_count, "IHost: `int get_wcListenerCount { get; }`");
shim_json_to_str!(host_webcaster_listener_get_listener, webcaster_listener_get_listener, "IHost: `string get_wcListener(int index)` — JSON value.", i32, serde_json::Value);
shim_get_json!(host_webcaster_listener_get_listeners, webcaster_listener_get_listeners, "IHost: `object get_allListeners { get; }` — JSON list.", Vec<serde_json::Value>);

shim_get_str!(host_get_startup, get_startup, "IHost: `string get_Startup { get; }`");
shim_void_str_str!(host_set_startup, set_startup, "IHost: `void setStartup(string startup, bool flag)` — flag is 'true'/'false' in the string slot.");
shim_get_str!(host_get_version, get_version, "IHost: `string get_Version { get; }`");
shim_void0!(host_restart, restart, "IHost: `void doRestart()`");

shim_set_i32!(host_vuscope_add_to_vuscope, vuscope_add_to_vuscope, "IHost: `void addToVuscope(IntPtr pBox)`");
shim_set_i32!(host_vuscope_remove_from_vuscope, vuscope_remove_from_vuscope, "IHost: `void removeFromVuscope(IntPtr pBox)`");
shim_set_f32!(host_vuscope_set_peak_left, vuscope_set_peak_left, "IHost: `void set_peakLeft { set; }`");
shim_set_f32!(host_vuscope_set_peak_right, vuscope_set_peak_right, "IHost: `void set_peakRight { set; }`");
shim_set_i32!(host_vuscope_set_playing, vuscope_set_playing, "IHost: `void set_isPlaying { set; }`");
shim_set_i32!(host_vuscope_set_scrolling, vuscope_set_scrolling, "IHost: `void set_scrollLeft { set; }`");
shim_set_i32!(host_vuscope_set_scrolling2, vuscope_set_scrolling2, "IHost: `void set_scrollRight { set; }`");
shim_void_str!(host_vuscope_set_clip, vuscope_set_clip, "IHost: `void set_clip { set; }`");
shim_set_f32!(host_vuscope_set_level_left, vuscope_set_level_left, "IHost: `void set_leftLevel { set; }`");
shim_set_f32!(host_vuscope_set_level_right, vuscope_set_level_right, "IHost: `void set_rightLevel { set; }`");

shim_get_i32!(host_sound_cards_get_count, sound_cards_get_count, "IHost: `int get_soundCardCount { get; }`");
shim_json_to_str!(host_sound_cards_get_get_sound_card, sound_cards_get_get_sound_card, "IHost: `object get_GetSoundCard(CardTypes type)` — card JSON.", i32, serde_json::Value);

/// Build a `HostVtable` whose entries all point at the wrapper shims above.
pub fn host_vtable() -> HostVtable {
    HostVtable {
        user_data: std::ptr::null_mut(),
        player_start: Some(host_player_start as FnVoidUnit),
        player_pause: Some(host_player_pause as FnVoidUnit),
        player_stop: Some(host_player_stop as FnVoidUnit),
        player_resume: Some(host_player_resume as FnVoidUnit),
        player_get_is_paused: Some(host_player_get_is_paused as FnGetI32),
        player_set_is_paused: Some(host_player_set_is_paused as FnSetI32),
        player_is_stopped: Some(host_player_is_stopped as FnGetI32),
        player_get_track: Some(host_player_get_track as FnGetStr),
        player_set_track: Some(host_player_set_track as FnSetStr),
        player_set_seek: Some(host_player_set_seek as FnSetI32),
        player_get_seek: Some(host_player_get_seek as FnGetI32),
        player_set_volume: Some(host_player_set_volume as FnSetI32),
        player_get_volume: Some(host_player_get_volume as FnGetI32),
        rotals_get_rot: Some(host_rotals_get_rot as FnGetStr),
        rotals_set_rot: Some(host_rotals_set_rot as FnSetStr),
        tracks_add: Some(host_tracks_add as FnVoidStr),
        tracks_clear: Some(host_tracks_clear as FnVoidUnit),
        tracks_set_auto_rot: Some(host_tracks_set_auto_rot as FnSetI32),
        core_enable_shutdown: Some(host_core_enable_shutdown as FnSetStr),
        core_disable_shutdown: Some(host_core_disable_shutdown as FnVoidUnit),
        core_cancel_shutdown: Some(host_core_cancel_shutdown as FnVoidUnit),
        web_is_enabled: Some(host_web_is_enabled as FnGetI32),
        web_start: Some(host_web_start as FnVoidUnit),
        web_stop: Some(host_web_stop as FnVoidUnit),
        web_restart: Some(host_web_restart as FnVoidUnit),
        web_add_stream: Some(host_web_add_stream as FnVoidStr),
        web_remove_stream: Some(host_web_remove_stream as FnVoidStr),
        debug_add_line: Some(host_debug_add_line as FnVoidStr),
        interop_hide_main_form: Some(host_interop_hide_main_form as FnVoidUnit),
        interop_unhide_main_form: Some(host_interop_unhide_main_form as FnVoidUnit),
        interop_is_hidden: Some(host_interop_is_hidden as FnGetI32),
        interop_show_plugin_window: Some(host_interop_show_plugin_window as FnUiReq),
        interop_hide_plugin_window: Some(host_interop_hide_plugin_window as FnVoidStr),
        interop_open_connection_string: Some(host_interop_open_connection_string as FnSetStr),
        config_get_setting: Some(host_config_get_setting as FnConfigGet),
        config_set_setting: Some(host_config_set_setting as FnVoidStrStr),
        mysql_query: Some(host_mysql_query as FnJsonToStr),
        mysql_insert: Some(host_mysql_insert as FnJsonToI32),
        get_songs: Some(host_get_songs as FnJsonToStr),
        media_open: Some(host_media_open as FnVoidStr),
        media_close: Some(host_media_close as FnVoidUnit),
        media_get_playing: Some(host_media_get_playing as FnGetI32),
        media_set_volume: Some(host_media_set_volume as FnSetI32),
        media_set_mute: Some(host_media_set_mute as FnSetI32),
        media_get_mute: Some(host_media_get_mute as FnGetI32),
        media_set_pan: Some(host_media_set_pan as FnSetI32),
        media_get_pan: Some(host_media_get_pan as FnGetI32),
        media_get_length: Some(host_media_get_length as FnGetI64),
        media_get_position: Some(host_media_get_position as FnGetI64),
        media_set_position: Some(host_media_set_position as FnSetI64),
        media_play: Some(host_media_play as FnVoidUnit),
        media_pause: Some(host_media_pause as FnVoidUnit),
        media_stop: Some(host_media_stop as FnVoidUnit),
        capi_set_message: Some(host_capi_set_message as FnVoidStr),
        engine_get_state: Some(host_engine_get_state as FnGetJson),
        engine_set_transition: Some(host_engine_set_transition as FnSetI32),
        engine_get_transition: Some(host_engine_get_transition as FnGetI32),
        engine_set_paused: Some(host_engine_set_paused as FnSetI32),
        engine_get_paused: Some(host_engine_get_paused as FnGetI32),
        webcaster_get_is_connected: Some(host_webcaster_get_is_connected as FnGetI32),
        webcaster_get_dnc_count: Some(host_webcaster_get_dnc_count as FnGetI64),
        webcaster_get_stream_uptime: Some(host_webcaster_get_stream_uptime as FnGetI64),
        webcaster_get_global_uptime: Some(host_webcaster_get_global_uptime as FnGetI64),
        webcaster_listener_get_count: Some(host_webcaster_listener_get_count as FnGetI32),
        webcaster_listener_get_listener: Some(host_webcaster_listener_get_listener as FnJsonToStr),
        webcaster_listener_get_listeners: Some(host_webcaster_listener_get_listeners as FnGetJson),
        get_startup: Some(host_get_startup as FnGetStr),
        set_startup: Some(host_set_startup as FnVoidStrStr),
        get_version: Some(host_get_version as FnGetStr),
        restart: Some(host_restart as FnVoidUnit),
        vuscope_add_to_vuscope: Some(host_vuscope_add_to_vuscope as FnSetI32),
        vuscope_remove_from_vuscope: Some(host_vuscope_remove_from_vuscope as FnSetI32),
        vuscope_set_peak_left: Some(host_vuscope_set_peak_left as FnSetF32),
        vuscope_set_peak_right: Some(host_vuscope_set_peak_right as FnSetF32),
        vuscope_set_playing: Some(host_vuscope_set_playing as FnSetI32),
        vuscope_set_scrolling: Some(host_vuscope_set_scrolling as FnSetI32),
        vuscope_set_scrolling2: Some(host_vuscope_set_scrolling2 as FnSetI32),
        vuscope_set_clip: Some(host_vuscope_set_clip as FnVoidStr),
        vuscope_set_level_left: Some(host_vuscope_set_level_left as FnSetF32),
        vuscope_set_level_right: Some(host_vuscope_set_level_right as FnSetF32),
        sound_cards_get_count: Some(host_sound_cards_get_count as FnGetI32),
        sound_cards_get_get_sound_card: Some(host_sound_cards_get_get_sound_card as FnJsonToStr),
    }
}

/// Plugin-side wrapper: implement `Host` by calling through a `HostVtable`
/// received from the host. This is what plugins see inside `init()`/actions.
#[derive(Clone)]
pub struct HostBridge {
    vt: HostVtable,
}

impl HostBridge {
    /// Wrap a host-provided vtable.
    ///
    /// # Safety
    /// `vt` must be a valid `HostVtable` (as produced by [`HostHandle::vtable`])
    /// whose `user_data` stays live for the whole lifetime of the bridge.
    pub unsafe fn new(vt: HostVtable) -> Self {
        Self { vt }
    }

    /// Wrap a partially-filled vtable (NULL entries fall back to no-op/defaults).
    pub fn from_partial(vt: HostVtable) -> Self {
        Self { vt }
    }
}

impl Host for HostBridge {
    fn player_start(&self) { unsafe { host_player_start(&self.vt) } }
    fn player_pause(&self) { unsafe { host_player_pause(&self.vt) } }
    fn player_stop(&self) { unsafe { host_player_stop(&self.vt) } }
    fn player_resume(&self) { unsafe { host_player_resume(&self.vt) } }
    fn player_get_is_paused(&self) -> bool { unsafe { host_player_get_is_paused(&self.vt) } }
    fn player_set_is_paused(&self, value: bool) { unsafe { host_player_set_is_paused(&self.vt, i32::from(value)) } }
    fn player_is_stopped(&self) -> bool { unsafe { host_player_is_stopped(&self.vt) } }
    fn player_get_track(&self) -> String { unsafe { consume_string(host_player_get_track_ptr(&self.vt)) } }
    fn player_set_track(&self, value: &str) { unsafe { host_player_set_track(&self.vt, to_cstr_ptr(value)) } }
    fn player_set_seek(&self, value: i32) { unsafe { host_player_set_seek(&self.vt, value) } }
    fn player_get_seek(&self) -> i32 { unsafe { host_player_get_seek(&self.vt) } }
    fn player_set_volume(&self, value: i32) { unsafe { host_player_set_volume(&self.vt, value) } }
    fn player_get_volume(&self) -> i32 { unsafe { host_player_get_volume(&self.vt) } }
    fn rotals_get_rot(&self) -> String { unsafe { consume_string(host_rotals_get_rot_ptr(&self.vt)) } }
    fn rotals_set_rot(&self, value: &str) { unsafe { host_rotals_set_rot(&self.vt, to_cstr_ptr(value)) } }
    fn tracks_add(&self, song: &SongData) {
        let json = to_json_ptr(song);
        if !json.is_null() {
            unsafe { host_tracks_add(&self.vt, json) };
            unsafe { radiocraft_abi_free_string(json) };
        }
    }
    fn tracks_clear(&self) { unsafe { host_tracks_clear(&self.vt) } }
    fn tracks_set_auto_rot(&self, value: bool) { unsafe { host_tracks_set_auto_rot(&self.vt, i32::from(value)) } }
    fn core_enable_shutdown(&self, reason: &str) { unsafe { host_core_enable_shutdown(&self.vt, to_cstr_ptr(reason)) } }
    fn core_disable_shutdown(&self) { unsafe { host_core_disable_shutdown(&self.vt) } }
    fn core_cancel_shutdown(&self) { unsafe { host_core_cancel_shutdown(&self.vt) } }
    fn web_is_enabled(&self) -> bool { unsafe { host_web_is_enabled(&self.vt) } }
    fn web_start(&self) { unsafe { host_web_start(&self.vt) } }
    fn web_stop(&self) { unsafe { host_web_stop(&self.vt) } }
    fn web_restart(&self) { unsafe { host_web_restart(&self.vt) } }
    fn web_add_stream(&self, file: &str) { unsafe { host_web_add_stream(&self.vt, to_cstr_ptr(file)) } }
    fn web_remove_stream(&self, file: &str) { unsafe { host_web_remove_stream(&self.vt, to_cstr_ptr(file)) } }
    fn debug_add_line(&self, message: &str) { unsafe { host_debug_add_line(&self.vt, to_cstr_ptr(message)) } }
    fn interop_hide_main_form(&self) { unsafe { host_interop_hide_main_form(&self.vt) } }
    fn interop_unhide_main_form(&self) { unsafe { host_interop_unhide_main_form(&self.vt) } }
    fn interop_is_hidden(&self) -> bool { unsafe { host_interop_is_hidden(&self.vt) } }
    fn interop_show_plugin_window(&self, request: &PluginUiRequest) {
        unsafe { host_interop_show_plugin_window(&self.vt, request) }
    }
    fn interop_hide_plugin_window(&self, window_name: &str) {
        unsafe { host_interop_hide_plugin_window(&self.vt, to_cstr_ptr(window_name)) }
    }
    fn interop_open_connection_string(&self, connection_string: &str) {
        unsafe { host_interop_open_connection_string(&self.vt, to_cstr_ptr(connection_string)) }
    }
    fn config_get_setting(&self, catalog: &str, setting: &str) -> String {
        let c_catalog = CString::new(catalog).unwrap_or_default();
        let c_setting = CString::new(setting).unwrap_or_default();
        let mut out: *mut c_char = std::ptr::null_mut();
        unsafe { host_config_get_setting(&self.vt, c_catalog.as_ptr(), c_setting.as_ptr(), &mut out) };
        consume_string(out)
    }
    fn config_set_setting(&self, catalog: &str, setting: &str) {
        unsafe { host_config_set_setting(&self.vt, to_cstr_ptr(catalog), to_cstr_ptr(setting)) }
    }
    fn mysql_query(&self, sql: &str, args: &[&str]) -> String {
        let input = to_json_ptr(&(sql, args));
        let out = if !input.is_null() {
            unsafe { host_mysql_query_raw(&self.vt, input) }
        } else {
            std::ptr::null_mut()
        };
        consume_string(out)
    }
    fn mysql_insert(&self, sql: &str, args: &[&str]) -> i32 {
        let input = to_json_ptr(&(sql, args));
        if input.is_null() {
            return 0;
        }
        unsafe { host_mysql_insert_raw(&self.vt, input) }
    }
    fn get_songs(&self, sql: &str) -> Vec<SongData> {
        let input = to_json_ptr(&sql);
        let p = if !input.is_null() {
            unsafe { host_get_songs_raw(&self.vt, input) }
        } else {
            std::ptr::null_mut()
        };
        consume_json(p)
    }
    fn media_open(&self, file: &str) { unsafe { host_media_open(&self.vt, to_cstr_ptr(file)) } }
    fn media_close(&self) { unsafe { host_media_close(&self.vt) } }
    fn media_get_playing(&self) -> bool { unsafe { host_media_get_playing(&self.vt) } }
    fn media_set_volume(&self, value: i32) { unsafe { host_media_set_volume(&self.vt, value) } }
    fn media_set_mute(&self, value: bool) { unsafe { host_media_set_mute(&self.vt, i32::from(value)) } }
    fn media_get_mute(&self) -> bool { unsafe { host_media_get_mute(&self.vt) } }
    fn media_set_pan(&self, value: i32) { unsafe { host_media_set_pan(&self.vt, value) } }
    fn media_get_pan(&self) -> i32 { unsafe { host_media_get_pan(&self.vt) } }
    fn media_get_length(&self) -> i64 { unsafe { host_media_get_length(&self.vt) } }
    fn media_get_position(&self) -> i64 { unsafe { host_media_get_position(&self.vt) } }
    fn media_set_position(&self, value: i64) { unsafe { host_media_set_position(&self.vt, value) } }
    fn media_play(&self) { unsafe { host_media_play(&self.vt) } }
    fn media_pause(&self) { unsafe { host_media_pause(&self.vt) } }
    fn media_stop(&self) { unsafe { host_media_stop(&self.vt) } }
    fn capi_set_message(&self, message: &str) { unsafe { host_capi_set_message(&self.vt, to_cstr_ptr(message)) } }
    fn engine_get_state(&self) -> StateInfo { unsafe { host_engine_get_state(&self.vt) } }
    fn engine_set_transition(&self, value: bool) { unsafe { host_engine_set_transition(&self.vt, i32::from(value)) } }
    fn engine_get_transition(&self) -> bool { unsafe { host_engine_get_transition(&self.vt) != 0 } }
    fn engine_set_paused(&self, value: bool) { unsafe { host_engine_set_paused(&self.vt, i32::from(value)) } }
    fn engine_get_paused(&self) -> bool { unsafe { host_engine_get_paused(&self.vt) } }
    fn webcaster_get_is_connected(&self) -> bool { unsafe { host_webcaster_get_is_connected(&self.vt) } }
    fn webcaster_get_dnc_count(&self) -> i64 { unsafe { host_webcaster_get_dnc_count(&self.vt) } }
    fn webcaster_get_stream_uptime(&self) -> i64 { unsafe { host_webcaster_get_stream_uptime(&self.vt) } }
    fn webcaster_get_global_uptime(&self) -> i64 { unsafe { host_webcaster_get_global_uptime(&self.vt) } }
    fn webcaster_listener_get_count(&self) -> i32 { unsafe { host_webcaster_listener_get_count(&self.vt) } }
    fn webcaster_listener_get_listener(&self, index: i32) -> serde_json::Value {
        unsafe { host_webcaster_listener_get_listener(&self.vt, index) }
    }
    fn webcaster_listener_get_listeners(&self) -> Vec<serde_json::Value> {
        unsafe { host_webcaster_listener_get_listeners(&self.vt) }
    }
    fn get_startup(&self) -> String { unsafe { consume_string(host_get_startup_ptr(&self.vt)) } }
    fn set_startup(&self, program: &str, enable: bool) {
        unsafe { host_set_startup(&self.vt, to_cstr_ptr(program), to_cstr_ptr(if enable { "true" } else { "false" })) }
    }
    fn get_version(&self) -> String { unsafe { consume_string(host_get_version_ptr(&self.vt)) } }
    fn restart(&self) { unsafe { host_restart(&self.vt) } }
    fn vuscope_add_to_vuscope(&self, widget: i32) { unsafe { host_vuscope_add_to_vuscope(&self.vt, widget) } }
    fn vuscope_remove_from_vuscope(&self, widget: i32) { unsafe { host_vuscope_remove_from_vuscope(&self.vt, widget) } }
    fn vuscope_set_peak_left(&self, value: f32) { unsafe { host_vuscope_set_peak_left(&self.vt, value) } }
    fn vuscope_set_peak_right(&self, value: f32) { unsafe { host_vuscope_set_peak_right(&self.vt, value) } }
    fn vuscope_set_playing(&self, value: bool) { unsafe { host_vuscope_set_playing(&self.vt, i32::from(value)) } }
    fn vuscope_set_scrolling(&self, value: bool) { unsafe { host_vuscope_set_scrolling(&self.vt, i32::from(value)) } }
    fn vuscope_set_scrolling2(&self, value: bool) { unsafe { host_vuscope_set_scrolling2(&self.vt, i32::from(value)) } }
    fn vuscope_set_clip(&self, clip: &str) { unsafe { host_vuscope_set_clip(&self.vt, to_cstr_ptr(clip)) } }
    fn vuscope_set_level_left(&self, value: f32) { unsafe { host_vuscope_set_level_left(&self.vt, value) } }
    fn vuscope_set_level_right(&self, value: f32) { unsafe { host_vuscope_set_level_right(&self.vt, value) } }
    fn sound_cards_get_count(&self) -> i32 { unsafe { host_sound_cards_get_count(&self.vt) } }
    fn get_get_sound_card(&self, card_type: i32) -> serde_json::Value {
        unsafe { host_sound_cards_get_get_sound_card(&self.vt, card_type) }
    }
}

// Direct vtable calls for the JSON-returning entries (the shim macros above
// already do the decode for `&self` wrappers, but HostBridge takes ownership of
// the returned string pointers so they are called raw here).
unsafe fn host_mysql_query_raw(vt: &HostVtable, input: *mut c_char) -> *mut c_char {
    match vt.mysql_query {
        Some(f) => host_call(|| unsafe { f(vt.user_data, input) }).unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}
unsafe fn host_mysql_insert_raw(vt: &HostVtable, input: *mut c_char) -> i32 {
    match vt.mysql_insert {
        Some(f) => host_call(|| unsafe { f(vt.user_data, input) }).unwrap_or(0),
        None => 0,
    }
}
unsafe fn host_get_songs_raw(vt: &HostVtable, input: *mut c_char) -> *mut c_char {
    match vt.get_songs {
        Some(f) => host_call(|| unsafe { f(vt.user_data, input) }).unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}
unsafe fn host_player_get_track_ptr(vt: &HostVtable) -> *mut c_char {
    let mut out: *mut c_char = std::ptr::null_mut();
    if let Some(f) = vt.player_get_track {
        let _ = host_call(|| unsafe { f(vt.user_data, &mut out) });
    }
    out
}
unsafe fn host_rotals_get_rot_ptr(vt: &HostVtable) -> *mut c_char {
    let mut out: *mut c_char = std::ptr::null_mut();
    if let Some(f) = vt.rotals_get_rot {
        let _ = host_call(|| unsafe { f(vt.user_data, &mut out) });
    }
    out
}
unsafe fn host_get_startup_ptr(vt: &HostVtable) -> *mut c_char {
    let mut out: *mut c_char = std::ptr::null_mut();
    if let Some(f) = vt.get_startup {
        let _ = host_call(|| unsafe { f(vt.user_data, &mut out) });
    }
    out
}
unsafe fn host_get_version_ptr(vt: &HostVtable) -> *mut c_char {
    let mut out: *mut c_char = std::ptr::null_mut();
    if let Some(f) = vt.get_version {
        let _ = host_call(|| unsafe { f(vt.user_data, &mut out) });
    }
    out
}
