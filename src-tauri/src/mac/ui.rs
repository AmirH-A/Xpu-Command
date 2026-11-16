//! Seeing and controlling other apps' windows, for one request.
//!
//! * Reading uses the Accessibility tree: real controls with names ("button Compose"), numbered so
//!   the model can refer to them. Numbers are valid only within this request.
//! * Acting prefers accessibility actions (press, focus); keystrokes are posted to the target
//!   process only (CGEventPostToPid), never to "whatever is in front".
//! * Mouse clicks (fallback) first check that the spot really belongs to the target app.
//! * Password fields are never typed into.
use super::ax::{self, El, V};
use super::policy::{Need, Target};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct RunningApp {
    pub pid: i32,
    pub bundle: String,
    pub name: String,
}

struct Entry {
    el: El,
    pid: i32,
    label: String,
}

#[derive(Clone, Copy)]
struct Shot {
    frame: (f64, f64, f64, f64),
    width_px: f64,
    height_px: f64,
}

/// One request's view of the screen: numbered controls and the last screenshot per app.
#[derive(Default)]
pub struct Session {
    entries: Mutex<Vec<Entry>>,
    shots: Mutex<HashMap<i32, Shot>>,
    web_enabled: Mutex<Vec<i32>>,
}

const MAX_ENTRIES: usize = 6000;
const MAX_VISIT: usize = 5000;
const MAX_LIST: usize = 400;
const MAX_CHARS: usize = 16_000;


extern "C" {
    fn proc_pidpath(pid: i32, buf: *mut c_void, size: u32) -> i32;
}

/// The bundle id and name of a process, read live from its executable's app bundle.
pub fn app_of_pid(pid: i32) -> Option<RunningApp> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let exe = PathBuf::from(String::from_utf8_lossy(&buf[..n as usize]).to_string());
    let app_dir = exe.ancestors().filter(|p| p.extension().is_some_and(|e| e == "app")).last()?.to_path_buf();
    let (bundle, name) = bundle_info(&app_dir)?;
    Some(RunningApp { pid, bundle, name })
}

/// (bundle id, display name) of an `.app` folder.
pub fn bundle_info(app: &std::path::Path) -> Option<(String, String)> {
    use core_foundation::base::TCFType;
    let url = core_foundation::url::CFURL::from_path(app, true)?;
    let b = core_foundation::bundle::CFBundle::new(url)?;
    let id = unsafe { core_foundation::bundle::CFBundleGetIdentifier(b.as_concrete_TypeRef()) };
    if id.is_null() {
        return None;
    }
    let id = unsafe { core_foundation::string::CFString::wrap_under_get_rule(id) }.to_string();
    let name = app.file_stem()?.to_string_lossy().to_string();
    Some((id, name))
}

/// A window on screen: (pid, window number, frame in points, owner name).
type ScreenWindow = (i32, u32, (f64, f64, f64, f64), String);

/// Windows on screen right now, front to back.
fn screen_windows() -> Vec<ScreenWindow> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly};
    let Some(list) = copy_window_info(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID) else { return Vec::new() };
    let mut out = Vec::new();
    for item in list.iter() {
        let d: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(*item as _) };
        let num = |k: &str| d.find(CFString::new(k)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_f64());
        let (Some(pid), Some(id), Some(layer)) = (num("kCGWindowOwnerPID"), num("kCGWindowNumber"), num("kCGWindowLayer")) else { continue };
        if layer != 0.0 {
            continue;
        }
        let Some(b) = d.find(CFString::new("kCGWindowBounds")).and_then(|v| v.downcast::<CFDictionary>()) else { continue };
        let b: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(b.as_concrete_TypeRef()) };
        let g = |k: &str| b.find(CFString::new(k)).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_f64()).unwrap_or(0.0);
        let frame = (g("X"), g("Y"), g("Width"), g("Height"));
        if frame.2 < 50.0 || frame.3 < 50.0 {
            continue;
        }
        let owner = d.find(CFString::new("kCGWindowOwnerName")).and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string()).unwrap_or_default();
        out.push((pid as i32, id as u32, frame, owner));
    }
    out
}

