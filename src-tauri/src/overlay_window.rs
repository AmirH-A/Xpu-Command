//! Showing the overlay without taking focus. A normal "show" makes the window key and activates the
//! app, so keystrokes (including the text we type in Mode A) would land in the overlay. Each OS has
//! a way to put a window on top *without* activating it; this uses it.
use tauri::WebviewWindow;

#[cfg(target_os = "macos")]
pub fn show_without_focus(w: &WebviewWindow) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    let Ok(ptr) = w.ns_window() else { return };
    let ns = ptr as *mut AnyObject;
    if ns.is_null() {
        return;
    }
    let w2 = w.clone();
    let _ = w.run_on_main_thread(move || unsafe {
        let ns = match w2.ns_window() {
            Ok(p) if !p.is_null() => p as *mut AnyObject,
            _ => return,
        };
        let behavior: usize = (1 << 0) | (1 << 8);
        let _: () = msg_send![ns, setCollectionBehavior: behavior];
        let _: () = msg_send![ns, orderFrontRegardless];
    });
}

#[cfg(target_os = "windows")]
pub fn show_without_focus(w: &WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};
    let Ok(h) = w.hwnd() else { return };
    let hwnd = HWND(h.0 as _);
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

/// Linux (GTK): a window that never takes focus, kept above others and out of the taskbar.
#[cfg(target_os = "linux")]
pub fn show_without_focus(w: &WebviewWindow) {
    use gtk::prelude::*;
    let w2 = w.clone();
    let _ = w.run_on_main_thread(move || {
        if let Ok(g) = w2.gtk_window() {
            g.set_accept_focus(false);
            g.set_focus_on_map(false);
            g.set_keep_above(true);
            g.set_skip_taskbar_hint(true);
            g.set_skip_pager_hint(true);
            g.set_type_hint(gtk::gdk::WindowTypeHint::Notification);
            g.show_all();
        }
    });
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub fn show_without_focus(w: &WebviewWindow) {
    let _ = w.show();
}
