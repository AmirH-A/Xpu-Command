//! Runs validated calls on macOS. Programs are started by absolute path and without a shell:
//! /usr/bin/open, /usr/bin/osascript and /usr/bin/shortcuts (and, for files, mdfind and textutil).
//! AppleScripts are constants. Values from the model are passed as arguments after "--" and read
//! with `item n of argv`, so no value can ever become script code.
use super::apps;
use super::tools::{Call, MediaAction, Player};
use super::ui;
use super::undo::{self, Step};
use chrono::{Datelike, NaiveDateTime, Timelike};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub use super::runner::MacExec;

const OPEN: &str = "/usr/bin/open";
pub(crate) const OSASCRIPT: &str = "/usr/bin/osascript";
const SHORTCUTS: &str = "/usr/bin/shortcuts";

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
                let was_running = is_running(&app.name);
                run_program(OPEN, &["-a".as_ref(), app.path.as_os_str()], Duration::from_secs(15))?;
                if !was_running {
                    undo::push(request, format!("closed {}", app.name), Step::QuitApp { name: app.name.clone() });
                }
                // Report what really happened: open, with a window, in front.
                let Some((bundle, _)) = ui::bundle_info(&app.path) else { return Ok(format!("Opened {}", app.name)) };
                match ui::wait_for_window(&bundle, Duration::from_secs(6)) {
                    Some(running) => match ui::activate(&running) {
                        Ok(()) => Ok(format!("Opened {}; it is in front", app.name)),
                        Err(_) => Ok(format!("Opened {}, but another app stayed in front; call open_app again or click in it", app.name)),
                    },
                    None => Ok(format!("Opened {}; it has no window yet (it may still be starting)", app.name)),
                }
            }
            Call::QuitApp { name } => {
                let all = apps::installed();
                let app = apps::find(&all, name)?;
                let r = osascript(QUIT, &[&app.name], "the app")?;
                if r != "quit" {
                    return Ok(format!("{} wasn't running", app.name));
                }
                undo::push(request, format!("reopened {}", app.name), Step::OpenApp { name: app.name.clone(), path: app.path.clone() });
                Ok(format!("Quit {}", app.name))
            }
            Call::OpenUrl { url } => {
                run_program(OPEN, &[url.as_str().as_ref()], Duration::from_secs(15))?;
                Ok(format!("Opened {}", url.host_str().unwrap_or("the page")))
            }
            Call::WebSearch { query } => {
                let u = reqwest::Url::parse_with_params("https://www.google.com/search", &[("q", query.as_str())]).map_err(|e| e.to_string())?;
                run_program(OPEN, &[u.as_str().as_ref()], Duration::from_secs(15))?;
                Ok(format!("Searched the web for \"{query}\""))
            }
            Call::Media { action, player } => {
                let a = match action {
                    MediaAction::Play => "play",
                    MediaAction::Pause => "pause",
                    MediaAction::Next => "next",
                    MediaAction::Previous => "previous",
                };
                let p = pick_player(*player)?;
                let (script, app) = match p {
                    Player::Spotify => (MEDIA_SPOTIFY, "Spotify"),
                    Player::Music => (MEDIA_MUSIC, "Music"),
                };
                let now = match osascript(script, &[a], app) {
                    Ok(now) => now,
                    // Without Automation permission for the player, the media keys still work
                    // (they go to whatever is playing) and need nothing more than Accessibility.
                    Err(e) if e.contains("isn't allowed to control") => {
                        media_key(*action)?;
                        return Ok(format!("Pressed the {a} media key (Xpu Command can't ask {app} directly: {e})"));
                    }
                    Err(e) => return Err(e),
                };
                match action {
                    MediaAction::Play => undo::push(request, format!("paused {app}"), Step::Media { play: false, player: p }),
                    MediaAction::Pause => undo::push(request, format!("resumed {app}"), Step::Media { play: true, player: p }),
                    MediaAction::Next | MediaAction::Previous => {}
                }
                Ok(if now.is_empty() { format!("{app}: {a}") } else { format!("{app}: {a}. Now: {now}") })
            }
            Call::PlayMusic { query, player } => match pick_player(*player)? {
                Player::Spotify => {
                    let u = spotify_search(query);
                    run_program(OPEN, &[u.as_ref()], Duration::from_secs(15))?;
                    Ok(format!("Opened Spotify search for \"{query}\"; the user picks what to play"))
                }
                Player::Music => {
                    let r = osascript(PLAY_MUSIC, &[query], "Music")?;
                    if r == "none" {
                        Err(format!("nothing in the Music library matches \"{query}\""))
                    } else {
                        Ok(format!("Playing {r} in Music"))
                    }
                }
            },
            Call::SetVolume { level, mute } => {
                let lvl = level.map(|l| l.to_string()).unwrap_or_default();
                let mode = match (level, mute) {
                    (_, Some(true)) => "mute",
                    (None, Some(false)) => "unmute",
                    _ => "level",
                };
                let before = osascript(GET_VOLUME, &[], "the volume").ok().and_then(|b| parse_volume(&b));
                let v = osascript(VOLUME, &[mode, &lvl], "the volume")?;
                if let Some((level, muted)) = before {
                    let what = format!("set the volume back to {level}%{}", if muted { ", muted" } else { "" });
                    undo::push(request, what, Step::Volume { level, muted });
                }
                Ok(if mode == "mute" { "Muted".into() } else { format!("Volume is {v}%") })
            }
            Call::DarkMode { on } => {
                let before = osascript(GET_DARK_MODE, &[], "System Events").ok().map(|b| b == "true");
                osascript(DARK_MODE, &[if *on { "on" } else { "off" }], "System Events")?;
                if let Some(was) = before.filter(|was| was != on) {
                    undo::push(request, format!("turned dark mode {}", if was { "back on" } else { "back off" }), Step::DarkMode { on: was });
                }
                Ok(format!("Dark mode {}", if *on { "on" } else { "off" }))
            }
            Call::ListShortcuts => Ok(shortcuts()?.join(", ")),
            Call::RunShortcut { name } => {
                let all = shortcuts()?;
                let exact = all.iter().find(|s| s.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no shortcut is called \"{name}\". Call list_shortcuts to see the names."))?;
                run_program(SHORTCUTS, &["run".as_ref(), exact.as_ref()], Duration::from_secs(60))?;
                Ok(format!("Ran the shortcut \"{exact}\""))
            }
            Call::CreateReminder { title, due, notes } => {
                let mut a: Vec<String> = vec![title.clone(), notes.clone().unwrap_or_default(), if due.is_some() { "1" } else { "0" }.into()];
                a.extend(parts(due.as_ref()));
                let out = osascript(&REMINDER, &a.iter().map(String::as_str).collect::<Vec<_>>(), "Reminders")?;
                let (list, id) = split_created(&out);
                if let Some(id) = id {
                    undo::push(request, format!("deleted the reminder \"{title}\""), Step::DeleteReminder { id });
                }
                Ok(format!("Created the reminder in \"{list}\""))
            }
            Call::CreateEvent { title, start, end, location } => {
                let mut a: Vec<String> = vec![title.clone(), location.clone().unwrap_or_default()];
                a.extend(parts(Some(start)));
                a.extend(parts(Some(end)));
                let out = osascript(&EVENT, &a.iter().map(String::as_str).collect::<Vec<_>>(), "Calendar")?;
                let (cal, uid) = split_created(&out);
                if let Some(uid) = uid {
                    undo::push(request, format!("deleted the event \"{title}\""), Step::DeleteEvent { uid, calendar: cal.clone() });
                }
                Ok(format!("Created the event in the \"{cal}\" calendar"))
            }
            Call::CreateNote { title, body } => {
                let id = osascript(NOTE, &[&note_html(title, body)], "Notes")?;
                if !id.is_empty() {
                    undo::push(request, format!("deleted the note \"{title}\""), Step::DeleteNote { id });
                }
                Ok("Created the note".into())
            }
            Call::DraftEmail { to, subject, body } => {
                let mut a: Vec<&str> = vec![subject, body];
                a.extend(to.iter().map(String::as_str));
                osascript(MAIL_DRAFT, &a, "Mail")?;
                Ok("Opened the draft in Mail; the user reviews and sends it".into())
            }
            other => Err(format!("{} needs a window", other.tool())),
        }
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
        Step::QuitApp { name } => osascript(QUIT, &[name], name).map(drop),
        Step::OpenApp { path, .. } => run_program(OPEN, &["-a".as_ref(), path.as_os_str()], Duration::from_secs(15)).map(drop),
        Step::Volume { level, muted } => osascript(RESTORE_VOLUME, &[&level.to_string(), if *muted { "true" } else { "false" }], "the volume").map(drop),
        Step::DarkMode { on } => osascript(DARK_MODE, &[if *on { "on" } else { "off" }], "System Events").map(drop),
        Step::Media { play, player } => {
            let (script, app) = match player {
                Player::Spotify => (MEDIA_SPOTIFY, "Spotify"),
                Player::Music => (MEDIA_MUSIC, "Music"),
            };
            osascript(script, &[if *play { "play" } else { "pause" }], app).map(drop)
        }
        Step::DeleteReminder { id } => osascript(DELETE_REMINDER, &[id], "Reminders").map(drop),
        Step::DeleteEvent { uid, calendar } => osascript(DELETE_EVENT, &[uid, calendar], "Calendar").map(drop),
        Step::DeleteNote { id } => osascript(DELETE_NOTE, &[id], "Notes").map(drop),
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
    /// Marked concealed or transient by a password manager: never read.
    Secret,
    Empty,
}

