//! RadioCraft window shell (docs/05 milestone 3).
//!
//! A thin Tauri webview that shows the server's web UI in a native window,
//! plus a tray icon and RadioDJ's close-to-tray policy. The playout server
//! itself is a separate process: unless a URL is given explicitly
//! (`--url`/`RADIOCRAFT_UI_URL`), the shell spawns the sibling
//! `radiocraft-server` binary on the default port (or reuses one that is
//! already listening) and hands it SIGTERM on exit so the rolling item gets
//! booked.

use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};
use radiocraft_net::autost::{CloseAction, ClosePolicy};
use radiocraft_settings::DatabaseSettings;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// Default loopback server address when the shell owns the server.
const DEFAULT_URL: &str = "http://127.0.0.1:8030/";

/// GLib log domains from the GTK/WebKit/appindicator C stack that emit
/// well-known cosmetic chatter at startup and window teardown (appindicator
/// deprecation nag, `GdkWindow ... unexpectedly destroyed` and the frame
/// clock/pointer assertions that follow it when WebKitGTK destroys the native
/// window before GTK notices, session-bus/at-spi noise on lean sessions).
/// These come from C code through GLib's log system, not from our Rust code,
/// so they bypass `tracing` and land straight on stderr. We install a global
/// GLib log *writer* for exactly these domains: their messages are captured
/// at `debug` level instead of hitting the terminal, and everything else
/// (including real GLib errors from other domains) keeps the default stderr
/// behaviour. A writer func (rather than per-domain handlers) is required
/// because parts of GDK log structured GLib records, which skip the classic
/// handler chain. Set `RUST_LOG=debug` to see the captured messages.
#[cfg(target_os = "linux")]
fn quiet_gtk_stack_noise() {
    const NOISY_DOMAINS: &[&str] = &[
        "Gtk",
        "Gdk",
        "GdkPixbuf",
        "GLib-GObject",
        "libayatana-appindicator",
        "libappindicator",
        "LIBDBUSMENU-GLIB",
        "dbind",
    ];
    glib::log_set_writer_func(|level, fields| {
        let field = |key: &str| {
            fields
                .iter()
                .find(|f| f.key() == key)
                .and_then(|f| f.value_str())
        };
        // GLib names the domain field GLIB_DOMAIN for records it builds
        // internally (which is what GTK/GDK emit); DOMAIN is only used by
        // callers of g_log_structured() that pass it explicitly.
        let domain = field("GLIB_DOMAIN").or_else(|| field("DOMAIN"));
        match domain {
            Some(domain) if NOISY_DOMAINS.contains(&domain) => {
                tracing::debug!(
                    target: "gtk",
                    domain,
                    ?level,
                    message = field("MESSAGE").unwrap_or_default(),
                    "gtk runtime"
                );
                glib::LogWriterOutput::Handled
            }
            _ => glib::log_writer_default(level, fields),
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn quiet_gtk_stack_noise() {}


fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();
    quiet_gtk_stack_noise();

    let mut args = std::env::args().skip(1);
    let mut url = std::env::var("RADIOCRAFT_UI_URL").ok();
    let mut config_dir = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url" => {
                url = args
                    .next()
                    .with_context(|| "--url needs an argument")?
                    .into();
            }
            _ if config_dir.is_none() => config_dir = Some(PathBuf::from(arg)),
            other => bail!("unknown argument: {other}"),
        }
    }
    let config_dir = config_dir.unwrap_or_else(radiocraft_server_dir);

    // Close-to-tray policy comes from the same settings file the server
    // reads, so `MinimizeToTray` behaves exactly like RadioDJ's.
    let db_settings = match config_dir.join(DatabaseSettings::FILE) {
        path if path.exists() => {
            DatabaseSettings::load(&path).with_context(|| DatabaseSettings::FILE)?
        }
        _ => DatabaseSettings::default(),
    };
    let policy = ClosePolicy::from_settings(&db_settings);

    // Server lifecycle: explicit URL = pure attach mode; otherwise reuse a
    // listening server, else spawn our own sibling binary.
    let mut server: Option<Child> = None;
    let url = match url {
        Some(u) => u,
        None => {
            let base = url_host(DEFAULT_URL)?;
            if port_open(&base) {
                tracing::info!("reusing existing server at {DEFAULT_URL}");
            } else {
                let child = spawn_server(&config_dir)?;
                wait_ready(&base, Duration::from_secs(10))?;
                server = Some(child);
            }
            DEFAULT_URL.to_string()
        }
    };

    // Token auth: the web UI reads `?token=` from the query string, so the
    // shell forwards the configured token into the page.
    let url = match std::env::var("RADIOCRAFT_AUTH_TOKEN") {
        Ok(token) if !token.trim().is_empty() => format!("{url}?token={}", token.trim()),
        _ => url,
    };
    let url: tauri::Url = url.parse().context("invalid UI URL")?;

    let app = tauri::Builder::default()
        .setup(move |app| {
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("RadioCraft")
                .inner_size(1380.0, 860.0)
                .min_inner_size(1024.0, 640.0)
                .build()?;
            window.set_focus().ok();

            let show = MenuItem::with_id(app, "show", "Show RadioCraft", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Exit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app: &AppHandle, event| match event.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if policy.on_close(false) == CloseAction::HideToTray {
                    api.prevent_close();
                    window.hide().ok();
                }
                // CloseAction::Exit: let the window close; the last window
                // destroyed ends the event loop.
            }
        })
        .build(tauri::generate_context!())
        .context("building the RadioCraft window")?;

    app.run(move |_app, event| {
        if let tauri::RunEvent::Exit = event {
            terminate_server(&mut server);
        }
    });
    Ok(())
}

/// Default config dir (same rule as `radiocraft-server`).
fn radiocraft_server_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("radiocraft");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("radiocraft")
}

/// `http://host:port` -> `host:port`.
fn url_host(url: &str) -> Result<String> {
    let parsed = tauri::Url::parse(url).context("invalid UI URL")?;
    let host = parsed.host_str().context("URL has no host")?;
    let port = parsed.port_or_known_default().context("URL has no port")?;
    Ok(format!("{host}:{port}"))
}

fn spawn_server(config_dir: &PathBuf) -> Result<Child> {
    let exe = std::env::current_exe().context("locating own executable")?;
    let sibling = exe
        .parent()
        .map(|dir| dir.join("radiocraft-server"))
        .filter(|p| p.exists());
    let program = sibling.unwrap_or_else(|| PathBuf::from("radiocraft-server"));
    tracing::info!("spawning {program:?} {config_dir:?}");
    let child = Command::new(&program)
        .arg(config_dir)
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", program.display()))?;
    Ok(child)
}

fn port_open(addr: &str) -> bool {
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return false;
    };
    addrs
        .any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok())
}

fn wait_ready(addr: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if port_open(addr) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("server did not become reachable at {addr} within {timeout:?}")
}

/// SIGTERM the child so its graceful shutdown books the rolling item.
fn terminate_server(server: &mut Option<Child>) {
    let Some(child) = server.as_mut() else {
        return;
    };
    #[cfg(unix)]
    {
        let pid = nix_pid(child);
        if pid != 0 {
            // SAFETY: plain kill of our own child; ESRCH is fine.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
    #[cfg(not(unix))]
    child.kill().ok();
    child.wait().ok();
}

#[cfg(unix)]
fn nix_pid(child: &Child) -> i32 {
    child.id() as i32
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        window.show().ok();
        window.set_focus().ok();
    }
}

fn toggle_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            window.hide().ok();
        } else {
            window.show().ok();
            window.set_focus().ok();
        }
    }
}
