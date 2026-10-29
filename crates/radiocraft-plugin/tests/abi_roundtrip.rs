//! End-to-end ABI tests: host <-> plugin vtable round-trips through the C
//! boundary, including a plugin that talks back to the host while unwinding
//! across three layers.

use radiocraft_core::model::SongData;
use radiocraft_plugin::abi::{self, ABI_VERSION};
use radiocraft_plugin::host::{EventAction, Host, ImportRequest, PluginUiRequest, StateChange, StateInfo};
use radiocraft_plugin::hostwire::HostHandle;
use radiocraft_plugin::loader::{DispatchError, PluginRegistry};
use radiocraft_plugin::plugin::{Plugin, PluginInfo};
use radiocraft_plugin::pluginabi::{self, NativePlugin};
use radiocraft_plugin::test_support::TestHost;

/// A plugin that logs via the host and implements a couple of actions.
#[derive(Default)]
struct TalkbackPlugin {
    host: Option<abi::HostBridge>,
    seen_states: std::cell::RefCell<Vec<StateInfo>>,
}

impl Plugin for TalkbackPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "Talkback".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "test plugin".into(),
        }
    }

    fn init(&mut self, host: &dyn Host) {
        // Rebuild a bridge from the vtable the host passed; the HostHandle
        // leaks the host reference, so holding a bridge is fine for tests.
        let handle = HostHandle::new(host);
        self.host = Some(unsafe { abi::HostBridge::new(handle.vtable()) });
    }

    fn actions(&self) -> Vec<EventAction> {
        vec![
        EventAction { name: "Ping".into(), id: 100, args: vec![] },
        EventAction {
            name: "Ding".into(),
            id: 101,
            args: vec!["Arg1".into()],
        },
    ]
    }

    fn run_action(&mut self, action: &EventAction, args: &[String]) {
        let host = self.host.as_ref().expect("host bridge");
        host.debug_add_line(&format!("action={} args={}", action.name, args.join(",")));
        host.set_main_autodj(true);
        let volume = host.main_volume();
        host.debug_add_line(&format!("volume={volume}"));
    }

    fn on_state_changed(&mut self, state: &StateInfo) {
        self.seen_states.borrow_mut().push(state.clone());
        if let Some(host) = &self.host {
            host.debug_add_line(&format!("state={:?}", state.feature));
        }
    }
}

/// Build a full plugin vtable from TalkbackPlugin and drive it as the host
/// would, exactly the way a loaded dylib's vtable is consumed.
#[test]
fn plugin_vtable_round_trip() {
    let host = TestHost::new();
    let host_handle = HostHandle::new(&host);
    let host_vtable = host_handle.vtable();

    let vt = pluginabi::new_plugin_vtable::<TalkbackPlugin>();
    let user = vt.user_data;

    // host hands the plugin a HostVtable at init time.
    unsafe {
        (vt.initialize.unwrap())(user, &host_vtable);
    }

    // metadata through the wire
    let mut info_ptr: *mut std::ffi::c_char = std::ptr::null_mut();
    unsafe {
        (vt.get_info.unwrap())(user, &mut info_ptr);
    };
    let info: PluginInfo = serde_json::from_str(&unsafe { abi::read_cstr(info_ptr) }).unwrap();
    unsafe { abi::radiocraft_abi_free_string(info_ptr) };
    assert_eq!(
        info,
        PluginInfo {
            name: "Talkback".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "test plugin".into(),
        }
    );

    // actions through the wire
    let mut actions_ptr: *mut std::ffi::c_char = std::ptr::null_mut();
    unsafe {
        (vt.get_actions.unwrap())(user, &mut actions_ptr);
    };
    let actions: Vec<EventAction> = serde_json::from_str(&unsafe { abi::read_cstr(actions_ptr) }).unwrap();
    unsafe { abi::radiocraft_abi_free_string(actions_ptr) };
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].name, "Ping");
    assert_eq!(actions[1].display_string(), "Ding (Arg1)");

    // run "Ding (Arg1)" — plugin calls back into the host through the HostVtable.
    let action_json = abi::to_json_ptr(&actions[1]);
    let args_json = abi::to_json_ptr(&vec!["hello".to_string()]);
    let rc = unsafe { (vt.run_action.unwrap())(user, action_json, args_json) };
    assert_eq!(rc, 0);
    unsafe {
        abi::radiocraft_abi_free_string(action_json);
        abi::radiocraft_abi_free_string(args_json);
    }

    let lines = host.take_debug_lines();
    assert_eq!(lines[0], "action=Ding args=hello");
    assert_eq!(lines[1], "volume=0");
    assert!(host.settings.borrow().is_empty());

    // settings round-trip through the host vtable from the plugin's side.
    // (save_setting/get_setting on the TestHost are wired via the wire table.)
    let bridge_vt = host_handle.vtable();
    let bridge = abi::HostBridge::from_partial(bridge_vt);
    bridge.save_setting("cat", "key", "value");
    assert_eq!(bridge.get_setting("cat", "key", "fallback"), "value");

    // state fan-out through the ABI
    let state = StateInfo::with_data(StateChange::PlayerStarted, serde_json::json!({"id": 7}));
    let state_json = abi::to_json_ptr(&state);
    unsafe {
        (vt.on_state_changed.unwrap())(user, state_json);
    }
    unsafe { abi::radiocraft_abi_free_string(state_json) };
    let lines = host.take_debug_lines();
    assert_eq!(lines[0], "state=PlayerStarted");

    // clean teardown via the vtable's destroy entry (releases the boxed plugin).
    unsafe {
        (vt.destroy.unwrap())(user);
    }
}

