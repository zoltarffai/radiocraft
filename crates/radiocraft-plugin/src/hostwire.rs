//! Host-side counterpart of the ABI: wraps an `&dyn Host` into a `HostVtable`
//! so plugins (Rust or C) can call it through the function-pointer table.
//!
//! Every wire function is panic-fenced (`catch_unwind`) so host panics never
//! cross the FFI boundary; decoding failures fall back to defaults, encoding
//! failures return NULL.

use std::ffi::{c_char, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::abi::{json_arg, to_cstr_ptr, to_json_ptr, HostVtable};
use crate::host::{EventAction, Host, ImportRequest, PluginUiRequest, StateInfo};
use radiocraft_core::model::SongData;

/// Owned handle that keeps the `&dyn Host` reference alive for the vtable's
/// `user_data`. The reference is leaked (`Box::into_raw`) because the plugin
/// may keep a copy of the vtable beyond this handle's lifetime (mirroring the
/// original host, where `IHost` stays valid for the whole plugin lifetime).
pub struct HostHandle {
    user_data: *mut c_void,
}

impl HostHandle {
    /// Build a handle around a host implementation.
    pub fn new(host: &dyn Host) -> Self {
        let boxed: Box<&dyn Host> = Box::new(host);
        Self {
            user_data: Box::into_raw(boxed) as *mut c_void,
        }
    }

    /// The complete function-pointer table for this host.
    pub fn vtable(&self) -> HostVtable {
        HostVtable {
            user_data: self.user_data,
            main_autodj: Some(wire_main_autodj),
            set_main_autodj: Some(wire_set_main_autodj),
            main_assisted: Some(wire_main_assisted),
            set_main_assisted: Some(wire_set_main_assisted),
            unmute_input: Some(wire_unmute_input),
            set_unmute_input: Some(wire_set_unmute_input),
            track_loop: Some(wire_track_loop),
            set_track_loop: Some(wire_set_track_loop),
            record: Some(wire_record),
            set_record: Some(wire_set_record),
            pause_events: Some(wire_pause_events),
            set_pause_events: Some(wire_set_pause_events),
            playlist2_full_screen: Some(wire_playlist2_full_screen),
            set_playlist2_full_screen: Some(wire_set_playlist2_full_screen),
            search_results: Some(wire_search_results),
            set_reload_zones: Some(wire_set_reload_zones),
            title_main: Some(wire_title_main),
            set_title_main: Some(wire_set_title_main),
            get_type_title: Some(wire_get_type_title),
            get_type_color: Some(wire_get_type_color),
            get_sound_card: Some(wire_get_sound_card),
            main_volume: Some(wire_main_volume),
            set_main_volume: Some(wire_set_main_volume),
            main_track_volume: Some(wire_main_track_volume),
            set_main_track_volume: Some(wire_set_main_track_volume),
            main_mute: Some(wire_main_mute),
            set_main_mute: Some(wire_set_main_mute),
            set_play_from_intro: Some(wire_set_play_from_intro),
            set_play_from_loop: Some(wire_set_play_from_loop),
            set_play_from_outro: Some(wire_set_play_from_outro),
            set_restart_track: Some(wire_set_restart_track),
            pause: Some(wire_pause),
            set_pause: Some(wire_set_pause),
            set_stop_playing: Some(wire_set_stop_playing),
            set_stop_playing_without_fade: Some(wire_set_stop_playing_without_fade),
            set_update_playlist: Some(wire_set_update_playlist),
            get_main_playlist: Some(wire_get_main_playlist),
            set_main_playlist: Some(wire_set_main_playlist),
            set_clear_playlist: Some(wire_set_clear_playlist),
            queue_count: Some(wire_queue_count),
            now_playing_handle: Some(wire_now_playing_handle),
            remaining_time: Some(wire_remaining_time),
            set_refresh_events: Some(wire_set_refresh_events),
            set_reload_categories: Some(wire_set_reload_categories),
            set_refresh_gui: Some(wire_set_refresh_gui),
            set_carts_changed: Some(wire_set_carts_changed),
            set_cart_started: Some(wire_set_cart_started),
            set_main_player_started: Some(wire_set_main_player_started),
            application_path: Some(wire_application_path),
            get_songs: Some(wire_get_songs),
            state_changed: Some(wire_state_changed),
            run_event: Some(wire_run_event),
            play_playlist_track: Some(wire_play_playlist_track),
            add_to_playlist_by_id: Some(wire_add_to_playlist_by_id),
            add_to_playlist: Some(wire_add_to_playlist),
            delete_from_playlist: Some(wire_delete_from_playlist),
            add_playlist_by_id: Some(wire_add_playlist_by_id),
            add_playlist_by_path: Some(wire_add_playlist_by_path),
            import_file: Some(wire_import_file),
            update_history: Some(wire_update_history),
            show_ui: Some(wire_show_ui),
            get_plugin_actions: Some(wire_get_plugin_actions),
            run_plugin_action: Some(wire_run_plugin_action),
            play_cart_by_number: Some(wire_play_cart_by_number),
            debug_add_line: Some(wire_debug_add_line),
            load_rotation_id: Some(wire_load_rotation_id),
            mysql_query: Some(wire_mysql_query),
            mysql_insert: Some(wire_mysql_insert),
            save_setting: Some(wire_save_setting),
            get_setting: Some(wire_get_setting),
        }
    }
}

// ---- wire helpers ----------------------------------------------------------

/// Recover the `&dyn Host` stored by `HostHandle::new`.
///
/// # Safety
/// `user` must be a `user_data` produced by [`HostHandle::new`].
unsafe fn host<'a>(user: *mut c_void) -> &'a dyn Host {
    unsafe { &**(user as *const &dyn Host) }
}

/// Run a host call, fencing panics: on panic return `fallback`.
fn fence<T: Copy, F: FnOnce() -> T>(f: F, fallback: T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => fallback,
    }
}

