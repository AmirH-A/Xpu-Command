//! The benchmark: real tasks run end to end through the same pipeline people use (context,
//! consent, gate, Claude Code, the Mac tools), each scored pass or fail by checking the result on
//! the Mac, with its time and steps. Questions are answered yes automatically; nothing is sent.
//!
//!   xpu-command --selftest-bench [task-id…]
//!
//! Needs a terminal allowed in Accessibility (and Screen Recording for screenshots). Files live in
//! ~/XpuBench, made fresh for each task and removed at the end; reminders, notes, events, renames
//! and moves are undone after each task.
use crate::mac::server::{Ask, Host, Outcome};
use crate::trace::{Builder, Route, Stats, Trace};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TASKS: &str = include_str!("../bench/tasks.json");
const DIR: &str = "XpuBench";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub say: String,
    /// Apps that must be installed, else the task is skipped.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Files to create in ~/XpuBench first: name → text.
    #[serde(default)]
    pub files: std::collections::BTreeMap<String, String>,
    /// Tool calls run directly before the request (no gate), e.g. quit an app.
    #[serde(default)]
    pub setup: Vec<(String, Value)>,
    #[serde(default)]
    pub expect: Vec<Expect>,
    /// Undo what the request changed, afterwards.
    #[serde(default)]
    pub undo: bool,
    /// Tool calls run directly afterwards.
    #[serde(default)]
    pub cleanup: Vec<(String, Value)>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Expect {
    /// The answer contains one of these (case-insensitive).
    Answer(Vec<String>),
    /// This tool ran and succeeded.
    Tool(String),
    /// This app is in front afterwards.
    Front(String),
    /// This path in ~/XpuBench exists (true) or not (false) afterwards.
    File(String, bool),
    /// The app's window shows this text.
    Window(String, String),
}

pub fn tasks() -> Vec<Task> {
    serde_json::from_str(TASKS).expect("bench/tasks.json is valid")
}

/// Answers every question with yes (never presses to cancel), and records the trace.
struct BenchHost {
    trace: Mutex<Option<Builder>>,
    log: Mutex<Vec<String>>,
}

impl Host for BenchHost {
    fn status(&self, _text: &str) {}
    fn ask(&self, ask: Ask, text: &str) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.log.lock().push(format!("asked {ask:?}: {text}"));
        match ask {
            Ask::Approve => drop(tx.send(())),
            Ask::Cancel => std::mem::forget(tx),
        }
        rx
    }
    fn end_ask(&self) {}
    fn record(&self, _tool: &str, _summary: &str, _outcome: Outcome, _detail: &str) {}
    fn highlight(&self, _frame: Option<(f64, f64, f64, f64)>) {}
    fn traced(&self, tool: &str, outcome: Outcome, ms: u64, detail: &str) {
        if let Some(t) = self.trace.lock().as_mut() {
            t.step(tool, outcome, ms, detail);
        }
    }
    fn grant(&self, question: &str) -> tokio::sync::oneshot::Receiver<crate::agents::approvals::Answer> {
        self.log.lock().push(format!("granted: {question}"));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(crate::agents::approvals::Answer::Yes);
        rx
    }
}

fn bench_dir() -> PathBuf {
    dirs::home_dir().expect("home folder").join(DIR)
}

/// Runs one tool directly, as setup or cleanup.
fn direct(exec: &crate::mac::exec::MacExec, tool: &str, args: &Value) -> Result<String, String> {
    use crate::mac::server::{Exec, Output};
    let call = crate::mac::tools::parse(tool, args, chrono::Local::now().naive_local())?;
    let target = exec.target(&call)?;
    match exec.run(&call, target.as_ref())? {
        Output::Text(t) => Ok(t),
        Output::Image { text, .. } => Ok(text),
    }
}

fn front_app() -> Option<String> {
    crate::mac::ui::front_pid().and_then(crate::mac::ui::app_of_pid).map(|a| a.name)
}