/// The app in front: the one with keyboard focus, else the owner of the frontmost window.
pub fn front_pid() -> Option<i32> {
    ax::focused_pid().or_else(|| screen_windows().first().map(|w| w.0))
}

/// Apps with a window on screen, front to back (live).
pub fn apps_on_screen() -> Vec<RunningApp> {
    let mut seen: Vec<i32> = Vec::new();
    let mut out = Vec::new();
    for (pid, _, _, _) in screen_windows() {
        if seen.contains(&pid) {
            continue;
        }
        seen.push(pid);
        if let Some(a) = app_of_pid(pid) {
            out.push(a);
        }
    }
    out
}

/// A running app with a window on screen, by spoken name.
pub fn find_app(spoken: &str) -> Result<RunningApp, String> {
    let on_screen = apps_on_screen();
    match super::apps::pick(&on_screen, |a| &a.name, spoken) {
        Ok(hit) => Ok(hit.clone()),
        Err(Some(e)) => Err(e),
        Err(None) => Err(format!("\"{spoken}\" has no window open. Open it with open_app first.")),
    }
}

/// The app's main window: focused, else main, else the first one.
fn main_window(pid: i32) -> Option<El> {
    let app = El::app(pid);
    app.element("AXFocusedWindow").or_else(|| app.element("AXMainWindow")).or_else(|| app.elements("AXWindows").into_iter().next())
}

pub fn window_title(pid: i32) -> Option<String> {
    main_window(pid).and_then(|w| w.string("AXTitle")).filter(|t| !t.is_empty())
}

pub fn window_frame(pid: i32) -> Option<(f64, f64, f64, f64)> {
    main_window(pid).and_then(|w| w.frame())
}

/// Brings the app to the front (via LaunchServices) and waits until it has keyboard focus.
pub fn activate(app: &RunningApp) -> Result<(), String> {
    if front_pid() == Some(app.pid) {
        return Ok(());
    }
    super::exec::run_program("/usr/bin/open", &["-b".as_ref(), app.bundle.as_ref()], Duration::from_secs(10))?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(2500) {
        if front_pid() == Some(app.pid) {
            std::thread::sleep(Duration::from_millis(120));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!("{} didn't come to the front", app.name))
}


fn role_name(role: &str, subrole: &str) -> Option<&'static str> {
    Some(match (role, subrole) {
        (_, "AXSecureTextField") => "password field",
        (_, "AXSearchField") => "search field",
        (_, "AXTabButton") => "tab",
        ("AXButton", _) => "button",
        ("AXLink", _) => "link",
        ("AXTextField", _) => "text field",
        ("AXTextArea", _) => "text area",
        ("AXComboBox", _) => "combo box",
        ("AXCheckBox", _) => "checkbox",
        ("AXRadioButton", _) => "radio",
        ("AXPopUpButton", _) => "popup",
        ("AXMenuButton", _) => "menu button",
        ("AXMenuItem", _) => "menu item",
        ("AXSlider", _) => "slider",
        ("AXIncrementor", _) => "stepper",
        ("AXHeading", _) => "heading",
        ("AXStaticText", _) => "text",
        ("AXImage", _) => "image",
        ("AXCell", _) => "cell",
        ("AXRow", _) => "row",
        ("AXDisclosureTriangle", _) => "disclosure",
        _ => return None,
    })
}

fn one_line(s: &str, max: usize) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().filter(|c| !crate::sanitize::is_dangerous(*c)).collect();
    if t.chars().count() > max {
        format!("{}…", t.chars().take(max).collect::<String>())
    } else {
        t
    }
}

const ATTRS: [&str; 9] = ["AXRole", "AXSubrole", "AXTitle", "AXDescription", "AXValue", "AXPlaceholderValue", "AXChildren", "AXEnabled", "AXFocused"];

impl Session {
    fn register(&self, el: El, pid: i32, label: String) -> Option<usize> {
        let mut e = self.entries.lock();
        if e.len() >= MAX_ENTRIES {
            return None;
        }
        e.push(Entry { el, pid, label });
        Some(e.len())
    }

