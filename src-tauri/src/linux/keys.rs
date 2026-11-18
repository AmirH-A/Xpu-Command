//! Keyboard input into other apps.
//!
//! * X11: XTest (through enigo), Unicode text included.
//! * Wayland: there is no general way for one app to type into another, so, in order:
//!   `wtype` (compositors with the virtual-keyboard protocol: Sway, Hyprland, KDE…), our own
//!   virtual keyboard through /dev/uinput (keys only, not text: letters depend on the layout), and
//!   `ydotool` (plain ASCII text). Text that none of them can type is inserted through AT-SPI by
//!   the caller instead.
use super::Session;
use parking_lot::Mutex;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Return,
    Tab,
    Backspace,
    Delete,
    Escape,
    Space,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
    /// A letter, digit or punctuation key, by the character on a US keyboard.
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mod {
    Ctrl,
    Shift,
    Alt,
    Super,
}

/// The key named like the tools name keys ("return", "pagedown", "f5", "l", "=").
pub fn named(k: &str) -> Option<Key> {
    Some(match k {
        "return" | "enter" => Key::Return,
        "tab" => Key::Tab,
        "delete" => Key::Backspace,
        "forwarddelete" => Key::Delete,
        "escape" => Key::Escape,
        "space" => Key::Space,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => Key::F(f[1..].parse().ok()?),
        c if c.chars().count() == 1 => {
            let ch = c.chars().next()?;
            if !(ch.is_ascii_alphanumeric() || "=-[]';\\,./`".contains(ch)) {
                return None;
            }
            Key::Char(ch.to_ascii_lowercase())
        }
        _ => return None,
    })
}

/// Modifiers as the tools name them. On Linux, "cmd" means Ctrl: the same shortcuts (copy,
/// paste, address bar) live on Ctrl here.
pub fn modifier(m: &str) -> Option<Mod> {
    Some(match m {
        "cmd" | "ctrl" => Mod::Ctrl,
        "shift" => Mod::Shift,
        "option" => Mod::Alt,
        "super" => Mod::Super,
        _ => return None,
    })
}

/// Types `text` where the keyboard focus is.
pub fn type_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    match super::session() {
        Session::X11 => x11_text(text),
        Session::Wayland => {
            let mut why = Vec::new();
            match wtype_text(text) {
                Ok(()) => return Ok(()),
                Err(e) => why.push(e),
            }
            match ydotool_text(text) {
                Ok(()) => return Ok(()),
                Err(e) => why.push(e),
            }
            Err(format!("no way to type here ({})", why.join("; ")))
        }
    }
}

/// Presses a key with modifiers where the keyboard focus is.
pub fn press(key: Key, mods: &[Mod]) -> Result<(), String> {
    match super::session() {
        Session::X11 => x11_press(key, mods),
        Session::Wayland => {
            let mut why = Vec::new();
            for f in [wtype_press, uinput_press, ydotool_press] {
                match f(key, mods) {
                    Ok(()) => return Ok(()),
                    Err(e) => why.push(e),
                }
            }
            Err(format!("no way to press keys here ({})", why.join("; ")))
        }
    }
}

/// Whether text can be typed by keyboard in this session (for setup checks).
pub fn can_type() -> Result<&'static str, String> {
    match super::session() {
        Session::X11 => Ok("XTest"),
        Session::Wayland => {
            if super::program("wtype").is_some() && !super::is_desktop("gnome") {
                Ok("wtype")
            } else if super::program("ydotool").is_some() && ydotool_socket().is_some() {
                Ok("ydotool")
            } else {
                Err("install wtype (Sway, Hyprland, KDE) or run ydotoold; GTK, Qt and Firefox fields are typed into through accessibility either way".into())
            }
        }
    }
}

fn enigo() -> Result<enigo::Enigo, String> {
    enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| format!("keyboard: {e}"))
}

fn x11_text(text: &str) -> Result<(), String> {
    use enigo::Keyboard;
    enigo()?.text(text).map_err(|e| format!("typing failed: {e}"))
}

