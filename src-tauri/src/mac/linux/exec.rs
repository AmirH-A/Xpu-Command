//! Runs validated calls on Linux. Helper programs are found only in the system's program folders
//! and started without a shell; values from the model are passed as single arguments (after "--"
//! where the program takes options), so no value can ever become an option or code.
//!
//! The Linux counterparts of the Mac's apps: media through MPRIS, volume through PipeWire or
//! PulseAudio, dark mode through the desktop's settings, reminders as notifications Xpu Command
//! keeps, events as calendar files the calendar app imports, notes as Markdown files, email drafts
//! through the mail app's mailto: handler, and "Shortcuts" as the user's own scripts in
//! ~/.config/xpu-command/shortcuts.
use super::apps;
use super::tools::{Call, MediaAction, Player};
use super::ui;
use super::undo::{self, Step};
use chrono::NaiveDateTime;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use super::runner::MacExec;

pub(crate) use crate::linux::run_program;

fn entry_of(app: &apps::App) -> Result<crate::linux::desktop::Entry, String> {
    crate::linux::desktop::cached().iter().find(|e| e.path == app.path).cloned().ok_or_else(|| format!("{} can't be started", app.name))
}

/// The fixed actions (apps, links, media, reminders…): no window involved.
pub(super) fn run_action(call: &Call, request: u64) -> Result<String, String> {
    match call {
        Call::ListApps => {
            let names: Vec<String> = apps::installed().into_iter().map(|a| a.name).take(400).collect();
            Ok(names.join(", "))
        }
        Call::OpenApp { name } => {
            let all = apps::installed();
            let app = apps::find(&all, name)?;
            let entry = entry_of(app)?;
            let was_running = ui::apps_on_screen().iter().any(|a| a.bundle == entry.id);
            crate::linux::desktop::launch(&entry, &[])?;
            if !was_running {
                undo::push(request, format!("closed {}", app.name), Step::QuitApp { name: app.name.clone() });
            }
            match ui::wait_for_window(&entry.id, Duration::from_secs(8)) {
                Some(running) => match ui::activate(&running) {
                    Ok(()) => Ok(format!("Opened {}; it is in front", app.name)),
                    Err(_) => Ok(format!("Opened {}, but another app stayed in front", app.name)),
                },
                None => Ok(format!("Opened {}; it has no window yet (it may still be starting)", app.name)),
            }
        }
        Call::QuitApp { name } => {
            let all = apps::installed();
            let app = apps::find(&all, name)?;
            if !quit(app)? {
                return Ok(format!("{} wasn't running", app.name));
            }
            undo::push(request, format!("reopened {}", app.name), Step::OpenApp { name: app.name.clone(), path: app.path.clone() });
            Ok(format!("Asked {} to quit", app.name))
        }
        Call::OpenUrl { url } => {
            crate::linux::open(url.as_str().as_ref())?;
            Ok(format!("Opened {}", url.host_str().unwrap_or("the page")))
        }
        Call::WebSearch { query } => {
            let u = reqwest::Url::parse_with_params("https://www.google.com/search", &[("q", query.as_str())]).map_err(|e| e.to_string())?;
            crate::linux::open(u.as_str().as_ref())?;
            Ok(format!("Searched the web for \"{query}\""))
        }
        Call::Media { action, player } => {
            let a = match action {
                MediaAction::Play => "Play",
                MediaAction::Pause => "Pause",
                MediaAction::Next => "Next",
                MediaAction::Previous => "Previous",
            };
            let p = pick_player(*player);
            let bus = player_bus(p).ok_or_else(|| match p {
                Player::Spotify => "Spotify isn't running; open it first (open_app)".to_string(),
                Player::Music => "no music app is playing or paused".to_string(),
            })?;
            let shown = crate::linux::mpris::identity(&bus);
            crate::linux::mpris::command(&bus, a)?;
            match action {
                MediaAction::Play => undo::push(request, format!("paused {shown}"), Step::Media { play: false, player: p }),
                MediaAction::Pause => undo::push(request, format!("resumed {shown}"), Step::Media { play: true, player: p }),
                MediaAction::Next | MediaAction::Previous => {}
            }
            std::thread::sleep(Duration::from_millis(250));
            let now = crate::linux::mpris::now_playing(&bus);
            let a = a.to_lowercase();
            Ok(if now.is_empty() { format!("{shown}: {a}") } else { format!("{shown}: {a}. Now: {now}") })
        }
        Call::PlayMusic { query, player } => match pick_player(*player) {
            Player::Spotify => {
                let u = spotify_search(query);
                match player_bus(Player::Spotify) {
                    Some(bus) if crate::linux::mpris::open_uri(&bus, &u).is_ok() => {}
                    _ => crate::linux::open(u.as_ref())?,
                }
                Ok(format!("Opened Spotify search for \"{query}\"; the user picks what to play"))
            }
            Player::Music => Err("play_music needs Spotify on Linux; other players can be played and paused with media".into()),
        },
        Call::SetVolume { level, mute } => {
            let before = crate::linux::system::volume().ok();
            let v = crate::linux::system::set_volume(*level, *mute)?;
            if let Some((level, muted)) = before {
                let what = format!("set the volume back to {level}%{}", if muted { ", muted" } else { "" });
                undo::push(request, what, Step::Volume { level, muted });
            }
            Ok(if *mute == Some(true) { "Muted".into() } else { format!("Volume is {v}%") })
        }
        Call::DarkMode { on } => {
            let before = crate::linux::system::dark_mode().ok();
            crate::linux::system::set_dark_mode(*on)?;
            if let Some(was) = before.filter(|was| was != on) {
                undo::push(request, format!("turned dark mode {}", if was { "back on" } else { "back off" }), Step::DarkMode { on: was });
            }
            Ok(format!("Dark mode {}", if *on { "on" } else { "off" }))
        }
        Call::ListShortcuts => {
            let all = shortcuts()?;
            if all.is_empty() {
                return Ok(format!("The user has no shortcuts yet. They are the user's own scripts, put in {}.", show_home(&shortcuts_dir()?)));
            }
            Ok(all.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "))
        }
        Call::RunShortcut { name } => {
            let all = shortcuts()?;
            let (exact, path) = all.iter().find(|s| s.0.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no shortcut is called \"{name}\". Call list_shortcuts to see the names."))?;
            run_program(&path.to_string_lossy(), &[], Duration::from_secs(60))?;
            Ok(format!("Ran the shortcut \"{exact}\""))
        }
        Call::CreateReminder { title, due, notes } => {
            let id = crate::linux::notify::add(title, notes.as_deref().unwrap_or(""), *due)?;
            match (id, due) {
                (Some(id), Some(d)) => {
                    undo::push(request, format!("deleted the reminder \"{title}\""), Step::DeleteReminder { id });
                    Ok(format!("Created the reminder for {}; Xpu Command shows it as a notification then", d.format("%A %-d %B, %H:%M")))
                }
                _ => Ok("Showed the reminder as a notification now (it had no time)".into()),
            }
        }
        Call::CreateEvent { title, start, end, location } => {
            let file = event_file(title, *start, *end, location.as_deref())?;
            crate::linux::open(file.as_os_str())?;
            Ok("Opened the event in the calendar app; the user adds it there".into())
        }
        Call::CreateNote { title, body } => {
            let p = write_note(title, body)?;
            undo::push(request, format!("deleted the note \"{title}\""), Step::DeleteNote { id: p.to_string_lossy().to_string() });
            Ok(format!("Created the note {}", show_home(&p)))
        }
        Call::DraftEmail { to, subject, body } => {
            crate::linux::open(mailto(to, subject, body).as_ref())?;
            Ok("Opened the draft in the mail app; the user reviews and sends it".into())
        }
        other => Err(format!("{} needs a window", other.tool())),
    }
}