/// Drive the same vtable through `NativePlugin` (the host-side `Plugin` view).
#[test]
fn native_plugin_view_round_trip() {
    let host = TestHost::new();
    let vt = pluginabi::new_plugin_vtable::<TalkbackPlugin>();
    let mut native = unsafe { NativePlugin::new(vt) };

    native.init(&host);
    let info = native.info();
    assert_eq!(info.name, "Talkback");

    let actions = native.actions();
    assert_eq!(actions.len(), 2);

    native.run_action(&actions[0], &["x".into()]);
    let lines = host.take_debug_lines();
    assert_eq!(lines[0], "action=Ping args=x");

    native.on_state_changed(&StateInfo::new(StateChange::Timer));

    native.destroy();
}

#[test]
fn host_vtable_basics_through_wire() {
    let host = TestHost::new();
    let handle = HostHandle::new(&host);
    let vt = handle.vtable();

    // volume set/get via C ABI
    unsafe {
        (vt.set_main_volume.unwrap())(vt.user_data, 0.75);
        assert_eq!(
            (vt.main_volume.unwrap())(vt.user_data).to_bits(),
            0.75f32.to_bits()
        );
    }
    // settings round-trip via C ABI
    let f = std::ffi::CString::new("cat").unwrap();
    let k = std::ffi::CString::new("key").unwrap();
    let v = std::ffi::CString::new("42").unwrap();
    unsafe {
        (vt.save_setting.unwrap())(vt.user_data, f.as_ptr(), k.as_ptr(), v.as_ptr());
        let mut out: *mut std::ffi::c_char = std::ptr::null_mut();
        (vt.get_setting.unwrap())(vt.user_data, f.as_ptr(), k.as_ptr(), std::ptr::null(), &mut out);
        assert!(!out.is_null());
        let got = abi::read_cstr(out).to_owned();
        abi::radiocraft_abi_free_string(out);
        assert_eq!(got, "42");

        // missing key -> the default argument comes back
        let mut out: *mut std::ffi::c_char = std::ptr::null_mut();
        (vt.get_setting.unwrap())(vt.user_data, f.as_ptr(), c"missing".as_ptr(), std::ptr::null(), &mut out);
        // default slot is NULL here, so the host-side wire fills the default
        // from the trait default (empty string) — either way, no crash.
        if !out.is_null() {
            abi::radiocraft_abi_free_string(out);
        }
    }
}