macro_rules! wire_get_bool {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void) -> c_int {
            fence(|| unsafe { host(user).$method() }, false).into()
        }
    };
}
macro_rules! wire_set_bool {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void, v: c_int) {
            let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
                host(user).$method(v != 0)
            }));
        }
    };
}
macro_rules! wire_get_i32 {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void) -> c_int {
            fence(|| unsafe { host(user).$method() }, 0)
        }
    };
}
macro_rules! wire_set_i32 {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void, v: c_int) {
            let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
                host(user).$method(v)
            }));
        }
    };
}
macro_rules! wire_get_f32 {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void) -> f32 {
            fence(|| unsafe { host(user).$method() }, 0.0)
        }
    };
}
macro_rules! wire_set_f32 {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void, v: f32) {
            let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
                host(user).$method(v)
            }));
        }
    };
}
macro_rules! wire_get_f64 {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void) -> f64 {
            fence(|| unsafe { host(user).$method() }, 0.0)
        }
    };
}
macro_rules! wire_get_str {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void, out: *mut *mut c_char) {
            if out.is_null() {
                return;
            }
            unsafe { *out = std::ptr::null_mut() };
            let s = catch_unwind(AssertUnwindSafe(|| unsafe { host(user).$method() }))
                .unwrap_or_default();
            unsafe { *out = to_cstr_ptr(&s) };
        }
    };
}
macro_rules! wire_set_str {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void, s: *const c_char) {
            let s = crate::abi::read_cstr(s);
            let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
                host(user).$method(s)
            }));
        }
    };
}
macro_rules! wire_get_json {
    ($name:ident, $method:ident) => {
        pub unsafe extern "C" fn $name(user: *mut c_void) -> *mut c_char {
            let v = catch_unwind(AssertUnwindSafe(|| unsafe { host(user).$method() }))
                .unwrap_or_default();
            to_json_ptr(&v)
        }
    };
}

wire_get_bool!(wire_main_autodj, main_autodj);
wire_set_bool!(wire_set_main_autodj, set_main_autodj);
wire_get_bool!(wire_main_assisted, main_assisted);
wire_set_bool!(wire_set_main_assisted, set_main_assisted);
wire_get_bool!(wire_unmute_input, unmute_input);
wire_set_bool!(wire_set_unmute_input, set_unmute_input);
wire_get_bool!(wire_track_loop, track_loop);
wire_set_bool!(wire_set_track_loop, set_track_loop);
wire_get_bool!(wire_record, record);
wire_set_bool!(wire_set_record, set_record);
wire_get_bool!(wire_pause_events, pause_events);
wire_set_bool!(wire_set_pause_events, set_pause_events);
wire_get_bool!(wire_playlist2_full_screen, playlist2_full_screen);
wire_set_bool!(wire_set_playlist2_full_screen, set_playlist2_full_screen);
wire_get_i32!(wire_search_results, search_results);
wire_set_bool!(wire_set_reload_zones, set_reload_zones);
wire_get_str!(wire_title_main, title_main);
wire_set_str!(wire_set_title_main, set_title_main);

