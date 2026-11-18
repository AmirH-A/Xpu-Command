//! A small, blocking AT-SPI client: the accessibility bus screen readers use to read and operate
//! other apps on Linux (GTK, Qt, Firefox, Chromium, LibreOffice…), on X11 and Wayland alike.
//!
//! * Plain method calls on one private connection to the accessibility bus; no proxies, so no
//!   property caches and no signal subscriptions per element.
//! * Every call has a deadline (the connection's method timeout), so a hung app can't hang us.
//! * Focus is followed through AT-SPI events on a listener thread; when no event has been seen
//!   (an app started before us), it is searched for in the active window.
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Structure, Value};

const ROOT_BUS: &str = "org.a11y.atspi.Registry";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
const NULL_PATH: &str = "/org/a11y/atspi/null";

/// AT-SPI roles (AtspiRole) this app tells apart.
pub mod role {
    pub const CHECK_BOX: u32 = 7;
    pub const CHECK_MENU_ITEM: u32 = 8;
    pub const COMBO_BOX: u32 = 11;
    pub const DIALOG: u32 = 16;
    pub const FRAME: u32 = 23;
    pub const IMAGE: u32 = 27;
    pub const LABEL: u32 = 29;
    pub const LIST_ITEM: u32 = 32;
    pub const MENU: u32 = 33;
    pub const MENU_BAR: u32 = 34;
    pub const MENU_ITEM: u32 = 35;
    pub const PAGE_TAB: u32 = 37;
    pub const PASSWORD_TEXT: u32 = 40;
    pub const POPUP_MENU: u32 = 41;
    pub const PUSH_BUTTON: u32 = 43;
    pub const RADIO_BUTTON: u32 = 44;
    pub const RADIO_MENU_ITEM: u32 = 45;
    pub const SLIDER: u32 = 51;
    pub const SPIN_BUTTON: u32 = 52;
    pub const TABLE_CELL: u32 = 56;
    pub const TEXT: u32 = 61;
    pub const TOGGLE_BUTTON: u32 = 62;
    pub const WINDOW: u32 = 69;
    pub const PARAGRAPH: u32 = 73;
    pub const ENTRY: u32 = 79;
    pub const HEADING: u32 = 83;
    pub const LINK: u32 = 88;
    pub const TABLE_ROW: u32 = 90;
    pub const TREE_ITEM: u32 = 91;
    pub const DOCUMENT_WEB: u32 = 95;
    pub const STATIC: u32 = 116;
    pub const PUSH_BUTTON_MENU: u32 = 129;
}

/// AT-SPI states (AtspiStateType): bit numbers in the two-word state set.
pub mod state {
    pub const ACTIVE: u32 = 1;
    pub const BUSY: u32 = 3;
    pub const CHECKED: u32 = 4;
    pub const DEFUNCT: u32 = 6;
    pub const EDITABLE: u32 = 7;
    pub const ENABLED: u32 = 8;
    pub const FOCUSED: u32 = 12;
    pub const SENSITIVE: u32 = 24;
    pub const SHOWING: u32 = 25;
    pub const VISIBLE: u32 = 30;
}

/// Coordinates relative to the screen (AtspiCoordType SCREEN).
const SCREEN: u32 = 0;

/// A state set: two 32-bit words.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct States(pub u64);

impl States {
    pub fn from_words(w: &[u32]) -> Self {
        let lo = w.first().copied().unwrap_or(0) as u64;
        let hi = w.get(1).copied().unwrap_or(0) as u64;
        States(lo | (hi << 32))
    }
    pub fn has(self, s: u32) -> bool {
        s < 64 && self.0 & (1u64 << s) != 0
    }
}

/// An accessible object of some app: its bus name on the accessibility bus and its object path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct El {
    pub bus: String,
    pub path: String,
}

struct Bus {
    conn: Connection,
    at: Instant,
}

static BUS: Mutex<Option<Bus>> = Mutex::new(None);
static PIDS: OnceLock<Mutex<HashMap<String, i32>>> = OnceLock::new();

