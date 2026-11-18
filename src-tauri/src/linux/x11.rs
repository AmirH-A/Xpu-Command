//! X11 (Xorg sessions): the window in front and its process, the windows on screen, bringing one
//! forward, a polite "close", the talk button (a passive button grab, so the press is ours alone),
//! synthetic pointer input (XTest) and window images. Not used on Wayland, where X11 only sees
//! XWayland apps.
use parking_lot::Mutex;
use std::sync::Arc;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

struct X {
    conn: RustConnection,
    root: Window,
}

static X: Mutex<Option<Arc<X>>> = Mutex::new(None);

fn x() -> Result<Arc<X>, String> {
    let mut slot = X.lock();
    if let Some(x) = slot.as_ref() {
        return Ok(x.clone());
    }
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("can't reach the X server: {e}"))?;
    let root = conn.setup().roots.get(screen).ok_or("no X screen")?.root;
    let x = Arc::new(X { conn, root });
    *slot = Some(x.clone());
    Ok(x)
}

fn atom(x: &X, name: &str) -> Option<u32> {
    x.conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom)
}

fn prop32(x: &X, w: Window, name: &str, ty: impl Into<u32>) -> Vec<u32> {
    let Some(a) = atom(x, name) else { return Vec::new() };
    match x.conn.get_property(false, w, a, ty.into(), 0, 4096).ok().and_then(|c| c.reply().ok()) {
        Some(r) => r.value32().map(|v| v.collect()).unwrap_or_default(),
        None => Vec::new(),
    }
}

fn prop_string(x: &X, w: Window, name: &str) -> Option<String> {
    let a = atom(x, name)?;
    let r = x.conn.get_property(false, w, a, AtomEnum::ANY, 0, 1024).ok()?.reply().ok()?;
    (!r.value.is_empty()).then(|| String::from_utf8_lossy(&r.value).trim_end_matches('\0').to_string())
}

/// The window the window manager says is active.
pub fn active_window() -> Option<Window> {
    let x = x().ok()?;
    prop32(&x, x.root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW).first().copied().filter(|w| *w != 0)
}

pub fn pid_of(w: Window) -> Option<i32> {
    let x = x().ok()?;
    prop32(&x, w, "_NET_WM_PID", AtomEnum::CARDINAL).first().map(|p| *p as i32).filter(|p| *p > 0)
}

/// The process of the active window.
pub fn active_pid() -> Option<i32> {
    active_window().and_then(pid_of)
}

pub fn title_of(w: Window) -> Option<String> {
    let x = x().ok()?;
    prop_string(&x, w, "_NET_WM_NAME").or_else(|| prop_string(&x, w, "WM_NAME")).filter(|t| !t.is_empty())
}

/// A window on screen: (window, pid, frame in screen pixels, title).
pub type ScreenWindow = (Window, i32, (f64, f64, f64, f64), String);

/// Normal app windows on the current workspace, front to back.
pub fn windows() -> Vec<ScreenWindow> {
    let Ok(x) = x() else { return Vec::new() };
    let stacking = prop32(&x, x.root, "_NET_CLIENT_LIST_STACKING", AtomEnum::WINDOW);
    let current = prop32(&x, x.root, "_NET_CURRENT_DESKTOP", AtomEnum::CARDINAL).first().copied();
    let hidden = atom(&x, "_NET_WM_STATE_HIDDEN");
    let skip_types: Vec<u32> = ["_NET_WM_WINDOW_TYPE_DOCK", "_NET_WM_WINDOW_TYPE_DESKTOP", "_NET_WM_WINDOW_TYPE_SPLASH", "_NET_WM_WINDOW_TYPE_NOTIFICATION"].iter().filter_map(|n| atom(&x, n)).collect();
    let mut out = Vec::new();
    for &w in stacking.iter().rev() {
        let states = prop32(&x, w, "_NET_WM_STATE", AtomEnum::ATOM);
        if hidden.is_some_and(|h| states.contains(&h)) {
            continue;
        }
        if prop32(&x, w, "_NET_WM_WINDOW_TYPE", AtomEnum::ATOM).iter().any(|t| skip_types.contains(t)) {
            continue;
        }
        if let (Some(cur), Some(d)) = (current, prop32(&x, w, "_NET_WM_DESKTOP", AtomEnum::CARDINAL).first().copied()) {
            if d != cur && d != 0xFFFF_FFFF {
                continue;
            }
        }
        let Some(frame) = frame_of(&x, w) else { continue };
        if frame.2 < 50.0 || frame.3 < 50.0 {
            continue;
        }
        let Some(pid) = pid_of(w) else { continue };
        out.push((w, pid, frame, title_of(w).unwrap_or_default()));
    }
    out
}