fn enigo_key(k: Key) -> enigo::Key {
    use enigo::Key as E;
    match k {
        Key::Return => E::Return,
        Key::Tab => E::Tab,
        Key::Backspace => E::Backspace,
        Key::Delete => E::Delete,
        Key::Escape => E::Escape,
        Key::Space => E::Space,
        Key::Left => E::LeftArrow,
        Key::Right => E::RightArrow,
        Key::Up => E::UpArrow,
        Key::Down => E::DownArrow,
        Key::Home => E::Home,
        Key::End => E::End,
        Key::PageUp => E::PageUp,
        Key::PageDown => E::PageDown,
        Key::F(n) => [E::F1, E::F2, E::F3, E::F4, E::F5, E::F6, E::F7, E::F8, E::F9, E::F10, E::F11, E::F12][usize::from(n.clamp(1, 12) - 1)],
        Key::Char(c) => E::Unicode(c),
    }
}

fn enigo_mod(m: Mod) -> enigo::Key {
    match m {
        Mod::Ctrl => enigo::Key::Control,
        Mod::Shift => enigo::Key::Shift,
        Mod::Alt => enigo::Key::Alt,
        Mod::Super => enigo::Key::Meta,
    }
}

fn x11_press(key: Key, mods: &[Mod]) -> Result<(), String> {
    use enigo::{Direction, Keyboard};
    let mut e = enigo()?;
    let fail = |e: enigo::InputError| format!("key press failed: {e}");
    for m in mods {
        e.key(enigo_mod(*m), Direction::Press).map_err(fail)?;
    }
    let r = e.key(enigo_key(key), Direction::Click).map_err(fail);
    for m in mods.iter().rev() {
        let _ = e.key(enigo_mod(*m), Direction::Release);
    }
    r
}

/// wtype key names (XKB keysyms).
fn keysym(k: Key) -> String {
    match k {
        Key::Return => "Return".into(),
        Key::Tab => "Tab".into(),
        Key::Backspace => "BackSpace".into(),
        Key::Delete => "Delete".into(),
        Key::Escape => "Escape".into(),
        Key::Space => "space".into(),
        Key::Left => "Left".into(),
        Key::Right => "Right".into(),
        Key::Up => "Up".into(),
        Key::Down => "Down".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::PageUp => "Prior".into(),
        Key::PageDown => "Next".into(),
        Key::F(n) => format!("F{n}"),
        Key::Char(c) => match c {
            '=' => "equal".into(),
            '-' => "minus".into(),
            '[' => "bracketleft".into(),
            ']' => "bracketright".into(),
            '\'' => "apostrophe".into(),
            ';' => "semicolon".into(),
            '\\' => "backslash".into(),
            ',' => "comma".into(),
            '.' => "period".into(),
            '/' => "slash".into(),
            '`' => "grave".into(),
            c => c.to_string(),
        },
    }
}

fn wtype_mod(m: Mod) -> &'static str {
    match m {
        Mod::Ctrl => "ctrl",
        Mod::Shift => "shift",
        Mod::Alt => "alt",
        Mod::Super => "logo",
    }
}

fn wtype() -> Result<String, String> {
    // GNOME's compositor has no virtual-keyboard protocol: wtype can't work there.
    if super::is_desktop("gnome") {
        return Err("wtype doesn't work on GNOME".into());
    }
    super::program("wtype").map(|p| p.to_string_lossy().to_string()).ok_or_else(|| "wtype isn't installed".into())
}

/// The arguments that make wtype type `text` exactly: a leading '-' is typed as a key, so no part
/// of the text is ever read as an option.
pub fn wtype_text_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut rest = text;
    while let Some(r) = rest.strip_prefix('-') {
        args.push("-k".into());
        args.push("minus".into());
        rest = r;
    }
    if !rest.is_empty() {
        args.push(rest.to_string());
    }
    args
}

fn wtype_text(text: &str) -> Result<(), String> {
    let prog = wtype()?;
    let args = wtype_text_args(text);
    let a: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    super::run_program(&prog, &a, Duration::from_secs(30)).map(drop)
}

