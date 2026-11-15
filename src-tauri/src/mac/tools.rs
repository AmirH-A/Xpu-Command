//! The Mac tools the assistant may call: their schemas, strict input checks, risk levels and the
//! one-line description shown in the pill. Pure (no I/O), so every rule here is unit-tested.
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use reqwest::Url;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

/// What the gate does before running a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Only looks something up.
    Read,
    /// Harmless and easy to reverse (open an app, set the volume): runs, shown in the pill.
    Safe,
    /// Changes the user's data (a reminder, a draft): shown first, runs unless cancelled.
    Change,
    /// Could do anything (a Shortcut): runs only after the user's explicit yes.
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaAction {
    Play,
    Pause,
    Next,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Player {
    Music,
    Spotify,
}

/// A validated tool call. Every string in here has passed `text()` (length, no control characters).
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    ListApps,
    OpenApp { name: String },
    QuitApp { name: String },
    OpenUrl { url: Url },
    WebSearch { query: String },
    Media { action: MediaAction, player: Option<Player> },
    PlayMusic { query: String, player: Option<Player> },
    SetVolume { level: Option<u8>, mute: Option<bool> },
    DarkMode { on: bool },
    ListShortcuts,
    RunShortcut { name: String },
    CreateReminder { title: String, due: Option<NaiveDateTime>, notes: Option<String> },
    CreateEvent { title: String, start: NaiveDateTime, end: NaiveDateTime, location: Option<String> },
    CreateNote { title: String, body: String },
    DraftEmail { to: Vec<String>, subject: String, body: String },
    ListWindows,
    ReadWindow { app: String },
    Screenshot { app: String },
    Click { element: u32 },
    TypeText { element: u32, text: String, submit: bool },
    PressKeys { app: String, mods: Vec<&'static str>, key: String },
    ChooseMenu { app: String, path: Vec<String> },
    Scroll { app: String, down: bool, amount: u8 },
    ClickAt { app: String, x: f64, y: f64 },
    Remember { text: String },
    Forget { text: String },
}

impl Call {
    pub fn tool(&self) -> &'static str {
        match self {
            Call::ListApps => "list_apps",
            Call::OpenApp { .. } => "open_app",
            Call::QuitApp { .. } => "quit_app",
            Call::OpenUrl { .. } => "open_url",
            Call::WebSearch { .. } => "web_search",
            Call::Media { .. } => "media",
            Call::PlayMusic { .. } => "play_music",
            Call::SetVolume { .. } => "set_volume",
            Call::DarkMode { .. } => "dark_mode",
            Call::ListShortcuts => "list_shortcuts",
            Call::RunShortcut { .. } => "run_shortcut",
            Call::CreateReminder { .. } => "create_reminder",
            Call::CreateEvent { .. } => "create_event",
            Call::CreateNote { .. } => "create_note",
            Call::DraftEmail { .. } => "draft_email",
            Call::ListWindows => "list_windows",
            Call::ReadWindow { .. } => "read_window",
            Call::Screenshot { .. } => "screenshot",
            Call::Click { .. } => "click",
            Call::TypeText { .. } => "type_text",
            Call::PressKeys { .. } => "press_keys",
            Call::ChooseMenu { .. } => "choose_menu",
            Call::Scroll { .. } => "scroll",
            Call::ClickAt { .. } => "click_at",
            Call::Remember { .. } => "remember",
            Call::Forget { .. } => "forget",
        }
    }

    /// For calls on an app window: whether they need to see it or control it.
    pub fn need(&self) -> Option<super::policy::Need> {
        use super::policy::Need;
        match self {
            Call::ReadWindow { .. } | Call::Screenshot { .. } => Some(Need::See),
            Call::Click { .. } | Call::TypeText { .. } | Call::PressKeys { .. } | Call::ChooseMenu { .. } | Call::Scroll { .. } | Call::ClickAt { .. } => Some(Need::Control),
            _ => None,
        }
    }

    pub fn flags(&self) -> super::policy::Flags {
        use super::policy::{risky_keys, risky_label, Flags};
        match self {
            Call::OpenUrl { .. } | Call::WebSearch { .. } | Call::DraftEmail { .. } | Call::RunShortcut { .. } | Call::PlayMusic { .. } => Flags { outbound: true, risky: false },
            Call::PressKeys { mods, key, .. } => Flags { outbound: false, risky: risky_keys(mods, key) },
            Call::ChooseMenu { path, .. } => Flags { outbound: false, risky: path.iter().any(|p| risky_label(p)) },
            Call::TypeText { submit: true, .. } => Flags { outbound: false, risky: false },
            _ => Flags::default(),
        }
    }

    pub fn risk(&self) -> Risk {
        match self {
            Call::ListApps | Call::ListShortcuts | Call::ListWindows | Call::ReadWindow { .. } | Call::Screenshot { .. } => Risk::Read,
            Call::Click { .. } | Call::TypeText { .. } | Call::PressKeys { .. } | Call::ChooseMenu { .. } | Call::Scroll { .. } | Call::ClickAt { .. } => Risk::Safe,
            Call::OpenApp { .. } | Call::OpenUrl { .. } | Call::WebSearch { .. } | Call::Media { .. } | Call::PlayMusic { .. } | Call::SetVolume { .. } | Call::DarkMode { .. } => Risk::Safe,
            Call::Remember { .. } | Call::Forget { .. } => Risk::Safe,
            Call::QuitApp { .. } | Call::CreateReminder { .. } | Call::CreateEvent { .. } | Call::CreateNote { .. } | Call::DraftEmail { .. } => Risk::Change,
            Call::RunShortcut { .. } => Risk::Confirm,
        }
    }

    /// One line for the pill, the spoken confirmation and the activity log.
    pub fn describe(&self) -> String {
        let s = match self {
            Call::ListApps => "Looking at your apps".to_string(),
            Call::OpenApp { name } => format!("Opening {name}"),
            Call::QuitApp { name } => format!("Quit {name}"),
            Call::OpenUrl { url } => format!("Opening {}", url.host_str().unwrap_or("the page")),
            Call::WebSearch { query } => format!("Searching the web for “{query}”"),
            Call::Media { action, .. } => match action {
                MediaAction::Play => "Playing",
                MediaAction::Pause => "Pausing",
                MediaAction::Next => "Next track",
                MediaAction::Previous => "Previous track",
            }
            .to_string(),
            Call::PlayMusic { query, .. } => format!("Playing “{query}”"),
            Call::SetVolume { mute: Some(true), .. } => "Muting".to_string(),
            Call::SetVolume { mute: Some(false), level: None } => "Unmuting".to_string(),
            Call::SetVolume { level, .. } => format!("Volume {}%", level.unwrap_or(50)),
            Call::DarkMode { on } => format!("Dark mode {}", if *on { "on" } else { "off" }),
            Call::ListShortcuts => "Looking at your Shortcuts".to_string(),
            Call::RunShortcut { name } => format!("Run the shortcut “{name}”"),
            Call::CreateReminder { title, due, .. } => match due {
                Some(d) => format!("Reminder: {title} · {}", when(d)),
                None => format!("Reminder: {title}"),
            },
            Call::CreateEvent { title, start, .. } => format!("Event: {title} · {}", when(start)),
            Call::CreateNote { title, .. } => format!("Note: {title}"),
            Call::DraftEmail { to, subject, .. } => match to.first() {
                Some(a) if to.len() > 1 => format!("Draft email to {a} +{}: {subject}", to.len() - 1),
                Some(a) => format!("Draft email to {a}: {subject}"),
                None => format!("Draft email: {subject}"),
            },
            Call::ListWindows => "Looking at open windows".to_string(),
            Call::ReadWindow { app } => format!("Reading {app}"),
            Call::Screenshot { app } => format!("Looking at {app}"),
            Call::Click { element } => format!("Clicking [{element}]"),
            Call::TypeText { element, text, submit } => format!("Typing “{}” into [{element}]{}", crate::agents::clip(text, 40), if *submit { " and pressing Return" } else { "" }),
            Call::PressKeys { app, mods, key } => format!("Pressing {}{} in {app}", mods.iter().map(|m| format!("{m}+")).collect::<String>(), key),
            Call::ChooseMenu { app, path } => format!("Menu {} in {app}", path.join(" › ")),
            Call::Scroll { app, down, .. } => format!("Scrolling {} in {app}", if *down { "down" } else { "up" }),
            Call::ClickAt { app, x, y } => format!("Clicking at ({x:.0}, {y:.0}) in {app}"),
            Call::Remember { text } => format!("Remembering: {text}"),
            Call::Forget { text } => format!("Forgetting “{text}”"),
        };
        crate::agents::clip(&s, 140)
    }
}