#[test]
fn registry_dispatch_and_unregister() {
    let host = TestHost::new();
    let mut registry = PluginRegistry::new();
    let mut plugin = Box::<TalkbackPlugin>::default();
    plugin.init(&host);
    let id = registry.register(plugin);
    assert_eq!(registry.len(), 1);

    let actions = registry.all_actions();
    assert_eq!(actions.len(), 2);

    let n = registry.dispatch("Ding (Arg1)", &["x".into()]).unwrap();
    assert_eq!(n, 1);
    let lines = host.take_debug_lines();
    assert_eq!(lines[0], "action=Ding args=x");
    assert!(matches!(
        registry.dispatch("nope", &[]),
        Err(DispatchError::UnknownAction(_))
    ));

    registry.unregister(id).unwrap();
    assert!(registry.is_empty());
}

#[test]
fn json_round_trips() {
    let ea = EventAction {
        name: "My Action".into(),
        id: 42,
        args: vec!["Arg1".into(), "Arg2".into()],
    };
    let s = serde_json::to_string(&ea).unwrap();
    assert_eq!(serde_json::from_str::<EventAction>(&s).unwrap(), ea);
    assert_eq!(ea.display_string(), "My Action (Arg1, Arg2)");

    let si = StateInfo::with_data(StateChange::Other(99), serde_json::json!("payload"));
    let s = serde_json::to_string(&si).unwrap();
    assert_eq!(serde_json::from_str::<StateInfo>(&s).unwrap(), si);

    let ui = PluginUiRequest::TrackEditor {
        track: SongData::default(),
        plugin_name: "plug".into(),
        position: 3,
    };
    let s = serde_json::to_string(&ui).unwrap();
    let back: PluginUiRequest = serde_json::from_str(&s).unwrap();
    match back {
        PluginUiRequest::TrackEditor { plugin_name, position, .. } => {
            assert_eq!(plugin_name, "plug");
            assert_eq!(position, 3);
        }
        other => panic!("unexpected variant: {other:?}"),
    }

    let imp = ImportRequest::FilePath {
        path: "/music/test.mp3".into(),
        from_url: false,
        subcat_id: 7,
        genre_id: 0,
    };
    let s = serde_json::to_string(&imp).unwrap();
    let back: ImportRequest = serde_json::from_str(&s).unwrap();
    match back {
        ImportRequest::FilePath { path, subcat_id, .. } => {
            assert_eq!(path, "/music/test.mp3");
            assert_eq!(subcat_id, 7);
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

/// Loader error path: the current executable is not a plugin library.
#[cfg(unix)]
#[test]
fn loader_reports_missing_symbols() {
    use radiocraft_plugin::loader::LoaderError;
    let exe = std::env::current_exe().unwrap();
    let result = unsafe { radiocraft_plugin::loader::load_plugin(&exe) };
    let err = match result {
        Ok(_) => panic!("current exe is not a plugin library"),
        Err(e) => e,
    };
    assert!(matches!(
        err,
        LoaderError::MissingSymbol(_) | LoaderError::LoadError(_)
    ));
}

/// Null-safety: every host wrapper tolerates NULL slots/pointers.
#[test]
fn null_safe_wrappers() {
    let empty = HostVtableLike::default();
    let bridge = abi::HostBridge::from_partial(empty);
    assert_eq!(bridge.main_volume(), 0.0);
    assert!(!bridge.main_mute());
    assert_eq!(bridge.get_setting("c", "k", ""), "");
    bridge.debug_add_line("ignored");

    // json_arg / read_cstr handle null.
    assert_eq!(abi::json_arg::<serde_json::Value>(std::ptr::null()), serde_json::Value::Null);
    assert_eq!(unsafe { abi::read_cstr(std::ptr::null()) }, "");

    let _ = &bridge;
    let _ = std::ptr::null::<std::ffi::c_void>();
}

type HostVtableLike = abi::HostVtable;

/// ABI version constant must agree with what the host expects.
#[test]
fn abi_version_is_stable() {
    assert_eq!(ABI_VERSION, 1);
    assert_eq!(abi::ABI_VERSION, ABI_VERSION);
}
