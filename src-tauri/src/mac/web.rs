//! Web pages in Safari and Chromium browsers (Chrome, Arc, Brave, Edge, Vivaldi).
//!
//! * The page is read and operated by a fixed script (`PAGE`) run in the front tab through the
//!   browser's own "JavaScript from Apple Events" bridge, which the user turns on once per browser.
//! * Values from the model reach the script only as one JSON argument, and the AppleScript that
//!   carries it is a constant per browser: no value can become code.
//! * Items are numbered with a per-request attribute name, so numbers from an earlier request or
//!   planted by the page itself never match.
//! * Password, card-number and one-time-code fields are never read or filled.
use super::policy::{Need, Target};
use super::ui::{self, RunningApp};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

macro_rules! chromium {
    ($id:literal) => {
        (
            $id,
            concat!("on run argv\ntell application id \"", $id, "\"\nif (count of windows) is 0 then error \"no window is open\"\nreturn execute active tab of front window javascript (item 1 of argv)\nend tell\nend run"),
            concat!("tell application id \"", $id, "\"\nreturn URL of active tab of front window\nend tell"),
        )
    };
}

macro_rules! safari {
    ($id:literal) => {
        (
            $id,
            concat!("on run argv\ntell application id \"", $id, "\"\nif (count of windows) is 0 then error \"no window is open\"\nreturn do JavaScript (item 1 of argv) in current tab of front window\nend tell\nend run"),
            concat!("tell application id \"", $id, "\"\nreturn URL of current tab of front window\nend tell"),
        )
    };
}

/// (bundle id, run-JavaScript script, front-tab URL script).
const BROWSERS: &[(&str, &str, &str)] = &[
    chromium!("com.google.Chrome"),
    chromium!("com.google.Chrome.beta"),
    chromium!("company.thebrowser.Browser"),
    chromium!("com.brave.Browser"),
    chromium!("com.microsoft.edgemac"),
    chromium!("com.vivaldi.Vivaldi"),
    chromium!("org.chromium.Chromium"),
    safari!("com.apple.Safari"),
    safari!("com.apple.SafariTechnologyPreview"),
];

pub fn is_browser(bundle: &str) -> bool {
    BROWSERS.iter().any(|b| b.0 == bundle)
}

fn scripts(app: &RunningApp) -> Result<&'static (&'static str, &'static str, &'static str), String> {
    BROWSERS.iter().find(|b| b.0 == app.bundle).ok_or_else(|| {
        format!("{} isn't a browser this works with (Safari, Chrome, Arc, Brave, Edge, Vivaldi); use read_window and click instead", app.name)
    })
}

const PAGE: &str = include_str!("page.js");

/// Runs `PAGE` with `args` in the app's front tab; returns its JSON answer.
fn run(app: &RunningApp, args: Value) -> Result<Value, String> {
    run_within(app, args, Duration::from_secs(20))
}

fn run_within(app: &RunningApp, args: Value, timeout: Duration) -> Result<Value, String> {
    let (_, script, _) = scripts(app)?;
    let js = format!("{PAGE}({args})");
    let out = super::exec::run_program(super::exec::OSASCRIPT, &["-e".as_ref(), script.as_ref(), "--".as_ref(), js.as_ref()], timeout).map_err(|e| friendly(&e, app))?;
    let v: Value = serde_json::from_str(&out).map_err(|_| format!("{} gave no answer from the page; is a web page open in its front window?", app.name))?;
    match v.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}

fn friendly(e: &str, app: &RunningApp) -> String {
    if e.contains("JavaScript") && (e.contains("turned off") || e.contains("Allow JavaScript") || e.contains("Apple Events") || e.contains("AppleScript")) {
        let how = if app.bundle.contains("Safari") {
            "in Safari, choose Develop › Allow JavaScript from Apple Events (no Develop menu? turn on Settings › Advanced › Show features for web developers)"
        } else {
            "in the browser's menu bar, choose View › Developer › Allow JavaScript from Apple Events"
        };
        return format!("{} doesn't let Xpu Command work on pages yet. The user can allow it once: {how}. Until then, use read_window and click.", app.name);
    }
    if e.contains("no window is open") {
        return format!("{} has no window open", app.name);
    }
    super::exec::friendly(e, &app.name)
}

