//! Installed apps as the desktop knows them: desktop entries (`.desktop` files) in the XDG data
//! folders, including Flatpak and Snap exports. An app's id is its desktop file id without
//! ".desktop" (e.g. "org.gnome.Nautilus", "firefox"): the Linux counterpart of a macOS bundle id.
//!
//! A running process is mapped to its app the way the desktop itself does: the systemd scope the
//! launcher put it in, the desktop file that launched it, its Flatpak id, then its program name.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub exec: String,
    pub wm_class: Option<String>,
    pub terminal: bool,
    /// Hidden from menus (helpers, handlers): never listed or opened by name, but still used to
    /// recognise running processes.
    pub no_display: bool,
}

/// XDG data folders, most important first (the user's own entries override the system's).
pub fn data_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let home = dirs::home_dir();
    match std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        Some(d) => v.push(PathBuf::from(d)),
        None => v.extend(home.as_ref().map(|h| h.join(".local/share"))),
    }
    v.extend(home.as_ref().map(|h| h.join(".local/share/flatpak/exports/share")));
    v.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    let sys = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|d| d.starts_with('/')).map(PathBuf::from));
    v.push(PathBuf::from("/var/lib/snapd/desktop"));
    let mut seen = Vec::new();
    v.retain(|d| {
        let keep = !seen.contains(d);
        seen.push(d.clone());
        keep
    });
    v
}

/// Every application entry, first by precedence for each id.
pub fn all() -> Vec<Entry> {
    let mut by_id: HashMap<String, Entry> = HashMap::new();
    for dir in data_dirs() {
        let apps = dir.join("applications");
        scan(&apps, &apps, 0, &mut by_id);
    }
    let mut v: Vec<Entry> = by_id.into_values().collect();
    v.sort_by_key(|e| e.name.to_lowercase());
    v
}

/// `all()`, re-read at most every 30 seconds (reading every desktop file takes a moment).
pub fn cached() -> std::sync::Arc<Vec<Entry>> {
    use std::time::{Duration, Instant};
    static CACHE: parking_lot::Mutex<Option<(Instant, std::sync::Arc<Vec<Entry>>)>> = parking_lot::Mutex::new(None);
    let mut c = CACHE.lock();
    match c.as_ref() {
        Some((at, v)) if at.elapsed() < Duration::from_secs(30) => v.clone(),
        _ => {
            let v = std::sync::Arc::new(all());
            *c = Some((Instant::now(), v.clone()));
            v
        }
    }
}

fn scan(root: &Path, dir: &Path, depth: u8, out: &mut HashMap<String, Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten().take(5000) {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth < 2 {
            scan(root, &p, depth + 1, out);
            continue;
        }
        if p.extension().and_then(|x| x.to_str()) != Some("desktop") {
            continue;
        }
        // The desktop file id: path below applications/, with '/' turned into '-'.
        let Some(rel) = p.strip_prefix(root).ok().and_then(|r| r.to_str()) else { continue };
        let id = rel.trim_end_matches(".desktop").replace('/', "-");
        if out.contains_key(&id) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        // A user entry that hides a system one still claims its id.
        match parse(&id, &p, &text) {
            Some(entry) => {
                out.insert(id, entry);
            }
            None => {
                out.insert(id.clone(), Entry { id, name: String::new(), path: p, exec: String::new(), wm_class: None, terminal: false, no_display: true });
            }
        }
    }
}

/// The [Desktop Entry] group of a desktop file. None for anything that isn't a visible-or-hidden
/// application with a command (links, directories, deleted entries).
pub fn parse(id: &str, path: &Path, text: &str) -> Option<Entry> {
    let mut in_main = false;
    let mut kv: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            kv.entry(k.trim()).or_insert(v.trim());
        }
    }
    if kv.get("Type").copied() != Some("Application") || kv.get("Hidden").copied() == Some("true") {
        return None;
    }
    let exec = unescape(kv.get("Exec")?);
    let name = unescape(kv.get("Name")?);
    if name.is_empty() || exec.is_empty() {
        return None;
    }
    Some(Entry {
        id: id.to_string(),
        name,
        path: path.to_path_buf(),
        exec,
        wm_class: kv.get("StartupWMClass").map(|s| s.to_string()),
        terminal: kv.get("Terminal").copied() == Some("true"),
        no_display: kv.get("NoDisplay").copied() == Some("true"),
    })
}

