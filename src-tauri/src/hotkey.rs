//! Keyboard shortcut as a second talk trigger (for trackpads and laptops). It uses the OS's
//! global-hotkey registration (macOS RegisterEventHotKey, Windows RegisterHotKey, X11 key grabs,
//! and on Wayland the desktop's GlobalShortcuts portal): the app is told only about this one
//! combination; it never sees any other key. Hold to talk, release to send.
use crate::controller::Controller;
use crate::error::{msg, Result};
use crate::input::ButtonEvent;
use crate::settings::Hotkey;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

pub fn shortcut(h: Hotkey) -> Shortcut {
    match h {
        Hotkey::CtrlAltSpace => Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space),
        Hotkey::AltShiftSpace => Shortcut::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::Space),
        Hotkey::CtrlShiftSpace => Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space),
    }
}

/// The shortcut bound through the Wayland portal, while one is.
#[cfg(target_os = "linux")]
static PORTAL: parking_lot::Mutex<Option<crate::linux::portal::Shortcut>> = parking_lot::Mutex::new(None);

/// Replaces the registered shortcut with `h` (or none).
pub fn apply(app: &AppHandle, h: Option<Hotkey>) -> Result<()> {
    #[cfg(target_os = "linux")]
    if crate::linux::session() == crate::linux::Session::Wayland {
        *PORTAL.lock() = None;
        let Some(h) = h else { return Ok(()) };
        let ctl: Arc<Controller> = app.state::<Arc<Controller>>().inner().clone();
        let handle = app.clone();
        // The desktop may show a dialog to confirm the keys: don't hold up the caller.
        std::thread::spawn(move || {
            let r = crate::linux::portal::bind_shortcut(h, move |down| {
                if ctl.hook.enabled.load(Ordering::Relaxed) {
                    ctl.on_button(&handle, if down { ButtonEvent::Pressed } else { ButtonEvent::Released });
                }
            });
            match r {
                Ok(s) => *PORTAL.lock() = Some(s),
                Err(e) => log::warn!("keyboard shortcut {h:?}: {e} (on Wayland it needs a desktop with the GlobalShortcuts portal: GNOME 48+, KDE Plasma, Hyprland)"),
            }
        });
        return Ok(());
    }
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| msg(format!("shortcut: {e}")))?;
    let Some(h) = h else { return Ok(()) };
    let ctl: Arc<Controller> = app.state::<Arc<Controller>>().inner().clone();
    gs.on_shortcut(shortcut(h), move |app, _s, ev| {
        if !ctl.hook.enabled.load(Ordering::Relaxed) {
            return;
        }
        let e = match ev.state {
            ShortcutState::Pressed => ButtonEvent::Pressed,
            ShortcutState::Released => ButtonEvent::Released,
        };
        ctl.on_button(app, e);
    })
    .map_err(|e| msg(format!("shortcut {h:?} is taken by another app: {e}")))
}