fn wtype_press(key: Key, mods: &[Mod]) -> Result<(), String> {
    let prog = wtype()?;
    let mut args: Vec<String> = Vec::new();
    for m in mods {
        args.extend(["-M".into(), wtype_mod(*m).into()]);
    }
    args.extend(["-k".into(), keysym(key)]);
    for m in mods.iter().rev() {
        args.extend(["-m".into(), wtype_mod(*m).into()]);
    }
    let a: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    super::run_program(&prog, &a, Duration::from_secs(10)).map(drop)
}

fn ydotool_socket() -> Option<std::path::PathBuf> {
    let candidates = [
        std::env::var_os("YDOTOOL_SOCKET").map(std::path::PathBuf::from),
        std::env::var_os("XDG_RUNTIME_DIR").map(|d| std::path::PathBuf::from(d).join(".ydotool_socket")),
        Some(std::path::PathBuf::from("/tmp/.ydotool_socket")),
    ];
    candidates.into_iter().flatten().find(|p| p.exists())
}

fn ydotool() -> Result<(String, std::path::PathBuf), String> {
    let p = super::program("ydotool").ok_or("ydotool isn't installed")?;
    let sock = ydotool_socket().ok_or("ydotoold isn't running")?;
    Ok((p.to_string_lossy().to_string(), sock))
}

fn ydotool_run(args: &[String]) -> Result<(), String> {
    let (prog, sock) = ydotool()?;
    let a: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    let mut cmd = std::process::Command::new(&prog);
    cmd.args(&a).env("YDOTOOL_SOCKET", &sock).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
    let out = {
        let _g = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        cmd.spawn().map_err(|e| format!("could not start ydotool: {e}"))?
    }
    .wait_with_output()
    .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("ydotool: {}", crate::agents::clip(String::from_utf8_lossy(&out.stderr).trim(), 200)))
    }
}

fn ydotool_text(text: &str) -> Result<(), String> {
    // ydotool types by US key positions: only plain ASCII comes out right.
    if !text.chars().all(|c| c == ' ' || c.is_ascii_graphic()) {
        return Err("ydotool can only type plain ASCII".into());
    }
    ydotool_run(&["type".into(), "--key-delay".into(), "4".into(), "--".into(), text.into()])
}

fn ydotool_press(key: Key, mods: &[Mod]) -> Result<(), String> {
    let code = |k: Key| scancode(k).ok_or_else(|| "that key can't be pressed this way".to_string());
    let mut args = vec!["key".to_string()];
    for m in mods {
        args.push(format!("{}:1", mod_scancode(*m)));
    }
    let k = code(key)?;
    args.push(format!("{k}:1"));
    args.push(format!("{k}:0"));
    for m in mods.iter().rev() {
        args.push(format!("{}:0", mod_scancode(*m)));
    }
    ydotool_run(&args)
}

/// Linux input event codes (linux/input-event-codes.h), US layout positions.
fn scancode(k: Key) -> Option<u16> {
    Some(match k {
        Key::Return => 28,
        Key::Tab => 15,
        Key::Backspace => 14,
        Key::Delete => 111,
        Key::Escape => 1,
        Key::Space => 57,
        Key::Left => 105,
        Key::Right => 106,
        Key::Up => 103,
        Key::Down => 108,
        Key::Home => 102,
        Key::End => 107,
        Key::PageUp => 104,
        Key::PageDown => 109,
        Key::F(n) => match n {
            1..=10 => 58 + u16::from(n),
            11 => 87,
            12 => 88,
            _ => return None,
        },
        Key::Char(c) => match c {
            '1'..='9' => 2 + (c as u16 - '1' as u16),
            '0' => 11,
            '-' => 12,
            '=' => 13,
            'q' => 16, 'w' => 17, 'e' => 18, 'r' => 19, 't' => 20, 'y' => 21, 'u' => 22, 'i' => 23, 'o' => 24, 'p' => 25,
            '[' => 26, ']' => 27,
            'a' => 30, 's' => 31, 'd' => 32, 'f' => 33, 'g' => 34, 'h' => 35, 'j' => 36, 'k' => 37, 'l' => 38,
            ';' => 39, '\'' => 40, '`' => 41, '\\' => 43,
            'z' => 44, 'x' => 45, 'c' => 46, 'v' => 47, 'b' => 48, 'n' => 49, 'm' => 50,
            ',' => 51, '.' => 52, '/' => 53,
            _ => return None,
        },
    })
}

