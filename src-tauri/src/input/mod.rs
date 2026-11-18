//! Global mouse-button hook. Only the configured talk button is captured; it is swallowed so it
//! doesn't also do its normal job (e.g. "Back" in a browser). Every other event passes untouched.
//! Synthetic (injected) clicks from other software are ignored, so no program can trigger the mic.
use crate::settings::TalkButton;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::Sender;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    Pressed,
    Released,
}

/// Shared with the hook thread: which button is the talk button, and whether capture is on.
pub struct HookConfig {
    button: AtomicU8,
    pub enabled: AtomicBool,
}

impl HookConfig {
    pub fn new(b: TalkButton) -> Self {
        Self { button: AtomicU8::new(code(b)), enabled: AtomicBool::new(true) }
    }
    pub fn set_button(&self, b: TalkButton) {
        self.button.store(code(b), Ordering::Relaxed);
    }
    pub fn button(&self) -> Option<TalkButton> {
        match self.button.load(Ordering::Relaxed) {
            0 => Some(TalkButton::Middle),
            1 => Some(TalkButton::Back),
            2 => Some(TalkButton::Forward),
            _ => None,
        }
    }
    fn matches(&self, b: TalkButton) -> bool {
        self.enabled.load(Ordering::Relaxed) && self.button() == Some(b)
    }
}

fn code(b: TalkButton) -> u8 {
    match b {
        TalkButton::Middle => 0,
        TalkButton::Back => 1,
        TalkButton::Forward => 2,
    }
}

/// Starts the hook on its own thread. Errors when the OS refuses (macOS without Accessibility,
/// Linux on Wayland without access to the mouse).
pub fn start(cfg: &'static HookConfig, tx: Sender<ButtonEvent>) -> crate::error::Result<()> {
    #[cfg(target_os = "macos")]
    return macos::start(cfg, tx);
    #[cfg(target_os = "windows")]
    return windows::start(cfg, tx);
    #[cfg(target_os = "linux")]
    return linux::start(cfg, tx);
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (cfg, tx);
        Err(crate::error::msg("this platform is not supported yet"))
    }
}
