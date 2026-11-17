//! One trace per request: what was asked, how it was handled, every step with its time and
//! outcome, and how long it all took. Kept on this Mac only (the last few hundred requests, in
//! the private config folder), so speed and reliability are measured from real use, and the
//! benchmark (`bench`) reports the same numbers.
use crate::mac::server::Outcome;
use serde::{Deserialize, Serialize};
use std::time::Instant;

const MAX_KEPT: usize = 500;
const FILE: &str = "traces.jsonl";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub tool: String,
    pub outcome: Outcome,
    pub ms: u64,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Typed where the cursor is.
    Typed,
    /// A spoken edit of the last dictation.
    Edit,
    /// Run at once, without the AI.
    Instant,
    Routine,
    /// Control my Mac, planned by the AI.
    Assistant,
    /// A coding agent on a project.
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub at: String,
    pub request: String,
    pub route: Route,
    pub model: Option<String>,
    /// Speech to text, when the request was spoken.
    pub stt_ms: Option<u64>,
    /// From the end of speech (or the start of the request) until the first step began.
    pub first_step_ms: Option<u64>,
    pub total_ms: u64,
    pub steps: Vec<Step>,
    pub ok: bool,
    /// The spoken answer, or the error.
    pub result: String,
}

impl Trace {
    /// Several steps (or a plan) count as a multi-step request.
    pub fn multi_step(&self) -> bool {
        self.steps.iter().filter(|s| s.tool != "plan").count() > 1
    }
}

/// Collects a trace while a request runs.
#[derive(Debug, Clone)]
pub struct Builder {
    started: Instant,
    request: String,
    route: Route,
    model: Option<String>,
    stt_ms: Option<u64>,
    first_step_ms: Option<u64>,
    steps: Vec<Step>,
}

impl Builder {
    /// `started`: when the user stopped talking (or the request began); `stt_ms`: how long speech
    /// to text took within that.
    pub fn new(request: &str, route: Route, started: Instant, stt_ms: Option<u64>) -> Self {
        Self { started, request: crate::agents::clip(request, 300), route, model: None, stt_ms, first_step_ms: None, steps: Vec::new() }
    }

    pub fn route(&mut self, route: Route) {
        self.route = route;
    }

    pub fn model(&mut self, model: &str) {
        self.model = Some(model.to_string());
    }

    /// A step that just ended after `ms`.
    pub fn step(&mut self, tool: &str, outcome: Outcome, ms: u64, detail: &str) {
        if self.first_step_ms.is_none() {
            self.first_step_ms = Some((self.started.elapsed().as_millis() as u64).saturating_sub(ms));
        }
        self.steps.push(Step { tool: tool.to_string(), outcome, ms, detail: crate::agents::clip(detail, 160) });
    }

    pub fn finish(self, ok: bool, result: &str) -> Trace {
        Trace {
            at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            request: self.request,
            route: self.route,
            model: self.model,
            stt_ms: self.stt_ms,
            first_step_ms: self.first_step_ms,
            total_ms: self.started.elapsed().as_millis() as u64,
            steps: self.steps,
            ok,
            result: crate::agents::clip(result, 300),
        }
    }
}

fn path() -> crate::error::Result<std::path::PathBuf> {
    Ok(crate::settings::config_dir()?.join(FILE))
}

