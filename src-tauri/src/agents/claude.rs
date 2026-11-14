//! Claude Code, headless: `claude -p --output-format stream-json`, prompt on stdin.
//! --restricted removes every tool that runs commands or code (Bash, PowerShell, …) and WebFetch,
//! and ignores the project's own settings files (a cloned repo can't add hooks that run code).
use super::{clip, file_name, Adapter, AgentEvent, VOICE_NOTE};
use crate::settings::Safety;
use serde_json::Value;
use std::path::Path;

pub struct Claude;

impl Adapter for Claude {
    fn binary_names(&self) -> &'static [&'static str] {
        &["claude"]
    }

    fn args(&self, _project: &Path, safety: Safety, resume: Option<&str>) -> Vec<String> {
        let mode = match safety {
            Safety::ReadOnly => "plan",
            Safety::Edit | Safety::AskCommands => "acceptEdits",
        };
        let mut a: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", mode, "--restricted", "--append-system-prompt", VOICE_NOTE]
            .iter()
            .map(|s| s.to_string())
            .collect();
        if let Some(id) = resume {
            a.extend(["--resume".to_string(), id.to_string()]);
        }
        a
    }

    fn stdin(&self, prompt: &str) -> String {
        prompt.to_string()
    }

    fn parse_line(&self, line: &str, last_text: &mut String) -> Option<AgentEvent> {
        let v: Value = serde_json::from_str(line).ok()?;
        match v.get("type")?.as_str()? {
            "system" if v.get("subtype").and_then(Value::as_str) == Some("init") => {
                let id = v.get("session_id").and_then(Value::as_str)?;
                super::valid_session_id(id).then(|| AgentEvent::Session { id: id.to_string() })
            }
            "assistant" => {
                let mut status = None;
                for block in v.pointer("/message/content")?.as_array()? {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = block.get("text").and_then(Value::as_str) {
                                *last_text = t.to_string();
                            }
                        }
                        Some("tool_use") => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                            if name.starts_with("mcp__mac__") {
                                continue;
                            }
                            let input = block.get("input");
                            let path = input.and_then(|i| i.get("file_path").or_else(|| i.get("path"))).and_then(Value::as_str).map(file_name);
                            status = Some(match (name, path) {
                                ("Read", Some(p)) => format!("Reading {p}"),
                                ("Edit" | "MultiEdit" | "Write" | "NotebookEdit", Some(p)) => format!("Editing {p}"),
                                ("Glob" | "Grep", _) => "Searching the project".to_string(),
                                ("TodoWrite", _) => "Planning".to_string(),
                                ("Task" | "Agent", _) => "Delegating a subtask".to_string(),
                                (n, _) => format!("Using {n}"),
                            });
                        }
                        _ => {}
                    }
                }
                status.map(|s| AgentEvent::Status { text: clip(&s, 80) })
            }
            "result" => {
                let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false) || v.get("subtype").and_then(Value::as_str) != Some("success");
                let text = v.get("result").and_then(Value::as_str).unwrap_or(last_text.as_str()).to_string();
                Some(if is_error {
                    AgentEvent::Failed { error: friendly_error(if text.is_empty() { "the agent reported an error" } else { &text }) }
                } else {
                    AgentEvent::Done { summary: crate::sanitize::speakable_summary(&text, 280) }
                })
            }
            _ => None,
        }
    }
}

/// The tools the "ask before commands" level offers. `--restricted` drops command tools unless
/// they are named here; each command still needs the user's yes.
pub const ASK_TOOLS: &str = "Bash,Read,Edit,Write,Glob,Grep,NotebookEdit,WebFetch,WebSearch,TodoWrite";

pub const ASK_NOTE: &str = " Commands and other actions outside editing files are approved by the user by voice, one at a time. \
If one is denied, don't retry it or work around it: say in one sentence what you wanted to do and why.";