/// The address of the accessibility bus, from the session bus (`org.a11y.Bus`).
fn a11y_address() -> Result<String, String> {
    if let Ok(a) = std::env::var("AT_SPI_BUS_ADDRESS") {
        if !a.is_empty() {
            return Ok(a);
        }
    }
    let session = Connection::session().map_err(|e| format!("no session bus: {e}"))?;
    let reply = session
        .call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())
        .map_err(|e| format!("the accessibility bus isn't running (at-spi2-core): {e}"))?;
    reply.body().deserialize::<String>().map_err(|e| e.to_string())
}

/// Turns accessibility on for apps that only build their tree when it is (GTK 4, Qt). It is the
/// same switch the desktop's own accessibility settings use; screen readers are not started.
pub fn enable() {
    let Ok(session) = Connection::session() else { return };
    let r = session.call_method(
        Some("org.a11y.Bus"),
        "/org/a11y/bus",
        Some("org.freedesktop.DBus.Properties"),
        "Set",
        &("org.a11y.Status", "IsEnabled", Value::from(true)),
    );
    if let Err(e) = r {
        log::info!("couldn't turn on accessibility: {e}");
    }
}

fn connect() -> Result<Connection, String> {
    let addr = a11y_address()?;
    zbus::blocking::connection::Builder::address(addr.as_str())
        .map_err(|e| e.to_string())?
        .method_timeout(Duration::from_millis(1500))
        .build()
        .map_err(|e| format!("couldn't connect to the accessibility bus: {e}"))
}

/// The shared connection, made on first use and remade if it broke.
fn conn() -> Result<Connection, String> {
    let mut slot = BUS.lock();
    if let Some(b) = slot.as_ref() {
        return Ok(b.conn.clone());
    }
    let c = connect()?;
    *slot = Some(Bus { conn: c.clone(), at: Instant::now() });
    Ok(c)
}

/// Drops the connection after a transport error, so the next call reconnects.
fn reset_if_broken(e: &zbus::Error) {
    if matches!(e, zbus::Error::InputOutput(_) | zbus::Error::Handshake(_)) {
        let mut slot = BUS.lock();
        if slot.as_ref().is_some_and(|b| b.at.elapsed() > Duration::from_secs(1)) {
            *slot = None;
        }
    }
}

/// Whether the accessibility bus answers at all.
pub fn available() -> bool {
    conn().is_ok_and(|c| desktop_children_with(&c).is_ok())
}

fn desktop_children_with(c: &Connection) -> Result<Vec<El>, String> {
    let r = c.call_method(Some(ROOT_BUS), ROOT_PATH, Some("org.a11y.atspi.Accessible"), "GetChildren", &()).map_err(|e| e.to_string())?;
    let v: Vec<(String, OwnedObjectPath)> = r.body().deserialize().map_err(|e| e.to_string())?;
    Ok(v.into_iter().filter_map(El::from_ref).collect())
}

/// The running apps that registered with the accessibility bus.
pub fn applications() -> Vec<El> {
    conn().and_then(|c| desktop_children_with(&c)).unwrap_or_default()
}

/// Turns AT-SPI errors into something the assistant (or the user) can act on.
fn friendly(e: zbus::Error) -> String {
    reset_if_broken(&e);
    let s = e.to_string();
    if s.contains("UnknownObject") || s.contains("UnknownMethod") && s.contains("Accessible") || s.contains("ServiceUnknown") || s.contains("NameHasNoOwner") {
        "that control no longer exists; read the window again".into()
    } else if s.contains("NoReply") || s.contains("Timeout") || s.contains("timed out") {
        "the app didn't answer in time".into()
    } else if s.contains("UnknownInterface") || s.contains("UnknownMethod") {
        "that control can't do this".into()
    } else {
        crate::agents::clip(&s, 200)
    }
}

impl El {
    fn from_ref((bus, path): (String, OwnedObjectPath)) -> Option<El> {
        let path = path.as_str().to_string();
        (path != NULL_PATH && !bus.is_empty()).then_some(El { bus, path })
    }

