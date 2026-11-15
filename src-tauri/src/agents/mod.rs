//! Mode B: run a coding agent on the project folder and follow it.
//!
//! Security model:
//! * The agent binary is resolved to an absolute path; it is spawned directly, never via a shell.
//! * The prompt goes in on stdin, not argv (argv is visible to every local process).
//! * Its working directory is the user's validated project folder.
//! * Safety levels map to each agent's own sandbox: read-only, or edit-only with no command tools.
//!   There is no "bypass permissions" level.
//! * Everything the agent prints is untrusted: it is parsed as JSON, and only short, cleaned
//!   strings reach the UI (rendered as text, never HTML) or the speech synthesizer.
pub mod approvals;
pub mod claude;
pub mod codex;

use crate::error::{msg, Result};
use crate::settings::{AgentKind, Safety};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

pub const VOICE_NOTE: &str = "The user's request was spoken aloud and transcribed automatically, so names of files, functions and \
variables may be misheard: match them to what exists in the project. When you finish, end with one or two plain sentences \
(no markdown, no code) saying what you did; they will be read aloud. Answer in the language the user spoke.";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The agent's conversation id, so the next request can continue it.
    Session { id: String },
    Status { text: String },
    Done { summary: String },
    Failed { error: String },
}

/// Session ids come from agent output: accept only plain id characters, so one can never become
/// an extra command-line flag when passed back to `--resume`.
pub fn valid_session_id(id: &str) -> bool {
    (8..=128).contains(&id.len()) && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What one agent needs: how to build its command line, and how to read one line of its output.
pub trait Adapter: Send + Sync {
    fn binary_names(&self) -> &'static [&'static str];
    /// `resume`: a session id from an earlier run (already validated) to continue.
    fn args(&self, project: &Path, safety: Safety, resume: Option<&str>) -> Vec<String>;
    /// Text written to the agent's stdin (the prompt).
    fn stdin(&self, prompt: &str) -> String;
    fn parse_line(&self, line: &str, last_text: &mut String) -> Option<AgentEvent>;
}

pub fn adapter(kind: AgentKind) -> &'static dyn Adapter {
    match kind {
        AgentKind::Claude => &claude::Claude,
        AgentKind::Codex => &codex::Codex,
    }
}

/// Short, display-safe text from agent output.
pub fn clip(s: &str, max: usize) -> String {
    let c = crate::sanitize::clean_transcript(s);
    if c.chars().count() <= max {
        c
    } else {
        format!("{}…", c.chars().take(max).collect::<String>())
    }
}

/// Truncates without the transcript cleaning `clip` does (keeps line breaks), for long tool output.
pub fn clip_raw(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

pub fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string())
}

fn search_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(h) = dirs::home_dir() {
        for d in [".local/bin", ".claude/local", ".npm-global/bin", ".volta/bin", ".bun/bin", "bin"] {
            v.push(h.join(d));
        }
        #[cfg(windows)]
        if let Some(a) = dirs::data_dir() {
            v.push(a.join("npm"));
        }
    }
    #[cfg(not(windows))]
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        v.push(PathBuf::from(d));
    }
    if let Some(p) = std::env::var_os("PATH") {
        v.extend(std::env::split_paths(&p));
    }
    v
}

pub fn find_binary(kind: AgentKind) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd"] } else { &[""] };
    for dir in search_dirs() {
        if !dir.is_absolute() {
            continue;
        }
        for name in adapter(kind).binary_names() {
            for ext in exts {
                let p = dir.join(format!("{name}{ext}"));
                if is_executable(&p) {
                    return p.canonicalize().ok();
                }
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}
#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// The PATH the agent gets: the user's PATH plus the install places above (agents written in Node
/// need `node`, which a GUI app's PATH usually lacks).
fn child_path(bin: &Path) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = bin.parent().map(|p| vec![p.to_path_buf()]).unwrap_or_default();
    dirs.extend(search_dirs().into_iter().filter(|d| d.is_absolute()));
    dirs.dedup();
    std::env::join_paths(dirs).unwrap_or_default()
}

pub struct Run {
    pub events: mpsc::Receiver<AgentEvent>,
    pub cancel: oneshot::Sender<()>,
}

pub struct Request<'a> {
    pub kind: AgentKind,
    pub prompt: &'a str,
    pub project: &'a Path,
    pub safety: Safety,
    pub resume: Option<&'a str>,
    pub timeout: std::time::Duration,
    /// Claude only: the profile folder to use (CLAUDE_CONFIG_DIR), already validated.
    pub claude_config_dir: Option<&'a Path>,
    /// Claude only: run as the Mac assistant (our tools only) instead of a coding agent.
    pub mac: Option<MacArgs<'a>>,
    /// Claude with `Safety::AskCommands`: the MCP config of this run's approvals server.
    pub approvals: Option<&'a Path>,
}

