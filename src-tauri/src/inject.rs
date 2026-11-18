//! Mode A: type the transcript into whatever app has focus, as keyboard input. We type instead of
//! pasting so the text never passes through the clipboard (clipboard managers and other apps can
//! read it). The text is already sanitized: no newline can sneak in and press Enter.
use crate::error::{msg, Result};
#[cfg(not(target_os = "linux"))]
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
    #[cfg(target_os = "linux")]
    return linux::type_text(text, submit);
    #[cfg(not(target_os = "linux"))]
    {
        let mut e = Enigo::new(&Settings::default()).map_err(|e| msg(format!("keyboard: {e}")))?;
        e.text(text).map_err(|e| msg(format!("typing failed: {e}")))?;
        if submit {
            e.key(Key::Return, Direction::Click).map_err(|e| msg(format!("typing failed: {e}")))?;
        }
        Ok(())
    }
}

/// What was typed last and where, so "scratch that" or "make that shorter" can replace it.
#[derive(Debug, Clone)]
pub struct Typed {
    pub text: String,
    pub pid: i32,
    pub at: std::time::Instant,
    /// The field's text right before it when it was typed ("" = it started the field; None = the
    /// app doesn't show its text). Used to notice your corrections to it.
    pub before: Option<String>,
}

/// Without readable text in the field, the typed text is reselected with the keyboard: only this
/// soon after typing, when nothing else is likely to have moved the cursor.
#[cfg(any(target_os = "macos", target_os = "linux"))]
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

/// The focused text field as the Accessibility API (AT-SPI on Linux) shows it (offsets in UTF-16
/// units).
#[cfg(any(target_os = "macos", target_os = "linux"))]
struct Field {
    pid: i32,
    value: Vec<u16>,
    /// Selection start and length; length 0 is the caret.
    sel: (usize, usize),
}

#[cfg(target_os = "linux")]
impl Field {
    fn read() -> Option<Field> {
        let el = crate::linux::atspi::focused_element()?;
        if el.is_password() {
            return None;
        }
        let text = el.text()?;
        let chars = text.chars().count();
        let (start, end) = el.selection().unwrap_or_else(|| {
            let c = el.caret().unwrap_or(chars);
            (c, c)
        });
        let (start, end) = (start.min(chars), end.min(chars));
        let (s16, e16) = (crate::linux::atspi::utf16_offset(&text, start), crate::linux::atspi::utf16_offset(&text, end));
        Some(Field { pid: el.pid()?, value: text.encode_utf16().collect(), sel: (s16, e16 - s16) })
    }
}

