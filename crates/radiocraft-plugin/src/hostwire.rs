//! Host-side ABI wiring: turn an `&dyn Host` implementation into a `HostVtable`
//! a plugin can call (the mirror of the plugin-side `HostBridge`).
//!
//! `user_data` is a heap `Box<&dyn Host>` (reborrowed from the handle). All wire
//! functions assume that layout; the payload encoding matches [`crate::abi`].

use std::ffi::{c_char, c_int, c_void};

use crate::abi::{read_cstr, to_cstr_ptr, to_json_ptr, HostVtable};
use crate::host::{Host, PluginUiRequest};
use radiocraft_core::model::SongData;

/// Decode a wire JSON pointer (never NULL-checked by callers via json_or).
fn decode<T: serde::de::DeserializeOwned>(ptr: *const c_char) -> Option<T> {
    if ptr.is_null() {
        return None;
    }
    serde_json::from_str(crate::abi::read_cstr(ptr)).ok()
}

/// Wire `user_data` back into a `&dyn Host`.
///
/// # Safety: `user` must be a live `Box<&dyn Host>` raw pointer.
unsafe fn host<'a>(user: *mut c_void) -> &'a dyn Host {
    unsafe { &**(user as *mut &dyn Host) }
}

macro_rules! wire_void0 {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void) {
            unsafe { host(user) }. $method();
        }
    };
}
macro_rules! wire_void_str {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void, s: *const c_char) {
            unsafe { host(user) }. $method(unsafe { read_cstr(s) });
        }
    };
}
macro_rules! wire_set_str {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void, s: *const c_char) {
            unsafe { host(user) }. $method(unsafe { read_cstr(s) });
        }
    };
}
macro_rules! wire_get_str {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void, out: *mut *mut c_char) {
            if out.is_null() {
                return;
            }
            let value = unsafe { host(user) }. $method();
            unsafe { *out = to_cstr_ptr(&value) };
        }
    };
}
macro_rules! wire_get_bool {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void) -> c_int {
            c_int::from(unsafe { host(user) }. $method())
        }
    };
}
macro_rules! wire_set_bool_as_i32 {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void, v: c_int) {
            unsafe { host(user) }. $method(v != 0);
        }
    };
}
macro_rules! wire_set_i32 {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void, v: c_int) {
            unsafe { host(user) }. $method(v);
        }
    };
}
macro_rules! wire_get_i32 {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void) -> c_int {
            unsafe { host(user) }. $method()
        }
    };
}
macro_rules! wire_get_i64 {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void) -> i64 {
            unsafe { host(user) }. $method()
        }
    };
}
macro_rules! wire_set_i64 {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void, v: i64) {
            unsafe { host(user) }. $method(v);
        }
    };
}
macro_rules! wire_set_f32 {
    ($name:ident, $method:ident) => {
        unsafe extern "C" fn $name(user: *mut c_void, v: f32) {
            unsafe { host(user) }. $method(v);
        }
    };
}