pub struct MacArgs<'a> {
    pub mcp_config: &'a Path,
    pub model: &'a str,
    pub system_prompt: &'a str,
}

pub fn start(req: Request<'_>) -> Result<Run> {
    let bin = find_binary(req.kind).ok_or_else(|| msg(format!("{} CLI not found; install it first", name(req.kind))))?;
    start_with(&bin, req)
}

/// Every child process the app starts goes through this lock. On macOS, std creates a child's pipes
/// and only then marks them close-on-exec; a process spawned by another thread in that gap inherits
/// them. If it inherits an agent's stdin, the agent never sees end-of-input and hangs. Serializing
/// spawns closes that gap (found by a test that failed about once in 50 runs).
pub static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn name(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude Code",
        AgentKind::Codex => "Codex",
    }
}

/// `start` with an explicit binary (tests use a fake agent).
pub fn start_with(bin: &Path, req: Request<'_>) -> Result<Run> {
    let project = crate::settings::validate_project_dir(req.project)?;
    let resume = req.resume.filter(|id| valid_session_id(id));
    let (kind, prompt, safety, timeout) = (req.kind, req.prompt, req.safety, req.timeout);
    let claude_config_dir = req.claude_config_dir.map(Path::to_path_buf);
    let bin = bin.to_path_buf();
    let a = adapter(kind);
    let args = match (&req.mac, kind) {
        (Some(m), AgentKind::Claude) => claude::mac_args(m, resume),
        (Some(_), _) => return Err(msg("Control my Mac uses Claude Code")),
        (None, AgentKind::Claude) if safety == Safety::AskCommands => {
            let cfg = req.approvals.ok_or_else(|| msg("voice approvals need their server"))?;
            claude::ask_args(cfg, resume)
        }
        (None, _) => a.args(&project, safety, resume),
    };
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(args)
        .current_dir(&project)
        .env("PATH", child_path(&bin))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let (AgentKind::Claude, Some(dir)) = (kind, &claude_config_dir) {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = {
        let _guard = SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        cmd.spawn().map_err(|e| msg(format!("could not start {}: {e}", name(kind))))?
    };
    let input = a.stdin(prompt);
    let mut stdin = child.stdin.take().ok_or_else(|| msg("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| msg("no stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| msg("no stderr"))?;
    let (tx, rx) = mpsc::channel(64);
    let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let _ = stdin.write_all(input.as_bytes()).await;
        let _ = stdin.shutdown().await;
    });
    let err_tail = std::sync::Arc::new(parking_lot::Mutex::new(String::new()));
    let et = err_tail.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            let mut t = et.lock();
            *t = clip(&l, 300);
        }
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        let mut last_text = String::new();
        let mut finished = false;
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut cancel_rx => {
                    let _ = child.kill().await;
                    let _ = tx.send(AgentEvent::Failed { error: "stopped".into() }).await;
                    return;
                }
                _ = &mut deadline => {
                    let _ = child.kill().await;
                    let _ = tx.send(AgentEvent::Failed { error: "timed out".into() }).await;
                    return;
                }
                line = lines.next_line() => match line {
                    Ok(Some(l)) => {
                        if l.len() > 4 * 1024 * 1024 { continue; }
                        if let Some(ev) = a.parse_line(&l, &mut last_text) {
                            let terminal = matches!(ev, AgentEvent::Done { .. } | AgentEvent::Failed { .. });
                            if tx.send(ev).await.is_err() { let _ = child.kill().await; return; }
                            if terminal {
                                finished = true;
                                break;
                            }
                        }
                    }
                    _ => break,
                }
            }
        }
        if finished {
            if tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await.is_err() {
                let _ = child.kill().await;
            }
            return;
        }
        let status = child.wait().await;
        if !finished {
            let ok = status.map(|s| s.success()).unwrap_or(false);
            let ev = if ok && !last_text.is_empty() {
                AgentEvent::Done { summary: crate::sanitize::speakable_summary(&last_text, 280) }
            } else {
                let tail = err_tail.lock().clone();
                let error = if tail.is_empty() {
                    "the agent exited without an answer".to_string()
                } else if kind == AgentKind::Claude {
                    claude::friendly_error(&tail)
                } else {
                    tail
                };
                AgentEvent::Failed { error }
            };
            let _ = tx.send(ev).await;
        }
    });
    Ok(Run { events: rx, cancel: cancel_tx })
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub kind: AgentKind,
    pub found: bool,
    pub path: Option<String>,
}