/// Asks a running app to quit, as its own Quit command would (the app may still ask to save).
/// False if it wasn't running.
fn quit(app: &apps::App) -> Result<bool, String> {
    let entry = entry_of(app)?;
    let running: Vec<ui::RunningApp> = ui::apps_on_screen().into_iter().filter(|a| a.bundle == entry.id).collect();
    if running.is_empty() {
        return Ok(false);
    }
    if crate::linux::session() == crate::linux::Session::X11 {
        let windows: Vec<u32> = crate::linux::x11::windows().into_iter().filter(|w| running.iter().any(|a| a.pid == w.1)).map(|w| w.0).collect();
        if !windows.is_empty() {
            for w in windows {
                crate::linux::x11::close(w)?;
            }
            return Ok(true);
        }
    }
    // Wayland: the app's own Quit menu command, else Ctrl+Q in it.
    for a in &running {
        if quit_from_menu(a.pid) {
            return Ok(true);
        }
    }
    let a = &running[0];
    ui::activate(a)?;
    crate::linux::keys::press(crate::linux::keys::Key::Char('q'), &[crate::linux::keys::Mod::Ctrl])?;
    Ok(true)
}

fn quit_from_menu(pid: i32) -> bool {
    use crate::linux::atspi::{self, role};
    let Some(app) = atspi::applications().into_iter().find(|a| a.pid() == Some(pid)) else { return false };
    let item = atspi::find(&app, 3000, |e| {
        e.role() == Some(role::MENU_ITEM) && e.name().is_some_and(|n| matches!(n.trim().trim_start_matches('_').to_lowercase().as_str(), "quit" | "exit"))
    });
    item.is_some_and(|i| i.do_action("click").or_else(|_| i.do_action("activate")).is_ok())
}