fn when(d: &NaiveDateTime) -> String {
    d.format("%a %-d %b %H:%M").to_string()
}

/// The tool list for MCP `tools/list`. Descriptions are written for the model.
pub fn schemas() -> Value {
    let s = |name: &str, desc: &str, props: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": desc,
            "inputSchema": { "type": "object", "properties": props, "required": required, "additionalProperties": false }
        })
    };
    let player = json!({ "type": "string", "enum": ["music", "spotify"], "description": "Optional. Default: Spotify if it is installed, else Music." });
    let local_time = "Local date and time, YYYY-MM-DDTHH:MM";
    json!([
        s("list_apps", "List the names of the apps installed on this Mac.", json!({}), &[]),
        s("open_app", "Open (or switch to) an installed app by name.", json!({ "name": { "type": "string", "description": "App name, e.g. \"Spotify\"" } }), &["name"]),
        s("quit_app", "Quit a running app. The user sees this first and can cancel it.", json!({ "name": { "type": "string" } }), &["name"]),
        s("open_url", "Open a web page (http or https) in the default browser.", json!({ "url": { "type": "string" } }), &["url"]),
        s("web_search", "Search the web in the default browser.", json!({ "query": { "type": "string" } }), &["query"]),
        s("media", "Control music playback.", json!({ "action": { "type": "string", "enum": ["play", "pause", "next", "previous"] }, "player": player }), &["action"]),
        s("play_music", "Play a playlist, song, artist or album. In Music it plays from the library; in Spotify it opens the search results.", json!({ "query": { "type": "string" }, "player": player }), &["query"]),
        s("set_volume", "Set the output volume (0-100) and/or mute or unmute.", json!({ "level": { "type": "integer", "minimum": 0, "maximum": 100 }, "mute": { "type": "boolean" } }), &[]),
        s("dark_mode", "Turn dark mode on or off.", json!({ "on": { "type": "boolean" } }), &["on"]),
        s("list_shortcuts", "List the user's Shortcuts (from the Shortcuts app).", json!({}), &[]),
        s("run_shortcut", "Run one of the user's Shortcuts by its exact name. The user must approve it first.", json!({ "name": { "type": "string" } }), &["name"]),
        s("create_reminder", "Create a reminder in the default Reminders list. The user sees it first and can cancel it.", json!({ "title": { "type": "string" }, "due": { "type": "string", "description": local_time }, "notes": { "type": "string" } }), &["title"]),
        s("create_event", "Create a calendar event. The user sees it first and can cancel it.", json!({ "title": { "type": "string" }, "start": { "type": "string", "description": local_time }, "end": { "type": "string", "description": "Optional; default: one hour after start" }, "location": { "type": "string" } }), &["title", "start"]),
        s("create_note", "Create a note in Notes. The user sees it first and can cancel it.", json!({ "title": { "type": "string" }, "body": { "type": "string" } }), &["title", "body"]),
        s("list_windows", "List the apps that have windows open, with the access the user gave each (off, see, control, ask).", json!({}), &[]),
        s("read_window", "Read an app's front window: its text and controls, numbered like [12]. Needs see access. Use the numbers with click and type_text. Re-read after the window changes.", json!({ "app": { "type": "string" } }), &["app"]),
        s("click", "Click a numbered control from the latest read_window. Needs control access.", json!({ "element": { "type": "integer", "minimum": 1 } }), &["element"]),
        s("type_text", "Type text into a numbered text field (focuses it first). Set submit to press Return after typing. Never used for passwords.", json!({ "element": { "type": "integer", "minimum": 1 }, "text": { "type": "string" }, "submit": { "type": "boolean" } }), &["element", "text"]),
        s("press_keys", "Press a key combination in an app, e.g. \"cmd+l\", \"escape\", \"pagedown\".", json!({ "app": { "type": "string" }, "keys": { "type": "string" } }), &["app", "keys"]),
        s("choose_menu", "Choose a menu-bar command, e.g. [\"File\", \"Export…\"].", json!({ "app": { "type": "string" }, "path": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 5 } }), &["app", "path"]),
        s("scroll", "Scroll an app's front window.", json!({ "app": { "type": "string" }, "direction": { "type": "string", "enum": ["up", "down"] }, "amount": { "type": "integer", "minimum": 1, "maximum": 20 } }), &["app", "direction"]),
        s("screenshot", "Fallback when read_window shows too little: an image of the app's front window. Needs see access and Screen Recording.", json!({ "app": { "type": "string" } }), &["app"]),
        s("click_at", "Fallback: click at x, y (pixels) in the app's latest screenshot.", json!({ "app": { "type": "string" }, "x": { "type": "number" }, "y": { "type": "number" } }), &["app", "x", "y"]),
        s("remember", "Save a short fact the user asked you to remember about them (a person, place, folder or preference), in their own words, e.g. \"Sara is my accountant, sara@example.com\". Only when the user asks to remember something.", json!({ "text": { "type": "string", "maxLength": 200 } }), &["text"]),
        s("forget", "Remove remembered facts that contain these words. Only when the user asks to forget something.", json!({ "text": { "type": "string" } }), &["text"]),
        s("draft_email", "Open a new email draft in Mail for the user to review and send. It is never sent automatically. Use only email addresses the user said; leave \"to\" empty otherwise.", json!({ "to": { "type": "array", "items": { "type": "string" }, "maxItems": 10 }, "subject": { "type": "string" }, "body": { "type": "string" } }), &["subject", "body"]),
    ])
}