fn mod_scancode(m: Mod) -> u16 {
    match m {
        Mod::Ctrl => 29,
        Mod::Shift => 42,
        Mod::Alt => 56,
        Mod::Super => 125,
    }
}

/// Our own virtual keyboard (/dev/uinput), made once and kept: the compositor needs a moment to
/// pick up a new device.
static UINPUT: Mutex<Option<evdev::uinput::VirtualDevice>> = Mutex::new(None);

fn uinput_press(key: Key, mods: &[Mod]) -> Result<(), String> {
    use evdev::{EventType, InputEvent, KeyCode};
    // Letters and punctuation depend on the keyboard layout: only layout-free keys go this way.
    if matches!(key, Key::Char(_)) {
        return Err("letter keys can't be pressed this way".into());
    }
    let code = scancode(key).ok_or("that key can't be pressed this way")?;
    let mut slot = UINPUT.lock();
    if slot.is_none() {
        let mut keys = evdev::AttributeSet::<KeyCode>::new();
        for c in 1u16..=127 {
            keys.insert(KeyCode::new(c));
        }
        let dev = evdev::uinput::VirtualDevice::builder()
            .map_err(|e| format!("can't use /dev/uinput: {e}"))?
            .name("Xpu Command keyboard")
            .with_keys(&keys)
            .map_err(|e| e.to_string())?
            .build()
            .map_err(|e| format!("can't use /dev/uinput: {e}"))?;
        *slot = Some(dev);
        std::thread::sleep(Duration::from_millis(300));
    }
    let dev = slot.as_mut().ok_or("no virtual keyboard")?;
    let ev = |c: u16, v: i32| InputEvent::new(EventType::KEY.0, c, v);
    for m in mods {
        dev.emit(&[ev(mod_scancode(*m), 1)]).map_err(|e| e.to_string())?;
    }
    dev.emit(&[ev(code, 1)]).map_err(|e| e.to_string())?;
    dev.emit(&[ev(code, 0)]).map_err(|e| e.to_string())?;
    for m in mods.iter().rev() {
        dev.emit(&[ev(mod_scancode(*m), 0)]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names() {
        assert_eq!(named("return"), Some(Key::Return));
        assert_eq!(named("delete"), Some(Key::Backspace), "the tools' delete is Backspace, like the Mac");
        assert_eq!(named("f12"), Some(Key::F(12)));
        assert_eq!(named("f13"), None);
        assert_eq!(named("L"), Some(Key::Char('l')));
        assert_eq!(named("é"), None);
        assert_eq!(modifier("cmd"), Some(Mod::Ctrl), "cmd shortcuts live on Ctrl on Linux");
        assert_eq!(modifier("option"), Some(Mod::Alt));
    }

    #[test]
    fn every_named_key_has_a_code_and_a_keysym() {
        for k in ["return", "tab", "delete", "forwarddelete", "escape", "space", "left", "right", "up", "down", "home", "end", "pageup", "pagedown", "f1", "f11", "a", "z", "0", "9", "=", "/", "`"] {
            let key = named(k).unwrap();
            assert!(scancode(key).is_some(), "{k}");
            assert!(!keysym(key).is_empty(), "{k}");
        }
        assert_eq!(scancode(Key::Char('l')), Some(38));
        assert_eq!(scancode(Key::F(1)), Some(59));
        assert_eq!(scancode(Key::F(10)), Some(68));
    }

    #[test]
    fn wtype_never_reads_text_as_an_option() {
        assert_eq!(wtype_text_args("hello"), ["hello"]);
        assert_eq!(wtype_text_args("--version"), ["-k", "minus", "-k", "minus", "version"]);
        assert_eq!(wtype_text_args("-"), ["-k", "minus"]);
        assert_eq!(wtype_text_args("a -b"), ["a -b"]);
    }
}