wire_void0!(w_player_start, player_start);
wire_void0!(w_player_pause, player_pause);
wire_void0!(w_player_stop, player_stop);
wire_void0!(w_player_resume, player_resume);
wire_get_bool!(w_player_get_is_paused, player_get_is_paused);
wire_set_bool_as_i32!(w_player_set_is_paused, player_set_is_paused);
wire_get_bool!(w_player_is_stopped, player_is_stopped);
wire_get_str!(w_player_get_track, player_get_track);
wire_set_str!(w_player_set_track, player_set_track);
wire_set_i32!(w_player_set_seek, player_set_seek);
wire_get_i32!(w_player_get_seek, player_get_seek);
wire_set_i32!(w_player_set_volume, player_set_volume);
wire_get_i32!(w_player_get_volume, player_get_volume);
wire_get_str!(w_rotals_get_rot, rotals_get_rot);
wire_set_str!(w_rotals_set_rot, rotals_set_rot);
wire_void0!(w_tracks_clear, tracks_clear);
wire_set_bool_as_i32!(w_tracks_set_auto_rot, tracks_set_auto_rot);
wire_set_str!(w_core_enable_shutdown, core_enable_shutdown);
wire_void0!(w_core_disable_shutdown, core_disable_shutdown);
wire_void0!(w_core_cancel_shutdown, core_cancel_shutdown);
wire_get_bool!(w_web_is_enabled, web_is_enabled);
wire_void0!(w_web_start, web_start);
wire_void0!(w_web_stop, web_stop);
wire_void0!(w_web_restart, web_restart);
wire_void_str!(w_web_add_stream, web_add_stream);
wire_void_str!(w_web_remove_stream, web_remove_stream);
wire_void_str!(w_debug_add_line, debug_add_line);
wire_void0!(w_interop_hide_main_form, interop_hide_main_form);
wire_void0!(w_interop_unhide_main_form, interop_unhide_main_form);
wire_get_bool!(w_interop_is_hidden, interop_is_hidden);
wire_set_str!(w_interop_open_connection_string, interop_open_connection_string);
wire_void_str!(w_media_open, media_open);
wire_void0!(w_media_close, media_close);
wire_get_bool!(w_media_get_playing, media_get_playing);
wire_set_i32!(w_media_set_volume, media_set_volume);
wire_set_bool_as_i32!(w_media_set_mute, media_set_mute);
wire_get_bool!(w_media_get_mute, media_get_mute);
wire_set_i32!(w_media_set_pan, media_set_pan);
wire_get_i32!(w_media_get_pan, media_get_pan);
wire_get_i64!(w_media_get_length, media_get_length);
wire_get_i64!(w_media_get_position, media_get_position);
wire_set_i64!(w_media_set_position, media_set_position);
wire_void0!(w_media_play, media_play);
wire_void0!(w_media_pause, media_pause);
wire_void0!(w_media_stop, media_stop);
wire_void_str!(w_capi_set_message, capi_set_message);
wire_set_bool_as_i32!(w_engine_set_transition, engine_set_transition);
wire_set_bool_as_i32!(w_engine_set_paused, engine_set_paused);
wire_get_bool!(w_engine_get_paused, engine_get_paused);
wire_get_bool!(w_webcaster_get_is_connected, webcaster_get_is_connected);
wire_get_i64!(w_webcaster_get_dnc_count, webcaster_get_dnc_count);
wire_get_i64!(w_webcaster_get_stream_uptime, webcaster_get_stream_uptime);
wire_get_i64!(w_webcaster_get_global_uptime, webcaster_get_global_uptime);
wire_get_i32!(w_webcaster_listener_get_count, webcaster_listener_get_count);
wire_get_str!(w_get_startup, get_startup);
wire_get_str!(w_get_version, get_version);
wire_void0!(w_restart, restart);
wire_set_i32!(w_vuscope_add_to_vuscope, vuscope_add_to_vuscope);
wire_set_i32!(w_vuscope_remove_from_vuscope, vuscope_remove_from_vuscope);
wire_set_f32!(w_vuscope_set_peak_left, vuscope_set_peak_left);
wire_set_f32!(w_vuscope_set_peak_right, vuscope_set_peak_right);
wire_set_bool_as_i32!(w_vuscope_set_playing, vuscope_set_playing);
wire_set_bool_as_i32!(w_vuscope_set_scrolling, vuscope_set_scrolling);
wire_set_bool_as_i32!(w_vuscope_set_scrolling2, vuscope_set_scrolling2);
wire_void_str!(w_vuscope_set_clip, vuscope_set_clip);
wire_set_f32!(w_vuscope_set_level_left, vuscope_set_level_left);
wire_set_f32!(w_vuscope_set_level_right, vuscope_set_level_right);
wire_get_i32!(w_sound_cards_get_count, sound_cards_get_count);

// Engine transition getter returns bool but vtable slot is FnGetI32.
unsafe extern "C" fn w_engine_get_transition(user: *mut c_void) -> c_int {
    c_int::from(unsafe { host(user) }.engine_get_transition())
}