/// Desktop-file string escapes: \s \n \t \r \\.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// The Exec line as program and arguments (Desktop Entry spec): quoted strings, field codes.
/// `files` replace %f/%F/%u/%U; other field codes are dropped. Errors on malformed lines.
pub fn exec_argv(exec: &str, name: &str, files: &[&str]) -> Result<Vec<String>, String> {
    let mut words: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut in_word, mut was_quoted) = (false, false, false);
    let mut it = exec.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
                was_quoted = true;
            }
            '\\' if quoted => match it.next() {
                Some(e @ ('"' | '`' | '$' | '\\')) => cur.push(e),
                Some(o) => {
                    cur.push('\\');
                    cur.push(o);
                }
                None => return Err("the app's command line is malformed".into()),
            },
            ' ' | '\t' if !quoted => {
                if in_word {
                    words.push((std::mem::take(&mut cur), was_quoted));
                    in_word = false;
                    was_quoted = false;
                }
            }
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quoted {
        return Err("the app's command line is malformed".into());
    }
    if in_word {
        words.push((cur, was_quoted));
    }
    let mut out = Vec::new();
    let mut placed = false;
    for (w, was_quoted) in words {
        // Field codes are only recognised outside quotes, as a whole word or inside one.
        if was_quoted {
            out.push(w);
            continue;
        }
        match w.as_str() {
            "%f" | "%u" => {
                if let Some(f) = files.first() {
                    out.push(f.to_string());
                }
                placed = true;
            }
            "%F" | "%U" => {
                out.extend(files.iter().map(|f| f.to_string()));
                placed = true;
            }
            "%i" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" | "%k" => {}
            "%c" => out.push(name.to_string()),
            _ => {
                let mut s = String::new();
                let mut chars = w.chars();
                while let Some(c) = chars.next() {
                    if c == '%' {
                        match chars.next() {
                            Some('%') => s.push('%'),
                            Some('c') => s.push_str(name),
                            _ => {}
                        }
                    } else {
                        s.push(c);
                    }
                }
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
    }
    if !placed && !files.is_empty() {
        out.extend(files.iter().map(|f| f.to_string()));
    }
    if out.is_empty() {
        return Err("the app's command line is empty".into());
    }
    Ok(out)
}

/// Starts the app (optionally with files), detached, as the desktop's launcher would.
pub fn launch(e: &Entry, files: &[&str]) -> Result<(), String> {
    if e.terminal {
        return Err(format!("{} runs in a terminal, so the assistant doesn't start it", e.name));
    }
    // gio launches exactly like the desktop (startup notification, systemd scope) when present.
    if files.is_empty() {
        if let Some(gio) = super::program("gio") {
            if super::run_program(&gio.to_string_lossy(), &["launch".as_ref(), e.path.as_os_str()], std::time::Duration::from_secs(15)).is_ok() {
                return Ok(());
            }
        }
    }
    let argv = exec_argv(&e.exec, &e.name, files)?;
    let prog = resolve_program(&argv[0]).ok_or_else(|| format!("{} can't be started: {} isn't installed", e.name, argv[0]))?;
    let args: Vec<&std::ffi::OsStr> = argv[1..].iter().map(std::ffi::OsStr::new).collect();
    super::spawn_detached(&prog, &args, None)
}

/// The program of an Exec line: absolute, or looked up in the system and user program folders.
fn resolve_program(p: &str) -> Option<PathBuf> {
    if p.starts_with('/') {
        let path = PathBuf::from(p);
        return path.is_file().then_some(path);
    }
    super::program(p).or_else(|| {
        let home = dirs::home_dir()?;
        [".local/bin", "bin"].iter().map(|d| home.join(d).join(p)).find(|x| x.is_file())
    })
}

/// systemd unit names escape '-' and other bytes as \xNN.
fn unescape_unit(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1] == b'x' {
            if let Ok(v) = u8::from_str_radix(&s[i + 2..i + 4], 16) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The app id from a process's cgroup, as launchers name it:
/// `app-gnome-<id>-<pid>.scope`, `app-<launcher>-<id>@<n>.service`, `app-<id>-<n>.scope`,
/// `app-flatpak-<id>-<n>.scope`, `snap.<name>.<app>-<uuid>.scope`.
pub fn id_from_cgroup(cgroup: &str, known: &dyn Fn(&str) -> bool) -> Option<String> {
    let unit = cgroup.lines().filter_map(|l| l.rsplit('/').next()).find(|u| u.starts_with("app-") || u.starts_with("snap."))?;
    let unit = unescape_unit(unit);
    if let Some(rest) = unit.strip_prefix("snap.") {
        let name = rest.split('.').next()?;
        return Some(format!("{name}_{name}")).filter(|i| known(i)).or_else(|| Some(name.to_string()));
    }
    let body = unit.strip_prefix("app-")?;
    let body = body.strip_suffix(".scope").or_else(|| body.strip_suffix(".service")).unwrap_or(body);
    let body = body.split('@').next().unwrap_or(body);
    // Try the longest piece that is a known desktop id, skipping a launcher prefix and a numeric
    // or random suffix.
    let parts: Vec<&str> = body.split('-').collect();
    for start in 0..parts.len().min(3) {
        for end in (start + 1..=parts.len()).rev() {
            let cand = parts[start..end].join("-");
            if known(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// The desktop entry of a running process, by its systemd scope, launch environment, Flatpak id or
/// program name (walking up to the process that started it, for helper processes).
pub fn entry_of_pid(pid: i32, entries: &[Entry]) -> Option<Entry> {
    let by_id = |id: &str| entries.iter().find(|e| e.id == id || e.id.eq_ignore_ascii_case(id)).cloned();
    let known = |id: &str| entries.iter().any(|e| e.id == id);
    let mut p = pid;
    for _ in 0..4 {
        if p <= 1 {
            break;
        }
        if let Ok(cg) = std::fs::read_to_string(format!("/proc/{p}/cgroup")) {
            if let Some(e) = id_from_cgroup(&cg, &known).and_then(|id| by_id(&id)) {
                return Some(e);
            }
        }
        if let Ok(env) = std::fs::read(format!("/proc/{p}/environ")) {
            for var in env.split(|b| *b == 0) {
                if let Some(v) = var.strip_prefix(b"GIO_LAUNCHED_DESKTOP_FILE=") {
                    let f = String::from_utf8_lossy(v);
                    if let Some(e) = entries.iter().find(|e| e.path.to_string_lossy() == f) {
                        return Some(e.clone());
                    }
                }
                if let Some(v) = var.strip_prefix(b"FLATPAK_ID=") {
                    if let Some(e) = by_id(&String::from_utf8_lossy(v)) {
                        return Some(e);
                    }
                }
            }
        }
        if let Some(e) = by_program(p, entries) {
            return Some(e);
        }
        p = super::parent_pid(p).unwrap_or(0);
    }
    None
}

/// The entry whose command (or window class) is this process's program.
fn by_program(pid: i32, entries: &[Entry]) -> Option<Entry> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string());
    let argv0 = std::fs::read(format!("/proc/{pid}/cmdline")).ok().and_then(|c| c.split(|b| *b == 0).next().map(|a| String::from_utf8_lossy(a).to_string()));
    let names: Vec<String> = [exe.as_ref().and_then(|e| e.file_name()).map(|n| n.to_string_lossy().to_string()), argv0.as_ref().and_then(|a| Path::new(a).file_name()).map(|n| n.to_string_lossy().to_string()), comm]
        .into_iter()
        .flatten()
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        return None;
    }
    let exe_full = exe.map(|e| e.to_string_lossy().to_string());
    let first_word = |e: &Entry| -> Option<String> {
        let argv = exec_argv(&e.exec, &e.name, &[]).ok()?;
        // Wrappers like "env FOO=1 app" or "flatpak run …" name the real program later.
        argv.into_iter().find(|w| !w.contains('=') && w != "env")
    };
    let visible_first = |a: &&Entry| !a.no_display;
    let mut hits: Vec<&Entry> = entries
        .iter()
        .filter(|e| {
            let w = first_word(e);
            w.as_deref().is_some_and(|w| exe_full.as_deref() == Some(w) || names.iter().any(|n| Path::new(w).file_name().is_some_and(|f| f.to_string_lossy() == *n)))
                || e.wm_class.as_deref().is_some_and(|c| names.iter().any(|n| n.eq_ignore_ascii_case(c)))
                || names.iter().any(|n| e.id.eq_ignore_ascii_case(n) || e.id.rsplit('.').next().is_some_and(|last| last.eq_ignore_ascii_case(n)))
        })
        .collect();
    hits.sort_by_key(|e| !visible_first(e));
    hits.first().map(|e| (*e).clone())
}

/// (app id, name) for a process: its desktop entry, else its program name (so every window can
/// still be listed, gated and remembered by a stable id).
pub fn app_of_pid(pid: i32, entries: &[Entry]) -> Option<(String, String)> {
    if let Some(e) = entry_of_pid(pid, entries).filter(|e| !e.name.is_empty()) {
        return Some((e.id, e.name));
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_string();
    if comm.is_empty() {
        return None;
    }
    let mut name = comm.clone();
    if let Some(f) = name.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    Some((comm.to_lowercase(), name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "[Desktop Entry]\nName=Firefox\nName[de]=Firefox-Browser\nExec=/usr/lib/firefox/firefox %u\nType=Application\nStartupWMClass=firefox\n\n[Desktop Action new-window]\nName=New Window\nExec=/usr/lib/firefox/firefox --new-window %u\n";

    #[test]
    fn entries_are_parsed_from_the_main_group_only() {
        let e = parse("firefox", Path::new("/usr/share/applications/firefox.desktop"), FIREFOX).unwrap();
        assert_eq!(e.name, "Firefox", "not a translation, not an action's name");
        assert_eq!(e.exec, "/usr/lib/firefox/firefox %u");
        assert_eq!(e.wm_class.as_deref(), Some("firefox"));
        assert!(!e.no_display && !e.terminal);
        assert!(parse("x", Path::new("/x"), "[Desktop Entry]\nType=Link\nName=X\nURL=https://a\n").is_none());
        assert!(parse("x", Path::new("/x"), "[Desktop Entry]\nType=Application\nName=X\nExec=x\nHidden=true\n").is_none());
        let hidden = parse("x", Path::new("/x"), "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n").unwrap();
        assert!(hidden.no_display);
    }

    #[test]
    fn exec_lines_follow_the_spec() {
        assert_eq!(exec_argv("/usr/lib/firefox/firefox %u", "Firefox", &[]).unwrap(), ["/usr/lib/firefox/firefox"]);
        assert_eq!(exec_argv("gimp-2.10 %U", "GIMP", &["/home/a/x.png", "/home/a/y b.png"]).unwrap(), ["gimp-2.10", "/home/a/x.png", "/home/a/y b.png"]);
        assert_eq!(exec_argv("\"/opt/My App/app\" --flag=\"a b\" %F", "A", &[]).unwrap(), ["/opt/My App/app", "--flag=a b"]);
        assert_eq!(exec_argv("app --name=%c 100%%", "Name", &[]).unwrap(), ["app", "--name=Name", "100%"]);
        assert_eq!(exec_argv("\"a \\\"b\\\"\"", "A", &[]).unwrap(), ["a \"b\""]);
        assert_eq!(exec_argv("viewer", "V", &["/home/a/f.pdf"]).unwrap(), ["viewer", "/home/a/f.pdf"], "no field code: the file goes last");
        assert!(exec_argv("\"unclosed", "A", &[]).is_err());
        assert!(exec_argv("%i %k", "A", &[]).is_err(), "nothing left to run");
        assert_eq!(exec_argv("app \"%u\"", "A", &["/f"]).unwrap(), ["app", "%u", "/f"], "quoted field codes stay text");
    }

    #[test]
    fn app_ids_from_systemd_scopes() {
        let known = |id: &str| ["org.gnome.Nautilus", "firefox", "org.mozilla.firefox", "code", "spotify_spotify"].contains(&id);
        let k: &dyn Fn(&str) -> bool = &known;
        assert_eq!(id_from_cgroup("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-org.gnome.Nautilus-4242.scope\n", k).as_deref(), Some("org.gnome.Nautilus"));
        assert_eq!(id_from_cgroup("0::/user.slice/user@1000.service/app.slice/app-flatpak-org.mozilla.firefox-1234.scope", k).as_deref(), Some("org.mozilla.firefox"));
        assert_eq!(id_from_cgroup("0::/user.slice/user@1000.service/app.slice/app-org.kde.krunner-code@0a1b.service", k).as_deref(), Some("code"));
        assert_eq!(id_from_cgroup("0::/user.slice/user@1000.service/app.slice/app-gnome-firefox-99.scope", k).as_deref(), Some("firefox"));
        assert_eq!(id_from_cgroup("0::/user.slice/user@1000.service/app.slice/snap.spotify.spotify-1f2e.scope", k).as_deref(), Some("spotify_spotify"));
        assert_eq!(id_from_cgroup("0::/user.slice/user@1000.service/app.slice/app-gnome-my\\x2dtool-7.scope", &|id| id == "my-tool").as_deref(), Some("my-tool"));
        assert_eq!(id_from_cgroup("0::/user.slice/user@1000.service/session.slice/org.gnome.Shell@wayland.service", k), None);
    }

    #[test]
    fn our_own_process_has_an_app() {
        let me = std::process::id() as i32;
        let (id, name) = app_of_pid(me, &[]).unwrap();
        assert!(!id.is_empty() && !name.is_empty());
    }
}