/// Reads the clipboard's text. Password managers mark what they copy (nspasteboard.org); that is
/// reported as a secret and never read.
const CLIPBOARD_READ: &str = r#"use AppleScript version "2.4"
use framework "AppKit"
on run argv
set pb to current application's NSPasteboard's generalPasteboard()
set ts to (pb's types()) as list
repeat with t in ts
set n to t as text
if n is "org.nspasteboard.ConcealedType" or n is "org.nspasteboard.TransientType" or n is "com.agilebits.onepassword" then return "secret"
end repeat
set v to pb's stringForType:(current application's NSPasteboardTypeString)
if v is missing value then return "empty"
return "text:" & (v as text)
end run"#;

const CLIPBOARD_WRITE: &str = r#"use AppleScript version "2.4"
use framework "AppKit"
on run argv
set pb to current application's NSPasteboard's generalPasteboard()
pb's clearContents()
if (count of argv) > 0 then pb's setString:(item 1 of argv) forType:(current application's NSPasteboardTypeString)
return "ok"
end run"#;

pub fn clipboard() -> Result<Clip, String> {
    let out = osascript(CLIPBOARD_READ, &[], "the clipboard")?;
    Ok(match out.as_str() {
        "secret" => Clip::Secret,
        "empty" => Clip::Empty,
        s => match s.strip_prefix("text:") {
            Some(t) if !t.is_empty() => Clip::Text(t.to_string()),
            _ => Clip::Empty,
        },
    })
}

