//! Seeing and controlling other apps' windows on Linux, for one request.
//!
//! * Reading uses the accessibility tree (AT-SPI): real controls with names ("button Compose"),
//!   numbered so the model can refer to them. Numbers are valid only within this request.
//! * Acting prefers accessibility actions (click, press, activate) and text editing, which work
//!   without moving the pointer or the keyboard focus, on X11 and Wayland alike.
//! * Pointer fallbacks (X11 only: Wayland doesn't let apps move the pointer) first check that the
//!   spot really belongs to the target app. Keys go only to the app in front.
//! * Password fields are never typed into.
use super::policy::{Need, Target};
use super::tools::ClickKind;
use crate::linux::atspi::{self, role, state, El};
use crate::linux::{keys, x11, Session as Display};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
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
}

const MAX_ENTRIES: usize = 6000;
const MAX_VISIT: usize = 3000;
const MAX_LIST: usize = 400;
const MAX_CHARS: usize = 16_000;

/// Desktop shell processes that register with AT-SPI but aren't apps the user works in.
const SHELLS: &[&str] = &["gnome-shell", "plasmashell", "kwin_wayland", "kwin_x11", "mutter-x11-frames", "xdg-desktop-portal-gnome", "xdg-desktop-portal-kde", "xdg-desktop-portal-gtk", "ibus-ui-gtk3", "ibus-extension-gtk3", "gsd-media-keys"];

/// The app id and name of a process, from the desktop's own records (see `linux::desktop`).
pub fn app_of_pid(pid: i32) -> Option<RunningApp> {
    let (bundle, name) = crate::linux::desktop::app_of_pid(pid, &crate::linux::desktop::cached())?;
    Some(RunningApp { pid, bundle, name })
}

/// (app id, display name) of an installed app, by its desktop file.
pub fn bundle_info(app: &Path) -> Option<(String, String)> {
    crate::linux::desktop::cached().iter().find(|e| e.path == app).map(|e| (e.id.clone(), e.name.clone()))
}

fn is_window_role(r: Option<u32>) -> bool {
    matches!(r, Some(role::FRAME | role::WINDOW | role::DIALOG))
}

/// The accessibility objects of a process's windows: (window, active, showing).
fn windows_of(pid: i32) -> Vec<(El, bool, bool)> {
    let mut out = Vec::new();
    for app in atspi::applications() {
        if app.pid() != Some(pid) {
            continue;
        }
        for w in app.children().into_iter().take(40) {
            if !is_window_role(w.role()) {
                continue;
            }
            let s = w.states();
            if s.has(state::DEFUNCT) {
                continue;
            }
            out.push((w, s.has(state::ACTIVE), s.has(state::SHOWING) || s.has(state::VISIBLE)));
        }
    }
    out
}

/// The app's main window: the active one, else one on screen, else the first.
fn main_window(pid: i32) -> Option<El> {
    let w = windows_of(pid);
    w.iter().find(|x| x.1).or_else(|| w.iter().find(|x| x.2)).or(w.first()).map(|x| x.0.clone())
}

/// The app in front: the X11 active window's process, or the AT-SPI active window's.
pub fn front_pid() -> Option<i32> {
    match crate::linux::session() {
        Display::X11 => x11::active_pid().or_else(|| atspi::active_window().and_then(|w| w.pid())),
        Display::Wayland => atspi::active_window().and_then(|w| w.pid()),
    }
}