pub fn status() -> Vec<AgentInfo> {
    [AgentKind::Claude, AgentKind::Codex]
        .into_iter()
        .map(|k| {
            let p = find_binary(k);
            AgentInfo { kind: k, found: p.is_some(), path: p.map(|p| p.display().to_string()) }
        })
        .collect()
}

#[cfg(all(test, unix))]
mod runner_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    struct Fixture {
        dir: PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    fn fixture(script: &str) -> (Fixture, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("xpu-agent-{}", uuid::Uuid::new_v4())).canonicalize_or_create();
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let bin = dir.join("fake-agent");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (Fixture { dir }, bin, project)
    }
    trait Canon {
        fn canonicalize_or_create(self) -> PathBuf;
    }
    impl Canon for PathBuf {
        fn canonicalize_or_create(self) -> PathBuf {
            std::fs::create_dir_all(&self).unwrap();
            self.canonicalize().unwrap()
        }
    }
    fn req<'a>(project: &'a Path, resume: Option<&'a str>, timeout: Duration) -> Request<'a> {
        Request { kind: AgentKind::Claude, prompt: "secret spoken prompt", project, safety: Safety::Edit, resume, timeout, claude_config_dir: None, mac: None, approvals: None }
    }
    async fn collect(mut run: Run) -> Vec<AgentEvent> {
        let mut v = Vec::new();
        while let Some(e) = run.events.recv().await {
            v.push(e);
        }
        v
    }

    #[tokio::test]
    async fn happy_path_prompt_on_stdin_and_cwd_is_the_project() {
        let (_f, bin, project) = fixture(
            r#"cat > "$PWD/stdin.txt"; printf '%s\n' "$@" > "$PWD/argv.txt"
echo '{"type":"system","subtype":"init","session_id":"11111111-2222-3333-4444-555555555555"}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/x/main.rs"}}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"Renamed the function. Tests pass."}'"#,
        );
        let ev = collect(start_with(&bin, req(&project, None, Duration::from_secs(10))).unwrap()).await;
        assert_eq!(
            ev,
            vec![
                AgentEvent::Session { id: "11111111-2222-3333-4444-555555555555".into() },
                AgentEvent::Status { text: "Editing main.rs".into() },
                AgentEvent::Done { summary: "Renamed the function. Tests pass.".into() },
            ]
        );
        assert_eq!(std::fs::read_to_string(project.join("stdin.txt")).unwrap(), "secret spoken prompt", "the agent ran in the project and got the prompt on stdin");
        let argv = std::fs::read_to_string(project.join("argv.txt")).unwrap();
        assert!(!argv.contains("secret spoken prompt"), "the prompt is not on the command line");
        assert!(argv.contains("--restricted"));
    }

    #[tokio::test]
    async fn resume_id_is_passed_only_if_it_is_a_plain_id() {
        let (_f, bin, project) = fixture(r#"cat >/dev/null; printf '%s\n' "$@" > "$PWD/argv.txt"; echo '{"type":"result","subtype":"success","result":"ok"}'"#);
        collect(start_with(&bin, req(&project, Some("11111111-2222-3333-4444-555555555555"), Duration::from_secs(10))).unwrap()).await;
        assert!(std::fs::read_to_string(project.join("argv.txt")).unwrap().contains("--resume\n11111111-2222-3333-4444-555555555555"));
        collect(start_with(&bin, req(&project, Some("--dangerously-skip-permissions"), Duration::from_secs(10))).unwrap()).await;
        let argv = std::fs::read_to_string(project.join("argv.txt")).unwrap();
        assert!(!argv.contains("--resume") && !argv.contains("dangerously"), "{argv}");
    }

    #[tokio::test]
    async fn claude_profile_is_passed_as_config_dir() {
        let (_f, bin, project) = fixture(r#"cat >/dev/null; echo "$CLAUDE_CONFIG_DIR" > "$PWD/profile.txt"; echo '{"type":"result","subtype":"success","result":"ok"}'"#);
        let profile = project.parent().unwrap().join("profile-dir");
        std::fs::create_dir_all(&profile).unwrap();
        let mut r = req(&project, None, Duration::from_secs(10));
        r.claude_config_dir = Some(&profile);
        collect(start_with(&bin, r).unwrap()).await;
        assert_eq!(std::fs::read_to_string(project.join("profile.txt")).unwrap().trim(), profile.display().to_string());
    }

    #[tokio::test]
    async fn a_crash_reports_the_last_stderr_line() {
        let (_f, bin, project) = fixture("cat >/dev/null; echo 'warming up' >&2; echo 'Error: disk full' >&2; exit 1");
        let ev = collect(start_with(&bin, req(&project, None, Duration::from_secs(10))).unwrap()).await;
        assert_eq!(ev, vec![AgentEvent::Failed { error: "Error: disk full".into() }]);
    }

    #[tokio::test]
    async fn an_expired_login_says_which_fix() {
        let (_f, bin, project) = fixture("cat >/dev/null; echo 'OAuth session expired and could not be refreshed' >&2; exit 1");
        match collect(start_with(&bin, req(&project, None, Duration::from_secs(10))).unwrap()).await.as_slice() {
            [AgentEvent::Failed { error }] => assert!(error.starts_with("Claude Code isn't signed in"), "{error}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_kills_the_agent() {
        let (_f, bin, project) = fixture(r#"echo $$ > "$PWD/pid.tmp"; mv "$PWD/pid.tmp" "$PWD/pid"; cat >/dev/null; sleep 30"#);
        let started = std::time::Instant::now();
        let run = start_with(&bin, req(&project, None, Duration::from_secs(5))).unwrap();
        let pid_file = project.join("pid");
        while !pid_file.exists() {
            assert!(started.elapsed() < Duration::from_secs(5), "the fake agent never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let ev = collect(run).await;
        assert_eq!(ev, vec![AgentEvent::Failed { error: "timed out".into() }]);
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let alive = std::process::Command::new("kill").args(["-0", pid.trim()]).status().unwrap().success();
        assert!(!alive, "the agent process is gone");
    }

    #[tokio::test]
    async fn stop_kills_the_agent() {
        let (_f, bin, project) = fixture("cat >/dev/null; sleep 30");
        let run = start_with(&bin, req(&project, None, Duration::from_secs(60))).unwrap();
        run.cancel.send(()).unwrap();
        let mut events = run.events;
        assert_eq!(events.recv().await, Some(AgentEvent::Failed { error: "stopped".into() }));
    }

    #[tokio::test]
    async fn project_folder_is_validated() {
        let (_f, bin, _project) = fixture("exit 0");
        let home = dirs::home_dir().unwrap();
        assert!(start_with(&bin, req(&home, None, Duration::from_secs(1))).is_err(), "never the whole home folder");
        assert!(start_with(&bin, req(Path::new("/"), None, Duration::from_secs(1))).is_err());
    }
}
