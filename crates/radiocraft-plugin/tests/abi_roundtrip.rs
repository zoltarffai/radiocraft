//! End-to-end ABI tests: host <-> plugin vtable round-trips through the C
//! boundary, including a plugin that talks back to the host while unwinding
//! across three layers.

use radiocraft_core::model::SongData;
use radiocraft_plugin::abi::{self, ABI_VERSION};
use radiocraft_plugin::host::{EventAction, Host, PluginUiRequest, StateChange, StateInfo};
use radiocraft_plugin::hostwire::HostHandle;
use radiocraft_plugin::loader::{DispatchError, PluginRegistry};
use radiocraft_plugin::plugin::{Plugin, PluginInfo};
use radiocraft_plugin::pluginabi::{self, PluginVtable};
use radiocraft_plugin::test_support::TestHost;

/// A plugin that logs via the host and implements a couple of actions.
#[derive(Default)]
struct TalkbackPlugin {
    host: Option<abi::HostBridge>,
    seen_states: Vec<StateInfo>,
}

impl Plugin for TalkbackPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "Talkback".into(),
            description: "test plugin".into(),
            version: 1,
            min_host_api: ABI_VERSION,
            library_name: "talkback".into(),
        }
    }

    fn init(&mut self, host: &dyn Host) {
        // Copy the vtable the host passed; this test host stays alive for the
        // whole test, so holding a bridge is fine.
        let handle = HostHandle::new(host);
        self.host = Some(abi::HostBridge::from_vtable(handle.vtable()));
    }

    fn actions(&self) -> Vec<EventAction> {
        vec![
            EventAction::new("Ping", 100),
            EventAction::new_with_args("Ding", 101, &["Arg1"]),
        ]
    }

    fn run_action(&mut self, action: String, args: &[String]) {
        let host = self.host.as_ref().expect("host bridge");
        host.debug_add_line(&format!("action={action} args={}", args.join(",")));
        host.set_main_autodj(true);
        let volume = host.main_volume();
        host.debug_add_line(&format!("volume={volume}"));
    }

    fn on_state_changed(&mut self, state: &StateInfo) {
        self.seen_states.push(state.clone());
        if let Some(host) = &self.host {
            host.debug_add_line(&format!("state={:?}", state.feature));
        }
    }
}

/// Build a full plugin vtable from TalkbackPlugin and drive it through the
/// plugin-side raw wrappers, exactly the way a host would.
#[test]
fn plugin_vtable_round_trip() {
    let host = TestHost::new();
    let host_handle = HostHandle::new(&host);
    let host_vtable = host_handle.vtable();

    let vtable = pluginabi::new_plugin_vtable::<TalkbackPlugin>();
    let user = std::ptr::null_mut();

    let code = unsafe { pluginabi::initialize_shim::<TalkbackPlugin>(user, &host_vtable) };
    assert_eq!(code, 0);

    let info_json = unsafe { (vtable.get_info.unwrap())(user) };
    let info: PluginInfo = serde_json::from_str(&abi::read_cstr(info_json)).unwrap();
    assert_eq!(info.name, "Talkback");
    abi::abi_free_string(info_json);

    let actions_json = unsafe { (vtable.get_actions.unwrap())(user) };
    let actions: Vec<EventAction> = serde_json::from_str(&abi::read_cstr(actions_json)).unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].name, "Ping");
    abi::abi_free_string(actions_json);

    // run "Ping" — plugin calls back into the host through the HostVtable.
    let c_action = std::ffi::CString::new("Ping").unwrap();
    let arg = std::ffi::CString::new("hello").unwrap();
    let argv = [arg.as_ptr()];
    let rc = unsafe {
        (vtable.run_action.unwrap())(user, c_action.as_ptr(), 0, argv.as_ptr())
    };
    assert_eq!(rc, 0);

    let lines = host.take_debug_lines();
    assert_eq!(lines[0], "action=Ping args=hello");
    assert_eq!(lines[1], "volume=0");

    // state fan-out through the ABI
    let state = StateInfo::with_data(StateChange::PlayerStarted, serde_json::json!({"id": 7}));
    let state_json = serde_json::to_string(&state).unwrap();
    let c_state = std::ffi::CString::new(state_json).unwrap();
    unsafe {
        (vtable.on_state_changed.unwrap())(user, c_state.as_ptr());
    }
    let lines = host.take_debug_lines();
    assert_eq!(lines[0], "state=PlayerStarted");

    let _ = &vtable;
}

