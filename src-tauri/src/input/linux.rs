//! Linux: the talk button.
//!
//! * X11: a passive grab of that one button on the root window: presses come to us only.
//! * Wayland: apps can't see global input, so the button is read from the mouse itself (evdev,
//!   /dev/input; the user must be in the `input` group). To keep its press from also reaching the
//!   app under the pointer, the mouse is taken over and every other event is passed on through a
//!   virtual copy of it (/dev/uinput). Without uinput access the press is only listened to.
//!   Virtual devices (other software's synthetic input) are ignored, as on the Mac.
use super::{ButtonEvent, HookConfig};
use crate::error::{msg, Error, Result};
use crate::settings::TalkButton;
use evdev::{Device, EventType, InputEvent, KeyCode};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

const PASSTHROUGH_NAME: &str = "Xpu Command mouse";

pub fn start(cfg: &'static HookConfig, tx: Sender<ButtonEvent>) -> Result<()> {
    match crate::linux::session() {
        crate::linux::Session::X11 => {
            let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
            std::thread::Builder::new().name("xpu-mouse-hook".into()).spawn(move || crate::linux::x11::grab_talk_button(cfg, tx, ready_tx))?;
            ready_rx.recv().unwrap_or_else(|_| Err(msg("mouse hook thread exited")))
        }
        crate::linux::Session::Wayland => start_evdev(cfg, tx),
    }
}

/// The talk button an evdev key code stands for (mice name the side buttons either way).
pub fn button_of(code: u16) -> Option<TalkButton> {
    match KeyCode::new(code) {
        KeyCode::BTN_MIDDLE => Some(TalkButton::Middle),
        KeyCode::BTN_SIDE | KeyCode::BTN_BACK => Some(TalkButton::Back),
        KeyCode::BTN_EXTRA | KeyCode::BTN_FORWARD => Some(TalkButton::Forward),
        _ => None,
    }
}

fn has_talk_buttons(d: &Device) -> bool {
    d.supported_keys().is_some_and(|k| [KeyCode::BTN_MIDDLE, KeyCode::BTN_SIDE, KeyCode::BTN_EXTRA, KeyCode::BTN_BACK, KeyCode::BTN_FORWARD].iter().any(|b| k.contains(*b)))
}

fn is_virtual(d: &Device) -> bool {
    d.input_id().bus_type() == evdev::BusType::BUS_VIRTUAL || d.name().is_some_and(|n| n == PASSTHROUGH_NAME)
}

/// Pointer devices with a talk button: real mice (and trackballs), never virtual devices.
fn mice() -> Vec<(PathBuf, Device)> {
    evdev::enumerate().filter(|(_, d)| has_talk_buttons(d) && !is_virtual(d)).collect()
}

/// Why the mouse can't be read, when it can't: no permission is the usual reason.
fn access_error() -> Error {
    let unreadable = std::fs::read_dir("/dev/input").map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().starts_with("event") && std::fs::File::open(e.path()).is_err())).unwrap_or(true);
    if unreadable {
        Error::Permission("the mouse (on Wayland, add yourself to the input group: sudo usermod -aG input $USER, then log out and in)".into())
    } else {
        msg("no mouse with a middle or side button was found")
    }
}

static HANDLED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn start_evdev(cfg: &'static HookConfig, tx: Sender<ButtonEvent>) -> Result<()> {
    let found = mice();
    if found.is_empty() {
        return Err(access_error());
    }
    for (path, dev) in found {
        watch(path, dev, cfg, tx.clone());
    }
    // New mice (plugged in, or Bluetooth reconnecting) are picked up too.
    std::thread::Builder::new().name("xpu-mouse-scan".into()).spawn(move || loop {
        std::thread::sleep(Duration::from_secs(3));
        for (path, dev) in mice() {
            watch(path, dev, cfg, tx.clone());
        }
    })?;
    Ok(())
}

fn watch(path: PathBuf, dev: Device, cfg: &'static HookConfig, tx: Sender<ButtonEvent>) {
    {
        let mut h = HANDLED.lock().unwrap_or_else(|p| p.into_inner());
        if !h.get_or_insert_with(HashSet::new).insert(path.clone()) {
            return;
        }
    }
    let name = dev.name().unwrap_or("mouse").to_string();
    let p = path.clone();
    let r = std::thread::Builder::new().name("xpu-mouse".into()).spawn(move || {
        if let Err(e) = run_device(dev, cfg, &tx) {
            log::info!("talk button: {name}: {e}");
        }
        HANDLED.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashSet::new).remove(&p);
    });
    if let Err(e) = r {
        log::warn!("talk button: {e}");
    }
}

/// A virtual copy of the mouse (same buttons and axes), to pass its other events on.
fn passthrough(dev: &Device) -> std::io::Result<evdev::uinput::VirtualDevice> {
    let mut b = evdev::uinput::VirtualDevice::builder()?.name(PASSTHROUGH_NAME);
    if let Some(k) = dev.supported_keys() {
        b = b.with_keys(k)?;
    }
    if let Some(r) = dev.supported_relative_axes() {
        b = b.with_relative_axes(r)?;
    }
    b.build()
}

fn run_device(mut dev: Device, cfg: &'static HookConfig, tx: &Sender<ButtonEvent>) -> std::io::Result<()> {
    // Only plain mice are taken over: a device with absolute axes (a touchpad, a tablet) is just
    // listened to, so it keeps working exactly as before.
    let plain = dev.supported_absolute_axes().map_or(true, |a| a.iter().next().is_none());
    let mut out = if plain {
        match passthrough(&dev) {
            Ok(v) => match dev.grab() {
                Ok(()) => Some(v),
                Err(e) => {
                    log::info!("talk button: can't take over the mouse ({e}); the press will also reach the app");
                    None
                }
            },
            Err(e) => {
                log::info!("talk button: no /dev/uinput access ({e}); the press will also reach the app");
                None
            }
        }
    } else {
        None
    };
    let mut batch: Vec<InputEvent> = Vec::new();
    loop {
        for ev in dev.fetch_events()? {
            if ev.event_type() == EventType::SYNCHRONIZATION {
                if let Some(v) = out.as_mut() {
                    if !batch.is_empty() {
                        v.emit(&batch)?;
                    }
                }
                batch.clear();
                continue;
            }
            if ev.event_type() == EventType::KEY {
                if let Some(b) = button_of(ev.code()) {
                    if cfg.matches(b) {
                        match ev.value() {
                            1 => {
                                let _ = tx.send(ButtonEvent::Pressed);
                            }
                            0 => {
                                let _ = tx.send(ButtonEvent::Released);
                            }
                            _ => {}
                        }
                        continue;
                    }
                }
            }
            batch.push(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evdev_button_codes() {
        assert_eq!(button_of(KeyCode::BTN_MIDDLE.code()), Some(TalkButton::Middle));
        assert_eq!(button_of(KeyCode::BTN_SIDE.code()), Some(TalkButton::Back));
        assert_eq!(button_of(KeyCode::BTN_BACK.code()), Some(TalkButton::Back));
        assert_eq!(button_of(KeyCode::BTN_EXTRA.code()), Some(TalkButton::Forward));
        assert_eq!(button_of(KeyCode::BTN_LEFT.code()), None, "left click is never captured");
        assert_eq!(button_of(KeyCode::BTN_RIGHT.code()), None, "right click is never captured");
    }
}