pub fn undo_last() -> String {
    let (mut undone, mut failed) = (Vec::new(), Vec::new());
    for e in undo::take_last_request() {
        match revert(&e.step) {
            Ok(()) => undone.push(e.what),
            Err(err) => {
                log::info!("undo {:?}: {err}", e.step);
                failed.push(e.what);
            }
        }
    }
    undo::summary(&undone, &failed)
}

fn revert(step: &Step) -> Result<(), String> {
    match step {
        Step::QuitApp { name } => {
            let all = apps::installed();
            let app = apps::find(&all, name)?;
            quit(app).map(drop)
        }
        Step::OpenApp { path, .. } => {
            let e = crate::linux::desktop::cached().iter().find(|e| e.path == *path).cloned().ok_or("that app is gone")?;
            crate::linux::desktop::launch(&e, &[])
        }
        Step::Volume { level, muted } => crate::linux::system::set_volume(Some(*level), Some(*muted)).map(drop),
        Step::DarkMode { on } => crate::linux::system::set_dark_mode(*on),
        Step::Media { play, player } => {
            let bus = player_bus(*player).ok_or("the player isn't running")?;
            crate::linux::mpris::command(&bus, if *play { "Play" } else { "Pause" })
        }
        Step::DeleteReminder { id } => crate::linux::notify::remove(id),
        Step::DeleteEvent { .. } => Err("events are added by the calendar app; remove it there".into()),
        Step::DeleteNote { id } => {
            let p = Path::new(id);
            if !p.exists() {
                return Ok(());
            }
            crate::linux::trash::trash(p).map(drop)
        }
        Step::ForgetFact { id } => crate::memory::remove(id).map(drop).map_err(|e| e.to_string()),
        Step::RememberFact { text } => crate::memory::add(text).map(drop).map_err(|e| e.to_string()),
        Step::DeleteRoutine { id } => crate::routines::remove(id).map(drop).map_err(|e| e.to_string()),
        Step::Clipboard { text } => set_clipboard(text.as_deref()),
        Step::MoveBack { from, to } => super::files::move_back(from, to),
        Step::RemoveFolder { path } => super::files::remove_folder(path),
    }
}