/// "Edit files, ask me before commands": edits in the project are accepted; every other action that
/// needs permission (commands, web, files outside the project) goes to the user through our
/// approvals server (`--permission-prompt-tool`). User and project settings are still ignored
/// (`--restricted`), and no other MCP server is loaded (`--strict-mcp-config`).
pub fn ask_args(mcp_config: &Path, resume: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-p".into(), "--output-format".into(), "stream-json".into(), "--verbose".into(),
        "--permission-mode".into(), "acceptEdits".into(),
        "--restricted".into(), "--tools".into(), ASK_TOOLS.into(),
        "--strict-mcp-config".into(), "--mcp-config".into(), mcp_config.display().to_string(),
        "--permission-prompt-tool".into(), "mcp__xpu__approve".into(),
        "--disallowedTools".into(), "mcp__xpu__approve".into(),
        "--append-system-prompt".into(), format!("{VOICE_NOTE}{ASK_NOTE}"),
    ];
    if let Some(id) = resume {
        a.extend(["--resume".to_string(), id.to_string()]);
    }
    a
}

/// The Mac assistant: every built-in tool off (`--tools ""`), user and project settings ignored
/// (`--restricted`), only our MCP server (`--strict-mcp-config`), and only its tools allowed. In print
/// mode anything not allowed is denied, never prompted.
pub fn mac_args(m: &super::MacArgs<'_>, resume: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-p".into(), "--output-format".into(), "stream-json".into(), "--verbose".into(),
        "--model".into(), m.model.into(),
        "--tools".into(), String::new(),
        "--restricted".into(),
        "--strict-mcp-config".into(), "--mcp-config".into(), m.mcp_config.display().to_string(),
        "--allowedTools".into(), "mcp__mac".into(),
        "--permission-mode".into(), "default".into(),
        "--system-prompt".into(), m.system_prompt.into(),
    ];
    if let Some(id) = resume {
        a.extend(["--resume".to_string(), id.to_string()]);
    }
    a
}

