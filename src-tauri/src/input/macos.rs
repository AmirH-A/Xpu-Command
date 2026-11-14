//! macOS: a Quartz event tap at the session level. Swallowing events needs an active (not
//! listen-only) tap, which macOS allows only with the Accessibility permission.
use super::{ButtonEvent, HookConfig};
use crate::error::{Error, Result};
use crate::settings::TalkButton;
use core_foundation::base::TCFType;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType, EventField};
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc::Sender;

extern "C" {
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
}

static TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// The events we subscribe to. The "tap disabled" notifications must NOT be listed: their type codes
/// (0xFFFFFFFE/F) don't fit in an event mask (the shift overflows), and macOS sends them anyway.
fn events_of_interest() -> Vec<CGEventType> {
    vec![CGEventType::OtherMouseDown, CGEventType::OtherMouseUp]
}

fn button_of(number: i64) -> Option<TalkButton> {
    match number {
        2 => Some(TalkButton::Middle),
        3 => Some(TalkButton::Back),
        4 => Some(TalkButton::Forward),
        _ => None,
    }
}

pub fn start(cfg: &'static HookConfig, tx: Sender<ButtonEvent>) -> Result<()> {
    if !crate::permissions::accessibility_trusted(false) {
        return Err(Error::Permission("Accessibility (to capture the talk button)".into()));
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    std::thread::Builder::new()
        .name("xpu-mouse-hook".into())
        .spawn(move || {
            let tap = CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                events_of_interest(),
                move |_proxy, ty, event| {
                    match ty {
                        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                            let p = TAP_PORT.load(Ordering::Relaxed);
                            if !p.is_null() {
                                unsafe { CGEventTapEnable(p, true) };
                            }
                            return Some(event.clone());
                        }
                        CGEventType::OtherMouseDown | CGEventType::OtherMouseUp => {}
                        _ => return Some(event.clone()),
                    }
                    if event.get_integer_value_field(EventField::EVENT_SOURCE_UNIX_PROCESS_ID) != 0 {
                        return Some(event.clone());
                    }
                    let n = event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
                    match button_of(n) {
                        Some(b) if cfg.matches(b) => {
                            let ev = if matches!(ty, CGEventType::OtherMouseDown) { ButtonEvent::Pressed } else { ButtonEvent::Released };
                            let _ = tx.send(ev);
                            None
                        }
                        _ => Some(event.clone()),
                    }
                },
            );
            let tap = match tap {
                Ok(t) => t,
                Err(()) => {
                    let _ = ready_tx.send(Err(Error::Permission("Accessibility (the event tap was refused)".into())));
                    return;
                }
            };
            TAP_PORT.store(tap.mach_port.as_concrete_TypeRef() as *mut c_void, Ordering::Relaxed);
            let source = match tap.mach_port.create_runloop_source(0) {
                Ok(s) => s,
                Err(()) => {
                    let _ = ready_tx.send(Err(crate::error::msg("could not attach the event tap")));
                    return;
                }
            };
            let run_loop = CFRunLoop::get_current();
            unsafe { run_loop.add_source(&source, kCFRunLoopCommonModes) };
            tap.enable();
            let _ = ready_tx.send(Ok(()));
            CFRunLoop::run_current();
        })?;
    ready_rx.recv().unwrap_or_else(|_| Err(crate::error::msg("mouse hook thread exited")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_mask_types_fit_in_the_mask() {
        for t in events_of_interest() {
            assert!((t as u64) < 64, "{t:?} can't be in an event mask");
        }
        assert!(!events_of_interest().iter().any(|t| matches!(t, CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput)));
    }

    #[test]
    fn quartz_button_numbers() {
        assert_eq!(button_of(2), Some(TalkButton::Middle));
        assert_eq!(button_of(3), Some(TalkButton::Back));
        assert_eq!(button_of(4), Some(TalkButton::Forward));
        assert_eq!(button_of(0), None, "left click is never captured");
        assert_eq!(button_of(1), None, "right click is never captured");
    }
}
