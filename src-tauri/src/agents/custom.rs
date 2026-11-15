use super::{clip, Adapter, AgentEvent, VOICE_NOTE};
use crate::error::{msg, Result};
use crate::settings::Safety;
use serde_json::Value;
use std::path::{Path, PathBuf};

const MAX_TEXT: usize = 4000;
const INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ksh", "csh", "tcsh", "nu", "pwsh", "powershell", "cmd", "python", "python3", "node", "deno", "bun", "ruby", "perl", "php", "lua", "osascript", "env",
    "sudo", "su", "xargs", "tclsh", "expect", "awk", "sed",
];

pub struct Custom;

impl Adapter for Custom {
    fn binary_names(&self) -> &'static [&'static str] {
        &[]
    }

    fn args(&self, _project: &Path, _safety: Safety, _resume: Option<&str>) -> Vec<String> {
        Vec::new()
    }

    fn stdin(&self, prompt: &str) -> String {
        format!("{VOICE_NOTE}\n\nRequest:\n{prompt}\n")
    }

    fn parse_line(&self, line: &str, last_text: &mut String) -> Option<AgentEvent> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            append(last_text, line);
            return None;
        };
        let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
        match field("type") {
            "session" => super::valid_session_id(field("id")).then(|| AgentEvent::Session { id: field("id").to_string() }),
            "status" => Some(AgentEvent::Status { text: clip(field("text"), 80) }),
            "text" => {
                append(last_text, field("text"));
                None
            }
            "done" => {
                let summary = if field("summary").is_empty() { last_text.clone() } else { field("summary").to_string() };
                Some(AgentEvent::Done { summary: crate::sanitize::speakable_summary(&summary, 280) })
            }
            "error" => Some(AgentEvent::Failed { error: clip(field("message"), 200) }),
            _ => {
                append(last_text, line);
                None
            }
        }
    }
}

fn append(acc: &mut String, text: &str) {
    if !acc.is_empty() {
        acc.push('\n');
    }
    acc.push_str(text);
    if acc.len() > MAX_TEXT {
        let cut = acc.len() - MAX_TEXT;
        let start = (cut..acc.len()).find(|i| acc.is_char_boundary(*i)).unwrap_or(acc.len());
        acc.drain(..start);
    }
}

pub fn safety_name(s: Safety) -> &'static str {
    match s {
        Safety::ReadOnly => "read_only",
        Safety::Edit => "edit",
        Safety::AskCommands => "ask_commands",
    }
}

pub fn validate_command(p: &Path) -> Result<PathBuf> {
    if !p.is_absolute() {
        return Err(msg("choose the agent program with Choose…"));
    }
    let real = p.canonicalize().map_err(|_| msg("that program doesn't exist"))?;
    if !super::is_executable(&real) {
        return Err(msg("that file isn't a program that can be run"));
    }
    let stem = real.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_lowercase();
    let base = stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if INTERPRETERS.contains(&stem.as_str()) || INTERPRETERS.contains(&base) {
        return Err(msg("a shell or script interpreter would run what you say as code: choose the agent itself, or a script that starts it"));
    }
    Ok(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_events_and_plain_text() {
        let a = Custom;
        let mut t = String::new();
        assert_eq!(a.parse_line(r#"{"type":"session","id":"abc12345-x"}"#, &mut t), Some(AgentEvent::Session { id: "abc12345-x".into() }));
        assert_eq!(a.parse_line(r#"{"type":"session","id":"--evil-flag"}"#, &mut t), None);
        assert_eq!(a.parse_line(r#"{"type":"status","text":"Reading main.rs"}"#, &mut t), Some(AgentEvent::Status { text: "Reading main.rs".into() }));
        assert_eq!(a.parse_line("Fixed the bug in the parser.", &mut t), None);
        assert_eq!(a.parse_line(r#"{"type":"text","text":"Tests pass."}"#, &mut t), None);
        assert_eq!(t, "Fixed the bug in the parser.\nTests pass.");
        assert_eq!(a.parse_line(r#"{"type":"done"}"#, &mut t), Some(AgentEvent::Done { summary: "Fixed the bug in the parser. Tests pass.".into() }));
        assert_eq!(a.parse_line(r#"{"type":"done","summary":"All good."}"#, &mut t), Some(AgentEvent::Done { summary: "All good.".into() }));
        assert_eq!(a.parse_line(r#"{"type":"error","message":"no API key"}"#, &mut t), Some(AgentEvent::Failed { error: "no API key".into() }));
        assert_eq!(a.args(Path::new("/p"), Safety::Edit, Some("abc12345")), Vec::<String>::new(), "nothing on the command line");
    }

    #[test]
    fn collected_text_is_bounded() {
        let mut t = String::new();
        for _ in 0..100 {
            append(&mut t, &"é".repeat(100));
        }
        assert!(t.len() <= MAX_TEXT);
    }

    #[cfg(unix)]
    #[test]
    fn shells_and_interpreters_are_refused() {
        for p in ["/bin/sh", "/bin/zsh", "/bin/bash", "/usr/bin/python3", "/usr/bin/osascript", "/usr/bin/env"] {
            if Path::new(p).exists() {
                assert!(validate_command(Path::new(p)).is_err(), "{p}");
            }
        }
        assert!(validate_command(Path::new("relative/agent")).is_err());
        assert!(validate_command(Path::new("/nonexistent/agent")).is_err());
        assert!(validate_command(Path::new("/etc/hosts")).is_err(), "not executable");
        assert!(validate_command(Path::new("/bin/ls")).is_ok());
    }
}