fn frame_of(x: &X, w: Window) -> Option<(f64, f64, f64, f64)> {
    let g = x.conn.get_geometry(w).ok()?.reply().ok()?;
    let t = x.conn.translate_coordinates(w, x.root, 0, 0).ok()?.reply().ok()?;
    Some((f64::from(t.dst_x), f64::from(t.dst_y), f64::from(g.width), f64::from(g.height)))
}

fn client_message(x: &X, w: Window, name: &str, data: [u32; 5]) -> Result<(), String> {
    let a = atom(x, name).ok_or("the window manager doesn't support this")?;
    let ev = ClientMessageEvent::new(32, w, a, data);
    x.conn
        .send_event(false, x.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    x.conn.flush().map_err(|e| e.to_string())
}

/// Asks the window manager to bring the window forward (as a taskbar would).
pub fn activate(w: Window) -> Result<(), String> {
    let x = x()?;
    client_message(&x, w, "_NET_ACTIVE_WINDOW", [2, 0, 0, 0, 0])
}

/// Asks the window to close, as its close button would (the app may still ask to save).
pub fn close(w: Window) -> Result<(), String> {
    let x = x()?;
    client_message(&x, w, "_NET_CLOSE_WINDOW", [0, 2, 0, 0, 0])
}

const MOTION: u8 = 6;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;

fn fake(x: &X, kind: u8, detail: u8, px: i16, py: i16) -> Result<(), String> {
    x.conn.xtest_fake_input(kind, detail, 0, x.root, px, py, 0).map_err(|e| e.to_string())?;
    x.conn.flush().map_err(|e| e.to_string())
}

/// Moves the pointer to a screen point.
pub fn move_to(px: f64, py: f64) -> Result<(), String> {
    let x = x()?;
    fake(&x, MOTION, 0, px.round() as i16, py.round() as i16)
}

/// Presses or releases a pointer button (1 left, 2 middle, 3 right, 4/5 wheel).
pub fn button(b: u8, down: bool) -> Result<(), String> {
    let x = x()?;
    fake(&x, if down { BUTTON_PRESS } else { BUTTON_RELEASE }, b, 0, 0)
}

/// The screen's pixels in a rectangle, as RGB.
pub fn capture(frame: (f64, f64, f64, f64)) -> Result<image::RgbImage, String> {
    let x = x()?;
    let (fx, fy, fw, fh) = frame;
    let screen = &x.conn.setup().roots[0];
    let (sw, sh) = (f64::from(screen.width_in_pixels), f64::from(screen.height_in_pixels));
    let (x0, y0) = (fx.max(0.0), fy.max(0.0));
    let (w, h) = ((fx + fw).min(sw) - x0, (fy + fh).min(sh) - y0);
    if w < 1.0 || h < 1.0 {
        return Err("the window is off screen".into());
    }
    let (w, h) = (w as u16, h as u16);
    let img = x
        .conn
        .get_image(xproto::ImageFormat::Z_PIXMAP, x.root, x0 as i16, y0 as i16, w, h, !0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| format!("couldn't read the screen: {e}"))?;
    let lsb = x.conn.setup().image_byte_order == xproto::ImageOrder::LSB_FIRST;
    let bpp = x.conn.setup().pixmap_formats.iter().find(|f| f.depth == img.depth).map(|f| f.bits_per_pixel).unwrap_or(32);
    if bpp != 32 {
        return Err(format!("unsupported screen format ({bpp} bits per pixel)"));
    }
    let need = usize::from(w) * usize::from(h) * 4;
    if img.data.len() < need {
        return Err("the screen image is incomplete".into());
    }
    let mut rgb = image::RgbImage::new(u32::from(w), u32::from(h));
    for (i, px) in rgb.pixels_mut().enumerate() {
        let p = &img.data[i * 4..i * 4 + 4];
        *px = if lsb { image::Rgb([p[2], p[1], p[0]]) } else { image::Rgb([p[1], p[2], p[3]]) };
    }
    Ok(rgb)
}

/// X11 button numbers of the talk buttons.
pub fn talk_button_number(b: crate::settings::TalkButton) -> u8 {
    match b {
        crate::settings::TalkButton::Middle => 2,
        crate::settings::TalkButton::Back => 8,
        crate::settings::TalkButton::Forward => 9,
    }
}

/// Holds the configured talk button (with any modifiers) on the root window: presses go to us
/// only, every other button and all keys stay untouched. Follows changes to the button and to
/// pause/resume; ends when the X connection does.
pub fn grab_talk_button(cfg: &'static crate::input::HookConfig, tx: std::sync::mpsc::Sender<crate::input::ButtonEvent>, ready: std::sync::mpsc::Sender<crate::error::Result<()>>) {
    use crate::input::ButtonEvent;
    let (conn, screen) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(crate::error::msg(format!("can't reach the X server: {e}"))));
            return;
        }
    };
    let root = conn.setup().roots[screen].root;
    let grab = |n: u8| -> Result<(), String> {
        conn.grab_button(false, root, EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE, xproto::GrabMode::ASYNC, xproto::GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, xproto::ButtonIndex::from(n), xproto::ModMask::ANY)
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|_| "another app already holds that mouse button".to_string())
    };
    let ungrab = |n: u8| {
        if let Ok(c) = conn.ungrab_button(xproto::ButtonIndex::from(n), root, xproto::ModMask::ANY) {
            let _ = c.check();
        }
    };
    let want = || cfg.enabled.load(std::sync::atomic::Ordering::Relaxed).then(|| cfg.button()).flatten().map(talk_button_number);
    let mut held: Option<u8> = want();
    if let Some(n) = held {
        if let Err(e) = grab(n) {
            let _ = ready.send(Err(crate::error::msg(e)));
            return;
        }
    }
    let _ = conn.flush();
    let _ = ready.send(Ok(()));
    loop {
        loop {
            match conn.poll_for_event() {
                Ok(Some(Event::ButtonPress(e))) if Some(e.detail) == held && e.response_type & 0x80 == 0 => {
                    let _ = tx.send(ButtonEvent::Pressed);
                }
                Ok(Some(Event::ButtonRelease(e))) if Some(e.detail) == held && e.response_type & 0x80 == 0 => {
                    let _ = tx.send(ButtonEvent::Released);
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    log::warn!("talk button: the X connection closed: {e}");
                    return;
                }
            }
        }
        let now = want();
        if now != held {
            if let Some(n) = held {
                ungrab(n);
            }
            if let Some(n) = now {
                if let Err(e) = grab(n) {
                    log::warn!("talk button: {e}");
                }
            }
            let _ = conn.flush();
            held = now;
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

/// Seconds since the last key press or mouse use, in any app (X11 only: only the timing is
/// read, never what was pressed). None on Wayland, which doesn't share it.
pub fn idle_seconds() -> Option<f64> {
    use x11rb::protocol::screensaver::ConnectionExt as _;
    if super::session() != super::Session::X11 {
        return None;
    }
    let x = x().ok()?;
    let r = x.conn.screensaver_query_info(x.root).ok()?.reply().ok()?;
    Some(f64::from(r.ms_since_user_input) / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talk_buttons_map_to_x11_numbers() {
        use crate::settings::TalkButton::*;
        assert_eq!(talk_button_number(Middle), 2);
        assert_eq!(talk_button_number(Back), 8);
        assert_eq!(talk_button_number(Forward), 9);
    }
}