/// Puts text on the clipboard (None: clears it).
pub fn set_clipboard(text: Option<&str>) -> Result<(), String> {
    osascript(CLIPBOARD_WRITE, &text.into_iter().collect::<Vec<_>>(), "the clipboard").map(drop)
}

fn split_created(out: &str) -> (String, Option<String>) {
    match out.split_once('\n') {
        Some((name, id)) if !id.trim().is_empty() => (name.trim().to_string(), Some(id.trim().to_string())),
        _ => (out.trim().to_string(), None),
    }
}

fn parse_volume(s: &str) -> Option<(u8, bool)> {
    let (level, muted) = s.split_once(',')?;
    Some((level.trim().parse::<u8>().ok()?.min(100), muted.trim() == "true"))
}

/// Year, month, day, hour, minute as separate arguments: AppleScript builds the date from numbers,
/// so the user's date format and language don't matter.
fn parts(d: Option<&NaiveDateTime>) -> Vec<String> {
    match d {
        Some(d) => vec![d.year().to_string(), d.month().to_string(), d.day().to_string(), d.hour().to_string(), d.minute().to_string()],
        None => vec!["0".into(); 5],
    }
}

/// A Spotify search link; the query is fully percent-encoded, so it stays one search term.
fn spotify_search(query: &str) -> String {
    format!("spotify:search:{}", percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC))
}

pub fn music_key(key: crate::airpods::MusicKey) -> Result<String, String> {
    let action = match key {
        crate::airpods::MusicKey::Next => "next",
        crate::airpods::MusicKey::Previous => "previous",
    };
    let (script, app) = if is_running("Spotify") {
        (MEDIA_SPOTIFY, "Spotify")
    } else if is_running("Music") {
        (MEDIA_MUSIC, "Music")
    } else {
        return Err("no music app is running".into());
    };
    osascript(script, &[action], app)
}