/// Apps with a window on screen, front first (live).
pub fn apps_on_screen() -> Vec<RunningApp> {
    let mut pids: Vec<i32> = Vec::new();
    match crate::linux::session() {
        Display::X11 => {
            for (_, pid, _, _) in x11::windows() {
                if !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
        }
        Display::Wayland => {
            let mut active = Vec::new();
            for app in atspi::applications() {
                let Some(pid) = app.pid() else { continue };
                if pids.contains(&pid) || active.contains(&pid) {
                    continue;
                }
                let mut shown = false;
                for w in app.children().into_iter().take(40) {
                    if !is_window_role(w.role()) {
                        continue;
                    }
                    let s = w.states();
                    if s.has(state::ACTIVE) {
                        active.push(pid);
                        shown = false;
                        break;
                    }
                    shown |= s.has(state::SHOWING) || s.has(state::VISIBLE);
                }
                if shown {
                    pids.push(pid);
                }
            }
            active.extend(pids);
            pids = active;
        }
    }
    pids.into_iter().filter_map(app_of_pid).filter(|a| !SHELLS.contains(&a.bundle.as_str())).collect()
}

/// A running app with a window on screen, by spoken name.
pub fn find_app(spoken: &str) -> Result<RunningApp, String> {
    let on_screen = apps_on_screen();
    match super::apps::pick(&on_screen, |a| &a.name, spoken) {
        Ok(hit) => Ok(hit.clone()),
        Err(Some(e)) => Err(e),
        Err(None) => open_window_of(spoken).ok_or_else(|| format!("\"{spoken}\" has no window open. Open it with open_app first.")),
    }
}

/// An installed app with no window on screen: starts it and waits for its window. Never for apps
/// the assistant may not touch or terminals.
fn open_window_of(spoken: &str) -> Option<RunningApp> {
    let installed = super::apps::installed();
    let app = super::apps::find(&installed, spoken).ok()?;
    let (bundle, _) = bundle_info(&app.path)?;
    if super::policy::class(&bundle) != super::policy::Class::Normal {
        return None;
    }
    let entry = crate::linux::desktop::cached().iter().find(|e| e.path == app.path).cloned()?;
    crate::linux::desktop::launch(&entry, &[]).ok()?;
    let found = wait_for_window(&bundle, Duration::from_secs(8))?;
    std::thread::sleep(Duration::from_millis(300));
    Some(found)
}

pub fn window_title(pid: i32) -> Option<String> {
    if crate::linux::session() == Display::X11 {
        if let Some(t) = x11::windows().into_iter().find(|w| w.1 == pid).map(|w| w.3).filter(|t| !t.is_empty()) {
            return Some(t);
        }
    }
    main_window(pid).and_then(|w| w.name()).filter(|t| !t.is_empty())
}

/// Where the app's front window is, in screen pixels. On Wayland usually unknown.
pub fn window_frame(pid: i32) -> Option<(f64, f64, f64, f64)> {
    match crate::linux::session() {
        Display::X11 => x11::windows().into_iter().find(|w| w.1 == pid).map(|w| w.2),
        Display::Wayland => main_window(pid).and_then(|w| w.frame()).filter(|f| f.0 > 0.0 || f.1 > 0.0),
    }
}

/// Whether the app (any of its processes) is in front.
pub fn is_front(bundle: &str, pid: i32) -> bool {
    match front_pid() {
        Some(p) if p == pid => true,
        Some(p) => app_of_pid(p).is_some_and(|a| a.bundle == bundle),
        None => false,
    }
}

fn wait_front(bundle: &str, pid: i32, wait: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < wait {
        if is_front(bundle, pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Brings the app to the front and waits until it really is: the window manager on X11; on
/// Wayland the app is asked to present its window (the compositor decides).
pub fn activate(app: &RunningApp) -> Result<(), String> {
    if is_front(&app.bundle, app.pid) {
        return Ok(());
    }
    if crate::linux::session() == Display::X11 {
        if let Some((w, _, _, _)) = x11::windows().into_iter().find(|w| w.1 == app.pid) {
            x11::activate(w)?;
            if wait_front(&app.bundle, app.pid, Duration::from_millis(1500)) {
                std::thread::sleep(Duration::from_millis(100));
                return Ok(());
            }
        }
    }
    if let Some(w) = main_window(app.pid) {
        let _ = w.grab_focus();
        if wait_front(&app.bundle, app.pid, Duration::from_millis(1200)) {
            std::thread::sleep(Duration::from_millis(100));
            return Ok(());
        }
    }
    Err(if crate::linux::session() == Display::Wayland {
        format!("{} isn't in front, and on Wayland only the user can bring it forward: ask them to click its window, then try again", app.name)
    } else {
        format!("{} didn't come to the front", app.name)
    })
}

/// The running app with this id that has a window on screen, waiting up to `wait` for one.
pub fn wait_for_window(bundle: &str, wait: Duration) -> Option<RunningApp> {
    let start = Instant::now();
    loop {
        if let Some(a) = apps_on_screen().into_iter().find(|a| a.bundle == bundle) {
            return Some(a);
        }
        if start.elapsed() >= wait {
            return None;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// What a control is, in words, from its AT-SPI role (and, for text, whether it is editable).
fn role_name(r: u32, s: atspi::States) -> Option<&'static str> {
    Some(match r {
        role::PASSWORD_TEXT => "password field",
        role::ENTRY => "text field",
        role::TEXT if s.has(state::EDITABLE) => {
            if s.has(17) {
                "text area"
            } else {
                "text field"
            }
        }
        role::TEXT | role::LABEL | role::STATIC | role::PARAGRAPH => "text",
        role::PAGE_TAB => "tab",
        role::PUSH_BUTTON | role::TOGGLE_BUTTON | role::PUSH_BUTTON_MENU => "button",
        role::LINK => "link",
        role::COMBO_BOX => "combo box",
        role::CHECK_BOX | role::CHECK_MENU_ITEM => "checkbox",
        role::RADIO_BUTTON | role::RADIO_MENU_ITEM => "radio",
        role::MENU => "menu",
        role::MENU_ITEM => "menu item",
        role::SLIDER => "slider",
        role::SPIN_BUTTON => "stepper",
        role::HEADING => "heading",
        role::IMAGE => "image",
        role::TABLE_CELL => "cell",
        role::TABLE_ROW | role::LIST_ITEM | role::TREE_ITEM => "row",
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

/// The text of a few children (for buttons and links without a name of their own).
fn child_text(el: &El) -> String {
    let mut parts = Vec::new();
    for c in el.children().into_iter().take(6) {
        if let Some(t) = c.name().filter(|t| !t.trim().is_empty()).or_else(|| c.text().filter(|t| !t.trim().is_empty())) {
            parts.push(t);
        }
    }
    parts.join(" ")
}

/// Reading and controlling other apps needs the accessibility bus.
fn trusted() -> Result<(), String> {
    if atspi::available() {
        Ok(())
    } else {
        Err("The accessibility bus (AT-SPI) isn't running, so Xpu Command can't read or control other apps. Turn on assistive technologies in the desktop's accessibility settings (or install at-spi2-core) and sign in again.".into())
    }
}

impl Session {
    fn register(&self, el: El, pid: i32, label: String) -> Option<usize> {
        let mut e = self.entries.lock();
        if e.len() >= MAX_ENTRIES {
            return None;
        }
        e.push(Entry { el, pid, label });
        Some(e.len())
    }

    /// The app's front window as numbered lines: "[12] button “Compose”".
    pub fn read_window(&self, app: &RunningApp) -> Result<String, String> {
        trusted()?;
        let wins = windows_of(app.pid);
        let win = wins.iter().find(|x| x.1).or_else(|| wins.iter().find(|x| x.2)).or(wins.first()).map(|x| x.0.clone()).ok_or_else(|| format!("{} has no window to read (if it is a Chromium or Electron app, start it with --force-renderer-accessibility)", app.name))?;
        let title = win.name().unwrap_or_default();
        let mut out = format!("{} — window “{}”\n", app.name, one_line(&title, 120));
        let mut stack: Vec<(El, u16)> = vec![(win.clone(), 0)];
        // Open popup menus are windows of their own: their items come first.
        let menus: Vec<El> = wins.iter().filter(|(w, _, showing)| *showing && *w != win && w.children().iter().any(|c| matches!(c.role(), Some(role::MENU | role::POPUP_MENU)))).map(|(w, _, _)| w.clone()).collect();
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
            let Some(r) = el.role() else { continue };
            let s = el.states();
            // Hidden parts (background tabs, closed panes) are skipped with their contents.
            if depth > 0 && !s.has(state::SHOWING) && !s.has(state::VISIBLE) {
                continue;
            }
            if depth < 60 {
                for c in el.children().into_iter().rev() {
                    stack.push((c, depth + 1));
                }
            }
            let Some(kind) = role_name(r, s) else { continue };
            let secure = kind == "password field";
            let is_field = matches!(kind, "text field" | "text area" | "combo box");
            let mut label = [el.name(), el.description()].into_iter().flatten().find(|x| !x.trim().is_empty()).unwrap_or_default();
            let value = if secure || !is_field { String::new() } else { el.text().unwrap_or_default() };
            if label.is_empty() && matches!(kind, "text" | "heading" | "cell" | "link") {
                label = el.text().unwrap_or_default();
            }
            if label.is_empty() && matches!(kind, "link" | "button" | "row" | "cell") {
                label = child_text(&el);
            }
            let label = one_line(&label, 160);
            if kind == "text" && (label.is_empty() || label == last_label) {
                continue;
            }
            if kind == "image" && label.is_empty() {
                continue;
            }
            let Some(id) = self.register(el.clone(), app.pid, format!("{kind} {label}")) else {
                truncated = true;
                break;
            };
            listed += 1;
            let mut line = format!("[{id}] {kind}");
            if !label.is_empty() {
                line.push_str(&format!(" “{label}”"));
            }
            if is_field {
                let val = one_line(&value, 200);
                if !val.is_empty() && val != label {
                    line.push_str(&format!(" = “{val}”"));
                }
                let ph = one_line(el.attributes().get("placeholder-text").map(String::as_str).unwrap_or(""), 60);
                if val.is_empty() && !ph.is_empty() {
                    line.push_str(&format!(" (placeholder “{ph}”)"));
                }
            }
            if matches!(kind, "checkbox" | "radio") {
                line.push_str(if s.has(state::CHECKED) { " [on]" } else { " [off]" });
            }
            if kind == "slider" {
                if let Some(v) = el.value() {
                    line.push_str(&format!(" = {v}"));
                }
            }
            if !s.has(state::ENABLED) && !s.has(state::SENSITIVE) {
                line.push_str(" (disabled)");
            }
            if s.has(state::FOCUSED) {
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
        if el.pid() != Some(pid) || el.states().has(state::DEFUNCT) || el.role().is_none() {
            return Err("that control is gone; read the window again".into());
        }
        let app = app_of_pid(pid).ok_or("that app has quit")?;
        Ok(Target { bundle: app.bundle, name: app.name, pid, need, label: Some(label), unknown_spot: false, frame: window_frame(pid) })
    }

    pub fn click(&self, id: u32, kind: ClickKind) -> Result<String, String> {
        trusted()?;
        let (el, pid, label) = self.entry(id)?;
        let actions = el.actions();
        let has = |a: &str| actions.iter().any(|x| x == a);
        let first_of = |list: &[&str]| list.iter().find(|a| has(a)).map(|a| a.to_string());
        match kind {
            ClickKind::Left => {
                if let Some(a) = first_of(&["click", "press", "activate", "jump", "toggle", "open", "select"]) {
                    match el.do_action(&a) {
                        Ok(()) => return Ok(format!("Clicked {label}")),
                        Err(e) => log::info!("{a} refused for {label}: {e}; clicking instead"),
                    }
                }
            }
            ClickKind::Double => {
                if let Some(a) = first_of(&["activate", "open"]) {
                    if el.do_action(&a).is_ok() {
                        return Ok(format!("Double-clicked {label}"));
                    }
                }
            }
            ClickKind::Right => {
                if let Some(a) = first_of(&["showmenu", "menu", "popup", "show menu"]) {
                    if el.do_action(&a).is_ok() {
                        return Ok(format!("Opened the menu of {label}; read_window lists its items"));
                    }
                }
            }
            ClickKind::Hover => {}
        }
        let app = app_of_pid(pid).ok_or("that app has quit")?;
        if crate::linux::session() == Display::X11 {
            if let Some((x, y, w, h)) = el.frame().filter(|f| f.2 >= 1.0 && f.3 >= 1.0) {
                activate(&app)?;
                mouse(&app, x + w / 2.0, y + h / 2.0, kind)?;
                if kind == ClickKind::Right {
                    return Ok(format!("Right-clicked {label}; read_window lists the menu's items"));
                }
                return Ok(format!("{} {label}", past(kind)));
            }
        }
        if kind == ClickKind::Left && el.grab_focus().is_ok() {
            activate(&app)?;
            if crate::permissions::secure_input_active() {
                return Err("a password field has the keyboard: not pressing keys".into());
            }
            keys::press(keys::Key::Space, &[])?;
            return Ok(format!("Focused {label} and pressed Space"));
        }
        Err(format!("{label} can't be clicked this way. Use the menu (choose_menu) or a keyboard shortcut instead"))
    }

    /// Drags one numbered control onto another (X11 only), as a person would.
    pub fn drag(&self, from: u32, to: u32) -> Result<String, String> {
        trusted()?;
        if crate::linux::session() != Display::X11 {
            return Err("dragging needs an X11 session; on Wayland, use move_files for files or menus and shortcuts instead".into());
        }
        let (a, apid, alabel) = self.entry(from)?;
        let (b, bpid, blabel) = self.entry(to)?;
        let (ax_, ay, aw, ah) = a.frame().ok_or("the control to drag has no position")?;
        let (bx, by, bw, bh) = b.frame().ok_or("the drop target has no position")?;
        let src = app_of_pid(apid).ok_or("that app has quit")?;
        activate(&src)?;
        let (p, q) = ((ax_ + aw / 2.0, ay + ah / 2.0), (bx + bw / 2.0, by + bh / 2.0));
        if owner_at(p.0, p.1) != Some(apid) {
            return Err(format!("something covers {alabel}; not dragging"));
        }
        if owner_at(q.0, q.1) != Some(bpid) {
            return Err(format!("{blabel} is covered by another window; put the two windows side by side and try again"));
        }
        x11::move_to(p.0, p.1)?;
        x11::button(1, true)?;
        std::thread::sleep(Duration::from_millis(120));
        const STEPS: u32 = 16;
        for i in 1..=STEPS {
            let t = f64::from(i) / f64::from(STEPS);
            x11::move_to(p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t)?;
            std::thread::sleep(Duration::from_millis(15));
        }
        std::thread::sleep(Duration::from_millis(150));
        x11::button(1, false)?;
        Ok(format!("Dragged {alabel} onto {blabel}"))
    }

    /// Whether the app's front window shows `needle` (any name, description or text).
    pub fn window_has(&self, app: &RunningApp, needle: &str) -> Result<bool, String> {
        trusted()?;
        let win = main_window(app.pid).ok_or_else(|| format!("{} has no window", app.name))?;
        let needle = needle.to_lowercase();
        let found = atspi::find(&win, MAX_VISIT, |e| {
            [e.name(), e.description()].into_iter().flatten().any(|t| t.to_lowercase().contains(&needle))
                || (matches!(e.role(), Some(role::TEXT | role::ENTRY | role::PARAGRAPH | role::STATIC | role::LABEL | role::HEADING)) && e.text().is_some_and(|t| t.to_lowercase().contains(&needle)))
        });
        Ok(found.is_some())
    }

    pub fn type_text(&self, id: u32, text: &str, submit: bool) -> Result<String, String> {
        trusted()?;
        let (el, pid, label) = self.entry(id)?;
        if label.starts_with("password field") || el.is_password() {
            return Err("that is a password field: the assistant never types passwords".into());
        }
        let app = app_of_pid(pid).ok_or("that app has quit")?;
        let multi = el.states().has(17);
        let text: String = if multi { text.to_string() } else { text.replace('\n', " ") };
        let _ = el.grab_focus();
        std::thread::sleep(Duration::from_millis(100));
        // X11: typed like a person (web apps see real key events); Wayland: written into the
        // field through accessibility, since keystrokes may not be possible there.
        let typed = match crate::linux::session() {
            Display::X11 => keyboard_into(&app, pid, &text).or_else(|e| insert_into(&el, &text).map_err(|_| e)),
            Display::Wayland => insert_into(&el, &text).or_else(|e| keyboard_into(&app, pid, &text).map_err(|k| format!("{e}; {k}"))),
        };
        typed?;
        if submit && el.do_action("activate").is_err() {
            activate(&app)?;
            keys::press(keys::Key::Return, &[])?;
        }
        Ok(format!("Typed {} characters into {label}{}", text.chars().count(), if submit { " and pressed Return" } else { "" }))
    }

    pub fn press_keys(&self, app: &RunningApp, mods: &[&str], key_name: &str) -> Result<String, String> {
        trusted()?;
        let key = keys::named(key_name).ok_or_else(|| format!("unknown key \"{key_name}\""))?;
        let m: Vec<keys::Mod> = mods.iter().filter_map(|m| keys::modifier(m)).collect();
        activate(app)?;
        if crate::permissions::secure_input_active() {
            return Err("a password field has the keyboard: not pressing keys".into());
        }
        keys::press(key, &m)?;
        Ok(format!("Pressed {} in {}", combo_text(mods, key_name), app.name))
    }

    pub fn choose_menu(&self, app: &RunningApp, path: &[String]) -> Result<String, String> {
        trusted()?;
        let win = main_window(app.pid).ok_or_else(|| format!("{} has no window", app.name))?;
        let bar = atspi::find(&win, 1500, |e| e.role() == Some(role::MENU_BAR)).ok_or_else(|| format!("{} has no menu bar; use its menu button (read_window) or a shortcut", app.name))?;
        let norm = |s: &str| s.trim().trim_end_matches('…').trim_end_matches("...").replace('_', "").trim().to_lowercase();
        let mut items = bar.children();
        let mut found: Option<El> = None;
        for (i, want) in path.iter().enumerate() {
            let hit = items.iter().find(|e| e.name().is_some_and(|t| norm(&t) == norm(want))).cloned();
            let Some(hit) = hit else {
                let names: Vec<String> = items.iter().filter_map(El::name).filter(|t| !t.is_empty()).take(30).collect();
                return Err(format!("no menu item \"{want}\". Available: {}", names.join(", ")));
            };
            if i + 1 < path.len() {
                let kids = hit.children();
                items = match kids.as_slice() {
                    [only] if matches!(only.role(), Some(role::MENU | role::POPUP_MENU)) => only.children(),
                    _ => kids,
                };
            }
            found = Some(hit);
        }
        let item = found.ok_or("empty menu path")?;
        let s = item.states();
        if !s.has(state::ENABLED) && !s.has(state::SENSITIVE) {
            return Err(format!("\"{}\" is disabled right now", path.join(" › ")));
        }
        let action = item.actions().into_iter().find(|a| matches!(a.as_str(), "click" | "activate" | "press")).ok_or("that menu item can't be chosen this way")?;
        item.do_action(&action)?;
        Ok(format!("Chose {} in {}", path.join(" › "), app.name))
    }

    pub fn scroll(&self, app: &RunningApp, down: bool, amount: u8) -> Result<String, String> {
        activate(app)?;
        match crate::linux::session() {
            Display::X11 => {
                let (x, y, w, h) = window_frame(app.pid).ok_or("no window to scroll")?;
                x11::move_to(x + w / 2.0, y + h / 2.0)?;
                for _ in 0..amount.max(1) {
                    x11::button(if down { 5 } else { 4 }, true)?;
                    x11::button(if down { 5 } else { 4 }, false)?;
                    std::thread::sleep(Duration::from_millis(12));
                }
            }
            Display::Wayland => {
                for _ in 0..amount.div_ceil(3).max(1) {
                    keys::press(if down { keys::Key::PageDown } else { keys::Key::PageUp }, &[])?;
                }
            }
        }
        Ok(format!("Scrolled {} in {}", if down { "down" } else { "up" }, app.name))
    }

    pub fn screenshot(&self, app: &RunningApp, _dir: &Path) -> Result<(String, Vec<u8>), String> {
        let frame = window_frame(app.pid);
        if crate::linux::session() == Display::X11 && frame.is_none() {
            return Err(format!("{} has no window on screen", app.name));
        }
        if crate::linux::session() == Display::X11 {
            activate(app)?;
        }
        let (jpeg, w, h, area) = crate::linux::screenshot::capture(frame)?;
        let (wpx, hpx) = (f64::from(w), f64::from(h));
        match area {
            Some(a) if crate::linux::session() == Display::X11 => {
                self.shots.lock().insert(app.pid, Shot { frame: a, width_px: wpx, height_px: hpx });
                Ok((format!("Screenshot of {}'s window, {wpx}×{hpx} pixels. click_at takes x, y in these pixels.", app.name), jpeg))
            }
            _ => Ok((format!("Screenshot of the screen, {wpx}×{hpx} pixels ({} should be in it). click_at isn't available on Wayland: use read_window and click.", app.name), jpeg)),
        }
    }

    /// Screen point for a pixel in the app's last screenshot.
    fn point_from_shot(&self, pid: i32, x: f64, y: f64) -> Result<(f64, f64), String> {
        let s = *self.shots.lock().get(&pid).ok_or("take a screenshot of this app first (click_at needs an X11 session)")?;
        if !(0.0..=s.width_px).contains(&x) || !(0.0..=s.height_px).contains(&y) {
            return Err("that point is outside the screenshot".into());
        }
        Ok((s.frame.0 + x * s.frame.2 / s.width_px, s.frame.1 + y * s.frame.3 / s.height_px))
    }

    /// For the gate: what is at that pixel (label if identifiable).
    pub fn target_at(&self, app: &RunningApp, x: f64, y: f64) -> Result<Target, String> {
        let (px, py) = self.point_from_shot(app.pid, x, y)?;
        if owner_at(px, py) != Some(app.pid) {
            return Err(format!("something else is on top of {} at that point", app.name));
        }
        let label = deepest_at(app.pid, px, py).and_then(|e| {
            let kind = role_name(e.role()?, e.states())?;
            let l = [e.name(), e.description()].into_iter().flatten().find(|x| !x.is_empty())?;
            Some(format!("{kind} {}", one_line(&l, 120)))
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

/// Writes text into a field through accessibility, at its cursor, and checks it arrived.
fn insert_into(el: &El, text: &str) -> Result<(), String> {
    if !el.interfaces().iter().any(|i| i == "org.a11y.atspi.EditableText") {
        return Err("that field can't be written into directly".into());
    }
    let before = el.text().unwrap_or_default();
    let at = el.caret().unwrap_or(before.chars().count()).min(before.chars().count());
    el.insert_text(at, text)?;
    std::thread::sleep(Duration::from_millis(60));
    let after = el.text().unwrap_or_default();
    if after == before || !after.contains(text.trim()) {
        return Err("the app ignored the text".into());
    }
    let _ = el.set_caret(at + text.chars().count());
    Ok(())
}

/// Types through the keyboard into the app in front; Return for line breaks.
fn keyboard_into(app: &RunningApp, pid: i32, text: &str) -> Result<(), String> {
    activate(app)?;
    if crate::permissions::secure_input_active() {
        return Err("a password field has the keyboard: not typing".into());
    }
    if front_pid() != Some(pid) {
        return Err(format!("{} lost the keyboard focus; not typing", app.name));
    }
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            keys::press(keys::Key::Return, &[])?;
        }
        let clean: String = line.chars().filter(|c| !c.is_control() || *c == '\t').collect();
        keys::type_text(&clean)?;
    }
    Ok(())
}

/// The process whose window is topmost at a screen point (X11).
fn owner_at(x: f64, y: f64) -> Option<i32> {
    x11::windows().into_iter().find(|(_, _, f, _)| x >= f.0 && y >= f.1 && x < f.0 + f.2 && y < f.1 + f.3).map(|w| w.1)
}

/// The deepest accessible element at a screen point, in the app's main window.
fn deepest_at(pid: i32, x: f64, y: f64) -> Option<El> {
    let mut cur = main_window(pid)?;
    for _ in 0..30 {
        match cur.element_at(x as i32, y as i32) {
            Some(next) if next != cur => cur = next,
            _ => break,
        }
    }
    Some(cur)
}

fn past(kind: ClickKind) -> &'static str {
    match kind {
        ClickKind::Left => "Clicked",
        ClickKind::Double => "Double-clicked",
        ClickKind::Right => "Right-clicked",
        ClickKind::Hover => "Pointed at",
    }
}

/// A click (or double, right click, or hover) at a screen point, only if that point shows the app.
fn mouse(app: &RunningApp, x: f64, y: f64, kind: ClickKind) -> Result<(), String> {
    if crate::linux::session() != Display::X11 {
        return Err("pointer clicks need an X11 session; use click with a number from read_window".into());
    }
    if owner_at(x, y) != Some(app.pid) {
        return Err(format!("something else is on top of {} there; not clicking", app.name));
    }
    x11::move_to(x, y)?;
    std::thread::sleep(Duration::from_millis(30));
    let (button, times) = match kind {
        ClickKind::Hover => return Ok(()),
        ClickKind::Right => (3, 1),
        ClickKind::Left => (1, 1),
        ClickKind::Double => (1, 2),
    };
    for _ in 0..times {
        x11::button(button, true)?;
        std::thread::sleep(Duration::from_millis(30));
        x11::button(button, false)?;
        std::thread::sleep(Duration::from_millis(60));
    }
    Ok(())
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
    if class(&app.bundle) == Class::Excluded || !matches!(policy.level(&app.bundle), Access::See | Access::Control) || !atspi::available() {
        return Some(c);
    }
    c.window = window_title(pid).map(|t| one_line(&t, 160));
    let focused = atspi::focused_element().filter(|e| e.pid() == Some(pid));
    if let Some(el) = focused.filter(|e| !e.is_password()) {
        if let (Some(text), Some((s, e))) = (el.text(), el.selection()) {
            let sel: String = text.chars().skip(s).take(e - s).collect();
            let sel = sel.trim().to_string();
            if !sel.is_empty() {
                c.selection = Some(sel);
            }
        }
    }
    if super::web::is_browser(&app.bundle) {
        c.url = super::web::url(&app);
    }
    c.selection = c.selection.map(|t| t.chars().filter(|ch| !crate::sanitize::is_dangerous(*ch) || matches!(ch, '\n' | '\t')).take(4000).collect());
    Some(c)
}

fn combo_text(mods: &[&str], key: &str) -> String {
    let mut parts: Vec<String> = mods.iter().map(|m| match *m { "cmd" | "ctrl" => "Ctrl", "shift" => "Shift", "option" => "Alt", other => other }.to_string()).collect();
    parts.push(key.to_uppercase());
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(combo_text(&["cmd", "shift"], "t"), "Ctrl+Shift+T");
    }

    #[test]
    fn one_line_cleans() {
        assert_eq!(one_line("a\n  b\u{202E}c", 10), "a bc");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }

    #[test]
    fn roles_in_words() {
        let none = atspi::States(0);
        let editable = atspi::States(1 << state::EDITABLE);
        assert_eq!(role_name(role::PASSWORD_TEXT, editable), Some("password field"));
        assert_eq!(role_name(role::TEXT, editable), Some("text field"));
        assert_eq!(role_name(role::TEXT, atspi::States(1 << state::EDITABLE | 1 << 17)), Some("text area"));
        assert_eq!(role_name(role::TEXT, none), Some("text"));
        assert_eq!(role_name(role::PUSH_BUTTON, none), Some("button"));
        assert_eq!(role_name(role::FRAME, none), None);
    }

    #[test]
    fn our_own_process_is_an_app() {
        assert!(app_of_pid(std::process::id() as i32).is_some());
    }
}