pub enum Clip {
    Text(String),
    /// Marked by a password manager: never read.
    Secret,
    Empty,
}

pub fn clipboard() -> Result<Clip, String> {
    Ok(match crate::linux::clipboard::read()? {
        crate::linux::clipboard::Content::Text(s) => Clip::Text(s),
        crate::linux::clipboard::Content::Secret => Clip::Secret,
        crate::linux::clipboard::Content::Empty => Clip::Empty,
    })
}

/// Puts text on the clipboard (None: clears it).
pub fn set_clipboard(text: Option<&str>) -> Result<(), String> {
    crate::linux::clipboard::write(text)
}

pub fn music_key(key: crate::airpods::MusicKey) -> Result<String, String> {
    crate::linux::mpris::music_key(key)
}

/// Spotify if it was asked for or is installed; otherwise whatever player is running.
fn pick_player(p: Option<Player>) -> Player {
    if let Some(p) = p {
        return p;
    }
    let has_spotify = apps::installed().iter().any(|a| a.name.eq_ignore_ascii_case("Spotify")) || crate::linux::mpris::players().iter().any(|n| n.contains("spotify"));
    if has_spotify {
        Player::Spotify
    } else {
        Player::Music
    }
}

fn player_bus(p: Player) -> Option<String> {
    match p {
        Player::Spotify => crate::linux::mpris::pick(Some("spotify")),
        Player::Music => crate::linux::mpris::pick(None),
    }
}

/// A Spotify search link; the query is fully percent-encoded, so it stays one search term.
fn spotify_search(query: &str) -> String {
    format!("spotify:search:{}", percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC))
}

/// A mailto: link (RFC 6068) with every part percent-encoded; it opens a draft, never sends.
pub fn mailto(to: &[String], subject: &str, body: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    const ADDRESS: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC.remove(b'@').remove(b'.').remove(b'-').remove(b'_');
    let to: Vec<String> = to.iter().map(|a| utf8_percent_encode(a, ADDRESS).to_string()).collect();
    let body = body.replace("\r\n", "\n").replace('\n', "\r\n");
    format!("mailto:{}?subject={}&body={}", to.join(","), enc(subject), enc(&body))
}

fn shortcuts_dir() -> Result<PathBuf, String> {
    let d = crate::settings::config_dir().map_err(|e| e.to_string())?.join("shortcuts");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

/// The user's shortcut scripts: executable files they put in the shortcuts folder, by name
/// (without extension).
fn shortcuts() -> Result<Vec<(String, PathBuf)>, String> {
    use std::os::unix::fs::PermissionsExt;
    let mut out = Vec::new();
    for e in std::fs::read_dir(shortcuts_dir()?).map_err(|e| e.to_string())?.flatten().take(300) {
        let p = e.path();
        let Ok(m) = std::fs::metadata(&p) else { continue };
        let name = p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if m.is_file() && m.permissions().mode() & 0o111 != 0 && !name.starts_with('.') && !name.is_empty() {
            out.push((name, p));
        }
    }
    out.sort();
    Ok(out)
}

fn show_home(p: &Path) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rel) => format!("~/{}", rel.display()),
        None => p.display().to_string(),
    }
}

/// iCalendar text escaping (RFC 5545): backslash, semicolon, comma, line breaks.
pub fn ics_text(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace("\r\n", "\\n").replace('\n', "\\n")
}

pub fn ics(uid: &str, title: &str, start: NaiveDateTime, end: NaiveDateTime, location: Option<&str>, stamp: chrono::DateTime<chrono::Utc>) -> String {
    let t = |d: NaiveDateTime| d.format("%Y%m%dT%H%M%S").to_string();
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//Xpu Command//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{uid}@xpucommand"),
        format!("DTSTAMP:{}", stamp.format("%Y%m%dT%H%M%SZ")),
        format!("DTSTART:{}", t(start)),
        format!("DTEND:{}", t(end)),
        format!("SUMMARY:{}", ics_text(title)),
    ];
    if let Some(l) = location.filter(|l| !l.is_empty()) {
        lines.push(format!("LOCATION:{}", ics_text(l)));
    }
    lines.extend(["END:VEVENT".to_string(), "END:VCALENDAR".into()]);
    lines.join("\r\n") + "\r\n"
}