// JSON/struct-bearing entries.
unsafe extern "C" fn w_tracks_add(user: *mut c_void, json: *const c_char) {
    if let Some(song) = decode::<SongData>(json) {
        unsafe { host(user) }.tracks_add(&song);
    }
}
unsafe extern "C" fn w_interop_show_plugin_window(user: *mut c_void, json: *const c_char) {
    if let Some(req) = decode::<PluginUiRequest>(json) {
        unsafe { host(user) }.interop_show_plugin_window(&req);
    }
}
unsafe extern "C" fn w_interop_hide_plugin_window(user: *mut c_void, name: *const c_char) {
    unsafe { host(user) }.interop_hide_plugin_window(unsafe { read_cstr(name) });
}
unsafe extern "C" fn w_config_set_setting(user: *mut c_void, catalog: *const c_char, setting: *const c_char) {
    unsafe { host(user) }.config_set_setting(unsafe { read_cstr(catalog) }, unsafe { read_cstr(setting) });
}
unsafe extern "C" fn w_config_get_setting(
    user: *mut c_void,
    catalog: *const c_char,
    setting: *const c_char,
    out: *mut *mut c_char,
) {
    if out.is_null() {
        return;
    }
    let value = unsafe { host(user) }.config_get_setting(unsafe { read_cstr(catalog) }, unsafe { read_cstr(setting) });
    unsafe { *out = to_cstr_ptr(&value) };
}
/// Wire payload for mysql_query / mysql_insert: JSON array `[sql, [args...]]`.
fn decode_query(json: *const c_char) -> Option<(String, Vec<String>)> {
    decode::<(String, Vec<String>)>(json).map(|(sql, args)| (sql, args))
}
unsafe extern "C" fn w_mysql_query(user: *mut c_void, json: *const c_char) -> *mut c_char {
    if let Some((sql, args)) = decode_query(json) {
        let result = unsafe { host(user) }.mysql_query(&sql, &args.iter().map(String::as_str).collect::<Vec<_>>());
        return to_cstr_ptr(&result);
    }
    std::ptr::null_mut()
}
unsafe extern "C" fn w_mysql_insert(user: *mut c_void, json: *const c_char) -> c_int {
    if let Some((sql, args)) = decode_query(json) {
        return unsafe { host(user) }.mysql_insert(&sql, &args.iter().map(String::as_str).collect::<Vec<_>>());
    }
    0
}
unsafe extern "C" fn w_get_songs(user: *mut c_void, json: *const c_char) -> *mut c_char {
    if let Some(sql) = decode::<String>(json) {
        let rows = unsafe { host(user) }.get_songs(&sql);
        return to_json_ptr(&rows);
    }
    std::ptr::null_mut()
}
unsafe extern "C" fn w_engine_get_state(user: *mut c_void) -> *mut c_char {
    to_json_ptr(&unsafe { host(user) }.engine_get_state())
}
unsafe extern "C" fn w_webcaster_listener_get_listener(user: *mut c_void, json: *const c_char) -> *mut c_char {
    if let Some(index) = decode::<i32>(json) {
        return to_json_ptr(&unsafe { host(user) }.webcaster_listener_get_listener(index));
    }
    std::ptr::null_mut()
}
unsafe extern "C" fn w_webcaster_listener_get_listeners(user: *mut c_void) -> *mut c_char {
    to_json_ptr(&unsafe { host(user) }.webcaster_listener_get_listeners())
}
unsafe extern "C" fn w_set_startup(user: *mut c_void, program: *const c_char, flag: *const c_char) {
    let enable = unsafe { read_cstr(flag) } == "true";
    unsafe { host(user) }.set_startup(unsafe { read_cstr(program) }, enable);
}
unsafe extern "C" fn w_sound_cards_get_get_sound_card(user: *mut c_void, json: *const c_char) -> *mut c_char {
    if let Some(card_type) = decode::<i32>(json) {
        return to_json_ptr(&unsafe { host(user) }.get_get_sound_card(card_type));
    }
    std::ptr::null_mut()
}

