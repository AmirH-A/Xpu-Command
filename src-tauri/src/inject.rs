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