/// The front tab's address, without running anything on the page.
pub fn url(app: &RunningApp) -> Option<String> {
    let (_, _, script) = scripts(app).ok()?;
    let out = super::exec::run_program(super::exec::OSASCRIPT, &["-e".as_ref(), script.as_ref()], Duration::from_secs(3)).ok()?;
    let u = out.trim();
    (u.starts_with("http://") || u.starts_with("https://")).then(|| crate::agents::clip(u, 300))
}

/// The page's selected text and address, for the "this" context.
pub fn selection(app: &RunningApp) -> Option<(String, String)> {
    let v = run_within(app, json!({ "op": "context", "nonce": "" }), Duration::from_secs(3)).ok()?;
    Some((v["url"].as_str()?.to_string(), v["text"].as_str().unwrap_or("").to_string()))
}

#[derive(Deserialize)]
struct Read {
    url: String,
    title: String,
    lines: Vec<String>,
    items: Vec<Item>,
    more: bool,
    frames: u32,
}

#[derive(Deserialize)]
struct Item {
    id: u32,
    kind: String,
    label: String,
}

struct Entry {
    pid: i32,
    bundle: String,
    label: String,
}

/// One request's numbered page items.
pub struct Pages {
    nonce: String,
    items: Mutex<Vec<Entry>>,
}

impl Default for Pages {
    fn default() -> Self {
        Self { nonce: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(), items: Mutex::new(Vec::new()) }
    }
}

const MAX_ITEMS: usize = 6000;

fn host(url: &str) -> String {
    reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default()
}

impl Pages {
    pub fn read(&self, app: &RunningApp) -> Result<String, String> {
        let start = self.items.lock().len() + 1;
        if start > MAX_ITEMS {
            return Err("too many page reads in one request".into());
        }
        let v = run(app, json!({ "op": "read", "nonce": self.nonce, "start": start, "maxChars": 15000, "maxLines": 600 }))?;
        let r: Read = serde_json::from_value(v).map_err(|e| format!("unexpected answer from the page: {e}"))?;
        {
            let mut items = self.items.lock();
            for (i, it) in r.items.iter().enumerate() {
                if it.id as usize != start + i {
                    return Err("the page changed while it was read; read it again".into());
                }
                items.push(Entry { pid: app.pid, bundle: app.bundle.clone(), label: format!("{} {}", it.kind, it.label).trim().to_string() });
            }
        }
        let mut out = format!("{} — “{}” — {}\n", app.name, crate::agents::clip(&r.title, 120), crate::agents::clip(&r.url, 300));
        for l in &r.lines {
            out.push_str(&clean(l));
            out.push('\n');
        }
        if r.lines.is_empty() {
            out.push_str("(the page shows nothing readable yet: wait, then read again)\n");
        }
        if r.more {
            out.push_str("… (the page is long; the rest is not shown)\n");
        }
        if r.frames > 0 {
            out.push_str("(parts of the page in embedded frames aren't shown; read_window may see them)\n");
        }
        Ok(out)
    }

    fn entry(&self, id: u32) -> Result<(i32, String, String), String> {
        let items = self.items.lock();
        let e = items.get((id as usize).wrapping_sub(1)).ok_or_else(|| format!("there is no page item [{id}]; call read_page first"))?;
        Ok((e.pid, e.bundle.clone(), e.label.clone()))
    }

    /// The live browser for an item: the same process that showed it.
    fn app_of(&self, id: u32) -> Result<(RunningApp, String), String> {
        let (pid, bundle, label) = self.entry(id)?;
        let app = ui::app_of_pid(pid).filter(|a| a.bundle == bundle).ok_or("that browser has quit")?;
        Ok((app, label))
    }

    /// For the gate: the browser, and what the item is right now (and, when submitting, the form's
    /// submit button, so "Send" or "Pay" needs a yes).
    pub fn target(&self, id: u32, need: Need, submit: bool) -> Result<Target, String> {
        let (app, _) = self.app_of(id)?;
        let v = run(&app, json!({ "op": "describe", "nonce": self.nonce, "id": id, "submit": submit }))?;
        if v["gone"] == true {
            return Err("that page item is gone; read the page again".into());
        }
        let mut label = format!("{} {}", v["kind"].as_str().unwrap_or(""), v["label"].as_str().unwrap_or("")).trim().to_string();
        if let Some(b) = v["submit"].as_str().filter(|b| !b.is_empty()) {
            label = format!("{label}, then {b}");
        }
        Ok(Target { frame: ui::window_frame(app.pid), bundle: app.bundle, name: app.name, pid: app.pid, need, label: Some(clean(&label)), unknown_spot: false })
    }