/// Owns the boxed `&dyn Host` carried as `user_data` and hands out a `HostVtable`
/// wired to that host. The handle must outlive any vtable copies it produces.
pub struct HostHandle<'a> {
    user: *mut c_void,
    _host: Box<&'a (dyn Host + 'static)>,
}

impl<'a> HostHandle<'a> {
    /// Wrap a live host implementation for ABI use.
    pub fn new(host: &'a dyn Host) -> Self {
        let host_ref: &'a dyn Host = host;
        let b = Box::new(host_ref);
        let user = Box::into_raw(b) as *mut c_void;
        Self {
            user,
            _host: unsafe { Box::from_raw(user as *mut &dyn Host) },
        }
    }

    /// Build a fresh `HostVtable` bound to this host. `user_data` stays owned by
    /// the handle (reborrowed); all entries are wired.
    pub fn vtable(&self) -> HostVtable {
        use crate::abi::{FnConfigGet, FnGetF32, FnGetI32, FnGetI64, FnGetJson, FnGetStr, FnJsonToI32, FnJsonToStr, FnSetF32, FnSetI32, FnSetI64, FnUiReq, FnVoidStr, FnVoidStrStr, FnVoidUnit};
        HostVtable {
            user_data: self.user,
            player_start: Some(w_player_start as FnVoidUnit),
            player_pause: Some(w_player_pause as FnVoidUnit),
            player_stop: Some(w_player_stop as FnVoidUnit),
            player_resume: Some(w_player_resume as FnVoidUnit),
            player_get_is_paused: Some(w_player_get_is_paused as FnGetI32),
            player_set_is_paused: Some(w_player_set_is_paused as FnSetI32),
            player_is_stopped: Some(w_player_is_stopped as FnGetI32),
            player_get_track: Some(w_player_get_track as FnGetStr),
            player_set_track: Some(w_player_set_track as FnSetStr_),
            player_set_seek: Some(w_player_set_seek as FnSetI32),
            player_get_seek: Some(w_player_get_seek as FnGetI32),
            player_set_volume: Some(w_player_set_volume as FnSetI32),
            player_get_volume: Some(w_player_get_volume as FnGetI32),
            rotals_get_rot: Some(w_rotals_get_rot as FnGetStr),
            rotals_set_rot: Some(w_rotals_set_rot as FnSetStr_),
            tracks_add: Some(w_tracks_add as FnVoidStr),
            tracks_clear: Some(w_tracks_clear as FnVoidUnit),
            tracks_set_auto_rot: Some(w_tracks_set_auto_rot as FnSetI32),
            core_enable_shutdown: Some(w_core_enable_shutdown as FnSetStr_),
            core_disable_shutdown: Some(w_core_disable_shutdown as FnVoidUnit),
            core_cancel_shutdown: Some(w_core_cancel_shutdown as FnVoidUnit),
            web_is_enabled: Some(w_web_is_enabled as FnGetI32),
            web_start: Some(w_web_start as FnVoidUnit),
            web_stop: Some(w_web_stop as FnVoidUnit),
            web_restart: Some(w_web_restart as FnVoidUnit),
            web_add_stream: Some(w_web_add_stream as FnVoidStr),
            web_remove_stream: Some(w_web_remove_stream as FnVoidStr),
            debug_add_line: Some(w_debug_add_line as FnVoidStr),
            interop_hide_main_form: Some(w_interop_hide_main_form as FnVoidUnit),
            interop_unhide_main_form: Some(w_interop_unhide_main_form as FnVoidUnit),
            interop_is_hidden: Some(w_interop_is_hidden as FnGetI32),
            interop_show_plugin_window: Some(w_interop_show_plugin_window as FnUiReq),
            interop_hide_plugin_window: Some(w_interop_hide_plugin_window as FnVoidStr),
            interop_open_connection_string: Some(w_interop_open_connection_string as FnSetStr_),
            config_get_setting: Some(w_config_get_setting as FnConfigGet),
            config_set_setting: Some(w_config_set_setting as FnVoidStrStr),
            mysql_query: Some(w_mysql_query as FnJsonToStr),
            mysql_insert: Some(w_mysql_insert as FnJsonToI32),
            get_songs: Some(w_get_songs as FnJsonToStr),
            media_open: Some(w_media_open as FnVoidStr),
            media_close: Some(w_media_close as FnVoidUnit),
            media_get_playing: Some(w_media_get_playing as FnGetI32),
            media_set_volume: Some(w_media_set_volume as FnSetI32),
            media_set_mute: Some(w_media_set_mute as FnSetI32),
            media_get_mute: Some(w_media_get_mute as FnGetI32),
            media_set_pan: Some(w_media_set_pan as FnSetI32),
            media_get_pan: Some(w_media_get_pan as FnGetI32),
            media_get_length: Some(w_media_get_length as FnGetI64),
            media_get_position: Some(w_media_get_position as FnGetI64),
            media_set_position: Some(w_media_set_position as FnSetI64),
            media_play: Some(w_media_play as FnVoidUnit),
            media_pause: Some(w_media_pause as FnVoidUnit),
            media_stop: Some(w_media_stop as FnVoidUnit),
            capi_set_message: Some(w_capi_set_message as FnVoidStr),
            engine_get_state: Some(w_engine_get_state as FnGetJson),
            engine_set_transition: Some(w_engine_set_transition as FnSetI32),
            engine_get_transition: Some(w_engine_get_transition as FnGetI32),
            engine_set_paused: Some(w_engine_set_paused as FnSetI32),
            engine_get_paused: Some(w_engine_get_paused as FnGetI32),
            webcaster_get_is_connected: Some(w_webcaster_get_is_connected as FnGetI32),
            webcaster_get_dnc_count: Some(w_webcaster_get_dnc_count as FnGetI64),
            webcaster_get_stream_uptime: Some(w_webcaster_get_stream_uptime as FnGetI64),
            webcaster_get_global_uptime: Some(w_webcaster_get_global_uptime as FnGetI64),
            webcaster_listener_get_count: Some(w_webcaster_listener_get_count as FnGetI32),
            webcaster_listener_get_listener: Some(w_webcaster_listener_get_listener as FnJsonToStr),
            webcaster_listener_get_listeners: Some(w_webcaster_listener_get_listeners as FnGetJson),
            get_startup: Some(w_get_startup as FnGetStr),
            set_startup: Some(w_set_startup as FnVoidStrStr),
            get_version: Some(w_get_version as FnGetStr),
            restart: Some(w_restart as FnVoidUnit),
            vuscope_add_to_vuscope: Some(w_vuscope_add_to_vuscope as FnSetI32),
            vuscope_remove_from_vuscope: Some(w_vuscope_remove_from_vuscope as FnSetI32),
            vuscope_set_peak_left: Some(w_vuscope_set_peak_left as FnSetF32),
            vuscope_set_peak_right: Some(w_vuscope_set_peak_right as FnSetF32),
            vuscope_set_playing: Some(w_vuscope_set_playing as FnSetI32),
            vuscope_set_scrolling: Some(w_vuscope_set_scrolling as FnSetI32),
            vuscope_set_scrolling2: Some(w_vuscope_set_scrolling2 as FnSetI32),
            vuscope_set_clip: Some(w_vuscope_set_clip as FnVoidStr),
            vuscope_set_level_left: Some(w_vuscope_set_level_left as FnSetF32),
            vuscope_set_level_right: Some(w_vuscope_set_level_right as FnSetF32),
            sound_cards_get_count: Some(w_sound_cards_get_count as FnGetI32),
            sound_cards_get_get_sound_card: Some(w_sound_cards_get_get_sound_card as FnJsonToStr),
        }
    }
}

// `FnSetStr` is a private type alias in abi; re-specified here to avoid
// exporting another public name.
type FnSetStr_ = unsafe extern "C" fn(*mut c_void, *const c_char);