/// Presses a media key (play/pause, next, previous) as the keyboard's media keys do. Run through
/// JavaScript for Automation, which posts the event itself: no Apple events to other apps.
fn media_key(action: MediaAction) -> Result<(), String> {
    let code = match action {
        MediaAction::Play | MediaAction::Pause => 16,
        MediaAction::Next => 17,
        MediaAction::Previous => 18,
    };
    const SCRIPT: &str = r#"ObjC.import('Cocoa'); ObjC.import('CoreGraphics');
function run(argv) {
  const code = parseInt(argv[0], 10);
  for (const down of [true, false]) {
    const flags = down ? 0xa00 : 0xb00;
    const e = $.NSEvent.otherEventWithTypeLocationModifierFlagsTimestampWindowNumberContextSubtypeData1Data2(14, $.NSMakePoint(0, 0), flags, 0, 0, $(), 8, (code << 16) | flags, -1);
    $.CGEventPost(0, e.CGEvent);
  }
  return "ok";
}"#;
    run_program(OSASCRIPT, &["-l".as_ref(), "JavaScript".as_ref(), "-e".as_ref(), SCRIPT.as_ref(), code.to_string().as_ref()], Duration::from_secs(10)).map(|_| ())
}

fn is_running(process: &str) -> bool {
    run_program("/usr/bin/pgrep", &["-xq".as_ref(), process.as_ref()], Duration::from_secs(5)).is_ok()
}

fn pick_player(p: Option<Player>) -> Result<Player, String> {
    if let Some(p) = p {
        return Ok(p);
    }
    let has_spotify = apps::installed().iter().any(|a| a.name == "Spotify");
    Ok(if has_spotify { Player::Spotify } else { Player::Music })
}

fn shortcuts() -> Result<Vec<String>, String> {
    let out = run_program(SHORTCUTS, &["list".as_ref()], Duration::from_secs(20))?;
    Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).take(300).map(str::to_string).collect())
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Notes takes HTML; the text is escaped so it shows exactly as written.
fn note_html(title: &str, body: &str) -> String {
    let lines: String = body.lines().map(|l| format!("<div>{}</div>", if l.trim().is_empty() { "<br>".to_string() } else { html_escape(l) })).collect();
    format!("<h1>{}</h1>{lines}", html_escape(title))
}

fn osascript(script: &str, args: &[&str], app: &str) -> Result<String, String> {
    let mut a: Vec<&std::ffi::OsStr> = vec!["-e".as_ref(), script.as_ref(), "--".as_ref()];
    a.extend(args.iter().map(|s| std::ffi::OsStr::new(*s)));
    run_program(OSASCRIPT, &a, Duration::from_secs(60)).map_err(|e| friendly(&e, app))
}

/// AppleScript errors the user can act on.
pub(crate) fn friendly(e: &str, app: &str) -> String {
    if e.contains("-1743") || e.contains("Not authorized to send Apple events") || e.contains("not allowed assistive access") {
        return format!("Xpu Command isn't allowed to control {app}. Allow it in System Settings › Privacy & Security › Automation.");
    }
    if e.contains("-1712") || e.contains("timed out") {
        return format!("{app} didn't answer. If macOS is asking whether Xpu Command may control {app}, click Allow, then try again.");
    }
    if e.contains("-600") || e.contains("isn't running") {
        return format!("{app} isn't running");
    }
    crate::agents::clip(e, 200)
}

