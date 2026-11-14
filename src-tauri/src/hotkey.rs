//! Keyboard shortcut as a second talk trigger (for trackpads and laptops). It uses the OS's
//! global-hotkey registration (macOS RegisterEventHotKey, Windows RegisterHotKey): the app is told
//! only about this one combination; it never sees any other key. Hold to talk, release to send.
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

/// Replaces the registered shortcut with `h` (or none).
pub fn apply(app: &AppHandle, h: Option<Hotkey>) -> Result<()> {
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