/// Checks one expectation; Err says what was wrong.
fn check(e: &Expect, t: &Trace, exec: &crate::mac::exec::MacExec) -> Result<(), String> {
    match e {
        Expect::Answer(any) => {
            let a = t.result.to_lowercase();
            any.iter().any(|w| a.contains(&w.to_lowercase())).then_some(()).ok_or_else(|| format!("answer lacks {any:?}: “{}”", t.result))
        }
        Expect::Tool(tool) => t.steps.iter().any(|s| s.tool == *tool && s.outcome == Outcome::Done).then_some(()).ok_or_else(|| format!("{tool} didn't run")),
        Expect::Front(app) => match front_app() {
            Some(f) if f.to_lowercase().contains(&app.to_lowercase()) => Ok(()),
            other => Err(format!("{app} isn't in front ({other:?} is)")),
        },
        Expect::File(rel, exists) => {
            let p = bench_dir().join(rel);
            (p.exists() == *exists).then_some(()).ok_or_else(|| format!("{} {}", rel, if *exists { "is missing" } else { "still exists" }))
        }
        Expect::Window(app, text) => {
            let out = direct(exec, "wait", &serde_json::json!({ "app": app, "text": text, "seconds": 4 }))?;
            out.contains("is showing").then_some(()).ok_or_else(|| format!("{app} doesn't show “{text}”"))
        }
    }
}

pub struct TaskResult {
    pub id: String,
    pub passed: bool,
    pub skipped: bool,
    pub why: String,
    pub trace: Option<Trace>,
}

