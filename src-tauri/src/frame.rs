//! The outline around the window the assistant is working on, so the user always sees which app is
//! being read or controlled. A transparent, click-through window that never takes focus.
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager};

const PAD: f64 = 4.0;

/// Outlines `frame` (screen points: x, y, width, height).
pub fn show(app: &AppHandle, frame: (f64, f64, f64, f64)) {
    let Some(w) = app.get_webview_window("frame") else { return };
    let (x, y, fw, fh) = frame;
    if !(fw.is_finite() && fh.is_finite()) || fw < 20.0 || fh < 20.0 {
        return;
    }
    let _ = w.set_position(LogicalPosition::new(x - PAD, y - PAD));
    let _ = w.set_size(LogicalSize::new(fw + 2.0 * PAD, fh + 2.0 * PAD));
    let _ = w.set_ignore_cursor_events(true);
    crate::overlay_window::show_without_focus(&w);
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("frame") {
        let _ = w.hide();
    }
}