pub unsafe extern "C" fn wire_get_type_title(user: *mut c_void, t: c_int) -> *mut c_char {
    let v = catch_unwind(AssertUnwindSafe(|| unsafe { host(user).get_type_title(t) }))
        .unwrap_or_default();
    to_json_ptr(&v)
}
pub unsafe extern "C" fn wire_get_type_color(user: *mut c_void, t: c_int) -> c_int {
    fence(|| unsafe { host(user).get_type_color(t) }, 0)
}
pub unsafe extern "C" fn wire_get_sound_card(user: *mut c_void, t: c_int) -> *mut c_char {
    let v = catch_unwind(AssertUnwindSafe(|| unsafe { host(user).get_sound_card(t) }))
        .unwrap_or_default();
    to_json_ptr(&v)
}

wire_get_f32!(wire_main_volume, main_volume);
wire_set_f32!(wire_set_main_volume, set_main_volume);
wire_get_f32!(wire_main_track_volume, main_track_volume);
wire_set_f32!(wire_set_main_track_volume, set_main_track_volume);
wire_get_bool!(wire_main_mute, main_mute);
wire_set_bool!(wire_set_main_mute, set_main_mute);
wire_set_bool!(wire_set_play_from_intro, set_play_from_intro);
wire_set_bool!(wire_set_play_from_loop, set_play_from_loop);
wire_set_bool!(wire_set_play_from_outro, set_play_from_outro);
wire_set_bool!(wire_set_restart_track, set_restart_track);
wire_get_bool!(wire_pause, pause);
wire_set_bool!(wire_set_pause, set_pause);
wire_set_bool!(wire_set_stop_playing, set_stop_playing);
wire_set_bool!(wire_set_stop_playing_without_fade, set_stop_playing_without_fade);
wire_set_bool!(wire_set_update_playlist, set_update_playlist);
wire_get_json!(wire_get_main_playlist, get_main_playlist);

pub unsafe extern "C" fn wire_set_main_playlist(user: *mut c_void, json: *const c_char) {
    let v = json_arg::<Vec<SongData>>(json);
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        host(user).set_main_playlist(&v)
    }));
}
wire_set_bool!(wire_set_clear_playlist, set_clear_playlist);
wire_get_i32!(wire_queue_count, queue_count);
wire_get_i32!(wire_now_playing_handle, now_playing_handle);
wire_get_f64!(wire_remaining_time, remaining_time);

wire_set_bool!(wire_set_refresh_events, set_refresh_events);
wire_set_bool!(wire_set_reload_categories, set_reload_categories);
wire_set_bool!(wire_set_refresh_gui, set_refresh_gui);
wire_set_bool!(wire_set_carts_changed, set_carts_changed);
wire_set_i32!(wire_set_cart_started, set_cart_started);
wire_set_i32!(wire_set_main_player_started, set_main_player_started);

pub unsafe extern "C" fn wire_application_path(user: *mut c_void, out: *mut *mut c_char) {
    if out.is_null() {
        return;
    }
    unsafe { *out = std::ptr::null_mut() };
    let p = catch_unwind(AssertUnwindSafe(|| unsafe { host(user).application_path() }))
        .unwrap_or_default();
    let s = p.to_string_lossy().into_owned();
    unsafe { *out = to_cstr_ptr(&s) };
}

pub unsafe extern "C" fn wire_get_songs(user: *mut c_void, sql: *const c_char) -> *mut c_char {
    let sql = crate::abi::read_cstr(sql);
    let v = catch_unwind(AssertUnwindSafe(|| unsafe { host(user).get_songs(sql) }))
        .unwrap_or_default();
    to_json_ptr(&v)
}

pub unsafe extern "C" fn wire_state_changed(user: *mut c_void, json: *const c_char) {
    let v = json_arg::<StateInfo>(json);
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        host(user).state_changed(&v)
    }));
}
pub unsafe extern "C" fn wire_run_event(user: *mut c_void, id: i64) -> c_int {
    fence(|| unsafe { host(user).run_event(id) }, false).into()
}