#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Name {
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UrlArgs {
    url: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    query: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaArgs {
    action: MediaAction,
    player: Option<Player>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlayArgs {
    query: String,
    player: Option<Player>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VolumeArgs {
    level: Option<i64>,
    mute: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DarkArgs {
    on: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReminderArgs {
    title: String,
    due: Option<String>,
    notes: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventArgs {
    title: String,
    start: String,
    end: Option<String>,
    location: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteArgs {
    title: String,
    body: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppArg {
    app: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ElementArg {
    element: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeArgs {
    element: u32,
    text: String,
    #[serde(default)]
    submit: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeysArgs {
    app: String,
    keys: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MenuArgs {
    app: String,
    path: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScrollArgs {
    app: String,
    direction: String,
    amount: Option<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClickAtArgs {
    app: String,
    x: f64,
    y: f64,
}

/// "cmd+shift+t" → (["cmd", "shift"], "t"). Modifier aliases are normalised; the key must exist.
pub fn parse_keys(s: &str) -> Result<(Vec<&'static str>, String), String> {
    let s = s.trim().to_lowercase();
    if s.is_empty() || s.len() > 40 {
        return Err("keys must look like \"cmd+l\"".into());
    }
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let (key, mods) = parts.split_last().ok_or("no key")?;
    let mut out: Vec<&'static str> = Vec::new();
    for m in mods {
        let m = match *m {
            "cmd" | "command" | "⌘" => "cmd",
            "shift" | "⇧" => "shift",
            "option" | "opt" | "alt" | "⌥" => "option",
            "ctrl" | "control" | "⌃" => "ctrl",
            other => return Err(format!("unknown modifier \"{other}\"")),
        };
        if !out.contains(&m) {
            out.push(m);
        }
    }
    let key = match *key {
        "esc" => "escape",
        "backspace" => "delete",
        "del" => "forwarddelete",
        "page down" => "pagedown",
        "page up" => "pageup",
        k => k,
    };
    if keycode(key).is_none() {
        return Err(format!("unknown key \"{key}\""));
    }
    Ok((out, key.to_string()))
}

/// macOS virtual key codes (ANSI positions).
pub fn keycode(k: &str) -> Option<u16> {
    Some(match k {
        "a" => 0, "s" => 1, "d" => 2, "f" => 3, "h" => 4, "g" => 5, "z" => 6, "x" => 7, "c" => 8, "v" => 9, "b" => 11,
        "q" => 12, "w" => 13, "e" => 14, "r" => 15, "y" => 16, "t" => 17, "1" => 18, "2" => 19, "3" => 20, "4" => 21,
        "6" => 22, "5" => 23, "=" => 24, "9" => 25, "7" => 26, "-" => 27, "8" => 28, "0" => 29, "]" => 30, "o" => 31,
        "u" => 32, "[" => 33, "i" => 34, "p" => 35, "l" => 37, "j" => 38, "'" => 39, "k" => 40, ";" => 41, "\\" => 42,
        "," => 43, "/" => 44, "n" => 45, "m" => 46, "." => 47, "`" => 50,
        "return" | "enter" => 36, "tab" => 48, "space" => 49, "delete" => 51, "escape" => 53, "forwarddelete" => 117,
        "home" => 115, "end" => 119, "pageup" => 116, "pagedown" => 121, "left" => 123, "right" => 124, "down" => 125, "up" => 126,
        "f1" => 122, "f2" => 120, "f3" => 99, "f4" => 118, "f5" => 96, "f6" => 97, "f7" => 98, "f8" => 100, "f9" => 101,
        "f10" => 109, "f11" => 103, "f12" => 111,
        _ => return None,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmailArgs {
    #[serde(default)]
    to: Vec<String>,
    subject: String,
    body: String,
}

fn args<T: DeserializeOwned>(v: &Value) -> Result<T, String> {
    let v = if v.is_null() { json!({}) } else { v.clone() };
    serde_json::from_value(v).map_err(|e| format!("invalid arguments: {e}"))
}

/// Checks one text field: trimmed, non-empty, at most `max` characters, and free of control,
/// bidi-override and zero-width characters (newlines and tabs only where `multiline`).
pub fn text(field: &str, s: &str, max: usize, multiline: bool) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if t.chars().count() > max {
        return Err(format!("{field} is longer than {max} characters"));
    }
    if t.chars().any(|c| crate::sanitize::is_dangerous(c) && !(multiline && (c == '\n' || c == '\t'))) {
        return Err(format!("{field} contains control characters"));
    }
    Ok(t.to_string())
}

fn opt_text(field: &str, s: Option<String>, max: usize, multiline: bool) -> Result<Option<String>, String> {
    match s {
        Some(s) if !s.trim().is_empty() => text(field, &s, max, multiline).map(Some),
        _ => Ok(None),
    }
}

/// An app or shortcut name: one line, short, and never something that reads as a command-line flag.
fn name(field: &str, s: &str) -> Result<String, String> {
    let n = text(field, s, 100, false)?;
    if n.starts_with('-') {
        return Err(format!("{field} can't start with \"-\""));
    }
    Ok(n)
}

fn element(n: u32) -> Result<u32, String> {
    if (1..=100_000).contains(&n) {
        Ok(n)
    } else {
        Err("element must be a number from read_window".into())
    }
}

pub fn web_url(s: &str) -> Result<Url, String> {
    let s = s.trim();
    if s.len() > 2048 {
        return Err("the address is too long".into());
    }
    let u = Url::parse(s).map_err(|_| "not a valid web address".to_string())?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("only http and https addresses can be opened".into());
    }
    if u.host_str().map_or(true, str::is_empty) {
        return Err("the address has no host".into());
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("addresses with a user name or password are not opened".into());
    }
    Ok(u)
}

pub fn email(s: &str) -> Result<String, String> {
    let e = s.trim();
    let bad = |c: char| c.is_whitespace() || c.is_control() || !c.is_ascii() || matches!(c, ',' | ';' | '<' | '>' | '"' | '(' | ')' | '[' | ']' | '\\' | ':');
    let ok = e.len() <= 254
        && !e.starts_with('-')
        && !e.chars().any(bad)
        && matches!(e.split('@').collect::<Vec<_>>().as_slice(), [l, d] if !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.'));
    if ok {
        Ok(e.to_string())
    } else {
        Err(format!("\"{}\" is not an email address; leave \"to\" empty and the user will fill it in", crate::agents::clip(e, 60)))
    }
}

/// A local date-time from the model, sanity-checked against `now`: from yesterday to five years ahead.
/// A plain date means 09:00.
pub fn local_time(field: &str, s: &str, now: NaiveDateTime) -> Result<NaiveDateTime, String> {
    let s = s.trim();
    let parsed = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M"))
        .or_else(|_| NaiveDate::parse_from_str(s, "%Y-%m-%d").map(|d| d.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap_or_default())))
        .map_err(|_| format!("{field} must be a local time like 2026-11-15T10:00"))?;
    let parsed = parsed.with_second(0).unwrap_or(parsed);
    if parsed < now - Duration::days(1) || parsed.year() > now.year() + 5 {
        return Err(format!("{field} {} is out of range", parsed.format("%Y-%m-%d %H:%M")));
    }
    Ok(parsed)
}

/// Parses and validates one tool call. `now` is the local time (for date checks).
pub fn parse(tool: &str, v: &Value, now: NaiveDateTime) -> Result<Call, String> {
    Ok(match tool {
        "list_apps" => {
            args::<Empty>(v)?;
            Call::ListApps
        }
        "list_shortcuts" => {
            args::<Empty>(v)?;
            Call::ListShortcuts
        }
        "open_app" => Call::OpenApp { name: name("name", &args::<Name>(v)?.name)? },
        "remember" => Call::Remember { text: text("text", &args::<Text>(v)?.text, crate::memory::MAX_TEXT, false)? },
        "forget" => Call::Forget { text: text("text", &args::<Text>(v)?.text, 100, false)? },
        "quit_app" => Call::QuitApp { name: name("name", &args::<Name>(v)?.name)? },
        "run_shortcut" => Call::RunShortcut { name: name("name", &args::<Name>(v)?.name)? },
        "open_url" => Call::OpenUrl { url: web_url(&args::<UrlArgs>(v)?.url)? },
        "web_search" => Call::WebSearch { query: text("query", &args::<Query>(v)?.query, 300, false)? },
        "media" => {
            let a: MediaArgs = args(v)?;
            Call::Media { action: a.action, player: a.player }
        }
        "play_music" => {
            let a: PlayArgs = args(v)?;
            Call::PlayMusic { query: text("query", &a.query, 120, false)?, player: a.player }
        }
        "set_volume" => {
            let a: VolumeArgs = args(v)?;
            if a.level.is_none() && a.mute.is_none() {
                return Err("give a level (0-100) or mute".into());
            }
            let level = a.level.map(|l| u8::try_from(l).ok().filter(|l| *l <= 100).ok_or("level must be 0-100")).transpose()?;
            Call::SetVolume { level, mute: a.mute }
        }
        "dark_mode" => Call::DarkMode { on: args::<DarkArgs>(v)?.on },
        "create_reminder" => {
            let a: ReminderArgs = args(v)?;
            Call::CreateReminder {
                title: text("title", &a.title, 200, false)?,
                due: a.due.filter(|d| !d.trim().is_empty()).map(|d| local_time("due", &d, now)).transpose()?,
                notes: opt_text("notes", a.notes, 2000, true)?,
            }
        }
        "create_event" => {
            let a: EventArgs = args(v)?;
            let start = local_time("start", &a.start, now)?;
            let end = match a.end.filter(|e| !e.trim().is_empty()) {
                Some(e) => local_time("end", &e, now)?,
                None => start + Duration::hours(1),
            };
            if end <= start || end - start > Duration::days(14) {
                return Err("end must be after start, and within 14 days of it".into());
            }
            Call::CreateEvent { title: text("title", &a.title, 200, false)?, start, end, location: opt_text("location", a.location, 200, false)? }
        }
        "create_note" => {
            let a: NoteArgs = args(v)?;
            Call::CreateNote { title: text("title", &a.title, 200, false)?, body: text("body", &a.body, 8000, true)? }
        }
        "draft_email" => {
            let a: EmailArgs = args(v)?;
            if a.to.len() > 10 {
                return Err("at most 10 recipients".into());
            }
            Call::DraftEmail {
                to: a.to.iter().map(|e| email(e)).collect::<Result<_, _>>()?,
                subject: text("subject", &a.subject, 200, false)?,
                body: text("body", &a.body, 8000, true)?,
            }
        }
        "list_windows" => {
            args::<Empty>(v)?;
            Call::ListWindows
        }
        "read_window" => Call::ReadWindow { app: name("app", &args::<AppArg>(v)?.app)? },
        "screenshot" => Call::Screenshot { app: name("app", &args::<AppArg>(v)?.app)? },
        "click" => Call::Click { element: element(args::<ElementArg>(v)?.element)? },
        "type_text" => {
            let a: TypeArgs = args(v)?;
            Call::TypeText { element: element(a.element)?, text: text("text", &a.text, 4000, true)?, submit: a.submit }
        }
        "press_keys" => {
            let a: KeysArgs = args(v)?;
            let (mods, key) = parse_keys(&a.keys)?;
            Call::PressKeys { app: name("app", &a.app)?, mods, key }
        }
        "choose_menu" => {
            let a: MenuArgs = args(v)?;
            if a.path.is_empty() || a.path.len() > 5 {
                return Err("path must have 1 to 5 items".into());
            }
            Call::ChooseMenu { app: name("app", &a.app)?, path: a.path.iter().map(|p| text("menu item", p, 80, false)).collect::<Result<_, _>>()? }
        }
        "scroll" => {
            let a: ScrollArgs = args(v)?;
            let down = match a.direction.as_str() {
                "down" => true,
                "up" => false,
                _ => return Err("direction must be up or down".into()),
            };
            let amount = a.amount.unwrap_or(5);
            if !(1..=20).contains(&amount) {
                return Err("amount must be 1-20".into());
            }
            Call::Scroll { app: name("app", &a.app)?, down, amount: amount as u8 }
        }
        "click_at" => {
            let a: ClickAtArgs = args(v)?;
            if !(a.x.is_finite() && a.y.is_finite() && a.x >= 0.0 && a.y >= 0.0 && a.x < 20000.0 && a.y < 20000.0) {
                return Err("x and y must be pixel positions in the screenshot".into());
            }
            Call::ClickAt { app: name("app", &a.app)?, x: a.x, y: a.y }
        }
        other => return Err(format!("unknown tool {:?}", crate::agents::clip(other, 40))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-11-14T15:00", "%Y-%m-%dT%H:%M").unwrap()
    }
    fn p(tool: &str, v: Value) -> Result<Call, String> {
        parse(tool, &v, now())
    }

    #[test]
    fn every_schema_parses_and_names_match() {
        let list = schemas();
        let names: Vec<&str> = list.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 26);
        for n in &names {
            let e = p(n, json!({ "zzz": 1 })).unwrap_err();
            assert!(e.contains("invalid arguments") || e.contains("give a level"), "{n}: {e}");
        }
        assert!(p("rm_rf", json!({})).unwrap_err().contains("unknown tool"));
    }

    #[test]
    fn risk_levels() {
        assert_eq!(p("list_apps", json!({})).unwrap().risk(), Risk::Read);
        assert_eq!(p("open_app", json!({"name":"Spotify"})).unwrap().risk(), Risk::Safe);
        assert_eq!(p("set_volume", json!({"level":30})).unwrap().risk(), Risk::Safe);
        assert_eq!(p("create_note", json!({"title":"a","body":"b"})).unwrap().risk(), Risk::Change);
        assert_eq!(p("draft_email", json!({"subject":"a","body":"b"})).unwrap().risk(), Risk::Change);
        assert_eq!(p("quit_app", json!({"name":"Safari"})).unwrap().risk(), Risk::Change);
        assert_eq!(p("run_shortcut", json!({"name":"Morning"})).unwrap().risk(), Risk::Confirm);
        assert_eq!(p("remember", json!({"text":"Sara is my accountant"})).unwrap().risk(), Risk::Safe);
        assert!(p("remember", json!({"text":"x".repeat(201)})).is_err());
    }

    #[test]
    fn urls_are_web_only() {
        assert!(p("open_url", json!({"url":"https://example.com/a?b=c"})).is_ok());
        for bad in ["file:///etc/passwd", "shortcuts://run-shortcut?name=x", "javascript:alert(1)", "ssh://host", "https://user:pw@example.com", "notaurl", "x-apple.systempreferences:com.apple.preference.security"] {
            assert!(p("open_url", json!({ "url": bad })).is_err(), "{bad}");
        }
        assert!(p("open_url", json!({"url": format!("https://e.com/{}", "a".repeat(3000))})).is_err());
    }

    #[test]
    fn text_rules() {
        assert!(p("open_app", json!({"name":""})).is_err());
        assert!(p("open_app", json!({"name":"--help"})).is_err(), "no flag-like names");
        assert!(p("open_app", json!({"name":"Safari\u{202E}"})).is_err(), "bidi override");
        assert!(p("create_reminder", json!({"title":"a\nb"})).is_err(), "titles are one line");
        assert!(p("create_note", json!({"title":"a","body":"line 1\nline 2"})).is_ok(), "bodies may have lines");
        assert!(p("create_note", json!({"title":"a","body":"x\u{0007}"})).is_err());
        assert!(p("web_search", json!({"query": "q".repeat(301)})).is_err());
        assert_eq!(p("open_app", json!({"name":"  Spotify "})).unwrap(), Call::OpenApp { name: "Spotify".into() });
    }

    #[test]
    fn dates() {
        let c = p("create_reminder", json!({"title":"Call the accountant","due":"2026-11-15T10:00"})).unwrap();
        assert_eq!(c.describe(), "Reminder: Call the accountant · Sun 15 Nov 10:00");
        let d = p("create_reminder", json!({"title":"x","due":"2026-11-15"})).unwrap();
        assert!(matches!(d, Call::CreateReminder { due: Some(t), .. } if t.hour() == 9));
        assert!(p("create_reminder", json!({"title":"x","due":"tomorrow"})).is_err());
        assert!(p("create_reminder", json!({"title":"x","due":"2020-01-01T10:00"})).is_err(), "the past");
        assert!(p("create_reminder", json!({"title":"x","due":"2040-01-01T10:00"})).is_err(), "too far");
        let e = p("create_event", json!({"title":"Standup","start":"2026-11-16T09:30"})).unwrap();
        assert!(matches!(e, Call::CreateEvent { start, end, .. } if end - start == Duration::hours(1)));
        assert!(p("create_event", json!({"title":"x","start":"2026-11-16T09:30","end":"2026-11-16T09:00"})).is_err());
    }

    #[test]
    fn email_rules() {
        assert!(p("draft_email", json!({"to":["sara@example.com"],"subject":"Late","body":"15 minutes"})).is_ok());
        for bad in ["Sara", "a@b", "a b@c.com", "a@b.com,c@d.com", "-x@y.com", "a@@b.com", "\"x\"@y.com", "a@.com"] {
            assert!(p("draft_email", json!({"to":[bad],"subject":"s","body":"b"})).is_err(), "{bad}");
        }
        let many: Vec<String> = (0..11).map(|i| format!("a{i}@b.com")).collect();
        assert!(p("draft_email", json!({"to":many,"subject":"s","body":"b"})).is_err());
    }

    #[test]
    fn window_tools() {
        assert_eq!(p("read_window", json!({"app":"Chrome"})).unwrap().need(), Some(crate::mac::policy::Need::See));
        assert_eq!(p("click", json!({"element":12})).unwrap().need(), Some(crate::mac::policy::Need::Control));
        assert!(p("click", json!({"element":0})).is_err());
        assert!(p("click", json!({"element":"12"})).is_err());
        let k = p("press_keys", json!({"app":"Chrome","keys":"Cmd+Shift+T"})).unwrap();
        assert_eq!(k, Call::PressKeys { app: "Chrome".into(), mods: vec!["cmd", "shift"], key: "t".into() });
        assert!(!k.flags().risky);
        assert!(p("press_keys", json!({"app":"Chrome","keys":"cmd+q"})).unwrap().flags().risky);
        assert!(p("press_keys", json!({"app":"Chrome","keys":"hyper+q"})).is_err());
        assert!(p("press_keys", json!({"app":"Chrome","keys":"cmd+nope"})).is_err());
        assert!(p("choose_menu", json!({"app":"Finder","path":["File","Move to Trash"]})).unwrap().flags().risky);
        assert!(!p("choose_menu", json!({"app":"Preview","path":["File","Export…"]})).unwrap().flags().risky);
        assert!(p("choose_menu", json!({"app":"Preview","path":[]})).is_err());
        assert!(p("type_text", json!({"element":3,"text":"hello\nworld"})).is_ok());
        assert!(p("type_text", json!({"element":3,"text":"x\u{001b}[2J"})).is_err(), "no escape sequences");
        assert!(p("scroll", json!({"app":"Chrome","direction":"sideways"})).is_err());
        assert!(p("click_at", json!({"app":"Chrome","x":-1,"y":5})).is_err());
        assert!(p("open_url", json!({"url":"https://a.com"})).unwrap().flags().outbound);
    }

    #[test]
    fn volume() {
        assert!(p("set_volume", json!({})).is_err());
        assert!(p("set_volume", json!({"level":101})).is_err());
        assert!(p("set_volume", json!({"level":-1})).is_err());
        assert_eq!(p("set_volume", json!({"mute":true})).unwrap().describe(), "Muting");
        assert_eq!(p("set_volume", json!({"level":30})).unwrap().describe(), "Volume 30%");
    }
}