/// Runs a program with a deadline; returns trimmed stdout, or a short error.
pub(crate) fn run_program(prog: &str, args: &[&std::ffi::OsStr], timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new(prog);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = {
        let _g = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        cmd.spawn().map_err(|e| format!("could not start {prog}: {e}"))?
    };
    let reader = |mut r: Box<dyn Read + Send>| std::thread::spawn(move || {
        let mut s = String::new();
        let _ = r.by_ref().take(256 * 1024).read_to_string(&mut s);
        let _ = std::io::copy(&mut r, &mut std::io::sink());
        s
    });
    let out = reader(Box::new(child.stdout.take().ok_or("no stdout")?));
    let err = reader(Box::new(child.stderr.take().ok_or("no stderr")?));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("it took too long and was stopped".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() {
        Ok(out.trim().to_string())
    } else {
        Err(if err.trim().is_empty() { format!("failed ({status})") } else { err.trim().to_string() })
    }
}


const QUIT: &str = r#"on run argv
  set appName to item 1 of argv
  if application appName is running then
    tell application appName to quit
    return "quit"
  end if
  return "not running"
end run"#;

const MEDIA_MUSIC: &str = r#"on run argv
  set a to item 1 of argv
  tell application "Music"
    if a is "play" then
      play
    else if a is "playpause" then
      playpause
    else if a is "pause" then
      pause
    else if a is "next" then
      next track
    else if a is "previous" then
      previous track
    end if
    try
      return (name of current track) & " by " & (artist of current track)
    on error
      return ""
    end try
  end tell
end run"#;

const MEDIA_SPOTIFY: &str = r#"on run argv
  set a to item 1 of argv
  tell application "Spotify"
    if a is "play" then
      play
    else if a is "playpause" then
      playpause
    else if a is "pause" then
      pause
    else if a is "next" then
      next track
    else if a is "previous" then
      previous track
    end if
    try
      return (name of current track) & " by " & (artist of current track)
    on error
      return ""
    end try
  end tell
end run"#;

const PLAY_MUSIC: &str = r#"on run argv
  set q to item 1 of argv
  tell application "Music"
    try
      set p to (first user playlist whose name is q)
      play p
      return "the playlist " & (name of p)
    end try
    set hits to (every track of library playlist 1 whose name contains q or artist contains q or album contains q)
    if (count of hits) is 0 then return "none"
    set t to item 1 of hits
    play t
    return (name of t) & " by " & (artist of t)
  end tell
end run"#;

const VOLUME: &str = r#"on run argv
  set m to item 1 of argv
  if m is "mute" then
    set volume with output muted
  else if m is "unmute" then
    set volume without output muted
  else
    set volume output volume ((item 2 of argv) as integer) without output muted
  end if
  return (output volume of (get volume settings)) as text
end run"#;

const DARK_MODE: &str = r#"on run argv
  tell application "System Events" to tell appearance preferences to set dark mode to ((item 1 of argv) is "on")
  return ""
end run"#;

const MKDATE: &str = r#"
on mkdate(p)
  set d to current date
  set day of d to 1
  set year of d to (item 1 of p) as integer
  set month of d to (item 2 of p) as integer
  set day of d to (item 3 of p) as integer
  set hours of d to (item 4 of p) as integer
  set minutes of d to (item 5 of p) as integer
  set seconds of d to 0
  return d
end mkdate"#;

const REMINDER_BODY: &str = r#"on run argv
  set t to item 1 of argv
  set n to item 2 of argv
  set hasDue to (item 3 of argv) is "1"
  if hasDue then set d to my mkdate(items 4 thru 8 of argv)
  tell application "Reminders"
    set l to default list
    tell l
      set r to make new reminder with properties {name:t}
    end tell
    if n is not "" then set body of r to n
    if hasDue then
      set due date of r to d
      set remind me date of r to d
    end if
    return (name of l) & linefeed & (id of r)
  end tell
end run"#;

const EVENT_BODY: &str = r#"on run argv
  set t to item 1 of argv
  set loc to item 2 of argv
  set s to my mkdate(items 3 thru 7 of argv)
  set e to my mkdate(items 8 thru 12 of argv)
  tell application "Calendar"
    set cals to (every calendar whose writable is true)
    if (count of cals) is 0 then error "There is no calendar that can be written to."
    set c to item 1 of cals
    tell c
      set ev to make new event with properties {summary:t, start date:s, end date:e}
    end tell
    if loc is not "" then set location of ev to loc
    return (name of c) & linefeed & (uid of ev)
  end tell
end run"#;

const NOTE: &str = r#"on run argv
  tell application "Notes"
    set n to make new note with properties {body:(item 1 of argv)}
    return id of n
  end tell
end run"#;

const GET_VOLUME: &str = r#"on run argv
  set v to get volume settings
  return ((output volume of v) as text) & "," & ((output muted of v) as text)
end run"#;

const RESTORE_VOLUME: &str = r#"on run argv
  set volume output volume ((item 1 of argv) as integer)
  if (item 2 of argv) is "true" then
    set volume with output muted
  else
    set volume without output muted
  end if
  return ""
end run"#;

const GET_DARK_MODE: &str = r#"on run argv
  tell application "System Events" to tell appearance preferences to return (dark mode) as text
end run"#;

const DELETE_REMINDER: &str = r#"on run argv
  tell application "Reminders" to delete reminder id (item 1 of argv)
  return ""
end run"#;

const DELETE_EVENT: &str = r#"on run argv
  tell application "Calendar"
    tell calendar (item 2 of argv)
      delete (every event whose uid is (item 1 of argv))
    end tell
  end tell
  return ""
end run"#;

const DELETE_NOTE: &str = r#"on run argv
  tell application "Notes" to delete note id (item 1 of argv)
  return ""
end run"#;

/// Opens a visible draft. There is deliberately no "send" anywhere in this script (tested).
const MAIL_DRAFT: &str = r#"on run argv
  set s to item 1 of argv
  set b to item 2 of argv
  tell application "Mail"
    set m to make new outgoing message with properties {subject:s, content:b, visible:true}
    tell m
      repeat with i from 3 to (count of argv)
        make new to recipient at end of to recipients with properties {address:(item i of argv)}
      end repeat
    end tell
    activate
  end tell
  return ""
end run"#;

static REMINDER: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| format!("{REMINDER_BODY}\n{MKDATE}"));
static EVENT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| format!("{EVENT_BODY}\n{MKDATE}"));