    /// Chrome, Electron apps and other Chromium-based apps build their accessibility tree only
    /// when asked; ask once per process.
    fn enable_web(&self, pid: i32) {
        let mut done = self.web_enabled.lock();
        if done.contains(&pid) {
            return;
        }
        done.push(pid);
        if El::app(pid).set_bool("AXManualAccessibility", true).is_ok() {
            std::thread::sleep(Duration::from_millis(400));
        }
    }

    /// The app's front window as numbered lines: "[12] button “Compose”".
    pub fn read_window(&self, app: &RunningApp) -> Result<String, String> {
        trusted()?;
        ax::set_global_timeout(1.5);
        self.enable_web(app.pid);
        let win = main_window(app.pid).ok_or_else(|| format!("{} has no window to read", app.name))?;
        let title = win.string("AXTitle").unwrap_or_default();
        let mut out = format!("{} — window “{}”\n", app.name, one_line(&title, 120));
        let mut stack: Vec<(El, u16)> = vec![(win, 0)];
        let menus: Vec<El> = El::app(app.pid).elements("AXChildren").into_iter().filter(|c| c.string("AXRole").as_deref() == Some("AXMenu")).collect();
        if !menus.is_empty() {
            out.push_str("A menu is open; its items come first:\n");
            stack.extend(menus.into_iter().map(|m| (m, 0)));
        }
        let (mut visited, mut listed) = (0usize, 0usize);
        let mut last_label = String::new();
        let mut truncated = false;
        while let Some((el, depth)) = stack.pop() {
            visited += 1;
            if visited > MAX_VISIT || listed >= MAX_LIST || out.len() > MAX_CHARS {
                truncated = true;
                break;
            }
            let v = el.attrs(&ATTRS);
            let s = |i: usize| match &v[i] {
                V::Str(x) => x.clone(),
                V::Num(n) => format!("{n}"),
                _ => String::new(),
            };
            let (role, subrole) = (s(0), s(1));
            if let V::Els(children) = &v[6] {
                if depth < 60 {
                    for c in children.iter().rev() {
                        stack.push((c.clone(), depth + 1));
                    }
                }
            }
            let Some(kind) = role_name(&role, &subrole) else { continue };
            let secure = kind == "password field";
            let mut label = [s(2), s(3)].into_iter().find(|x| !x.trim().is_empty()).unwrap_or_default();
            let value = if secure { String::new() } else { s(4) };
            if label.is_empty() && matches!(kind, "text" | "heading" | "cell") {
                label = value.clone();
            }
            if label.is_empty() && matches!(kind, "link" | "button" | "row" | "cell") {
                label = child_text(&el);
            }
            let label = one_line(&label, 160);
            if kind == "text" && (label.is_empty() || label == last_label) {
                continue;
            }
            if matches!(kind, "image") && label.is_empty() {
                continue;
            }
            let Some(id) = self.register(el, app.pid, format!("{kind} {label}")) else {
                truncated = true;
                break;
            };
            listed += 1;
            let mut line = format!("[{id}] {kind}");
            if !label.is_empty() {
                line.push_str(&format!(" “{label}”"));
            }
            let is_field = matches!(kind, "text field" | "search field" | "text area" | "combo box");
            if is_field {
                let val = one_line(&value, 200);
                if !val.is_empty() && val != label {
                    line.push_str(&format!(" = “{val}”"));
                }
                let ph = one_line(&s(5), 60);
                if val.is_empty() && !ph.is_empty() {
                    line.push_str(&format!(" (placeholder “{ph}”)"));
                }
            }
            if matches!(kind, "checkbox" | "radio") {
                line.push_str(if s(4) == "1" { " [on]" } else { " [off]" });
            }
            if matches!(v[7], V::Bool(false)) {
                line.push_str(" (disabled)");
            }
            if matches!(v[8], V::Bool(true)) {
                line.push_str(" (focused)");
            }
            if secure {
                line.push_str(" (never typed into)");
            }
            last_label = label;
            out.push_str(&line);
            out.push('\n');
        }
        if truncated {
            out.push_str("… (more not shown: scroll, or use a screenshot)\n");
        }
        if listed == 0 {
            out.push_str("(no readable controls: try screenshot)\n");
        }
        Ok(out)
    }