#[cfg(target_os = "macos")]
impl Field {
    fn read() -> Option<Field> {
        // Read before every dictation: an app that is slow to answer must not hold up typing.
        crate::mac::ax::set_global_timeout(0.5);
        let el = crate::mac::ax::focused_element()?;
        if matches!(el.attr("AXSubrole"), crate::mac::ax::V::Str(s) if s == "AXSecureTextField") {
            return None;
        }
        let value: Vec<u16> = el.string("AXValue")?.encode_utf16().collect();
        let (start, len) = el.range("AXSelectedTextRange")?;
        (start + len <= value.len()).then_some(Field { pid: el.pid()?, value, sel: (start, len) })
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Field {
    fn before(&self, n: usize) -> String {
        utf16_slice(&self.value, self.sel.0.saturating_sub(n), self.sel.0)
    }

    fn after(&self, n: usize) -> String {
        let at = self.sel.0 + self.sel.1;
        utf16_slice(&self.value, at, (at + n).min(self.value.len()))
    }

    fn text(&self) -> String {
        String::from_utf16_lossy(&self.value)
    }

    fn selected(&self) -> String {
        String::from_utf16_lossy(&self.value[self.sel.0..self.sel.0 + self.sel.1])
    }
}

/// `value[from..to]` as text, without half of a character cut at either end (emoji and other
/// characters outside the basic plane take two UTF-16 units).
pub fn utf16_slice(value: &[u16], from: usize, to: usize) -> String {
    let (mut from, mut to) = (from.min(value.len()), to.min(value.len()));
    if from > 0 && from < to && (0xDC00..=0xDFFF).contains(&value[from]) {
        from += 1;
    }
    if to > from && (0xD800..=0xDBFF).contains(&value[to - 1]) {
        to -= 1;
    }
    String::from_utf16_lossy(&value[from..to.max(from)])
}

/// What a dictation did: the text typed (with any space added before it), the field's text before
/// it, and a correction you made to the previous dictation in the same field, if one was found.
#[derive(Debug, Clone, Default)]
pub struct Inserted {
    pub typed: String,
    pub before: Option<String>,
    pub fix: Option<(String, String)>,
}

/// Types a dictation where the cursor is, fitted to the text around it (spaces, capitals, a final
/// period mid-sentence). First it looks at what became of the last dictation in this field, so a
/// word you corrected there can be offered for your words.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn type_in_place(text: &str, style: crate::dictation::Style, submit: bool, last: Option<&Typed>) -> Result<Inserted> {
    let field = Field::read();
    let fix = match (last, &field) {
        (Some(l), Some(f)) if f.pid == l.pid => l.before.as_deref().and_then(|b| crate::vocab::find_fix(&l.text, b, &f.text())),
        _ => None,
    };
    let (joined, before) = match &field {
        Some(f) => (crate::dictation::fit_to_context(text, &f.before(40), &f.after(8), style), Some(f.before(24))),
        None => (text.to_string(), None),
    };
    type_text(&joined, submit)?;
    Ok(Inserted { typed: joined, before, fix })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn type_in_place(text: &str, _style: crate::dictation::Style, submit: bool, _last: Option<&Typed>) -> Result<Inserted> {
    type_text(text, submit)?;
    Ok(Inserted { typed: text.to_string(), ..Default::default() })
}

/// The text selected in the focused field, with its app: what "make this more formal" rewrites.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn selection() -> Option<(i32, String)> {
    let f = Field::read()?;
    let s = f.selected();
    (!s.trim().is_empty()).then_some((f.pid, s))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn selection() -> Option<(i32, String)> {
    None
}

/// Replaces the selection with `new` if it is still `original` in app `pid`: through
/// Accessibility, or by typing over it where the app ignores that.
#[cfg(target_os = "macos")]
pub fn replace_selection_with(pid: i32, original: &str, new: &str) -> Result<()> {
    use crate::i18n::tr;
    // Line breaks are written through Accessibility only: typed, a line break is Return.
    if new.is_empty() || new.chars().any(|c| c.is_control() && c != '\n') {
        return Err(msg("refusing to type control characters"));
    }
    if crate::permissions::secure_input_active() {
        return Err(msg("a password field or secure keyboard entry is active; not editing"));
    }
    let changed = || msg(tr("The selection changed, so I left it as it is"));
    let f = Field::read().filter(|f| f.pid == pid && f.selected() == original).ok_or_else(changed)?;
    let el = crate::mac::ax::focused_element().ok_or_else(changed)?;
    let was = f.text();
    if el.set_string("AXSelectedText", new).is_err() || el.string("AXValue").is_some_and(|now| now == was) {
        // Web views and Electron apps ignore the write: typing replaces the selection instead.
        if new.contains('\n') {
            return Err(msg(tr("This app doesn't let me replace text with several lines; select one paragraph")));
        }
        let mut e = Enigo::new(&Settings::default()).map_err(|e| msg(format!("keyboard: {e}")))?;
        replace_selection(&mut e, new)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub use linux::replace_selection_with;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn replace_selection_with(_pid: i32, _original: &str, _new: &str) -> Result<()> {
    Err(msg("Editing by voice works on macOS and Linux"))
}

/// Replaces the last typed text with `new` (empty: deletes it). Through Accessibility it first
/// checks the text is still in the field and selects exactly it, so nothing else is touched.
#[cfg(target_os = "macos")]
pub fn replace_typed(t: &Typed, new: &str) -> Result<()> {
    use crate::i18n::tr;
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
    let el = ax::focused_element().filter(|e| e.pid() == Some(t.pid)).ok_or_else(|| msg(tr("The app I typed into isn't in front any more")))?;
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
            let start = locate(&value, caret, &typed).ok_or_else(|| msg(tr("I can't find what I typed in this field any more")))?;
            el.set_range("AXSelectedTextRange", start, typed.len()).map_err(msg)?;
            let selected = el.string("AXSelectedText").unwrap_or_default();
            if selected != t.text {
                return Err(msg(tr("I couldn't select what I typed, so I left it as it is")));
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
        _ => return Err(msg(tr("This app doesn't show me its text, so I can only edit right after typing"))),
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

#[cfg(target_os = "linux")]
pub use linux::replace_typed;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn replace_typed(_t: &Typed, _new: &str) -> Result<()> {
    Err(msg("Editing by voice works on macOS and Linux"))
}

/// Linux: typing goes through the keyboard where the session allows (X11: XTest; Wayland: wtype
/// or ydotool) and through accessibility otherwise, which also edits text in place.
#[cfg(target_os = "linux")]
mod linux {
    use super::{locate, Field, Typed, KEYS_FALLBACK};
    use crate::error::{msg, Result};
    use crate::i18n::tr;
    use crate::linux::atspi::{self, El};
    use crate::linux::keys::{self, Key, Mod};
    use crate::linux::Session;

    fn editable(el: &El) -> bool {
        el.interfaces().iter().any(|i| i == "org.a11y.atspi.EditableText")
    }

    /// Writes `text` at the focused field's cursor through accessibility (replacing a selection),
    /// and checks that it arrived.
    fn insert_at_caret(text: &str) -> std::result::Result<(), String> {
        let el = atspi::focused_element().ok_or("no text field has the focus")?;
        if el.is_password() {
            return Err("not typing into a password field".into());
        }
        if !editable(&el) {
            return Err("the focused field can't be written into directly".into());
        }
        let before = el.text().unwrap_or_default();
        let n = before.chars().count();
        let at = match el.selection() {
            Some((s, e)) => {
                el.delete_text(s, e)?;
                s
            }
            None => el.caret().unwrap_or(n).min(n),
        };
        el.insert_text(at, text)?;
        std::thread::sleep(std::time::Duration::from_millis(40));
        if el.text().unwrap_or_default() == before {
            return Err("the app ignored the text".into());
        }
        let _ = el.set_caret(at + text.chars().count());
        Ok(())
    }

    pub fn type_text(text: &str, submit: bool) -> Result<()> {
        let typed = match crate::linux::session() {
            Session::X11 => keys::type_text(text).or_else(|k| insert_at_caret(text).map_err(|a| format!("{k}; {a}"))),
            Session::Wayland => insert_at_caret(text).or_else(|a| keys::type_text(text).map_err(|k| format!("{a}; {k}"))),
        };
        typed.map_err(|e| msg(format!("typing failed: {e}")))?;
        if submit {
            if let Err(e) = keys::press(Key::Return, &[]) {
                // Wayland without a keyboard tool: the field's own "activate" is Return.
                let done = atspi::focused_element().is_some_and(|el| el.do_action("activate").is_ok());
                if !done {
                    return Err(msg(format!("couldn't press Return: {e}")));
                }
            }
        }
        Ok(())
    }

    /// Replaces UTF-16 range [start, end) of the focused field's text with `new`, through
    /// accessibility; Err when the app doesn't take it.
    fn replace_range(el: &El, value: &str, start16: usize, end16: usize, new: &str) -> std::result::Result<(), String> {
        if !editable(el) {
            return Err("not editable through accessibility".into());
        }
        let (s, e) = (atspi::char_offset(value, start16), atspi::char_offset(value, end16));
        el.delete_text(s, e)?;
        if !new.is_empty() {
            el.insert_text(s, new)?;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
        if el.text().unwrap_or_default() == value {
            return Err("the app ignored the change".into());
        }
        let _ = el.set_caret(s + new.chars().count());
        Ok(())
    }

    /// Types `new` over the current selection (empty: deletes it).
    fn type_over(new: &str) -> Result<()> {
        let r = if new.is_empty() {
            keys::press(Key::Backspace, &[])
        } else {
            let mut r = Ok(());
            for (i, line) in new.split('\n').enumerate() {
                if i > 0 {
                    r = keys::press(Key::Return, &[]);
                }
                if r.is_ok() && !line.is_empty() {
                    r = keys::type_text(line);
                }
                if r.is_err() {
                    break;
                }
            }
            r
        };
        r.map_err(|e| msg(format!("typing failed: {e}")))
    }

    pub fn replace_selection_with(pid: i32, original: &str, new: &str) -> Result<()> {
        if new.is_empty() || new.chars().any(|c| c.is_control() && c != '\n') {
            return Err(msg("refusing to type control characters"));
        }
        if crate::permissions::secure_input_active() {
            return Err(msg("a password field or secure keyboard entry is active; not editing"));
        }
        let changed = || msg(tr("The selection changed, so I left it as it is"));
        let f = Field::read().filter(|f| f.pid == pid && f.selected() == original).ok_or_else(changed)?;
        let el = atspi::focused_element().ok_or_else(changed)?;
        let value = f.text();
        if replace_range(&el, &value, f.sel.0, f.sel.0 + f.sel.1, new).is_err() {
            if new.contains('\n') && !el.states().has(17) {
                return Err(msg(tr("This app doesn't let me replace text with several lines; select one paragraph")));
            }
            type_over(new)?;
        }
        Ok(())
    }

    pub fn replace_typed(t: &Typed, new: &str) -> Result<()> {
        if new.chars().any(char::is_control) {
            return Err(msg("refusing to type control characters"));
        }
        if crate::permissions::secure_input_active() {
            return Err(msg("a password field or secure keyboard entry is active; not editing"));
        }
        let el = atspi::focused_element().filter(|e| e.pid() == Some(t.pid)).ok_or_else(|| msg(tr("The app I typed into isn't in front any more")))?;
        if el.is_password() {
            return Err(msg("not editing a password field"));
        }
        let typed: Vec<u16> = t.text.encode_utf16().collect();
        match el.text().filter(|v| !v.is_empty()) {
            Some(v) => {
                let value: Vec<u16> = v.encode_utf16().collect();
                let caret = el.caret().map_or(value.len(), |c| atspi::utf16_offset(&v, c));
                let start = locate(&value, caret, &typed).ok_or_else(|| msg(tr("I can't find what I typed in this field any more")))?;
                if replace_range(&el, &v, start, start + typed.len(), new).is_ok() {
                    return Ok(());
                }
                // Select exactly what was typed, then type over it.
                let (s, e) = (atspi::char_offset(&v, start), atspi::char_offset(&v, start + typed.len()));
                el.set_selection(s, e).map_err(msg)?;
                let sel = el.selection().map(|(a, b)| v.chars().skip(a).take(b - a).collect::<String>());
                if sel.as_deref() != Some(t.text.as_str()) {
                    return Err(msg(tr("I couldn't select what I typed, so I left it as it is")));
                }
                type_over(new)
            }
            None if t.at.elapsed() < KEYS_FALLBACK => {
                for _ in 0..t.text.chars().count() {
                    keys::press(Key::Left, &[Mod::Shift]).map_err(|e| msg(format!("typing failed: {e}")))?;
                }
                type_over(new)
            }
            None => Err(msg(tr("This app doesn't show me its text, so I can only edit right after typing"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{locate, utf16_slice};

    #[test]
    fn slices_never_cut_a_character_in_half() {
        let v: Vec<u16> = "ab👋cd".encode_utf16().collect();
        assert_eq!(utf16_slice(&v, 0, 2), "ab");
        assert_eq!(utf16_slice(&v, 0, 3), "ab", "the emoji's first half is left out");
        assert_eq!(utf16_slice(&v, 3, 6), "cd", "and its second half");
        assert_eq!(utf16_slice(&v, 0, 99), "ab👋cd");
        assert_eq!(utf16_slice(&v, 5, 2), "");
    }

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
