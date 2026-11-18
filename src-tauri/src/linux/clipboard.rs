//! The clipboard, through wl-clipboard (Wayland) or xclip / xsel (X11). Password managers mark
//! what they copy (`x-kde-passwordManagerHint`, used by KeePassXC, KDE and others): that is
//! reported as a secret and never read.
use super::Session;
use std::time::Duration;

pub enum Content {
    Text(String),
    Secret,
    Empty,
}

const SECRET_TYPES: &[&str] = &["x-kde-passwordManagerHint"];

/// Whether a list of clipboard types (one per line) marks a secret.
pub fn marks_secret(types: &str) -> bool {
    types.lines().any(|t| SECRET_TYPES.contains(&t.trim()))
}

pub fn read() -> Result<Content, String> {
    let t = Duration::from_secs(5);
    match super::session() {
        Session::Wayland => {
            if super::program("wl-paste").is_none() {
                return Err("reading the clipboard needs wl-clipboard (package wl-clipboard)".into());
            }
            let types = match super::run("wl-paste", &["--list-types"], t) {
                Ok(s) => s,
                Err(e) if e.contains("No selection") || e.contains("empty") => return Ok(Content::Empty),
                Err(e) => return Err(e),
            };
            if marks_secret(&types) {
                return Ok(Content::Secret);
            }
            if !types.lines().any(|l| l.starts_with("text/") || l == "UTF8_STRING" || l == "STRING" || l == "TEXT") {
                return Ok(Content::Empty);
            }
            let s = super::run("wl-paste", &["--no-newline", "--type", "text"], t).unwrap_or_default();
            Ok(if s.is_empty() { Content::Empty } else { Content::Text(s) })
        }
        Session::X11 => {
            if super::program("xclip").is_some() {
                let types = super::run("xclip", &["-selection", "clipboard", "-o", "-t", "TARGETS"], t).unwrap_or_default();
                if marks_secret(&types) {
                    return Ok(Content::Secret);
                }
                let s = super::run("xclip", &["-selection", "clipboard", "-o", "-t", "UTF8_STRING"], t).unwrap_or_default();
                return Ok(if s.is_empty() { Content::Empty } else { Content::Text(s) });
            }
            if super::program("xsel").is_some() {
                let s = super::run("xsel", &["--clipboard", "--output"], t).unwrap_or_default();
                return Ok(if s.is_empty() { Content::Empty } else { Content::Text(s) });
            }
            Err("reading the clipboard needs xclip or xsel".into())
        }
    }
}

/// Puts text on the clipboard (None clears it). The helper keeps serving it in the background.
pub fn write(text: Option<&str>) -> Result<(), String> {
    match super::session() {
        Session::Wayland => {
            let p = super::program("wl-copy").ok_or("the clipboard needs wl-clipboard (package wl-clipboard)")?;
            match text {
                Some(s) => super::spawn_detached(&p, &[], Some(s.as_bytes())),
                None => super::run("wl-copy", &["--clear"], Duration::from_secs(5)).map(drop),
            }
        }
        Session::X11 => {
            if let Some(p) = super::program("xclip") {
                return super::spawn_detached(&p, &["-selection".as_ref(), "clipboard".as_ref(), "-i".as_ref()], Some(text.unwrap_or("").as_bytes()));
            }
            if let Some(p) = super::program("xsel") {
                return match text {
                    Some(s) => super::spawn_detached(&p, &["--clipboard".as_ref(), "--input".as_ref()], Some(s.as_bytes())),
                    None => super::run("xsel", &["--clipboard", "--clear"], Duration::from_secs(5)).map(drop),
                };
            }
            Err("the clipboard needs xclip or xsel".into())
        }
    }
    .map(|()| std::thread::sleep(Duration::from_millis(80)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn password_manager_marks_are_secrets() {
        assert!(super::marks_secret("text/plain\nx-kde-passwordManagerHint\nUTF8_STRING"));
        assert!(!super::marks_secret("text/plain;charset=utf-8\nUTF8_STRING"));
    }
}