    fn entry(&self, id: u32) -> Result<(El, i32, String), String> {
        let e = self.entries.lock();
        let x = e.get((id as usize).wrapping_sub(1)).ok_or_else(|| format!("there is no control [{id}]; read the window first"))?;
        Ok((x.el.clone(), x.pid, x.label.clone()))
    }

    /// The app and control a numbered element belongs to (for the gate), checked live.
    pub fn target_of(&self, id: u32, need: Need) -> Result<Target, String> {
        let (el, pid, label) = self.entry(id)?;
        if el.pid() != Some(pid) {
            return Err("that control is gone; read the window again".into());
        }
        let app = app_of_pid(pid).ok_or("that app has quit")?;
        Ok(Target { bundle: app.bundle, name: app.name, pid, need, label: Some(label), unknown_spot: false, frame: window_frame(pid) })
    }


    pub fn click(&self, id: u32, kind: ClickKind) -> Result<String, String> {
        trusted()?;
        let (el, pid, label) = self.entry(id)?;
        let actions = el.actions();
        let done = |verb: &str| Ok(format!("{verb} {label}"));
        match kind {
            ClickKind::Left if actions.iter().any(|a| a == "AXPress") => {
                el.perform("AXPress")?;
                return done("Clicked");
            }
            ClickKind::Right if el.frame().is_none() && actions.iter().any(|a| a == "AXShowMenu") => {
                match el.perform("AXShowMenu") {
                    Err(e) if !e.contains("in time") => return Err(e),
                    _ => return Ok(format!("Opened the menu of {label}; read_window lists its items")),
                }
            }
            _ => {}
        }
        let (x, y, w, h) = el.frame().ok_or("that control has no position")?;
        let app = app_of_pid(pid).ok_or("that app has quit")?;
        activate(&app)?;
        mouse(&app, x + w / 2.0, y + h / 2.0, kind)?;
        if kind == ClickKind::Right {
            return Ok(format!("Right-clicked {label}; read_window lists the menu's items"));
        }
        done(past(kind))
    }

    /// Drags one numbered control onto another (in the same app or another one), as a person would:
    /// press, move in small steps, release. Both spots must really show those apps.
    pub fn drag(&self, from: u32, to: u32) -> Result<String, String> {
        trusted()?;
        let (a, apid, alabel) = self.entry(from)?;
        let (b, bpid, blabel) = self.entry(to)?;
        let (ax_, ay, aw, ah) = a.frame().ok_or("the control to drag has no position")?;
        let (bx, by, bw, bh) = b.frame().ok_or("the drop target has no position")?;
        let src = app_of_pid(apid).ok_or("that app has quit")?;
        activate(&src)?;
        let (p, q) = ((ax_ + aw / 2.0, ay + ah / 2.0), (bx + bw / 2.0, by + bh / 2.0));
        if ax::element_at(p.0, p.1).and_then(|e| e.pid()) != Some(apid) {
            return Err(format!("something covers {alabel}; not dragging"));
        }
        if ax::element_at(q.0, q.1).and_then(|e| e.pid()) != Some(bpid) {
            return Err(format!("{blabel} is covered by another window; put the two windows side by side and try again"));
        }
        mouse_drag(p, q)?;
        Ok(format!("Dragged {alabel} onto {blabel}"))
    }

    /// Whether the app's front window shows `needle` (any title, value or description), case-insensitive.
    pub fn window_has(&self, app: &RunningApp, needle: &str) -> Result<bool, String> {
        trusted()?;
        self.enable_web(app.pid);
        let win = main_window(app.pid).ok_or_else(|| format!("{} has no window", app.name))?;
        let needle = needle.to_lowercase();
        let mut stack = vec![(win, 0u16)];
        let mut visited = 0;
        while let Some((el, depth)) = stack.pop() {
            visited += 1;
            if visited > MAX_VISIT {
                break;
            }
            let v = el.attrs(&["AXTitle", "AXValue", "AXDescription", "AXChildren"]);
            for x in &v[..3] {
                if let V::Str(t) = x {
                    if t.to_lowercase().contains(&needle) {
                        return Ok(true);
                    }
                }
            }
            if let V::Els(children) = &v[3] {
                if depth < 60 {
                    stack.extend(children.iter().map(|c| (c.clone(), depth + 1)));
                }
            }
        }
        Ok(false)
    }

