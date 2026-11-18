//! Desktop notifications (org.freedesktop.Notifications) and the reminders that use them.
//!
//! Linux has no system-wide reminders app, so a reminder is kept by Xpu Command itself (a private
//! file, mode 600) and shown as a notification at its time, also after a restart. Reminders
//! without a time are shown right away, so nothing is saved silently.
use chrono::NaiveDateTime;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::Value;

/// Shows a notification; returns its id.
pub fn show(summary: &str, body: &str, urgent: bool) -> Result<u32, String> {
    let c = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
    let hints: HashMap<&str, Value> = HashMap::from([("urgency", Value::from(if urgent { 2u8 } else { 1u8 })), ("desktop-entry", Value::from("xpu-command"))]);
    let actions: Vec<&str> = Vec::new();
    let r = c
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &("Xpu Command", 0u32, "alarm-symbolic", summary, body, actions, hints, if urgent { 0i32 } else { -1i32 }),
        )
        .map_err(|e| format!("no notifications on this desktop: {e}"))?;
    r.body().deserialize::<u32>().map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    /// Local time, YYYY-MM-DDTHH:MM.
    pub due: Option<String>,
    #[serde(default)]
    pub shown: bool,
}

const MAX: usize = 500;
static LOCK: Mutex<()> = Mutex::new(());

fn file() -> Result<std::path::PathBuf, String> {
    Ok(crate::settings::config_dir().map_err(|e| e.to_string())?.join("reminders.json"))
}

fn load() -> Vec<Reminder> {
    file().ok().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(list: &[Reminder]) -> Result<(), String> {
    let f = file()?;
    let tmp = f.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut o = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        o.write_all(&serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        o.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &f).map_err(|e| e.to_string())
}

const FMT: &str = "%Y-%m-%dT%H:%M";

/// Adds a reminder; returns its id (None for one without a time: it is shown now, not kept).
pub fn add(title: &str, notes: &str, due: Option<NaiveDateTime>) -> Result<Option<String>, String> {
    let Some(due) = due else {
        show(&format!("Reminder: {title}"), notes, false)?;
        return Ok(None);
    };
    let _g = LOCK.lock();
    let mut list = load();
    list.retain(|r| !r.shown);
    if list.len() >= MAX {
        return Err("too many reminders are waiting".into());
    }
    let r = Reminder { id: uuid::Uuid::new_v4().simple().to_string(), title: title.into(), notes: notes.into(), due: Some(due.format(FMT).to_string()), shown: false };
    list.push(r.clone());
    save(&list)?;
    start();
    Ok(Some(r.id))
}

pub fn remove(id: &str) -> Result<(), String> {
    let _g = LOCK.lock();
    let mut list = load();
    let n = list.len();
    list.retain(|r| r.id != id);
    if list.len() == n {
        return Err("that reminder is gone".into());
    }
    save(&list)
}

/// The reminders due at `now` that haven't been shown (missed ones from up to a day ago too).
pub fn due(list: &[Reminder], now: NaiveDateTime) -> Vec<usize> {
    list.iter()
        .enumerate()
        .filter(|(_, r)| !r.shown)
        .filter_map(|(i, r)| {
            let d = NaiveDateTime::parse_from_str(r.due.as_deref()?, FMT).ok()?;
            (d <= now && now - d < chrono::Duration::hours(24)).then_some(i)
        })
        .collect()
}

/// Starts the reminder clock (once).
pub fn start() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new().name("xpu-reminders".into()).spawn(|| loop {
            tick();
            std::thread::sleep(Duration::from_secs(15));
        });
    });
}

fn tick() {
    let _g = LOCK.lock();
    let mut list = load();
    let now = chrono::Local::now().naive_local();
    let hits = due(&list, now);
    if hits.is_empty() {
        return;
    }
    for i in hits {
        let r = &mut list[i];
        match show(&format!("Reminder: {}", r.title), &r.notes, true) {
            Ok(_) => r.shown = true,
            Err(e) => log::warn!("reminder: {e}"),
        }
    }
    // Shown reminders with a time are done; keep the file small.
    list.retain(|r| !(r.shown && r.due.is_some()));
    if let Err(e) = save(&list) {
        log::warn!("reminders: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, FMT).unwrap()
    }

    #[test]
    fn due_reminders() {
        let r = |id: &str, due: Option<&str>, shown: bool| Reminder { id: id.into(), title: id.into(), notes: String::new(), due: due.map(String::from), shown };
        let list = vec![
            r("past", Some("2026-11-18T09:00"), false),
            r("future", Some("2026-11-18T11:00"), false),
            r("shown", Some("2026-11-18T09:30"), true),
            r("undated", None, false),
            r("stale", Some("2026-11-16T09:00"), false),
        ];
        assert_eq!(due(&list, at("2026-11-18T10:00")), [0], "only unshown, due, and within a day");
        assert_eq!(due(&list, at("2026-11-18T11:00")), [0, 1]);
    }
}
