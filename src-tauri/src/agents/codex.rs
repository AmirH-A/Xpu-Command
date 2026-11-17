//! OpenAI Codex CLI, headless: `codex exec --json`, prompt on stdin ("-").
//! Sandbox: read-only, or workspace-write (edits only inside the project folder; no network).
use super::{clip, Adapter, AgentEvent, VOICE_NOTE};
use crate::settings::Safety;
use serde_json::Value;
use std::path::Path;

pub struct Codex;

impl Adapter for Codex {
    fn binary_names(&self) -> &'static [&'static str] {
        &["codex"]
    }

    fn args(&self, project: &Path, safety: Safety, resume: Option<&str>) -> Vec<String> {
        let sandbox = match safety {
            Safety::ReadOnly | Safety::AskCommands => "read-only",
            Safety::Edit => "workspace-write",
        };
        let mut a: Vec<String> = vec!["exec".into(), "--json".into(), "--sandbox".into(), sandbox.into(), "--cd".into(), project.display().to_string(), "--skip-git-repo-check".into()];
        if let Some(id) = resume {
            a.extend(["resume".to_string(), id.to_string()]);
        }
        a.push("-".into());
        a
    }

    fn stdin(&self, prompt: &str) -> String {
        format!("{VOICE_NOTE}\n\nRequest:\n{prompt}\n")
    }

    fn parse_line(&self, line: &str, last_text: &mut String) -> Option<AgentEvent> {
        let v: Value = serde_json::from_str(line).ok()?;
        let item = v.get("item");
        let item_type = item.and_then(|i| i.get("type")).and_then(Value::as_str);
        match v.get("type")?.as_str()? {
            "thread.started" => {
                let id = v.get("thread_id").and_then(Value::as_str)?;
                super::valid_session_id(id).then(|| AgentEvent::Session { id: id.to_string() })
            }
            "item.started" => match item_type? {
                "command_execution" => {
                    let cmd = item?.get("command").and_then(Value::as_str).unwrap_or("a command");
                    Some(AgentEvent::Status { text: clip(&crate::i18n::trf("Running: {command}", &[("command", cmd)]), 80) })
                }
                "file_change" => Some(AgentEvent::Status { text: crate::i18n::tr("Editing files").into() }),
                "reasoning" => Some(AgentEvent::Status { text: "Thinking".into() }),
                "web_search" => Some(AgentEvent::Status { text: crate::i18n::tr("Searching the web").into() }),
                _ => None,
            },
            "item.completed" => {
                if item_type == Some("agent_message") {
                    if let Some(t) = item?.get("text").and_then(Value::as_str) {
                        *last_text = t.to_string();
                    }
                }
                None
            }
            "turn.completed" => Some(AgentEvent::Done { summary: crate::sanitize::speakable_summary(last_text, 280) }),
            "turn.failed" => {
                let m = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("the agent failed");
                Some(AgentEvent::Failed { error: friendly_error(m) })
            }
            "error" => {
                let m = v.get("message").and_then(Value::as_str).unwrap_or("");
                let text = if m.starts_with("Reconnecting") { "Reconnecting…".to_string() } else { friendly_error(m) };
                Some(AgentEvent::Status { text })
            }
            _ => None,
        }
    }
}

fn friendly_error(m: &str) -> String {
    if m.contains("401") || m.contains("Unauthorized") {
        return crate::i18n::tr("Codex isn't signed in: run `codex login` in a terminal").into();
    }
    if m.contains("429") || m.to_lowercase().contains("rate limit") {
        return crate::i18n::tr("Codex is rate limited; try again shortly").into();
    }
    clip(m, 160)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_levels() {
        let ro = Codex.args(Path::new("/p"), Safety::ReadOnly, None);
        assert!(ro.windows(2).any(|w| w[0] == "--sandbox" && w[1] == "read-only"));
        let ed = Codex.args(Path::new("/p"), Safety::Edit, None);
        assert!(ed.windows(2).any(|w| w[0] == "--sandbox" && w[1] == "workspace-write"));
        assert!(!ed.iter().any(|x| x.contains("danger") || x.contains("full-access")));
        assert_eq!(ed.last().map(String::as_str), Some("-"), "prompt comes from stdin");
        let r = Codex.args(Path::new("/p"), Safety::Edit, Some("01a1d122-1b36-7790-b0f5-ea037d19318d"));
        let at = |x: &str| r.iter().position(|a| a == x).unwrap();
        assert!(at("--sandbox") < at("resume"), "sandbox applies to the resumed thread");
        assert_eq!(&r[at("resume") + 1], "01a1d122-1b36-7790-b0f5-ea037d19318d");
    }

    #[test]
    fn exec_json_events() {
        let mut last = String::new();
        let run = r#"{"type":"item.started","item":{"id":"1","type":"command_execution","command":"cargo test"}}"#;
        assert_eq!(Codex.parse_line(run, &mut last), Some(AgentEvent::Status { text: "Running: cargo test".into() }));
        let msg = r#"{"type":"item.completed","item":{"id":"2","type":"agent_message","text":"Added tests for the parser."}}"#;
        assert_eq!(Codex.parse_line(msg, &mut last), None);
        let done = r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#;
        assert_eq!(Codex.parse_line(done, &mut last), Some(AgentEvent::Done { summary: "Added tests for the parser.".into() }));
        let fail = r#"{"type":"turn.failed","error":{"message":"rate limited"}}"#;
        assert_eq!(Codex.parse_line(fail, &mut last), Some(AgentEvent::Failed { error: "Codex is rate limited; try again shortly".into() }));
        let thread = r#"{"type":"thread.started","thread_id":"01a1d122-1b36-7790-b0f5-ea037d19318d"}"#;
        assert_eq!(Codex.parse_line(thread, &mut last), Some(AgentEvent::Session { id: "01a1d122-1b36-7790-b0f5-ea037d19318d".into() }));
    }

    #[test]
    fn retries_are_not_failures_and_auth_errors_are_readable() {
        let mut last = String::new();
        let retry = r#"{"type":"error","message":"Reconnecting... 4/5 (unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: https://api.openai.com/v1/responses)"}"#;
        assert_eq!(Codex.parse_line(retry, &mut last), Some(AgentEvent::Status { text: "Reconnecting…".into() }));
        let fail = r#"{"type":"turn.failed","error":{"message":"unexpected status 401 Unauthorized: Missing bearer or basic authentication in header"}}"#;
        assert_eq!(Codex.parse_line(fail, &mut last), Some(AgentEvent::Failed { error: "Codex isn't signed in: run `codex login` in a terminal".into() }));
    }
}