    pub fn type_text(&self, id: u32, text: &str, submit: bool) -> Result<String, String> {
        trusted()?;
        let (el, pid, label) = self.entry(id)?;
        if label.starts_with("password field") || matches!(el.attr("AXSubrole"), V::Str(s) if s == "AXSecureTextField") {
            return Err("that is a password field: the assistant never types passwords".into());
        }
        let app = app_of_pid(pid).ok_or("that app has quit")?;
        activate(&app)?;
        let _ = el.set_bool("AXFocused", true);
        std::thread::sleep(Duration::from_millis(150));
        if crate::permissions::secure_input_active() {
            return Err("a password field has the keyboard: not typing".into());
        }
        if ax::focused_pid() != Some(pid) {
            return Err(format!("{} lost the keyboard focus; not typing", app.name));
        }
        type_into(pid, text)?;
        if submit {
            key(pid, 36, Default::default())?;
        }
        Ok(format!("Typed {} characters into {label}{}", text.chars().count(), if submit { " and pressed Return" } else { "" }))
    }

    pub fn press_keys(&self, app: &RunningApp, mods: &[&str], key_name: &str) -> Result<String, String> {
        trusted()?;
        let code = super::tools::keycode(key_name).ok_or_else(|| format!("unknown key \"{key_name}\""))?;
        activate(app)?;
        if crate::permissions::secure_input_active() {
            return Err("a password field has the keyboard: not pressing keys".into());
        }
        key(app.pid, code, flags(mods))?;
        Ok(format!("Pressed {} in {}", combo_text(mods, key_name), app.name))
    }

    pub fn choose_menu(&self, app: &RunningApp, path: &[String]) -> Result<String, String> {
        trusted()?;
        let bar = El::app(app.pid).element("AXMenuBar").ok_or_else(|| format!("{} has no menu bar", app.name))?;
        let norm = |s: &str| s.trim().trim_end_matches('…').trim_end_matches("...").trim().to_lowercase();
        let mut items = bar.elements("AXChildren");
        let mut found: Option<El> = None;
        for (i, want) in path.iter().enumerate() {
            let hit = items.iter().find(|e| e.string("AXTitle").is_some_and(|t| norm(&t) == norm(want))).cloned();
            let Some(hit) = hit else {
                let names: Vec<String> = items.iter().filter_map(|e| e.string("AXTitle")).filter(|t| !t.is_empty()).take(30).collect();
                return Err(format!("no menu item \"{want}\". Available: {}", names.join(", ")));
            };
            if i + 1 < path.len() {
                items = hit.elements("AXChildren").into_iter().next().map(|m| m.elements("AXChildren")).unwrap_or_default();
            }
            found = Some(hit);
        }
        let item = found.ok_or("empty menu path")?;
        if matches!(item.attr("AXEnabled"), V::Bool(false)) {
            return Err(format!("\"{}\" is disabled right now", path.join(" › ")));
        }
        item.perform("AXPress")?;
        Ok(format!("Chose {} in {}", path.join(" › "), app.name))
    }

    pub fn scroll(&self, app: &RunningApp, down: bool, amount: u8) -> Result<String, String> {
        let (x, y, w, h) = window_frame(app.pid).ok_or("no window to scroll")?;
        activate(app)?;
        let lines = i32::from(amount) * 3 * if down { -1 } else { 1 };
        scroll_at(app.pid, x + w / 2.0, y + h / 2.0, lines)?;
        Ok(format!("Scrolled {} in {}", if down { "down" } else { "up" }, app.name))
    }


