//! Cleans a transcript before it goes anywhere. The important case is typing into a terminal:
//! a newline there is Enter, so a stray "\n" would run a command. We remove every control
//! character (newlines, ESC sequences, BEL, DEL, C1 controls, bidi overrides) and collapse whitespace.

pub const MAX_CHARS: usize = 4000;

pub fn clean_transcript(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_CHARS));
    let mut last_space = true;
    for c in raw.chars() {
        let c = if c.is_whitespace() { ' ' } else { c };
        if is_dangerous(c) {
            continue;
        }
        if c == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(c);
        if out.chars().count() >= MAX_CHARS {
            break;
        }
    }
    let t = strip_sound_tags(&out);
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let lowered = t.to_lowercase();
    let noise = ["(silence)", "(inaudible)"];
    if noise.iter().any(|n| lowered == *n) {
        return String::new();
    }
    t
}

/// Removes every "[…]" tag (Whisper uses square brackets only for non-speech) and "(…)" or "*…*"
/// tags that name a sound. Ordinary parentheses in speech are kept.
fn strip_sound_tags(text: &str) -> String {
    const SOUNDS: [&str; 12] = ["knock", "tap", "click", "thud", "bang", "noise", "sound", "music", "silence", "inaudible", "blank", "beep"];
    let is_sound = |inner: &str| {
        let l = inner.to_lowercase();
        inner.len() <= 40 && SOUNDS.iter().any(|w| l.contains(w))
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(['[', '(', '*']) {
        let open = rest[i..].chars().next().unwrap();
        let close = match open { '[' => ']', '(' => ')', _ => '*' };
        match rest[i + 1..].find(close) {
            Some(j) if open == '[' || is_sound(&rest[i + 1..i + 1 + j]) => {
                out.push_str(&rest[..i]);
                out.push(' ');
                rest = &rest[i + 1 + j + 1..];
            }
            _ => {
                out.push_str(&rest[..=i]);
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub(crate) fn is_dangerous(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}'..='\u{200F}' | '\u{FEFF}')
}

/// A short, speakable summary of an agent's final message: plain text, first sentences, capped.
pub fn speakable_summary(text: &str, max: usize) -> String {
    let mut plain = String::new();
    let mut in_code = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || l.is_empty() {
            continue;
        }
        let l = l.trim_start_matches(['#', '-', '*', '>', ' ']);
        plain.push_str(&l.replace(['`', '*', '_'], ""));
        plain.push(' ');
    }
    let plain = clean_transcript(&plain);
    if plain.chars().count() <= max {
        return plain;
    }
    let cut: String = plain.chars().take(max).collect();
    if let Some(i) = cut.rfind(['.', '!', '?']) {
        if i > max / 3 {
            return cut[..=i].to_string();
        }
    }
    match cut.rfind(' ') {
        Some(i) => format!("{}…", &cut[..i]),
        None => cut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newlines_and_escapes_never_survive() {
        let t = clean_transcript("run the tests\nrm -rf /\r\x1b[31m red \x07 bell");
        assert!(!t.contains('\n') && !t.contains('\r') && !t.contains('\x1b') && !t.contains('\x07'));
        assert_eq!(t, "run the tests rm -rf / [31m red bell");
    }

    #[test]
    fn bidi_and_zero_width_are_removed() {
        assert_eq!(clean_transcript("a\u{202E}b\u{200B}c\u{FEFF}"), "abc");
    }

    #[test]
    fn sound_tags_are_removed_anywhere() {
        assert_eq!(clean_transcript("Fix the login bug. [knocking]"), "Fix the login bug.");
        assert_eq!(clean_transcript("*knock knock* add a test (taps)"), "add a test");
        assert_eq!(clean_transcript("[BLANK_AUDIO]"), "");
        assert_eq!(clean_transcript("call f(x) with 2*3*4"), "call f(x) with 2*3*4", "ordinary text stays");
        assert_eq!(clean_transcript("rename (the old one) please"), "rename (the old one) please");
    }

    #[test]
    fn whitespace_collapses_and_noise_is_empty() {
        assert_eq!(clean_transcript("  fix   the\tbug  "), "fix the bug");
        assert_eq!(clean_transcript(" [BLANK_AUDIO] "), "");
    }

    #[test]
    fn length_is_capped() {
        assert_eq!(clean_transcript(&"a".repeat(10_000)).len(), MAX_CHARS);
    }

    #[test]
    fn summary_drops_code_and_markdown() {
        let s = speakable_summary("## Done\n```rust\nfn x() {}\n```\n- Fixed **login** in `auth.rs`.\n- Tests pass.", 200);
        assert_eq!(s, "Done Fixed login in auth.rs. Tests pass.");
    }

    #[test]
    fn summary_cuts_at_a_sentence() {
        let s = speakable_summary("First sentence is here. Second one is much longer and goes on and on.", 40);
        assert_eq!(s, "First sentence is here.");
    }
}
