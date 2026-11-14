//! Windows: a low-level mouse hook (WH_MOUSE_LL) on a thread with its own message loop.
//! No elevation needed. Returning 1 swallows the event.
use super::{ButtonEvent, HookConfig};
use crate::error::Result;
use crate::settings::TalkButton;
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage, HC_ACTION, LLMHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

static STATE: OnceLock<(&'static HookConfig, std::sync::Mutex<Sender<ButtonEvent>>)> = OnceLock::new();
const XBUTTON1: u16 = 1;
const XBUTTON2: u16 = 2;

pub(crate) fn classify(msg: u32, mouse_data: u32) -> Option<(TalkButton, ButtonEvent)> {
    let x = (mouse_data >> 16) as u16;
    match msg {
        WM_MBUTTONDOWN => Some((TalkButton::Middle, ButtonEvent::Pressed)),
        WM_MBUTTONUP => Some((TalkButton::Middle, ButtonEvent::Released)),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let b = match x {
                XBUTTON1 => TalkButton::Back,
                XBUTTON2 => TalkButton::Forward,
                _ => return None,
            };
            Some((b, if msg == WM_XBUTTONDOWN { ButtonEvent::Pressed } else { ButtonEvent::Released }))
        }
        _ => None,
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some((cfg, tx)) = STATE.get() {
            let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let injected = info.flags & LLMHF_INJECTED != 0;
            if !injected {
                if let Some((b, ev)) = classify(wparam.0 as u32, info.mouseData) {
                    if cfg.matches(b) {
                        if let Ok(tx) = tx.lock() {
                            let _ = tx.send(ev);
                        }
                        return LRESULT(1);
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

pub fn start(cfg: &'static HookConfig, tx: Sender<ButtonEvent>) -> Result<()> {
    if STATE.set((cfg, std::sync::Mutex::new(tx))).is_err() {
        return Err(crate::error::msg("mouse hook already running"));
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    std::thread::Builder::new().name("xpu-mouse-hook".into()).spawn(move || unsafe {
        let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        match SetWindowsHookExW(WH_MOUSE_LL, Some(hook), module, 0) {
            Ok(_) => {
                let _ = ready_tx.send(Ok(()));
            }
            Err(e) => {
                let _ = ready_tx.send(Err(crate::error::msg(format!("mouse hook refused: {e}"))));
                return;
            }
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    })?;
    ready_rx.recv().unwrap_or_else(|_| Err(crate::error::msg("mouse hook thread exited")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn x_buttons_and_middle() {
        assert_eq!(classify(WM_XBUTTONDOWN, 1 << 16), Some((TalkButton::Back, ButtonEvent::Pressed)));
        assert_eq!(classify(WM_XBUTTONUP, 2 << 16), Some((TalkButton::Forward, ButtonEvent::Released)));
        assert_eq!(classify(WM_MBUTTONDOWN, 0), Some((TalkButton::Middle, ButtonEvent::Pressed)));
        assert_eq!(classify(0x0201 /* WM_LBUTTONDOWN */, 0), None);
    }
}