    pub fn click(&self, id: u32, kind: super::tools::ClickKind) -> Result<String, String> {
        let (app, label) = self.app_of(id)?;
        let how = match kind {
            super::tools::ClickKind::Left => "left",
            super::tools::ClickKind::Double => "double",
            super::tools::ClickKind::Right => "right",
            super::tools::ClickKind::Hover => "hover",
        };
        let v = run(&app, json!({ "op": "click", "nonce": self.nonce, "id": id, "how": how }))?;
        if v["gone"] == true {
            return Err("that page item is gone; read the page again".into());
        }
        let verb = match kind {
            super::tools::ClickKind::Hover => "Pointed at",
            super::tools::ClickKind::Right => "Right-clicked",
            super::tools::ClickKind::Double => "Double-clicked",
            super::tools::ClickKind::Left => "Clicked",
        };
        Ok(format!("{verb} {label} on {}. If the page changes, wait, then read_page again.", host(v["url"].as_str().unwrap_or(""))))
    }

    pub fn fill(&self, id: u32, text: &str, submit: bool) -> Result<String, String> {
        let (app, label) = self.app_of(id)?;
        let v = run(&app, json!({ "op": "fill", "nonce": self.nonce, "id": id, "text": text, "submit": submit }))?;
        if v["gone"] == true {
            return Err("that page item is gone; read the page again".into());
        }
        if v["secret"] == true {
            return Err("that is a password or card field: the assistant never fills those".into());
        }
        if let Some(options) = v["options"].as_array() {
            let names: Vec<&str> = options.iter().filter_map(Value::as_str).collect();
            return Err(format!("no option “{text}”. Options: {}", names.join(", ")));
        }
        if v["field"] == false {
            return Err(format!("{label} isn't something to fill; use click_page"));
        }
        let what = v["chose"].as_str().map(|c| format!("Chose “{c}” in {label}")).unwrap_or_else(|| format!("Filled {label} with {} characters", text.chars().count()));
        Ok(format!("{what}{}", if submit { " and submitted it" } else { "" }))
    }

    /// Whether the page has loaded and shows `text` (None: just loaded).
    pub fn probe(&self, app: &RunningApp, text: Option<&str>) -> Result<bool, String> {
        let v = run(app, json!({ "op": "probe", "nonce": self.nonce, "text": text }))?;
        Ok(v["ready"] == true && v["found"] == true)
    }
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| !crate::sanitize::is_dangerous(*c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_constants_per_browser() {
        assert!(is_browser("com.google.Chrome") && is_browser("com.apple.Safari"));
        assert!(!is_browser("org.mozilla.firefox"));
        for (id, js, url) in BROWSERS {
            assert!(js.contains(&format!("application id \"{id}\"")) && js.contains("(item 1 of argv)"));
            assert!(url.contains("URL of"));
        }
        let ff = RunningApp { pid: 1, bundle: "org.mozilla.firefox".into(), name: "Firefox".into() };
        assert!(run(&ff, json!({})).unwrap_err().contains("use read_window"));
    }

    #[test]
    fn the_page_script_is_one_function_taking_json() {
        let js = PAGE.trim();
        assert!(js.starts_with("(function (A) {") && js.ends_with("})"));
        let call = format!("{PAGE}({})", json!({ "op": "fill", "text": "\"); alert(1); (\"" }));
        assert!(call.contains(r#""text":"\"); alert(1); (\"""#), "values stay inside a JSON string");
    }

    #[test]
    fn friendly_messages_say_how_to_turn_it_on() {
        let chrome = RunningApp { pid: 1, bundle: "com.google.Chrome".into(), name: "Chrome".into() };
        let safari = RunningApp { pid: 1, bundle: "com.apple.Safari".into(), name: "Safari".into() };
        let m = friendly("execution error: Executing JavaScript through AppleScript is turned off. (12)", &chrome);
        assert!(m.contains("View › Developer › Allow JavaScript from Apple Events"), "{m}");
        let m = friendly("You must enable the 'Allow JavaScript from Apple Events' option in Safari's Develop menu to use 'do JavaScript'.", &safari);
        assert!(m.contains("Develop › Allow JavaScript"), "{m}");
        assert_eq!(host("https://mail.google.com/mail/u/0/#inbox"), "mail.google.com");
    }
}