/// The event as a calendar file in the private config folder (old ones are cleared).
fn event_file(title: &str, start: NaiveDateTime, end: NaiveDateTime, location: Option<&str>) -> Result<PathBuf, String> {
    let dir = crate::settings::config_dir().map_err(|e| e.to_string())?.join("events");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(86_400)) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let p = dir.join(format!("{uid}.ics"));
    std::fs::write(&p, ics(&uid, title, start, end, location, chrono::Utc::now())).map_err(|e| e.to_string())?;
    Ok(p)
}

/// A file name from a title: no path separators, control characters or leading dots.
pub fn note_name(title: &str) -> String {
    let n: String = title.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { ' ' } else { c }).collect();
    let n = n.split_whitespace().filter(|w| !w.chars().all(|c| c == '.')).collect::<Vec<_>>().join(" ");
    let n = n.trim_start_matches('.').trim().chars().take(80).collect::<String>();
    if n.is_empty() {
        "Note".into()
    } else {
        n
    }
}

/// A Markdown note in ~/Documents/Notes, never over an existing file.
fn write_note(title: &str, body: &str) -> Result<PathBuf, String> {
    let docs = dirs::document_dir().or_else(|| dirs::home_dir().map(|h| h.join("Documents"))).ok_or("no Documents folder")?;
    let dir = docs.join("Notes");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let base = note_name(title);
    for n in 1..500 {
        let name = if n == 1 { format!("{base}.md") } else { format!("{base} {n}.md") };
        let p = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(format!("# {title}\n\n{body}\n").as_bytes()).map_err(|e| e.to_string())?;
                return Ok(p);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("too many notes with that title".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_links_are_encoded_and_never_send() {
        let m = mailto(&["sara@example.com".into(), "a+b@x.org".into()], "Re: invoice & dates", "Hi Sara,\nsee you?");
        assert_eq!(m, "mailto:sara@example.com,a%2Bb@x.org?subject=Re%3A%20invoice%20%26%20dates&body=Hi%20Sara%2C%0D%0Asee%20you%3F");
        assert!(!m.to_lowercase().contains("send"));
    }

    #[test]
    fn calendar_files_follow_rfc_5545() {
        let d = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap();
        let stamp = chrono::DateTime::parse_from_rfc3339("2026-11-18T09:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let s = ics("u1", "Lunch; with Sara, Ali", d("2026-11-20T12:30"), d("2026-11-20T13:30"), Some("Café\nCorner"), stamp);
        assert!(s.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert!(s.contains("\r\nDTSTART:20261120T123000\r\nDTEND:20261120T133000\r\n"));
        assert!(s.contains("\r\nSUMMARY:Lunch\\; with Sara\\, Ali\r\n"));
        assert!(s.contains("\r\nLOCATION:Café\\nCorner\r\n"));
        assert!(s.contains("DTSTAMP:20261118T090000Z") && s.ends_with("END:VCALENDAR\r\n"));
    }

    #[test]
    fn note_names_are_plain_file_names() {
        assert_eq!(note_name("Groceries / Friday"), "Groceries Friday");
        assert_eq!(note_name("../../etc"), "etc");
        assert_eq!(note_name("..."), "Note");
        assert_eq!(note_name("a\u{7}b"), "a b");
    }

    #[test]
    fn spotify_link_is_encoded() {
        assert_eq!(spotify_search("lo-fi & chill"), "spotify:search:lo%2Dfi%20%26%20chill");
    }
}