    pub fn screenshot(&self, app: &RunningApp, dir: &std::path::Path) -> Result<(String, Vec<u8>), String> {
        if !screen_capture_allowed() {
            return Err("Screen Recording is off for Xpu Command. Allow it in System Settings › Privacy & Security › Screen & System Audio Recording, or use read_window.".into());
        }
        let (_, wid, frame, _) = screen_windows().into_iter().find(|w| w.0 == app.pid).ok_or_else(|| format!("{} has no window on screen", app.name))?;
        let file = dir.join(format!("shot-{}.jpg", uuid::Uuid::new_v4().simple()));
        struct Rm(PathBuf);
        impl Drop for Rm {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _rm = Rm(file.clone());
        let f = file.as_os_str();
        super::exec::run_program("/usr/sbin/screencapture", &["-x".as_ref(), "-o".as_ref(), "-t".as_ref(), "jpg".as_ref(), "-l".as_ref(), wid.to_string().as_ref(), f], Duration::from_secs(15))?;
        super::exec::run_program("/usr/bin/sips", &["-Z".as_ref(), "1400".as_ref(), f], Duration::from_secs(15))?;
        let dims = super::exec::run_program("/usr/bin/sips", &["-g".as_ref(), "pixelWidth".as_ref(), "-g".as_ref(), "pixelHeight".as_ref(), f], Duration::from_secs(15))?;
        let num = |k: &str| dims.lines().find(|l| l.trim().starts_with(k)).and_then(|l| l.split(':').nth(1)).and_then(|v| v.trim().parse::<f64>().ok());
        let (wpx, hpx) = (num("pixelWidth").ok_or("bad screenshot")?, num("pixelHeight").ok_or("bad screenshot")?);
        let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("screenshot too large".into());
        }
        self.shots.lock().insert(app.pid, Shot { frame, width_px: wpx, height_px: hpx });
        Ok((format!("Screenshot of {}'s window, {wpx}×{hpx} pixels. click_at takes x, y in these pixels.", app.name), bytes))
    }

    /// Screen point for a pixel in the app's last screenshot.
    fn point_from_shot(&self, pid: i32, x: f64, y: f64) -> Result<(f64, f64), String> {
        let s = *self.shots.lock().get(&pid).ok_or("take a screenshot of this app first")?;
        if !(0.0..=s.width_px).contains(&x) || !(0.0..=s.height_px).contains(&y) {
            return Err("that point is outside the screenshot".into());
        }
        Ok((s.frame.0 + x * s.frame.2 / s.width_px, s.frame.1 + y * s.frame.3 / s.height_px))
    }

    /// For the gate: what is at that pixel (label if identifiable).
    pub fn target_at(&self, app: &RunningApp, x: f64, y: f64) -> Result<Target, String> {
        let (px, py) = self.point_from_shot(app.pid, x, y)?;
        let el = ax::element_at(px, py);
        if el.as_ref().and_then(El::pid) != Some(app.pid) {
            return Err(format!("something else is on top of {} at that point", app.name));
        }
        let label = el.and_then(|e| {
            let v = e.attrs(&["AXRole", "AXSubrole", "AXTitle", "AXDescription"]);
            let s = |i: usize| match &v[i] { V::Str(x) => x.clone(), _ => String::new() };
            let kind = role_name(&s(0), &s(1))?;
            let l = [s(2), s(3)].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
            (!l.is_empty()).then(|| format!("{kind} {}", one_line(&l, 120)))
        });
        Ok(Target { bundle: app.bundle.clone(), name: app.name.clone(), pid: app.pid, need: Need::Control, unknown_spot: label.is_none(), label, frame: window_frame(app.pid) })
    }

    pub fn click_at(&self, app: &RunningApp, x: f64, y: f64, kind: ClickKind) -> Result<String, String> {
        let (px, py) = self.point_from_shot(app.pid, x, y)?;
        activate(app)?;
        mouse(app, px, py, kind)?;
        Ok(format!("{} ({x:.0}, {y:.0}) in {}", past(kind), app.name))
    }
}

/// Reading and controlling other apps needs the Accessibility permission.
fn trusted() -> Result<(), String> {
    if crate::permissions::accessibility_trusted(false) {
        Ok(())
    } else {
        Err("Accessibility is off for Xpu Command, so it can't read or control other apps. Turn it on in System Settings › Privacy & Security › Accessibility.".into())
    }
}

fn child_text(el: &El) -> String {
    let mut parts = Vec::new();
    for c in el.elements("AXChildren").into_iter().take(6) {
        let v = c.attrs(&["AXRole", "AXValue", "AXTitle"]);
        if let (V::Str(r), V::Str(t)) = (&v[0], &v[1]) {
            if r == "AXStaticText" && !t.trim().is_empty() {
                parts.push(t.clone());
            }
        } else if let V::Str(t) = &v[2] {
            if !t.trim().is_empty() {
                parts.push(t.clone());
            }
        }
    }
    parts.join(" ")
}