async fn run_task(task: &Task, model: crate::settings::MacModel, profile: Option<PathBuf>) -> TaskResult {
    let installed = crate::mac::apps::installed();
    if let Some(missing) = task.requires.iter().find(|r| crate::mac::apps::find(&installed, r).is_err()) {
        return TaskResult { id: task.id.clone(), passed: false, skipped: true, why: format!("{missing} isn't installed"), trace: None };
    }
    let dir = bench_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("make ~/XpuBench");
    for (name, text) in &task.files {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("make a bench folder");
        }
        std::fs::write(p, text).expect("write a bench file");
    }
    let mut policy = crate::mac::policy::Policy { allow_all: true, ..Default::default() };
    policy.levels.insert(crate::mac::policy::FILES.into(), crate::mac::policy::Access::Control);
    let tools = crate::mac::exec::MacExec::new(policy.clone(), crate::mac::workspace().expect("workspace")).without_learning();
    for (tool, args) in &task.setup {
        if let Err(e) = direct(&tools, tool, args) {
            log::info!("setup {tool}: {e}");
        }
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    let host = Arc::new(BenchHost { trace: Mutex::new(Some(Builder::new(&task.say, Route::Assistant, Instant::now(), None))), log: Mutex::new(Vec::new()) });
    let opts = crate::mac::pipeline::Options {
        text: task.say.clone(),
        policy,
        share_context: true,
        memory: false,
        model,
        language: "en".into(),
        profile,
        resume: None,
        limits: crate::mac::server::Limits { grace: Duration::from_millis(300), ..Default::default() },
    };
    let started = match crate::mac::pipeline::start(host.clone(), opts).await {
        Ok(s) => s,
        Err(e) => return TaskResult { id: task.id.clone(), passed: false, skipped: false, why: format!("didn't start: {e}"), trace: None },
    };
    if let Some(t) = host.trace.lock().as_mut() {
        t.model(crate::mac::model_name(started.model));
    }
    let (mut run, keep) = (started.run, started.keep);
    let mut result: Option<(bool, String)> = None;
    while let Some(ev) = run.events.recv().await {
        match ev {
            crate::agents::AgentEvent::Done { summary } => result = Some((true, summary)),
            crate::agents::AgentEvent::Failed { error } => result = Some((false, error)),
            _ => {}
        }
        if result.is_some() {
            break;
        }
    }
    drop(keep);
    let (ok, said) = result.unwrap_or((false, "no answer".into()));
    let trace = host.trace.lock().take().expect("trace").finish(ok, &said);
    let failed: Vec<String> = if ok { task.expect.iter().filter_map(|e| check(e, &trace, &tools).err()).collect() } else { vec![said.clone()] };
    if task.undo {
        let undone = tokio::task::spawn_blocking(crate::mac::exec::undo_last).await.unwrap_or_default();
        log::info!("undo: {undone}");
    }
    for (tool, args) in &task.cleanup {
        if let Err(e) = direct(&tools, tool, args) {
            log::info!("cleanup {tool}: {e}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let mut trace = trace;
    trace.ok = failed.is_empty();
    TaskResult { id: task.id.clone(), passed: failed.is_empty(), skipped: false, why: failed.join("; "), trace: Some(trace) }
}

/// Runs the tasks (all, or those named), prints a report, saves the results, and returns whether
/// every target was met.
pub fn run(only: &[String]) -> bool {
    let s = crate::settings::load();
    let model = match std::env::var("XPU_MODEL").as_deref() {
        Ok("haiku") => crate::settings::MacModel::Haiku,
        Ok("sonnet") => crate::settings::MacModel::Sonnet,
        Ok("auto") => crate::settings::MacModel::Auto,
        _ => s.mac_model,
    };
    let profile = s.claude_profile.as_deref().and_then(|p| crate::settings::claude_profile_dir(p).ok());
    if !crate::permissions::accessibility_trusted(false) {
        eprintln!("This terminal isn't allowed in System Settings › Privacy & Security › Accessibility; window tasks will fail.");
    }
    let all = tasks();
    let picked: Vec<&Task> = all.iter().filter(|t| only.is_empty() || only.contains(&t.id)).collect();
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let mut outcomes = Vec::new();
    println!("Benchmark: {} tasks, model {model:?}\n", picked.len());
    for task in picked {
        let o = rt.block_on(run_task(task, model, profile.clone()));
        let line = match &o.trace {
            Some(t) => format!(
                "{} {:<22} {:>5.1} s  {:>2} steps  {:<6}  {}",
                if o.passed { "PASS" } else { "FAIL" },
                o.id,
                t.total_ms as f64 / 1000.0,
                t.steps.len(),
                t.model.as_deref().unwrap_or(""),
                if o.passed { String::new() } else { crate::agents::clip(&o.why, 140) }
            ),
            None => format!("{} {:<22} {}", if o.skipped { "SKIP" } else { "FAIL" }, o.id, o.why),
        };
        println!("{line}");
        outcomes.push(o);
    }
    let traces: Vec<&Trace> = outcomes.iter().filter_map(|o| o.trace.as_ref()).collect();
    let failed_starts = outcomes.iter().filter(|o| !o.skipped && o.trace.is_none()).count();
    let mut stats = Stats::of(traces.iter().copied());
    stats.count += failed_starts;
    println!("\n{} ran, {} skipped", stats.count, outcomes.iter().filter(|o| o.skipped).count());
    println!("one step:   {} tasks, typical {:.1} s, slow {:.1} s, slowest {:.1} s", stats.one_step.count, stats.one_step.p50_ms as f64 / 1e3, stats.one_step.p90_ms as f64 / 1e3, stats.one_step.max_ms as f64 / 1e3);
    println!("multi-step: {} tasks, typical {:.1} s, slow {:.1} s, slowest {:.1} s", stats.multi_step.count, stats.multi_step.p50_ms as f64 / 1e3, stats.multi_step.p90_ms as f64 / 1e3, stats.multi_step.max_ms as f64 / 1e3);
    let verdicts = stats.verdicts();
    for (v, ok) in &verdicts {
        println!("{} {v}", if *ok { "MET " } else { "MISS" });
    }
    let report = serde_json::json!({
        "at": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "model": format!("{model:?}"),
        "stats": stats,
        "tasks": outcomes.iter().map(|o| serde_json::json!({ "id": o.id, "passed": o.passed, "skipped": o.skipped, "why": o.why, "trace": o.trace })).collect::<Vec<_>>(),
    });
    if let Ok(dir) = crate::settings::config_dir().map(|d| d.join("bench")) {
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        if std::fs::write(&file, serde_json::to_vec_pretty(&report).unwrap_or_default()).is_ok() {
            println!("\nSaved {}", file.display());
        }
    }
    verdicts.iter().all(|(_, ok)| *ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_list_is_valid_and_safe() {
        let t = tasks();
        assert!(t.len() >= 30, "{} tasks", t.len());
        let mut ids: Vec<&str> = t.iter().map(|x| x.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), t.len(), "ids are unique");
        for task in &t {
            assert!(!task.expect.is_empty(), "{} checks nothing", task.id);
            for (tool, args) in task.setup.iter().chain(&task.cleanup) {
                crate::mac::tools::parse(tool, args, chrono::Local::now().naive_local()).unwrap_or_else(|e| panic!("{}: {tool}: {e}", task.id));
            }
            let say = task.say.to_lowercase();
            for word in ["send", "pay", "buy", "delete", "trash", "post"] {
                assert!(!say.split_whitespace().any(|w| w == word), "{} must not {word} anything", task.id);
            }
        }
    }
}