pub unsafe extern "C" fn wire_play_playlist_track(user: *mut c_void, n: c_int, fade: c_int) {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        host(user).play_playlist_track(n, fade != 0)
    }));
}
pub unsafe extern "C" fn wire_add_to_playlist_by_id(user: *mut c_void, id: i64, pos: c_int) {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        host(user).add_to_playlist_by_id(id, pos)
    }));
}
pub unsafe extern "C" fn wire_add_to_playlist(user: *mut c_void, json: *const c_char, pos: c_int) {
    let v = json_arg::<SongData>(json);
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        host(user).add_to_playlist(&v, pos)
    }));
}
wire_set_i32!(wire_delete_from_playlist, delete_from_playlist);
pub unsafe extern "C" fn wire_add_playlist_by_id(user: *mut c_void, id: i64, pos: c_int) -> c_int {
    fence(|| unsafe { host(user).add_playlist_by_id(id, pos) }, false).into()
}
pub unsafe extern "C" fn wire_add_playlist_by_path(
    user: *mut c_void,
    path: *const c_char,
    pos: c_int,
) -> c_int {
    let path = crate::abi::read_cstr(path);
    fence(|| unsafe { host(user).add_playlist_by_path(path, pos) }, false).into()
}
pub unsafe extern "C" fn wire_import_file(user: *mut c_void, json: *const c_char) -> i64 {
    let req = json_arg::<ImportRequest>(json);
    fence(|| unsafe { host(user).import_file(&req) }, 0)
}
pub unsafe extern "C" fn wire_update_history(user: *mut c_void, json: *const c_char) -> c_int {
    let track = json_arg::<SongData>(json);
    fence(|| unsafe { host(user).update_history(&track) }, false).into()
}

pub unsafe extern "C" fn wire_show_ui(user: *mut c_void, json: *const c_char) {
    let req = json_arg::<PluginUiRequest>(json);
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        host(user).show_ui(req)
    }));
}

pub unsafe extern "C" fn wire_get_plugin_actions(user: *mut c_void) -> *mut c_char {
    let v = catch_unwind(AssertUnwindSafe(|| unsafe { host(user).get_plugin_actions() }))
        .unwrap_or_default();
    to_json_ptr(&v)
}
pub unsafe extern "C" fn wire_run_plugin_action(
    user: *mut c_void,
    action_json: *const c_char,
    args_json: *const c_char,
) {
    let action = json_arg::<EventAction>(action_json);
    let args = json_arg::<Vec<String>>(args_json);
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        host(user).run_plugin_action(&action, &args)
    }));
}
wire_set_i32!(wire_play_cart_by_number, play_cart_by_number);
wire_set_str!(wire_debug_add_line, debug_add_line);

pub unsafe extern "C" fn wire_load_rotation_id(user: *mut c_void, id: i64) -> c_int {
    fence(|| unsafe { host(user).load_rotation_id(id) }, false).into()
}

pub unsafe extern "C" fn wire_mysql_query(
    user: *mut c_void,
    sql: *const c_char,
    args_json: *const c_char,
) -> *mut c_char {
    let sql = crate::abi::read_cstr(sql);
    let args = json_arg::<Vec<String>>(args_json);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = catch_unwind(AssertUnwindSafe(|| unsafe { host(user).mysql_query(sql, &refs) }))
        .unwrap_or_default();
    to_cstr_ptr(&out)
}
pub unsafe extern "C" fn wire_mysql_insert(
    user: *mut c_void,
    sql: *const c_char,
    args_json: *const c_char,
) -> c_int {
    let sql = crate::abi::read_cstr(sql);
    let args = json_arg::<Vec<String>>(args_json);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    fence(|| unsafe { host(user).mysql_insert(sql, &refs) }, 0)
}

pub unsafe extern "C" fn wire_save_setting(
    user: *mut c_void,
    file: *const c_char,
    key: *const c_char,
    value: *const c_char,
) {
    let (file, key, value) = (
        crate::abi::read_cstr(file),
        crate::abi::read_cstr(key),
        crate::abi::read_cstr(value),
    );
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        host(user).save_setting(file, key, value)
    }));
}
pub unsafe extern "C" fn wire_get_setting(
    user: *mut c_void,
    file: *const c_char,
    key: *const c_char,
    default: *const c_char,
    out: *mut *mut c_char,
) {
    if out.is_null() {
        return;
    }
    unsafe { *out = std::ptr::null_mut() };
    let (file, key, default) = (
        crate::abi::read_cstr(file),
        crate::abi::read_cstr(key),
        crate::abi::read_cstr(default),
    );
    let v = catch_unwind(AssertUnwindSafe(|| unsafe {
        host(user).get_setting(file, key, default)
    }))
    .unwrap_or_default();
    unsafe { *out = to_cstr_ptr(&v) };
}

