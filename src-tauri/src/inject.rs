//! Mode A: type the transcript into whatever app has focus, as keyboard input. We type instead of
//! pasting so the text never passes through the clipboard (clipboard managers and other apps can
//! read it). The text is already sanitized: no newline can sneak in and press Enter.
use crate::error::{msg, Result};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

pub fn type_text(text: &str, submit: bool) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    debug_assert!(!text.chars().any(char::is_control), "transcript must be sanitized before typing");
    if text.chars().any(char::is_control) {
        return Err(msg("refusing to type control characters"));
    }
    if crate::permissions::secure_input_active() {
        return Err(msg("a password field or secure keyboard entry is active; not typing"));
    }
    if !crate::permissions::accessibility_trusted(false) {
        return Err(crate::error::Error::Permission("Accessibility (to type into other apps)".into()));
    }
    let mut e = Enigo::new(&Settings::default()).map_err(|e| msg(format!("keyboard: {e}")))?;
    e.text(text).map_err(|e| msg(format!("typing failed: {e}")))?;
    if submit {
        e.key(Key::Return, Direction::Click).map_err(|e| msg(format!("typing failed: {e}")))?;
    }
    Ok(())
}

/// What was typed last and where, so "scratch that" or "make that shorter" can replace it.
#[derive(Debug, Clone)]
pub struct Typed {
    pub text: String,
    pub pid: i32,
    pub at: std::time::Instant,
}

/// Without readable text in the field, the typed text is reselected with the keyboard: only this
/// soon after typing, when nothing else is likely to have moved the cursor.
#[cfg(target_os = "macos")]
const KEYS_FALLBACK: std::time::Duration = std::time::Duration::from_secs(60);

/// Where `typed` sits in the field's text (UTF-16 units, like the Accessibility API): ending at the
/// caret if it is there, else its last occurrence. None if it isn't in the field.
pub fn locate(value: &[u16], caret: usize, typed: &[u16]) -> Option<usize> {
    if typed.is_empty() || typed.len() > value.len() {
        return None;
    }
    let caret = caret.min(value.len());
    if caret >= typed.len() && &value[caret - typed.len()..caret] == typed {
        return Some(caret - typed.len());
    }
    value.windows(typed.len()).rposition(|w| w == typed)
}

/// Types `text` after the last dictation, with a space between them when the field shows the
/// cursor right after it ("…on Friday." + "Thanks!" → "…on Friday. Thanks!"). Returns what was typed.
#[cfg(target_os = "macos")]
pub fn type_after(text: &str, submit: bool, last: Option<&Typed>) -> Result<String> {
    let joined = match last.zip(crate::mac::ax::focused_element()) {
        Some((l, el)) if el.pid() == Some(l.pid) => {
            let value: Vec<u16> = el.string("AXValue").unwrap_or_default().encode_utf16().collect();
            let typed: Vec<u16> = l.text.encode_utf16().collect();
            let caret = el.range("AXSelectedTextRange").map(|r| r.0);
            match caret {
                Some(c) if c >= typed.len() && c <= value.len() && value[c - typed.len()..c] == typed[..] && text.starts_with(char::is_alphanumeric) => format!(" {text}"),
                _ => text.to_string(),
            }
        }
        _ => text.to_string(),
    };
    type_text(&joined, submit)?;
    Ok(joined)
}

#[cfg(not(target_os = "macos"))]
pub fn type_after(text: &str, submit: bool, _last: Option<&Typed>) -> Result<String> {
    type_text(text, submit)?;
    Ok(text.to_string())
}

/// Replaces the last typed text with `new` (empty: deletes it). Through Accessibility it first
/// checks the text is still in the field and selects exactly it, so nothing else is touched.
#[cfg(target_os = "macos")]
pub fn replace_typed(t: &Typed, new: &str) -> Result<()> {
    use crate::mac::ax;
    if new.chars().any(char::is_control) {
        return Err(msg("refusing to type control characters"));
    }
    if crate::permissions::secure_input_active() {
        return Err(msg("a password field or secure keyboard entry is active; not editing"));
    }
    if !crate::permissions::accessibility_trusted(false) {
        return Err(crate::error::Error::Permission("Accessibility (to type into other apps)".into()));
    }
    ax::set_global_timeout(1.0);
    let el = ax::focused_element().filter(|e| e.pid() == Some(t.pid)).ok_or_else(|| msg("The app I typed into isn't in front any more"))?;
    if matches!(el.attr("AXSubrole"), ax::V::Str(s) if s == "AXSecureTextField") {
        return Err(msg("not editing a password field"));
    }
    let typed: Vec<u16> = t.text.encode_utf16().collect();
    let mut e = Enigo::new(&Settings::default()).map_err(|e| msg(format!("keyboard: {e}")))?;
    let value = el.string("AXValue");
    match value {
        Some(v) if !v.is_empty() => {
            let value: Vec<u16> = v.encode_utf16().collect();
            let caret = el.range("AXSelectedTextRange").map_or(value.len(), |r| r.0 + r.1);
            let start = locate(&value, caret, &typed).ok_or_else(|| msg("I can't find what I typed in this field any more"))?;
            el.set_range("AXSelectedTextRange", start, typed.len()).map_err(msg)?;
            let selected = el.string("AXSelectedText").unwrap_or_default();
            if selected != t.text {
                return Err(msg("I couldn't select what I typed, so I left it as it is"));
            }
            if el.set_string("AXSelectedText", new).is_err() || el.string("AXValue").is_some_and(|after| after == v) {
                // Some apps (web views, Electron) ignore the write: type over the selection instead.
                replace_selection(&mut e, new)?;
            }
        }
        _ if t.at.elapsed() < KEYS_FALLBACK => {
            for _ in 0..t.text.chars().count() {
                e.key(Key::Shift, Direction::Press).map_err(|e| msg(format!("typing failed: {e}")))?;
                e.key(Key::LeftArrow, Direction::Click).map_err(|e| msg(format!("typing failed: {e}")))?;
            }
            e.key(Key::Shift, Direction::Release).map_err(|e| msg(format!("typing failed: {e}")))?;
            replace_selection(&mut e, new)?;
        }
        _ => return Err(msg("This app doesn't show me its text, so I can only edit right after typing")),
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn replace_selection(e: &mut Enigo, new: &str) -> Result<()> {
    if new.is_empty() {
        e.key(Key::Backspace, Direction::Click).map_err(|e| msg(format!("typing failed: {e}")))
    } else {
        e.text(new).map_err(|e| msg(format!("typing failed: {e}")))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn replace_typed(_t: &Typed, _new: &str) -> Result<()> {
    Err(msg("Editing by voice works on macOS"))
}

#[cfg(test)]
mod tests {
    use super::locate;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn finds_the_typed_text_at_the_caret_first() {
        let v = u("Hi. Ship it Friday. More text. Ship it Friday.");
        let t = u("Ship it Friday.");
        assert_eq!(locate(&v, v.len(), &t), Some(31), "the copy right before the caret");
        assert_eq!(locate(&v, 19, &t), Some(4), "the caret is after the first copy");
        assert_eq!(locate(&v, 10, &t), Some(31), "elsewhere: the last copy");
        assert_eq!(locate(&v, 0, &u("absent")), None);
        assert_eq!(locate(&u("ab"), 2, &u("")), None);
        let fa = u("سلام 👋 فردا می‌بینمت");
        assert_eq!(locate(&fa, fa.len(), &u("فردا می‌بینمت")), Some(u("سلام 👋 ").len()), "UTF-16 units, like the Accessibility API");
    }
}