use super::tools::ClickKind;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField, ScrollEventUnit};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

extern "C" {
    fn CGEventSetLocation(event: *mut c_void, location: CGPoint);
}

pub fn screen_capture_allowed() -> bool {
    crate::permissions::screen_recording()
}

fn source() -> Result<CGEventSource, String> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| "could not create input events".to_string())
}

fn key(pid: i32, code: u16, flags: CGEventFlags) -> Result<(), String> {
    for down in [true, false] {
        let e = CGEvent::new_keyboard_event(source()?, code, down).map_err(|_| "key event failed")?;
        e.set_flags(flags);
        e.post_to_pid(pid);
        std::thread::sleep(Duration::from_millis(8));
    }
    Ok(())
}

/// Types text into the process: Unicode strings in small chunks (layout-independent), Return for
/// line breaks, Tab for tabs. Posted to that process only.
fn type_into(pid: i32, text: &str) -> Result<(), String> {
    let mut chunk = String::new();
    let flush = |chunk: &mut String| -> Result<(), String> {
        if chunk.is_empty() {
            return Ok(());
        }
        for down in [true, false] {
            let e = CGEvent::new_keyboard_event(source()?, 0, down).map_err(|_| "key event failed")?;
            e.set_string(chunk);
            e.post_to_pid(pid);
        }
        std::thread::sleep(Duration::from_millis(12));
        chunk.clear();
        Ok(())
    };
    for c in text.chars() {
        match c {
            '\n' => {
                flush(&mut chunk)?;
                key(pid, 36, CGEventFlags::CGEventFlagNull)?;
            }
            '\t' => {
                flush(&mut chunk)?;
                key(pid, 48, CGEventFlags::CGEventFlagNull)?;
            }
            c if c.is_control() => {}
            c => {
                chunk.push(c);
                if chunk.encode_utf16().count() >= 16 {
                    flush(&mut chunk)?;
                }
            }
        }
    }
    flush(&mut chunk)
}

fn past(kind: ClickKind) -> &'static str {
    match kind {
        ClickKind::Left => "Clicked",
        ClickKind::Double => "Double-clicked",
        ClickKind::Right => "Right-clicked",
        ClickKind::Hover => "Pointed at",
    }
}

fn post_mouse(ty: CGEventType, p: CGPoint, button: CGMouseButton, clicks: i64) -> Result<(), String> {
    let e = CGEvent::new_mouse_event(source()?, ty, p, button).map_err(|_| "mouse event failed")?;
    if clicks > 0 {
        e.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, clicks);
    }
    e.post(CGEventTapLocation::HID);
    std::thread::sleep(Duration::from_millis(30));
    Ok(())
}

/// A click (or double, right click, or hover) at a screen point, only if that point belongs to the app.
fn mouse(app: &RunningApp, x: f64, y: f64, kind: ClickKind) -> Result<(), String> {
    if ax::element_at(x, y).and_then(|e| e.pid()) != Some(app.pid) {
        return Err(format!("something else is on top of {} there; not clicking", app.name));
    }
    let p = CGPoint::new(x, y);
    match kind {
        ClickKind::Hover => post_mouse(CGEventType::MouseMoved, p, CGMouseButton::Left, 0),
        ClickKind::Right => {
            post_mouse(CGEventType::RightMouseDown, p, CGMouseButton::Right, 1)?;
            post_mouse(CGEventType::RightMouseUp, p, CGMouseButton::Right, 1)
        }
        ClickKind::Left | ClickKind::Double => {
            let n = if kind == ClickKind::Double { 2 } else { 1 };
            for clicks in 1..=n {
                post_mouse(CGEventType::LeftMouseDown, p, CGMouseButton::Left, clicks)?;
                post_mouse(CGEventType::LeftMouseUp, p, CGMouseButton::Left, clicks)?;
            }
            Ok(())
        }
    }
}