/// Appends a trace, keeping the newest `MAX_KEPT`.
pub fn save(t: &Trace) {
    let write = || -> crate::error::Result<()> {
        let p = path()?;
        let mut lines: Vec<String> = std::fs::read_to_string(&p).unwrap_or_default().lines().map(str::to_string).collect();
        lines.push(serde_json::to_string(t)?);
        let skip = lines.len().saturating_sub(MAX_KEPT);
        let tmp = p.with_extension("jsonl.tmp");
        std::fs::write(&tmp, lines[skip..].join("\n") + "\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(tmp, p)?;
        Ok(())
    };
    if let Err(e) = write() {
        log::warn!("saving the trace: {e}");
    }
}

pub fn load() -> Vec<Trace> {
    path().ok().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Success and speed over a set of traces, against the targets.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    pub count: usize,
    pub ok: usize,
    pub one_step: Speed,
    pub multi_step: Speed,
    pub goal_success: f64,
    pub goal_one_step_ms: u64,
    pub goal_multi_step_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Speed {
    pub count: usize,
    pub p50_ms: u64,
    pub p90_ms: u64,
    pub max_ms: u64,
}

pub const TARGET_SUCCESS: f64 = 0.90;
pub const TARGET_ONE_STEP_MS: u64 = 3000;
pub const TARGET_MULTI_STEP_MS: u64 = 8000;

impl Stats {
    pub fn of<'a>(traces: impl IntoIterator<Item = &'a Trace>) -> Stats {
        let traces: Vec<&Trace> = traces.into_iter().collect();
        let speed = |multi: bool| speed(traces.iter().filter(|t| t.ok && t.multi_step() == multi).map(|t| t.total_ms).collect());
        Stats {
            count: traces.len(),
            ok: traces.iter().filter(|t| t.ok).count(),
            one_step: speed(false),
            multi_step: speed(true),
            goal_success: TARGET_SUCCESS,
            goal_one_step_ms: TARGET_ONE_STEP_MS,
            goal_multi_step_ms: TARGET_MULTI_STEP_MS,
        }
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn success(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.ok as f64 / self.count as f64 }
    }

    /// Each target with whether it is met.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn verdicts(&self) -> Vec<(String, bool)> {
        vec![
            (format!("success {:.0}% (target {:.0}%)", self.success() * 100.0, TARGET_SUCCESS * 100.0), self.success() >= TARGET_SUCCESS),
            (format!("one-step typical {:.1} s (target {:.1} s)", self.one_step.p50_ms as f64 / 1000.0, TARGET_ONE_STEP_MS as f64 / 1000.0), self.one_step.count == 0 || self.one_step.p50_ms <= TARGET_ONE_STEP_MS),
            (format!("multi-step typical {:.1} s (target {:.1} s)", self.multi_step.p50_ms as f64 / 1000.0, TARGET_MULTI_STEP_MS as f64 / 1000.0), self.multi_step.count == 0 || self.multi_step.p50_ms <= TARGET_MULTI_STEP_MS),
        ]
    }
}

fn speed(mut ms: Vec<u64>) -> Speed {
    if ms.is_empty() {
        return Speed::default();
    }
    ms.sort_unstable();
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    Speed { count: ms.len(), p50_ms: at(0.5), p90_ms: at(0.9), max_ms: *ms.last().unwrap() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(ok: bool, steps: usize, ms: u64) -> Trace {
        let mut b = Builder::new("open Notes", Route::Assistant, Instant::now(), Some(150));
        for _ in 0..steps {
            b.step("open_app", Outcome::Done, 10, "Opened Notes");
        }
        let mut t = b.finish(ok, "Opened Notes.");
        t.total_ms = ms;
        t
    }

    #[test]
    fn steps_and_first_step_time_are_recorded() {
        let mut b = Builder::new("a\nb", Route::Instant, Instant::now() - std::time::Duration::from_millis(500), Some(120));
        b.step("open_app", Outcome::Done, 100, "x".repeat(400).as_str());
        b.step("set_volume", Outcome::Failed, 5, "boom");
        let t = b.finish(true, "ok");
        assert_eq!(t.request, "a b");
        let first = t.first_step_ms.unwrap();
        assert!((390..=460).contains(&first), "500 ms in, minus the 100 ms the first step took: {first}");
        assert_eq!(t.steps.len(), 2);
        assert!(t.steps[0].detail.chars().count() <= 161);
        assert!(t.total_ms >= 500);
        assert!(t.multi_step());
    }

    #[test]
    fn stats_against_the_targets() {
        let mut ts: Vec<Trace> = (0..9).map(|i| trace(true, 1, 1000 + i * 200)).collect();
        ts.push(trace(false, 1, 500));
        ts.extend((0..3).map(|i| trace(true, 3, 6000 + i * 1000)));
        let s = Stats::of(&ts);
        assert_eq!((s.count, s.ok), (13, 12));
        assert_eq!(s.one_step.count, 9, "failed requests don't count for speed");
        assert_eq!(s.one_step.p50_ms, 1800);
        assert_eq!(s.one_step.max_ms, 2600);
        assert_eq!(s.multi_step.p50_ms, 7000);
        let v = s.verdicts();
        assert!(v[0].1 && v[1].1 && v[2].1, "{v:?}");
        let slow = Stats::of(&[trace(true, 1, 9000), trace(false, 1, 100)]);
        assert!(!slow.verdicts()[0].1 && !slow.verdicts()[1].1);
        assert_eq!(Stats::of(&[]).one_step, Speed::default());
    }

    #[test]
    fn traces_round_trip_as_json() {
        let t = trace(true, 2, 1234);
        let back: Trace = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }
}
