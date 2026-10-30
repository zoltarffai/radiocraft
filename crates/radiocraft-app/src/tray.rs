//! System-tray integration, compiled only with the `tray` cargo feature.
//!
//! Close-to-tray itself is pure policy ([`radiocraft_net::autost::ClosePolicy`])
//! plus eframe viewport commands, so it works without this module; the tray
//! icon is only the way to bring the window back once it is hidden.

use anyhow::{Context, Result};
use eframe::egui::{self, ViewportCommand};
use tray_icon::TrayIcon;

/// A minimal 2x2 pixel map so platforms always have something to render;
/// the real application icon can be supplied later.
pub fn placeholder_image() -> tray_icon::Icon {
    let rgba = vec![0u8; 2 * 2 * 4];
    tray_icon::Icon::from_rgba(rgba, 2, 2).unwrap_or_else(|_| {
        tray_icon::Icon::from_rgba(vec![128u8; 2 * 2 * 4], 2, 2)
            .expect("a flat-colour icon always fits the RGBA contract")
    })
}

/// Owns the tray icon: dropping it removes the icon again.
pub struct TrayGuard {
    _icon: TrayIcon,
}

impl TrayGuard {
    /// Poll the tray menus and click events.  When the user picks the icon
    /// or its "Restore" entry, the hidden window is brought back.  Returns
    /// true when a restore was requested.
    pub fn poll(&self, ctx: &egui::Context) -> bool {
        let mut restore = false;
        while let Some(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
            match event {
                tray_icon::TrayIconEvent::Click { .. }
                | tray_icon::TrayIconEvent::DoubleClicked { .. } => restore = true,
                tray_icon::TrayIconEvent::MenuAction(action) => {
                    if action.id() == "restore" {
                        restore = true;
                    }
                }
                _ => {}
            }
        }
        // drain the menu channel so stale entries do not replay later
        while tray_icon::menu::MenuEvent::receiver().try_recv().is_ok() {}
        if restore {
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        restore
    }
}

/// Install the icon and its "Restore" menu.
pub fn install(ctx: &egui::Context, icon: tray_icon::Icon) -> Result<TrayGuard> {
    let _ = ctx; // polling happens from the app's update loop

    let mut menu = tray_icon::menu::Menu::new();
    let restore = tray_icon::menu::MenuItem::with_id("restore", "Restore RadioCraft", true, None);
    menu
        .append(&restore)
        .context("appending the tray menu entry")?;

    let attributes = tray_icon::TrayIconAttributes::new()
        .with_icon(icon)
        .with_tooltip("RadioCraft")
        .with_menu(Box::new(menu));

    let _icon = TrayIcon::new(attributes).context("creating the tray icon")?;
    Ok(TrayGuard { _icon })
}