    fn call<B, R>(&self, iface: &str, method: &str, body: &B) -> Result<R, String>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let c = conn()?;
        let reply = c.call_method(Some(self.bus.as_str()), self.path.as_str(), Some(iface), method, body).map_err(friendly)?;
        reply.body().deserialize::<R>().map_err(|e| e.to_string())
    }

    fn prop(&self, iface: &str, name: &str) -> Result<OwnedValue, String> {
        self.call("org.freedesktop.DBus.Properties", "Get", &(iface, name))
    }

    pub fn name(&self) -> Option<String> {
        self.prop("org.a11y.atspi.Accessible", "Name").ok().and_then(|v| String::try_from(v).ok())
    }

    pub fn description(&self) -> Option<String> {
        self.prop("org.a11y.atspi.Accessible", "Description").ok().and_then(|v| String::try_from(v).ok())
    }

    pub fn role(&self) -> Option<u32> {
        self.call("org.a11y.atspi.Accessible", "GetRole", &()).ok()
    }

    pub fn states(&self) -> States {
        self.call::<_, Vec<u32>>("org.a11y.atspi.Accessible", "GetState", &()).map(|w| States::from_words(&w)).unwrap_or_default()
    }

    pub fn children(&self) -> Vec<El> {
        self.call::<_, Vec<(String, OwnedObjectPath)>>("org.a11y.atspi.Accessible", "GetChildren", &())
            .map(|v| v.into_iter().take(2000).filter_map(El::from_ref).collect())
            .unwrap_or_default()
    }

    pub fn attributes(&self) -> HashMap<String, String> {
        self.call("org.a11y.atspi.Accessible", "GetAttributes", &()).unwrap_or_default()
    }

    pub fn interfaces(&self) -> Vec<String> {
        self.call("org.a11y.atspi.Accessible", "GetInterfaces", &()).unwrap_or_default()
    }

    /// The process that owns this element (asked of the bus, so an app can't lie about it).
    pub fn pid(&self) -> Option<i32> {
        let cache = PIDS.get_or_init(Default::default);
        if let Some(p) = cache.lock().get(&self.bus) {
            return Some(*p);
        }
        let c = conn().ok()?;
        let r = c.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "GetConnectionUnixProcessID", &(self.bus.as_str(),)).ok()?;
        let pid = r.body().deserialize::<u32>().ok()? as i32;
        let mut m = cache.lock();
        if m.len() > 500 {
            m.clear();
        }
        m.insert(self.bus.clone(), pid);
        Some(pid)
    }

    /// Position and size on screen. None when the app doesn't know (Wayland often can't).
    pub fn frame(&self) -> Option<(f64, f64, f64, f64)> {
        let (x, y, w, h): (i32, i32, i32, i32) = self.call("org.a11y.atspi.Component", "GetExtents", &(SCREEN,)).ok()?;
        (w > 0 && h > 0 && x > -10_000 && y > -10_000).then(|| (f64::from(x), f64::from(y), f64::from(w), f64::from(h)))
    }

    pub fn grab_focus(&self) -> Result<(), String> {
        let ok: bool = self.call("org.a11y.atspi.Component", "GrabFocus", &())?;
        if ok { Ok(()) } else { Err("the app refused to focus that control".into()) }
    }

    /// The deepest element at a screen point inside this one (usually an application or window).
    pub fn element_at(&self, x: i32, y: i32) -> Option<El> {
        let r: (String, OwnedObjectPath) = self.call("org.a11y.atspi.Component", "GetAccessibleAtPoint", &(x, y, SCREEN)).ok()?;
        El::from_ref(r)
    }

    /// Action names, e.g. "click", "press", "activate", "toggle", "jump".
    pub fn actions(&self) -> Vec<String> {
        match self.call::<_, Vec<(String, String, String)>>("org.a11y.atspi.Action", "GetActions", &()) {
            Ok(v) => v.into_iter().take(30).map(|a| a.0.to_lowercase()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn do_action(&self, name: &str) -> Result<(), String> {
        let i = self.actions().iter().position(|a| a == name).ok_or_else(|| format!("that control has no \"{name}\" action"))?;
        let ok: bool = self.call("org.a11y.atspi.Action", "DoAction", &(i as i32,))?;
        if ok { Ok(()) } else { Err(format!("the app refused \"{name}\"")) }
    }

    pub fn character_count(&self) -> Option<i32> {
        self.prop("org.a11y.atspi.Text", "CharacterCount").ok().and_then(|v| i32::try_from(v).ok())
    }

    /// The element's text (Text interface), whole.
    pub fn text(&self) -> Option<String> {
        let n = self.character_count()?;
        if !(0..=200_000).contains(&n) {
            return None;
        }
        self.call("org.a11y.atspi.Text", "GetText", &(0i32, n)).ok()
    }

    pub fn caret(&self) -> Option<usize> {
        let v = self.prop("org.a11y.atspi.Text", "CaretOffset").ok()?;
        usize::try_from(i32::try_from(v).ok()?).ok()
    }

    /// The first selection (start, end) in characters, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let n: i32 = self.call("org.a11y.atspi.Text", "GetNSelections", &()).ok()?;
        if n < 1 {
            return None;
        }
        let (s, e): (i32, i32) = self.call("org.a11y.atspi.Text", "GetSelection", &(0i32,)).ok()?;
        let (s, e) = (s.min(e), s.max(e));
        (s >= 0 && e > s).then_some((s as usize, e as usize))
    }

    pub fn set_selection(&self, start: usize, end: usize) -> Result<(), String> {
        let n: i32 = self.call("org.a11y.atspi.Text", "GetNSelections", &()).unwrap_or(0);
        let ok: bool = if n > 0 {
            self.call("org.a11y.atspi.Text", "SetSelection", &(0i32, start as i32, end as i32))?
        } else {
            self.call("org.a11y.atspi.Text", "AddSelection", &(start as i32, end as i32))?
        };
        if ok { Ok(()) } else { Err("the app refused to select that text".into()) }
    }

    pub fn set_caret(&self, at: usize) -> Result<(), String> {
        let ok: bool = self.call("org.a11y.atspi.Text", "SetCaretOffset", &(at as i32,))?;
        if ok { Ok(()) } else { Err("the app refused to move the cursor".into()) }
    }

    pub fn insert_text(&self, at: usize, text: &str) -> Result<(), String> {
        let len = text.chars().count() as i32;
        let ok: bool = self.call("org.a11y.atspi.EditableText", "InsertText", &(at as i32, text, len))?;
        if ok { Ok(()) } else { Err("the app refused the text".into()) }
    }

    pub fn delete_text(&self, start: usize, end: usize) -> Result<(), String> {
        let ok: bool = self.call("org.a11y.atspi.EditableText", "DeleteText", &(start as i32, end as i32))?;
        if ok { Ok(()) } else { Err("the app refused to delete the text".into()) }
    }

    /// A document attribute ("DocURL" in Firefox, "URI" in Chromium).
    pub fn document_attribute(&self, name: &str) -> Option<String> {
        self.call::<_, String>("org.a11y.atspi.Document", "GetAttributeValue", &(name,)).ok().filter(|s| !s.is_empty())
    }

    /// Current value of a slider, spin button or progress bar.
    pub fn value(&self) -> Option<f64> {
        self.prop("org.a11y.atspi.Value", "CurrentValue").ok().and_then(|v| f64::try_from(v).ok())
    }

    pub fn is_password(&self) -> bool {
        self.role() == Some(role::PASSWORD_TEXT)
    }
}

/// The element that last reported keyboard focus, from the event listener.
static FOCUS: Mutex<Option<El>> = Mutex::new(None);
static LISTENING: OnceLock<()> = OnceLock::new();

/// Follows keyboard focus through AT-SPI events, on its own thread and connection.
pub fn listen() {
    LISTENING.get_or_init(|| {
        let r = std::thread::Builder::new().name("xpu-atspi-events".into()).spawn(|| {
            let mut backoff = Duration::from_secs(1);
            loop {
                if let Err(e) = follow_focus() {
                    log::info!("accessibility events: {e}");
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        });
        if let Err(e) = r {
            log::warn!("accessibility events: {e}");
        }
    });
}

fn follow_focus() -> Result<(), String> {
    let c = connect()?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&c).map_err(|e| e.to_string())?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.a11y.atspi.Event.Object")
        .map_err(|e| e.to_string())?
        .member("StateChanged")
        .map_err(|e| e.to_string())?
        .build();
    dbus.add_match_rule(rule).map_err(|e| e.to_string())?;
    // Apps only send events someone registered for.
    c.call_method(Some(ROOT_BUS), "/org/a11y/atspi/registry", Some("org.a11y.atspi.Registry"), "RegisterEvent", &("object:state-changed:focused",))
        .map_err(|e| e.to_string())?;
    for msg in zbus::blocking::MessageIterator::from(&c) {
        let Ok(msg) = msg else { continue };
        let h = msg.header();
        if h.member().map(|m| m.as_str()) != Some("StateChanged") {
            continue;
        }
        let (Some(sender), Some(path)) = (h.sender().map(|s| s.to_string()), h.path().map(|p| p.to_string())) else { continue };
        let body = msg.body();
        let Ok(body) = body.deserialize::<Structure>() else { continue };
        let f = body.fields();
        let (Some(Value::Str(detail)), Some(Value::I32(on))) = (f.first(), f.get(1)) else { continue };
        if detail.as_str() != "focused" {
            continue;
        }
        let el = El { bus: sender, path };
        let mut slot = FOCUS.lock();
        if *on == 1 {
            *slot = Some(el);
        } else if slot.as_ref() == Some(&el) {
            *slot = None;
        }
    }
    Err("the accessibility bus closed".into())
}

/// The window that is active (has the keyboard), across all apps.
pub fn active_window() -> Option<El> {
    for app in applications() {
        for w in app.children().into_iter().take(40) {
            let s = w.states();
            if s.has(state::ACTIVE) && !s.has(state::DEFUNCT) {
                return Some(w);
            }
        }
    }
    None
}

/// The element with keyboard focus, in whatever app is in front: the last focus event if it is
/// still focused, else a bounded search of the active window.
pub fn focused_element() -> Option<El> {
    listen();
    if let Some(el) = FOCUS.lock().clone() {
        if el.states().has(state::FOCUSED) {
            return Some(el);
        }
    }
    let win = active_window()?;
    let found = find(&win, 4000, |e| {
        let s = e.states();
        s.has(state::FOCUSED) && !s.has(state::DEFUNCT)
    });
    if let Some(el) = &found {
        *FOCUS.lock() = Some(el.clone());
    }
    found
}

/// Depth-first search (front to back) for the first element that matches, visiting at most `limit`.
pub fn find(root: &El, limit: usize, mut hit: impl FnMut(&El) -> bool) -> Option<El> {
    let mut stack = vec![(root.clone(), 0u16)];
    let mut visited = 0;
    while let Some((el, depth)) = stack.pop() {
        visited += 1;
        if visited > limit {
            return None;
        }
        if hit(&el) {
            return Some(el);
        }
        if depth < 60 {
            let kids = el.children();
            stack.extend(kids.into_iter().rev().map(|k| (k, depth + 1)));
        }
    }
    None
}

/// Character offset → UTF-16 offset in `text` (AT-SPI counts characters; the shared editing code
/// counts UTF-16 units like the macOS Accessibility API).
pub fn utf16_offset(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf16).sum()
}

/// UTF-16 offset → character offset (rounded down to a whole character).
pub fn char_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.chars().enumerate() {
        if units + c.len_utf16() > utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_bits() {
        let s = States::from_words(&[1 << state::FOCUSED | 1 << state::ENABLED, 1]);
        assert!(s.has(state::FOCUSED) && s.has(state::ENABLED) && s.has(32));
        assert!(!s.has(state::ACTIVE) && !s.has(99));
    }

    #[test]
    fn offsets_between_characters_and_utf16() {
        let t = "ab👋cd";
        assert_eq!(utf16_offset(t, 3), 4, "the emoji takes two units");
        assert_eq!(char_offset(t, 4), 3);
        assert_eq!(char_offset(t, 3), 2, "half an emoji rounds down");
        assert_eq!(char_offset(t, 99), 5);
        let fa = "سلام می‌بینمت";
        assert_eq!(utf16_offset(fa, fa.chars().count()), fa.encode_utf16().count());
    }

    #[test]
    fn null_references_are_dropped() {
        assert!(El::from_ref((":1.2".into(), OwnedObjectPath::try_from(NULL_PATH).unwrap())).is_none());
        assert!(El::from_ref((":1.2".into(), OwnedObjectPath::try_from("/org/a11y/atspi/accessible/5").unwrap())).is_some());
    }
}