fn mouse_drag(from: (f64, f64), to: (f64, f64)) -> Result<(), String> {
    let at = |t: f64| CGPoint::new(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
    post_mouse(CGEventType::MouseMoved, at(0.0), CGMouseButton::Left, 0)?;
    post_mouse(CGEventType::LeftMouseDown, at(0.0), CGMouseButton::Left, 1)?;
    std::thread::sleep(Duration::from_millis(120));
    const STEPS: u32 = 16;
    for i in 1..=STEPS {
        post_mouse(CGEventType::LeftMouseDragged, at(f64::from(i) / f64::from(STEPS)), CGMouseButton::Left, 0)?;
    }
    std::thread::sleep(Duration::from_millis(150));
    post_mouse(CGEventType::LeftMouseUp, at(1.0), CGMouseButton::Left, 1)
}

/// The front app; and, only for apps the user lets the assistant see (See or Control), its window
/// title, the page address and the selected text. Nothing from password fields or excluded apps.
pub fn context(policy: &super::policy::Policy) -> Option<super::Context> {
    use super::policy::{class, Access, Class};
    let pid = front_pid()?;
    if pid == std::process::id() as i32 {
        return None;
    }
    let app = app_of_pid(pid)?;
    let mut c = super::Context { bundle: app.bundle.clone(), app: app.name.clone(), window: None, url: None, selection: None };
    if class(&app.bundle) == Class::Excluded || !matches!(policy.level(&app.bundle), Access::See | Access::Control) || !crate::permissions::accessibility_trusted(false) {
        return Some(c);
    }
    ax::set_global_timeout(1.0);
    c.window = window_title(pid).map(|t| one_line(&t, 160));
    let secure = crate::permissions::secure_input_active();
    if !secure {
        c.selection = El::app(pid)
            .element("AXFocusedUIElement")
            .filter(|el| !matches!(el.attr("AXSubrole"), V::Str(s) if s == "AXSecureTextField"))
            .and_then(|el| el.string("AXSelectedText"))
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
    }
    if super::web::is_browser(&app.bundle) {
        match super::web::selection(&app) {
            Some((url, text)) => {
                c.url = Some(url).filter(|u| u.starts_with("http"));
                if c.selection.is_none() && !secure && !text.trim().is_empty() {
                    c.selection = Some(text.trim().to_string());
                }
            }
            None => c.url = super::web::url(&app),
        }
    }
    c.selection = c.selection.map(|t| t.chars().filter(|ch| !crate::sanitize::is_dangerous(*ch) || matches!(ch, '\n' | '\t')).take(4000).collect());
    Some(c)
}

fn scroll_at(pid: i32, x: f64, y: f64, lines: i32) -> Result<(), String> {
    let e = CGEvent::new_scroll_event(source()?, ScrollEventUnit::LINE, 1, lines, 0, 0).map_err(|_| "scroll event failed")?;
    unsafe { CGEventSetLocation(foreign_types::ForeignType::as_ptr(&e) as *mut c_void, CGPoint::new(x, y)) };
    e.post_to_pid(pid);
    Ok(())
}

pub fn flags(mods: &[&str]) -> CGEventFlags {
    let mut f = CGEventFlags::CGEventFlagNull;
    for m in mods {
        f |= match *m {
            "cmd" => CGEventFlags::CGEventFlagCommand,
            "shift" => CGEventFlags::CGEventFlagShift,
            "option" => CGEventFlags::CGEventFlagAlternate,
            "ctrl" => CGEventFlags::CGEventFlagControl,
            _ => CGEventFlags::CGEventFlagNull,
        };
    }
    f
}

fn combo_text(mods: &[&str], key: &str) -> String {
    let mut s: String = mods.iter().map(|m| match *m { "cmd" => "⌘", "shift" => "⇧", "option" => "⌥", "ctrl" => "⌃", _ => "" }).collect();
    s.push_str(&key.to_uppercase());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(combo_text(&["cmd", "shift"], "t"), "⌘⇧T");
    }

    #[test]
    fn one_line_cleans() {
        assert_eq!(one_line("a\n  b\u{202E}c", 10), "a bc");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }

    #[test]
    fn live_system_queries_work() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let _ = app_of_pid(std::process::id() as i32);
        let finder = std::path::Path::new("/System/Library/CoreServices/Finder.app");
        assert_eq!(bundle_info(finder).map(|b| b.0).as_deref(), Some("com.apple.finder"));
    }
}