/// Sign-in problems get an actionable message; with several profiles, the fix is per profile.
pub fn friendly_error(m: &str) -> String {
    let l = m.to_lowercase();
    if l.contains("oauth") || l.contains("authenticat") || l.contains("401") || l.contains("not logged in") || l.contains("login") {
        return "Claude Code isn't signed in for this profile: run it in a terminal (e.g. claude-work) and use /login, or pick another profile in Settings".into();
    }
    clip(m, 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_maps_to_sandbox_and_never_bypasses() {
        for s in [Safety::ReadOnly, Safety::Edit] {
            let a = Claude.args(Path::new("/p"), s, None);
            assert!(a.contains(&"--restricted".to_string()), "no command tools, no project settings");
            assert!(!a.iter().any(|x| x.contains("dangerously") || x == "bypassPermissions"));
        }
        assert!(Claude.args(Path::new("/p"), Safety::ReadOnly, None).contains(&"plan".to_string()));
        assert!(Claude.args(Path::new("/p"), Safety::Edit, None).contains(&"acceptEdits".to_string()));
        let r = Claude.args(Path::new("/p"), Safety::Edit, Some("0b7f3c1e-1111-2222-3333-444455556666"));
        assert!(r.windows(2).any(|w| w[0] == "--resume" && w[1] == "0b7f3c1e-1111-2222-3333-444455556666"));
        assert!(r.contains(&"--restricted".to_string()), "a resumed session keeps the same limits");
    }

    #[test]
    fn ask_level_routes_permissions_to_the_user() {
        let a = ask_args(Path::new("/x/mcp-2.json"), Some("0b7f3c1e-1111-2222-3333-444455556666"));
        let after = |f: &str| a.iter().position(|x| x == f).map(|i| a[i + 1].clone());
        assert_eq!(after("--permission-prompt-tool").as_deref(), Some("mcp__xpu__approve"));
        assert_eq!(after("--permission-mode").as_deref(), Some("acceptEdits"));
        assert!(after("--tools").unwrap().split(',').any(|t| t == "Bash"), "commands are possible, each asked");
        assert_eq!(after("--mcp-config").as_deref(), Some("/x/mcp-2.json"));
        assert!(a.contains(&"--restricted".to_string()) && a.contains(&"--strict-mcp-config".to_string()));
        assert!(!a.iter().any(|x| x.contains("dangerously") || x == "bypassPermissions"));
        assert!(a.windows(2).any(|w| w[0] == "--resume"));
    }

    #[test]
    fn mac_assistant_has_only_our_tools() {
        let m = super::super::MacArgs { mcp_config: Path::new("/x/mcp-1.json"), model: "haiku", system_prompt: "SP" };
        let a = mac_args(&m, None);
        let after = |f: &str| a.iter().position(|x| x == f).map(|i| a[i + 1].clone());
        assert_eq!(after("--tools").as_deref(), Some(""), "no built-in tools");
        assert_eq!(after("--allowedTools").as_deref(), Some("mcp__mac"));
        assert_eq!(after("--mcp-config").as_deref(), Some("/x/mcp-1.json"));
        assert_eq!(after("--model").as_deref(), Some("haiku"));
        assert!(a.contains(&"--strict-mcp-config".to_string()) && a.contains(&"--restricted".to_string()));
        assert!(!a.iter().any(|x| x.contains("dangerously") || x == "bypassPermissions" || x == "acceptEdits"));
        let mut last = String::new();
        let mac_tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"mcp__mac__open_app","input":{"name":"Spotify"}}]}}"#;
        assert_eq!(Claude.parse_line(mac_tool, &mut last), None, "the tool server shows its own status");
    }

    #[test]
    fn prompt_is_not_in_argv() {
        let a = Claude.args(Path::new("/p"), Safety::Edit, None);
        assert!(!a.iter().any(|x| x.contains("secret prompt")));
        assert_eq!(Claude.stdin("secret prompt"), "secret prompt");
    }

    #[test]
    fn stream_json_events() {
        let mut last = String::new();
        let tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/p/src/auth.rs"}}]}}"#;
        assert_eq!(Claude.parse_line(tool, &mut last), Some(AgentEvent::Status { text: "Editing auth.rs".into() }));
        let text = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Working on it"}]}}"#;
        assert_eq!(Claude.parse_line(text, &mut last), None);
        assert_eq!(last, "Working on it");
        let done = r#"{"type":"result","subtype":"success","is_error":false,"result":"Fixed the login bug.\nTests pass."}"#;
        assert_eq!(Claude.parse_line(done, &mut last), Some(AgentEvent::Done { summary: "Fixed the login bug. Tests pass.".into() }));
        let fail = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":""}"#;
        assert!(matches!(Claude.parse_line(fail, &mut last), Some(AgentEvent::Failed { .. })));
        assert_eq!(Claude.parse_line("not json", &mut last), None);
        assert_eq!(Claude.parse_line(r#"{"type":"system","subtype":"init"}"#, &mut last), None);
        let init = r#"{"type":"system","subtype":"init","session_id":"0b7f3c1e-1111-2222-3333-444455556666"}"#;
        assert_eq!(Claude.parse_line(init, &mut last), Some(AgentEvent::Session { id: "0b7f3c1e-1111-2222-3333-444455556666".into() }));
        let evil = r#"{"type":"system","subtype":"init","session_id":"--dangerously-skip-permissions"}"#;
        assert_eq!(Claude.parse_line(evil, &mut last), None, "an id that looks like a flag is dropped");
    }

    #[test]
    fn expired_login_is_explained() {
        let mut last = String::new();
        let e = r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate. API Error: 401 OAuth session expired and could not be refreshed"}"#;
        match Claude.parse_line(e, &mut last) {
            Some(AgentEvent::Failed { error }) => assert!(error.starts_with("Claude Code isn't signed in"), "{error}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn hostile_output_is_neutralised() {
        let mut last = String::new();
        let evil = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/p/a\u001b[2J\nrm -rf.rs"}}]}}"#;
        match Claude.parse_line(evil, &mut last) {
            Some(AgentEvent::Status { text }) => assert!(!text.chars().any(char::is_control), "{text:?}"),
            other => panic!("{other:?}"),
        }
    }
}