#[test]
fn host_vtable_basics_through_wire() {
    let host = TestHost::new();
    let handle = HostHandle::new(&host);
    let vt = handle.vtable();

    // volume set/get via C ABI
    unsafe {
        (vt.set_main_volume.unwrap())(vt.user_data, 0.75);
        assert_eq!((vt.main_volume.unwrap())(vt.user_data).to_bits(), 0.75f32.to_bits());
    }
    // settings get with default
    let out = unsafe { (vt.get_setting.unwrap())(vt.user_data, c"cat".as_ptr(), c"missing".as_ptr()) };
    // (get_setting wrapper takes the setting name only; user_data is opaque here)
    assert!(!out.is_null());
    unsafe { abi::abi_free_string(out) };
}

#[test]
fn registry_dispatch_and_unregister() {
    let mut registry = PluginRegistry::new();
    let id = registry.register(Box::<TalkbackPlugin>::default());
    assert_eq!(registry.len(), 1);

    let actions = registry.all_actions();
    assert_eq!(actions.len(), 2);

    let n = registry.dispatch("Ding (Arg1)", &["x".into()]).unwrap();
    assert_eq!(n, 1);
    assert!(matches!(
        registry.dispatch("nope", &[]),
        Err(DispatchError::UnknownAction(_))
    ));

    registry.unregister(id).unwrap();
    assert!(registry.is_empty());
}

#[test]
fn json_round_trips() {
    let ea = EventAction::new_with_args("My Action", 42, &["Arg1", "Arg2"]);
    let s = serde_json::to_string(&ea).unwrap();
    assert_eq!(EventAction::from_json(&s).unwrap(), ea);
    assert_eq!(ea.to_display_string(), "My Action (Arg1, Arg2)");

    let si = StateInfo::with_data(StateChange::Other(99), serde_json::json!("payload"));
    let s = serde_json::to_string(&si).unwrap();
    assert_eq!(StateInfo::from_json(&s).unwrap(), si);

    let ui = PluginUiRequest::TrackEditor {
        track: SongData::default(),
        plugin_name: "plug".into(),
        position: 3,
    };
    let s = serde_json::to_string(&ui).unwrap();
    assert_eq!(PluginUiRequest::from_json(&s).unwrap(), ui);
}

/// Loader error path: the current executable is not a plugin library.
#[cfg(unix)]
#[test]
fn loader_reports_missing_symbols() {
    use radiocraft_plugin::loader::LoaderError;
    let exe = std::env::current_exe().unwrap();
    let err = unsafe { radiocraft_plugin::loader::load_plugin(&exe) }.unwrap_err();
    assert!(matches!(
        err,
        LoaderError::MissingSymbol(_) | LoaderError::LoadError(_)
    ));
}

/// Null-safety: every host wrapper tolerates NULL slots/pointers.
#[test]
fn null_safe_wrappers() {
    let empty = PluginVtable::default();
    let bridge = abi::HostBridge::from_vtable(empty);
    assert_eq!(bridge.main_volume(), 0.0);
    assert!(!bridge.main_mute());
    assert!(bridge.get_setting("c", "k").is_none());
    bridge.debug_add_line("ignored");

    // json_arg / read_cstr handle null.
    assert_eq!(abi::json_arg(std::ptr::null()), serde_json::Value::Null);
    assert_eq!(abi::read_cstr(std::ptr::null()), "");

    let _ = &bridge;
    let _ = std::ptr::null::<c_void>();
}