#[cfg(test)]
mod tests {
    use super::*;

    fn all_scripts() -> Vec<&'static str> {
        vec![
            QUIT, MEDIA_MUSIC, MEDIA_SPOTIFY, PLAY_MUSIC, VOLUME, DARK_MODE, &REMINDER, &EVENT, NOTE, MAIL_DRAFT, GET_VOLUME, RESTORE_VOLUME, GET_DARK_MODE, DELETE_REMINDER,
            DELETE_EVENT, DELETE_NOTE,
        ]
    }

    #[test]
    fn scripts_take_values_only_from_argv_and_never_run_code() {
        for s in all_scripts() {
            assert!(s.starts_with("on run argv"), "{s}");
            let l = s.to_lowercase();
            for banned in ["do shell script", "run script", "load script", "store script", "system attribute", "open location"] {
                assert!(!l.contains(banned), "{banned} in {s}");
            }
        }
    }

    /// Every script compiles (macOS only). Compiling sends no Apple events: nothing runs.
    #[test]
    fn scripts_compile() {
        if !cfg!(target_os = "macos") {
            return;
        }
        for (i, src) in all_scripts().into_iter().enumerate() {
            let f = std::env::temp_dir().join(format!("xpu-script-{}-{i}.applescript", uuid::Uuid::new_v4().simple()));
            std::fs::write(&f, src).unwrap();
            let r = run_program("/usr/bin/osacompile", &["-o".as_ref(), "/dev/null".as_ref(), f.as_os_str()], Duration::from_secs(30));
            let _ = std::fs::remove_file(&f);
            assert!(r.is_ok(), "script {i} does not compile: {r:?}\n{src}");
        }
    }

    #[test]
    fn created_ids_and_volume_are_parsed() {
        assert_eq!(split_created("Reminders\nx-apple-reminder://ABC"), ("Reminders".into(), Some("x-apple-reminder://ABC".into())));
        assert_eq!(split_created("Home"), ("Home".into(), None));
        assert_eq!(split_created("Home\n  "), ("Home".into(), None));
        assert_eq!(parse_volume("44,false"), Some((44, false)));
        assert_eq!(parse_volume("12,true"), Some((12, true)));
        assert_eq!(parse_volume("missing value,missing value"), None);
    }

    #[test]
    fn mail_never_sends() {
        assert!(!MAIL_DRAFT.to_lowercase().contains("send"));
        assert!(MAIL_DRAFT.contains("visible:true"));
    }

    #[test]
    fn note_text_is_escaped() {
        let h = note_html("A <b>", "x & y\n\n<script>");
        assert_eq!(h, "<h1>A &lt;b&gt;</h1><div>x &amp; y</div><div><br></div><div>&lt;script&gt;</div>");
    }

    #[test]
    fn spotify_link_is_encoded() {
        assert_eq!(spotify_search("lo-fi & chill"), "spotify:search:lo%2Dfi%20%26%20chill");
    }

    #[test]
    fn date_parts() {
        let d = NaiveDateTime::parse_from_str("2026-11-15T10:05", "%Y-%m-%dT%H:%M").unwrap();
        assert_eq!(parts(Some(&d)), ["2026", "11", "15", "10", "5"]);
        assert_eq!(parts(None).len(), 5);
    }

    #[test]
    fn permission_errors_are_explained() {
        assert!(friendly("execution error: Not authorized to send Apple events to Reminders. (-1743)", "Reminders").contains("Automation"));
        assert!(friendly("System Events got an error: AppleEvent timed out. (-1712)", "System Events").contains("click Allow"));
    }

    /// Real osascript run (macOS only): argv values stay data, even when they look like flags or code.
    #[test]
    fn osascript_argv_is_data() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let echo = "on run argv\n  return (item 1 of argv) & \"|\" & (item 2 of argv)\nend run";
        let r = osascript(echo, &["-e", "\" & (do shell script \"echo pwned\") & \""], "test").unwrap();
        assert_eq!(r, "-e|\" & (do shell script \"echo pwned\") & \"");
    }
}
