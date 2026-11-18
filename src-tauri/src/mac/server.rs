//! The MCP tool server Claude Code talks to during one "Control my Mac" request.
//!
//! * Listens on 127.0.0.1 only, on a random port, and only while one request runs.
//! * Every request must carry the run's random bearer token (compared in constant time), the exact
//!   Host header, and no Origin header (browsers always send one: a web page can't reach it).
//! * JSON-RPC over HTTP POST (MCP "streamable HTTP", JSON responses only), bodies up to 1 MB.
//! * Tool calls go through the gate: validate → by risk: run / show-then-run unless cancelled /
//!   run only on the user's yes → execute → record. Calls are serialized and capped per request.
use super::policy::{self, Decision, Policy, RunState, Target};
use super::tools::{self, Call};
use base64::Engine;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// What the user is asked, and what pressing (button, shortcut or double knock) means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// "This will happen in a moment": a press cancels it.
    Cancel,
    /// "Allow this?": a press approves it.
    Approve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
    Cancelled,
    Declined,
    Refused,
}

/// The app side: the pill, the press, the activity log. A fake implements it in tests.
pub trait Host: Send + Sync + 'static {
    fn status(&self, text: &str);
    /// Shows the question; the receiver fires when the user presses.
    fn ask(&self, ask: Ask, text: &str) -> oneshot::Receiver<()>;
    /// The question is over (answered or timed out): presses are talk presses again.
    fn end_ask(&self);
    fn record(&self, tool: &str, summary: &str, outcome: Outcome, detail: &str);
    /// Outline the window being worked on (screen points), or remove the outline.
    fn highlight(&self, frame: Option<(f64, f64, f64, f64)>);
    /// Shows the assistant's plan and the step it is on.
    fn plan(&self, _plan: &super::plan::Plan) {}
    /// Asks for access to an app: yes (this request), always (remembered), or no. By default a
    /// press means yes.
    fn grant(&self, question: &str) -> oneshot::Receiver<crate::agents::approvals::Answer> {
        press_means_yes(self.ask(Ask::Approve, question))
    }
    /// A step ended after `ms` (including any question to the user), for the request's trace.
    fn traced(&self, _tool: &str, _outcome: Outcome, _ms: u64, _detail: &str) {}
    /// The user said "always": keep this app allowed from now on.
    fn allow_always(&self, _bundle: &str, _name: &str, _need: policy::Need) {}
    /// Asks a coding agent's permission question (show, speak, listen). The receiver yields the
    /// answer and how it was given. The default answers nothing (= deny).
    fn approval(&self, _q: &crate::agents::approvals::Question) -> oneshot::Receiver<(crate::agents::approvals::Answer, Via)> {
        oneshot::channel().1
    }
}

/// A press (within the wait) as a yes.
pub fn press_means_yes(press: oneshot::Receiver<()>) -> oneshot::Receiver<crate::agents::approvals::Answer> {
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        if press.await.is_ok() {
            let _ = tx.send(crate::agents::approvals::Answer::Yes);
        }
    });
    rx
}

/// How the user answered a permission question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Voice,
    Press,
}

/// What a tool produced.
pub enum Output {
    Text(String),
    /// A JPEG screenshot plus a caption.
    Image { text: String, jpeg: Vec<u8> },
}

/// Runs validated calls on the system. Blocking; called off the async threads.
pub trait Exec: Send + Sync + 'static {
    /// The app a window call touches, resolved live by the system (None if it touches no window).
    fn target(&self, call: &Call) -> Result<Option<Target>, String>;
    /// Runs the call. For window calls, `target` is what the gate approved; implementations must
    /// check it still matches (the same process) before acting.
    fn run(&self, call: &Call, target: Option<&Target>) -> Result<Output, String>;
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub grace: Duration,
    pub confirm: Duration,
    pub max_calls: u32,
    /// How long a coding agent's permission question waits for an answer.
    pub answer: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self { grace: Duration::from_secs(3), confirm: Duration::from_secs(15), max_calls: 40, answer: Duration::from_secs(45) }
    }
}

/// What the server offers.
enum Toolset {
    /// "Control my Mac": the Mac tools, gated by App access.
    Mac { exec: Arc<dyn Exec>, policy: Policy, run: parking_lot::Mutex<RunState> },
    /// A coding agent's permission prompts (`approve`), answered by voice.
    Approvals { always: parking_lot::Mutex<std::collections::HashSet<String>> },
}

struct State {
    host: Arc<dyn Host>,
    tools: Toolset,
    limits: Limits,
    token: String,
    host_header: String,
    calls: tokio::sync::Mutex<u32>,
    closed: std::sync::atomic::AtomicBool,
}

/// A running server. Dropping it stops it.
pub struct Server {
    pub port: u16,
    pub token: String,
    /// The MCP server name Claude Code sees ("mac" or "xpu"); tools are `mcp__<name>__<tool>`.
    pub name: &'static str,
    _stop: oneshot::Sender<()>,
}

impl Server {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    /// The `--mcp-config` JSON for Claude Code: one HTTP server named "mac".
    pub fn mcp_config(&self) -> Value {
        json!({ "mcpServers": { self.name: { "type": "http", "url": self.url(), "headers": { "Authorization": format!("Bearer {}", self.token) } } } })
    }
}

fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// The "Control my Mac" server: Mac tools, gated by App access. `seen`: apps whose content the
/// request already carries (the user's selected text); the gate treats them as read, so acting
/// elsewhere or sending anything out needs a yes.
/// `named`: apps the user asked for (see `consent`), allowed for this request without a question.
pub async fn start(host: Arc<dyn Host>, exec: Arc<dyn Exec>, policy: Policy, limits: Limits, seen: Vec<String>, named: Vec<String>) -> std::io::Result<Server> {
    let run = RunState { read_apps: seen.into_iter().collect(), named: named.into_iter().collect(), ..Default::default() };
    serve(host, Toolset::Mac { exec, policy, run: parking_lot::Mutex::new(run) }, limits, "mac").await
}

/// The approvals server: one `approve` tool for Claude Code's `--permission-prompt-tool`.
pub async fn start_approvals(host: Arc<dyn Host>, limits: Limits) -> std::io::Result<Server> {
    serve(host, Toolset::Approvals { always: Default::default() }, limits, "xpu").await
}

pub async fn run_steps(host: Arc<dyn Host>, exec: Arc<dyn Exec>, policy: Policy, limits: Limits, steps: &[(String, Value)]) -> (Vec<String>, Option<String>) {
    let st = State {
        host,
        tools: Toolset::Mac { exec, policy, run: Default::default() },
        limits,
        token: String::new(),
        host_header: String::new(),
        calls: tokio::sync::Mutex::new(0),
        closed: Default::default(),
    };
    let mut done = Vec::new();
    for (name, args) in steps {
        let (out, is_error) = call_tool(&st, name, args).await;
        let msg = match out {
            Output::Text(t) => t,
            Output::Image { text, .. } => text,
        };
        if is_error || msg.starts_with("The user ") {
            let why = msg.strip_prefix("Refused: ").or_else(|| msg.strip_prefix("Failed: ")).unwrap_or(&msg).to_string();
            return (done, Some(why));
        }
        done.push(msg);
    }
    (done, None)
}

async fn serve(host: Arc<dyn Host>, tools: Toolset, limits: Limits, name: &'static str) -> std::io::Result<Server> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let token = new_token();
    let state = Arc::new(State { host, tools, limits, token: token.clone(), host_header: format!("127.0.0.1:{port}"), calls: tokio::sync::Mutex::new(0), closed: Default::default() });
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    state.closed.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                acc = listener.accept() => {
                    let Ok((stream, peer)) = acc else { continue };
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    let st = state.clone();
                    tokio::spawn(async move {
                        let svc = hyper::service::service_fn(move |req| handle(st.clone(), req));
                        let _ = hyper::server::conn::http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(stream), svc).await;
                    });
                }
            }
        }
    });
    Ok(Server { port, token, name, _stop: stop_tx })
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn reply(status: StatusCode, body: Option<Value>) -> Response<Full<Bytes>> {
    let mut r = Response::builder().status(status);
    let bytes = match body {
        Some(v) => {
            r = r.header("Content-Type", "application/json");
            Bytes::from(v.to_string())
        }
        None => Bytes::new(),
    };
    r.body(Full::new(bytes)).unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

async fn handle(st: Arc<State>, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    if st.closed.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(reply(StatusCode::GONE, None));
    }
    let h = req.headers();
    let header = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).unwrap_or("");
    if req.uri().path() != "/mcp" {
        return Ok(reply(StatusCode::NOT_FOUND, None));
    }
    if h.contains_key("origin") || header("host") != st.host_header {
        return Ok(reply(StatusCode::FORBIDDEN, None));
    }
    let expected = format!("Bearer {}", st.token);
    if !ct_eq(header("authorization").as_bytes(), expected.as_bytes()) {
        return Ok(reply(StatusCode::UNAUTHORIZED, None));
    }
    if req.method() != hyper::Method::POST {
        return Ok(reply(StatusCode::METHOD_NOT_ALLOWED, None));
    }
    if !header("content-type").starts_with("application/json") {
        return Ok(reply(StatusCode::UNSUPPORTED_MEDIA_TYPE, None));
    }
    let body = match Limited::new(req.into_body(), 1024 * 1024).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return Ok(reply(StatusCode::PAYLOAD_TOO_LARGE, None)),
    };
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return Ok(reply(StatusCode::BAD_REQUEST, Some(rpc_error(Value::Null, -32700, "parse error"))));
    };
    if !msg.is_object() {
        return Ok(reply(StatusCode::BAD_REQUEST, Some(rpc_error(Value::Null, -32600, "one request per message"))));
    }
    let Some(id) = msg.get("id").cloned() else {
        return Ok(reply(StatusCode::ACCEPTED, None));
    };
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let v = params.get("protocolVersion").and_then(Value::as_str).filter(|v| v.len() == 10).unwrap_or("2025-06-18");
            json!({ "protocolVersion": v, "capabilities": { "tools": {} }, "serverInfo": { "name": "xpu-mac", "version": env!("CARGO_PKG_VERSION") } })
        }
        "ping" => json!({}),
        "tools/list" => match st.tools {
            Toolset::Mac { .. } => {
                let mut list = tools::schemas();
                if let Some(a) = list.as_array_mut() {
                    a.push(super::plan::schema());
                }
                json!({ "tools": list })
            }
            Toolset::Approvals { .. } => json!({ "tools": crate::agents::approvals::schema() }),
        },
        "tools/call" if matches!(st.tools, Toolset::Approvals { .. }) => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            approval_call(&st, name, &args).await
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            let (out, is_error) = if name == super::plan::TOOL { show_plan(&st, &args) } else { call_tool(&st, name, &args).await };
            let content = match out {
                Output::Text(t) => json!([{ "type": "text", "text": t }]),
                Output::Image { text, jpeg } => json!([
                    { "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(jpeg), "mimeType": "image/jpeg" },
                    { "type": "text", "text": text }
                ]),
            };
            json!({ "content": content, "isError": is_error })
        }
        _ => return Ok(reply(StatusCode::OK, Some(rpc_error(id, -32601, "method not found")))),
    };
    Ok(reply(StatusCode::OK, Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))))
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A coding agent's permission prompt: ask the user, return Claude Code's allow/deny result.
async fn approval_call(st: &State, name: &str, args: &Value) -> Value {
    use crate::agents::approvals::{self, Answer};
    let Toolset::Approvals { always } = &st.tools else { unreachable!("checked by the caller") };
    let input = args.get("input").cloned().unwrap_or(Value::Null);
    if name != "approve" || !input.is_object() {
        st.host.record("approve", name, Outcome::Refused, "malformed permission request");
        return approvals::response(None, &input, "Malformed permission request.");
    }
    let _one_at_a_time = st.calls.lock().await;
    let tool = args.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let q = approvals::question(tool, &input);
    if always.lock().contains(&q.key) {
        st.host.record("approve", &q.what, Outcome::Done, "allowed (always)");
        return approvals::response(Some(Answer::Yes), &input, "");
    }
    let rx = st.host.approval(&q);
    let got = tokio::time::timeout(st.limits.answer, rx).await.ok().and_then(Result::ok);
    st.host.end_ask();
    let answer = match got {
        Some((a, Via::Press)) if q.danger.is_some() && a != Answer::No => None,
        Some((a, _)) => Some(a),
        None => None,
    };
    if answer == Some(Answer::Always) {
        always.lock().insert(q.key.clone());
    }
    let (outcome, detail, reason) = match answer {
        Some(Answer::Yes) => (Outcome::Done, "allowed", ""),
        Some(Answer::Always) => (Outcome::Done, "allowed (always this request)", ""),
        Some(Answer::No) => (Outcome::Declined, "the user said no", "The user said no. Don't retry it; say briefly what you wanted to do and ask what they prefer."),
        None => (Outcome::Declined, "no answer", "The user didn't answer, so this was not allowed. Don't retry it; say what you wanted to do."),
    };
    st.host.record("approve", &q.what, outcome, detail);
    approvals::response(answer, &input, reason)
}

/// The plan is shown, never gated or counted: it runs nothing.
fn show_plan(st: &State, args: &Value) -> (Output, bool) {
    match super::plan::parse(args) {
        Ok(p) => {
            if p.now == 1 {
                st.host.record(super::plan::TOOL, &p.summary(), Outcome::Done, "");
            }
            st.host.plan(&p);
            st.host.traced(super::plan::TOOL, Outcome::Done, 0, p.current());
            text("Shown to the user.", false)
        }
        Err(e) => text(format!("Refused: {e}"), true),
    }
}

/// Text to the model; `is_error`.
fn text(t: impl Into<String>, is_error: bool) -> (Output, bool) {
    (Output::Text(t.into()), is_error)
}

/// What the pill says: for window calls, the real control and app instead of a number.
fn describe(call: &Call, target: Option<&Target>) -> String {
    let base = call.describe();
    let Some(t) = target else { return base };
    let label = t.label.as_deref().map(|l| crate::agents::clip(l, 60));
    use crate::i18n::trf;
    let s = match (call, label) {
        (Call::TypeText { text, submit, .. }, Some(l)) => trf(
            if *submit { "Typing “{text}” into {label} in {app}, then Return" } else { "Typing “{text}” into {label} in {app}" },
            &[("text", &crate::agents::clip(text, 40)), ("label", &l), ("app", &t.name)],
        ),
        (Call::ClickAt { kind, .. } | Call::Click { kind, .. } | Call::ClickPage { kind, .. }, Some(l)) => trf("{verb} {label} in {app}", &[("verb", kind.verb()), ("label", &l), ("app", &t.name)]),
        (Call::FillPage { text, submit, .. }, Some(l)) => trf(
            if *submit { "Filling {label} in {app} with “{text}”, then submitting" } else { "Filling {label} in {app} with “{text}”" },
            &[("label", &l), ("app", &t.name), ("text", &crate::agents::clip(text, 40))],
        ),
        (Call::Drag { .. }, Some(l)) => trf("Dragging {label} in {app}", &[("label", &l), ("app", &t.name)]),
        _ => base,
    };
    crate::agents::clip(&s, 140)
}

/// Waits for the user's press: true if pressed within `wait`.
async fn pressed(st: &State, ask: Ask, what: &str, wait: Duration) -> bool {
    let rx = st.host.ask(ask, what);
    let r = tokio::select! {
        r = rx => r.is_ok(),
        _ = tokio::time::sleep(wait) => false,
    };
    st.host.end_ask();
    r
}

/// Records a step in the activity log and its time (since the call began) in the trace.
fn log(st: &State, started: std::time::Instant, tool: &str, summary: &str, outcome: Outcome, detail: &str) {
    st.host.record(tool, summary, outcome, detail);
    st.host.traced(tool, outcome, started.elapsed().as_millis() as u64, if detail.is_empty() { summary } else { detail });
}

/// The gate. Validate → resolve the real app → decide (app access, risk, history, step-by-step) →
/// ask if needed → run → record.
async fn call_tool(st: &State, name: &str, args: &Value) -> (Output, bool) {
    let started = std::time::Instant::now();
    let Toolset::Mac { exec: st_exec, policy: st_policy, run: st_run } = &st.tools else {
        return text("Refused: this server has no Mac tools", true);
    };
    let mut calls = st.calls.lock().await;
    *calls += 1;
    let tool = crate::agents::clip(name, 40);
    if *calls > st.limits.max_calls {
        log(st, started, &tool, "", Outcome::Refused, "too many actions in one request");
        return text(format!("Refused: at most {} actions per request. Stop and tell the user what is left.", st.limits.max_calls), true);
    }
    let call = match tools::parse(name, args, chrono::Local::now().naive_local()) {
        Ok(c) => c,
        Err(e) => {
            log(st, started, &tool, "", Outcome::Refused, &e);
            return text(format!("Refused: {e}"), true);
        }
    };
    let exec = st_exec.clone();
    let c = call.clone();
    let target = match tokio::task::spawn_blocking(move || exec.target(&c)).await.unwrap_or_else(|_| Err("the check crashed".into())) {
        Ok(t) => t,
        Err(e) => {
            log(st, started, call.tool(), &call.describe(), Outcome::Failed, &e);
            return text(format!("Failed: {e}"), true);
        }
    };
    let what = describe(&call, target.as_ref());
    let decision = loop {
        let d = policy::decide(call.risk(), target.as_ref(), call.flags(), st_policy, &st_run.lock());
        match (d, target.as_ref()) {
            (Decision::Grant(need), Some(t)) => {
                let question = crate::i18n::trf(
                    if need == policy::Need::See { "Let the assistant see {app} for this request" } else { "Let the assistant control {app} for this request" },
                    &[("app", &t.name)],
                );
                st.host.highlight(t.frame);
                let rx = st.host.grant(&question);
                let answer = tokio::time::timeout(st.limits.answer, rx).await.ok().and_then(Result::ok);
                st.host.end_ask();
                use crate::agents::approvals::Answer;
                match answer {
                    Some(Answer::Always) => {
                        st.host.allow_always(&t.bundle, &t.name, need);
                        log(st, started, call.tool(), &question, Outcome::Done, "allowed always");
                    }
                    Some(Answer::Yes) => log(st, started, call.tool(), &question, Outcome::Done, "allowed for this request"),
                    _ => {
                        st.host.highlight(None);
                        log(st, started, call.tool(), &question, Outcome::Declined, "");
                        return text(format!("The user did not allow access to {}. Do not retry; say so. They can allow it in Settings › App access.", t.name), false);
                    }
                }
                st_run.lock().grants.insert(t.bundle.clone(), need);
            }
            (d, _) => break d,
        }
    };
    match decision {
        Decision::Refuse(why) => {
            log(st, started, call.tool(), &what, Outcome::Refused, &why);
            return text(format!("Refused: {why}"), true);
        }
        Decision::Grant(_) => unreachable!("handled above"),
        Decision::Run => {}
        Decision::Grace => {
            st.host.highlight(target.as_ref().and_then(|t| t.frame));
            if pressed(st, Ask::Cancel, &what, st.limits.grace).await {
                log(st, started, call.tool(), &what, Outcome::Cancelled, "");
                return text("The user cancelled this action. Do not retry it; say it was cancelled.", false);
            }
        }
        Decision::Confirm => {
            st.host.highlight(target.as_ref().and_then(|t| t.frame));
            if !pressed(st, Ask::Approve, &what, st.limits.confirm).await {
                log(st, started, call.tool(), &what, Outcome::Declined, "");
                return text("The user did not approve this action. Do not retry it; say it was not run.", false);
            }
            if let Some(t) = &target {
                st_run.lock().trusted.insert(t.bundle.clone());
            }
        }
    }
    st.host.status(&what);
    if let Some(t) = &target {
        st.host.highlight(t.frame);
    }
    let exec = st_exec.clone();
    let (c, t) = (call.clone(), target.clone());
    let r = tokio::task::spawn_blocking(move || exec.run(&c, t.as_ref())).await.unwrap_or_else(|_| Err("the action crashed".into()));
    match r {
        Ok(out) => {
            if call.reveals() {
                st_run.lock().read_apps.insert(target.as_ref().map_or_else(|| "clipboard".to_string(), |t| t.bundle.clone()));
            }
            let summary = match &out {
                Output::Text(m) => m.clone(),
                Output::Image { text, .. } => text.clone(),
            };
            let detail = if call.reveals() { "read".to_string() } else { summary };
            log(st, started, call.tool(), &what, Outcome::Done, &detail);
            match out {
                Output::Text(m) => text(crate::agents::clip_raw(&m, 20_000), false),
                img => (img, false),
            }
        }
        Err(e) => {
            log(st, started, call.tool(), &what, Outcome::Failed, &e);
            text(format!("Failed: {e}"), true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac::policy::Access;
    use parking_lot::Mutex;

    #[derive(Default)]
    struct FakeHost {
        /// What to do when asked: Some(delay) presses after the delay, None never presses.
        press_after: Option<Duration>,
        /// How the user answers permission questions (None: never).
        answer: Option<(crate::agents::approvals::Answer, Via)>,
        log: Mutex<Vec<String>>,
    }
    impl Host for FakeHost {
        fn status(&self, t: &str) {
            self.log.lock().push(format!("status {t}"));
        }
        fn ask(&self, a: Ask, t: &str) -> oneshot::Receiver<()> {
            self.log.lock().push(format!("ask {a:?} {t}"));
            let (tx, rx) = oneshot::channel();
            match self.press_after {
                Some(d) => {
                    tokio::spawn(async move {
                        tokio::time::sleep(d).await;
                        let _ = tx.send(());
                    });
                }
                None => std::mem::forget(tx),
            }
            rx
        }
        fn end_ask(&self) {
            self.log.lock().push("end_ask".into());
        }
        fn record(&self, tool: &str, _s: &str, o: Outcome, _d: &str) {
            self.log.lock().push(format!("record {tool} {o:?}"));
        }
        fn highlight(&self, f: Option<(f64, f64, f64, f64)>) {
            self.log.lock().push(format!("highlight {}", f.is_some()));
        }
        fn plan(&self, p: &crate::mac::plan::Plan) {
            self.log.lock().push(format!("plan {} {}", p.progress(), p.current()));
        }
        fn grant(&self, q: &str) -> oneshot::Receiver<crate::agents::approvals::Answer> {
            match self.answer {
                Some((a, _)) => {
                    self.log.lock().push(format!("grant {q} → {a:?}"));
                    let (tx, rx) = oneshot::channel();
                    let _ = tx.send(a);
                    rx
                }
                None => press_means_yes(self.ask(Ask::Approve, q)),
            }
        }
        fn allow_always(&self, bundle: &str, _name: &str, need: crate::mac::policy::Need) {
            self.log.lock().push(format!("always {bundle} {need:?}"));
        }
        fn approval(&self, q: &crate::agents::approvals::Question) -> oneshot::Receiver<(crate::agents::approvals::Answer, Via)> {
            self.log.lock().push(format!("approval {}", q.what));
            let (tx, rx) = oneshot::channel();
            match self.answer {
                Some(a) => {
                    let _ = tx.send(a);
                }
                None => std::mem::forget(tx),
            }
            rx
        }
    }

    #[derive(Default)]
    struct FakeExec(Mutex<Vec<String>>);
    impl Exec for FakeExec {
        fn target(&self, c: &Call) -> Result<Option<Target>, String> {
            if c.on_files() {
                return Ok(Some(Target { bundle: policy::FILES.into(), name: "your files".into(), pid: 0, need: c.need().unwrap(), label: None, unknown_spot: false, frame: None }));
            }
            let app = match c {
                Call::ReadWindow { app } | Call::Screenshot { app } | Call::PressKeys { app, .. } | Call::Scroll { app, .. } | Call::ChooseMenu { app, .. } | Call::Wait { app, .. } => app.clone(),
                Call::Click { .. } | Call::TypeText { .. } => "Chrome".into(),
                _ => return Ok(None),
            };
            let bundle = match app.as_str() {
                "Chrome" => "com.google.Chrome",
                "Notes" => "com.apple.Notes",
                "Keychain Access" => "com.apple.keychainaccess",
                "Terminal" => "com.apple.Terminal",
                _ => return Err(format!("{app} has no window open")),
            };
            let label = match c {
                Call::Click { element: 99, .. } => Some("button Send".to_string()),
                Call::Click { .. } => Some("button Compose".to_string()),
                _ => None,
            };
            Ok(Some(Target { bundle: bundle.into(), name: app, pid: 42, need: c.need().unwrap(), label, unknown_spot: false, frame: Some((0.0, 0.0, 800.0, 600.0)) }))
        }
        fn run(&self, c: &Call, _t: Option<&Target>) -> Result<Output, String> {
            self.0.lock().push(c.tool().into());
            if matches!(c, Call::OpenApp { name } if name == "Broken") {
                return Err("boom".into());
            }
            if matches!(c, Call::Screenshot { .. }) {
                return Ok(Output::Image { text: "shot".into(), jpeg: vec![0xff, 0xd8, 0xff] });
            }
            Ok(Output::Text("ok".into()))
        }
    }

    fn quick() -> Limits {
        Limits { grace: Duration::from_millis(150), confirm: Duration::from_millis(300), max_calls: 6, answer: Duration::from_millis(300) }
    }

    async fn serve(press: Option<Duration>) -> (Server, Arc<FakeHost>, Arc<FakeExec>) {
        serve_with(press, Policy::default()).await
    }

    async fn serve_with(press: Option<Duration>, policy: Policy) -> (Server, Arc<FakeHost>, Arc<FakeExec>) {
        let host = Arc::new(FakeHost { press_after: press, ..Default::default() });
        let exec = Arc::new(FakeExec::default());
        let s = start(host.clone(), exec.clone(), policy, quick(), Vec::new(), Vec::new()).await.unwrap();
        (s, host, exec)
    }

    fn allow(pairs: &[(&str, Access)]) -> Policy {
        Policy { levels: pairs.iter().map(|(b, a)| (b.to_string(), *a)).collect(), ..Default::default() }
    }

    async fn post(s: &Server, body: Value) -> (u16, Value) {
        post_raw(s, body.to_string(), &format!("Bearer {}", s.token), None).await
    }

    async fn post_raw(s: &Server, body: String, auth: &str, origin: Option<&str>) -> (u16, Value) {
        let mut r = reqwest::Client::new().post(s.url()).header("Content-Type", "application/json").header("Authorization", auth).body(body);
        if let Some(o) = origin {
            r = r.header("Origin", o);
        }
        let resp = r.send().await.unwrap();
        let code = resp.status().as_u16();
        (code, resp.json::<Value>().await.unwrap_or(Value::Null))
    }

    async fn call(s: &Server, name: &str, args: Value) -> (String, bool) {
        let (code, v) = post(s, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).await;
        assert_eq!(code, 200, "{v}");
        (v["result"]["content"][0]["text"].as_str().unwrap().to_string(), v["result"]["isError"].as_bool().unwrap())
    }

    #[tokio::test]
    async fn protocol_basics() {
        let (s, _, _) = serve(None).await;
        let (c, v) = post(&s, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})).await;
        assert_eq!(c, 200);
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
        assert!(v["result"]["capabilities"]["tools"].is_object());
        let (c, _) = post(&s, json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
        assert_eq!(c, 202);
        let (_, v) = post(&s, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), if cfg!(target_os = "linux") { 39 } else { 42 }, "Linux has no page tools");
        assert!(tools.iter().any(|t| t["name"] == "plan"));
        let (_, v) = post(&s, json!({"jsonrpc":"2.0","id":3,"method":"resources/list"})).await;
        assert_eq!(v["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn rejects_without_token_from_browsers_and_odd_requests() {
        let (s, _, exec) = serve(None).await;
        let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"open_app","arguments":{"name":"Safari"}}}).to_string();
        assert_eq!(post_raw(&s, body.clone(), "", None).await.0, 401);
        assert_eq!(post_raw(&s, body.clone(), "Bearer wrong", None).await.0, 401);
        assert_eq!(post_raw(&s, body.clone(), &format!("Bearer {}x", s.token), None).await.0, 401);
        assert_eq!(post_raw(&s, body.clone(), &format!("Bearer {}", s.token), Some("https://evil.example")).await.0, 403);
        let big = reqwest::Client::new().post(s.url()).header("Content-Type", "application/json").header("Authorization", format!("Bearer {}", s.token)).body("x".repeat(2 * 1024 * 1024)).send().await;
        assert!(big.map(|r| r.status().as_u16() == 413).unwrap_or(true), "oversized bodies are refused, by a 413 or by closing the connection");
        assert_eq!(post_raw(&s, "[1,2]".into(), &format!("Bearer {}", s.token), None).await.0, 400);
        let c = reqwest::Client::new();
        assert_eq!(c.get(s.url()).header("Authorization", format!("Bearer {}", s.token)).send().await.unwrap().status().as_u16(), 405);
        assert_eq!(c.post(format!("http://127.0.0.1:{}/other", s.port)).send().await.unwrap().status().as_u16(), 404);
        let r = c.post(s.url()).header("Host", format!("evil.example:{}", s.port)).header("Authorization", format!("Bearer {}", s.token)).header("Content-Type", "application/json").body(body).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 403);
        assert!(exec.0.lock().is_empty(), "nothing ran");
    }

    #[tokio::test]
    async fn safe_actions_run_and_bad_input_is_refused() {
        let (s, host, exec) = serve(None).await;
        assert_eq!(call(&s, "open_app", json!({"name":"Safari"})).await, ("ok".into(), false));
        let (t, err) = call(&s, "open_url", json!({"url":"file:///etc/passwd"})).await;
        assert!(err && t.starts_with("Refused"), "{t}");
        let (t, err) = call(&s, "open_app", json!({"name":"Broken"})).await;
        assert!(err && t.contains("boom"));
        assert_eq!(*exec.0.lock(), ["open_app", "open_app"]);
        let log = host.log.lock().join("\n");
        assert!(log.contains("record open_url Refused") && log.contains("record open_app Failed"), "{log}");
    }

    #[tokio::test]
    async fn the_plan_is_shown_but_never_gated_or_counted() {
        let (s, host, exec) = serve(None).await;
        for now in 1..=8 {
            let (t, err) = call(&s, "plan", json!({"steps":["Open Mail","Reply"],"now": now.min(2)})).await;
            assert_eq!((t.as_str(), err), ("Shown to the user.", false));
        }
        let (t, err) = call(&s, "plan", json!({"steps":[]})).await;
        assert!(err && t.starts_with("Refused"), "{t}");
        assert_eq!(call(&s, "open_app", json!({"name":"Safari"})).await, ("ok".into(), false), "plan calls don't use up the action limit");
        assert_eq!(*exec.0.lock(), ["open_app"]);
        let log = host.log.lock().join("\n");
        assert!(log.contains("plan 1/2 Open Mail") && log.contains("plan 2/2 Reply"), "{log}");
        assert_eq!(log.matches("record plan Done").count(), 1, "the plan is logged once, when it starts");
    }

    #[tokio::test]
    async fn change_runs_after_grace_unless_cancelled() {
        let (s, host, exec) = serve(None).await;
        let t0 = std::time::Instant::now();
        assert!(!call(&s, "create_note", json!({"title":"a","body":"b"})).await.1);
        assert!(t0.elapsed() >= Duration::from_millis(150), "waited for the grace period");
        assert_eq!(*exec.0.lock(), ["create_note"]);
        assert!(host.log.lock().iter().any(|l| l == "end_ask"));

        let (s, host, exec) = serve(Some(Duration::from_millis(30))).await;
        let (t, _) = call(&s, "create_note", json!({"title":"a","body":"b"})).await;
        assert!(t.contains("cancelled"));
        assert!(exec.0.lock().is_empty(), "a cancelled action never runs");
        assert!(host.log.lock().iter().any(|l| l == "record create_note Cancelled"));
    }

    #[tokio::test]
    async fn routine_steps_run_through_the_same_gate_and_stop_at_a_cancel() {
        let steps = |v: &[(&str, Value)]| v.iter().map(|(n, a)| (n.to_string(), a.clone())).collect::<Vec<_>>();
        let host = Arc::new(FakeHost::default());
        let exec = Arc::new(FakeExec::default());
        let (done, stop) = run_steps(host.clone(), exec.clone(), Policy::default(), quick(), &steps(&[("open_app", json!({"name":"Slack"})), ("set_volume", json!({"level":30}))])).await;
        assert_eq!((done.len(), stop), (2, None));
        assert_eq!(*exec.0.lock(), ["open_app", "set_volume"]);

        let host = Arc::new(FakeHost { press_after: Some(Duration::from_millis(30)), ..Default::default() });
        let exec = Arc::new(FakeExec::default());
        let (done, stop) = run_steps(host, exec.clone(), Policy::default(), quick(), &steps(&[("open_app", json!({"name":"Slack"})), ("quit_app", json!({"name":"Mail"})), ("open_app", json!({"name":"Notes"}))])).await;
        assert_eq!(done.len(), 1);
        assert!(stop.unwrap().contains("cancelled"));
        assert_eq!(*exec.0.lock(), ["open_app"], "nothing runs after a cancel");

        let (done, stop) = run_steps(Arc::new(FakeHost::default()), Arc::new(FakeExec::default()), Policy::default(), quick(), &steps(&[("open_url", json!({"url":"file:///etc/passwd"}))])).await;
        assert!(done.is_empty() && stop.is_some());
    }

    #[tokio::test]
    async fn confirm_needs_a_yes() {
        let (s, host, exec) = serve(None).await;
        let (t, _) = call(&s, "run_shortcut", json!({"name":"Morning"})).await;
        assert!(t.contains("did not approve"));
        assert!(exec.0.lock().is_empty());
        assert!(host.log.lock().iter().any(|l| l == "record run_shortcut Declined"));

        let (s, _, exec) = serve(Some(Duration::from_millis(30))).await;
        assert_eq!(call(&s, "run_shortcut", json!({"name":"Morning"})).await, ("ok".into(), false));
        assert_eq!(*exec.0.lock(), ["run_shortcut"]);
    }

    #[tokio::test]
    async fn calls_per_request_are_capped() {
        let (s, _, exec) = serve(None).await;
        for _ in 0..6 {
            assert!(!call(&s, "set_volume", json!({"level":10})).await.1);
        }
        let (t, err) = call(&s, "set_volume", json!({"level":10})).await;
        assert!(err && t.contains("at most 6"));
        assert_eq!(exec.0.lock().len(), 6);
    }

    #[tokio::test]
    async fn a_named_app_runs_without_asking_and_always_is_remembered() {
        let host = Arc::new(FakeHost::default());
        let exec = Arc::new(FakeExec::default());
        let s = start(host.clone(), exec.clone(), Policy::default(), quick(), Vec::new(), vec!["com.apple.Notes".into()]).await.unwrap();
        assert!(!call(&s, "read_window", json!({"app":"Notes"})).await.1, "the app the user asked for");
        assert!(!host.log.lock().iter().any(|l| l.starts_with("ask")), "no question: {:?}", host.log.lock());

        let host = Arc::new(FakeHost { answer: Some((crate::agents::approvals::Answer::Always, Via::Voice)), ..Default::default() });
        let s = start(host.clone(), exec.clone(), Policy::default(), quick(), Vec::new(), Vec::new()).await.unwrap();
        assert!(!call(&s, "read_window", json!({"app":"Chrome"})).await.1);
        assert!(!call(&s, "read_window", json!({"app":"Chrome"})).await.1);
        let log = host.log.lock().join("\n");
        assert_eq!(log.matches("grant ").count(), 1, "asked once: {log}");
        assert!(log.contains("always com.google.Chrome See"), "{log}");

        let host = Arc::new(FakeHost { answer: Some((crate::agents::approvals::Answer::No, Via::Voice)), ..Default::default() });
        let s = start(host.clone(), exec, Policy::default(), quick(), Vec::new(), Vec::new()).await.unwrap();
        let (t, _) = call(&s, "read_window", json!({"app":"Chrome"})).await;
        assert!(t.contains("did not allow access"), "{t}");
        assert!(!host.log.lock().iter().any(|l| l.starts_with("always")));
    }

    #[tokio::test]
    async fn app_access_is_enforced_with_just_this_time_grants() {
        let (s, host, exec) = serve(None).await;
        let (t, _) = call(&s, "read_window", json!({"app":"Chrome"})).await;
        assert!(t.contains("did not allow access"), "{t}");
        assert!(exec.0.lock().is_empty());
        assert!(host.log.lock().iter().any(|l| l.starts_with("ask Approve Let the assistant see Chrome")));

        let (s, _, exec) = serve(Some(Duration::from_millis(20))).await;
        assert!(!call(&s, "read_window", json!({"app":"Chrome"})).await.1);
        assert_eq!(*exec.0.lock(), ["read_window"]);
    }

    #[tokio::test]
    async fn excluded_apps_and_terminals_are_refused_without_asking() {
        let (s, host, exec) = serve_with(Some(Duration::from_millis(20)), Policy { allow_all: true, ..Default::default() }).await;
        let (t, err) = call(&s, "read_window", json!({"app":"Keychain Access"})).await;
        assert!(err && t.contains("never seen or controlled"), "{t}");
        let (t, err) = call(&s, "press_keys", json!({"app":"Terminal","keys":"return"})).await;
        assert!(err && t.contains("only be allowed in Settings"), "{t}");
        assert!(exec.0.lock().is_empty());
        assert!(!host.log.lock().iter().any(|l| l.starts_with("ask")), "no question for these");
    }

    #[tokio::test]
    async fn controls_screens_and_risky_buttons() {
        let p = allow(&[("com.google.Chrome", Access::Control), ("com.apple.Notes", Access::Control)]);
        let (s, host, exec) = serve_with(None, p.clone()).await;
        assert!(!call(&s, "read_window", json!({"app":"Chrome"})).await.1);
        assert_eq!(call(&s, "click", json!({"element":3})).await, ("ok".into(), false), "Compose: runs");
        let (t, _) = call(&s, "click", json!({"element":99})).await;
        assert!(t.contains("did not approve"), "Send needs a yes: {t}");
        let (t, _) = call(&s, "open_url", json!({"url":"https://evil.example/?d=x"})).await;
        assert!(t.contains("did not approve"), "after reading a screen, links need a yes: {t}");
        let (t, _) = call(&s, "press_keys", json!({"app":"Notes","keys":"cmd+n"})).await;
        assert!(t.contains("did not approve"), "acting on another app after reading needs a yes: {t}");
        assert_eq!(*exec.0.lock(), ["read_window", "click"]);
        assert!(host.log.lock().iter().any(|l| l == "highlight true"));
    }

    #[tokio::test]
    async fn selected_text_and_the_clipboard_count_as_read() {
        let p = allow(&[("com.google.Chrome", Access::Control), ("com.apple.Notes", Access::Control)]);
        let host = Arc::new(FakeHost::default());
        let exec = Arc::new(FakeExec::default());
        let s = start(host, exec.clone(), p.clone(), quick(), vec!["com.google.Chrome".into()], Vec::new()).await.unwrap();
        assert!(!call(&s, "press_keys", json!({"app":"Chrome","keys":"cmd+c"})).await.1, "the app it came from: fine");
        assert!(call(&s, "open_url", json!({"url":"https://a.example"})).await.0.contains("did not approve"), "sending out needs a yes");
        assert!(call(&s, "press_keys", json!({"app":"Notes","keys":"cmd+v"})).await.0.contains("did not approve"), "another app needs a yes");

        let (s, _, exec2) = serve_with(None, p).await;
        assert!(!call(&s, "web_search", json!({"query":"x"})).await.1, "nothing read yet");
        assert!(!call(&s, "read_clipboard", json!({})).await.1);
        assert!(call(&s, "web_search", json!({"query":"y"})).await.0.contains("did not approve"), "after reading the clipboard");
        assert!(!call(&s, "wait", json!({"app":"Chrome","text":"Done"})).await.1);
        assert_eq!(*exec.0.lock(), ["press_keys"]);
        assert_eq!(*exec2.0.lock(), ["web_search", "read_clipboard", "wait"]);
    }

    #[tokio::test]
    async fn files_are_gated_like_an_app() {
        let (s, host, exec) = serve(None).await;
        let (t, _) = call(&s, "find_files", json!({"query":"invoice"})).await;
        assert!(t.contains("did not allow access to your files"), "{t}");
        assert!(host.log.lock().iter().any(|l| l.starts_with("ask Approve Let the assistant see your files")));
        assert!(exec.0.lock().is_empty());

        let (s, _, exec) = serve_with(None, allow(&[(policy::FILES, Access::See)])).await;
        assert!(!call(&s, "find_files", json!({"query":"invoice"})).await.1);
        assert!(call(&s, "move_files", json!({"paths":["~/a"],"to":"~/b"})).await.0.contains("did not allow"), "see can't change files");

        let (s, _, exec2) = serve_with(None, allow(&[(policy::FILES, Access::Control)])).await;
        assert!(!call(&s, "move_files", json!({"paths":["~/a"],"to":"~/b"})).await.1, "runs after the grace period");
        assert!(call(&s, "trash_files", json!({"paths":["~/a"]})).await.0.contains("did not approve"), "the Trash needs a yes");
        assert_eq!(*exec.0.lock(), ["find_files"]);
        assert_eq!(*exec2.0.lock(), ["move_files"]);
    }

    #[tokio::test]
    async fn screenshots_are_images() {
        let (s, _, _) = serve_with(None, allow(&[("com.google.Chrome", Access::See)])).await;
        let (_, v) = post(&s, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"screenshot","arguments":{"app":"Chrome"}}})).await;
        assert_eq!(v["result"]["content"][0]["type"], "image");
        assert_eq!(v["result"]["content"][0]["mimeType"], "image/jpeg");
        assert_eq!(v["result"]["content"][0]["data"], "/9j/");
    }

    #[tokio::test]
    async fn ask_level_and_step_by_step_confirm_everything() {
        let (s, _, exec) = serve_with(None, allow(&[("com.google.Chrome", Access::Ask)])).await;
        assert!(call(&s, "read_window", json!({"app":"Chrome"})).await.0.contains("did not approve"));
        let p = Policy { step_by_step: true, ..allow(&[("com.google.Chrome", Access::Control)]) };
        let (s2, _, exec2) = serve_with(None, p).await;
        assert!(!call(&s2, "read_window", json!({"app":"Chrome"})).await.1, "reading isn't an action");
        assert!(call(&s2, "set_volume", json!({"level":5})).await.0.contains("did not approve"));
        assert!(exec.0.lock().is_empty());
        assert_eq!(*exec2.0.lock(), ["read_window"]);
        drop(s);
    }

    async fn approvals(answer: Option<(crate::agents::approvals::Answer, Via)>) -> (Server, Arc<FakeHost>) {
        let host = Arc::new(FakeHost { answer, ..Default::default() });
        (start_approvals(host.clone(), quick()).await.unwrap(), host)
    }

    async fn ask(s: &Server, command: &str) -> Value {
        let (code, v) = post(s, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"approve","arguments":{"tool_name":"Bash","input":{"command":command},"tool_use_id":"t1"}}})).await;
        assert_eq!(code, 200, "{v}");
        serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn voice_approvals() {
        use crate::agents::approvals::Answer;
        let (s, host) = approvals(Some((Answer::Yes, Via::Voice))).await;
        let (_, v) = post(&s, json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await;
        assert_eq!(v["result"]["tools"][0]["name"], "approve", "only the approve tool");
        assert_eq!(s.mcp_config()["mcpServers"]["xpu"]["type"], "http");
        let r = ask(&s, "npm test").await;
        assert_eq!(r, json!({"behavior":"allow","updatedInput":{"command":"npm test"}}));
        assert!(host.log.lock().iter().any(|l| l == "approval run `npm test`"));

        let (s, _) = approvals(Some((Answer::No, Via::Voice))).await;
        let r = ask(&s, "npm test").await;
        assert_eq!(r["behavior"], "deny");
        assert!(r["message"].as_str().unwrap().contains("said no"));

        let (s, _) = approvals(None).await;
        let r = ask(&s, "npm test").await;
        assert_eq!(r["behavior"], "deny");
        assert!(r["message"].as_str().unwrap().contains("didn't answer"));

        let (_, v) = post(&s, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"open_app","arguments":{"name":"Safari"}}})).await;
        let body: Value = serde_json::from_str(v["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["behavior"], "deny");
    }

    #[tokio::test]
    async fn dangerous_commands_need_a_spoken_yes() {
        use crate::agents::approvals::Answer;
        let (s, _) = approvals(Some((Answer::Yes, Via::Press))).await;
        assert_eq!(ask(&s, "npm test").await["behavior"], "allow", "a press allows normal commands");
        assert_eq!(ask(&s, "rm -rf build").await["behavior"], "deny", "a press can't allow deleting files");
        let (s, _) = approvals(Some((Answer::Yes, Via::Voice))).await;
        assert_eq!(ask(&s, "rm -rf build").await["behavior"], "allow", "a spoken yes can");
    }

    #[tokio::test]
    async fn always_skips_the_next_question_for_the_same_command() {
        use crate::agents::approvals::Answer;
        let (s, host) = approvals(Some((Answer::Always, Via::Voice))).await;
        assert_eq!(ask(&s, "npm test").await["behavior"], "allow");
        assert_eq!(ask(&s, "npm test").await["behavior"], "allow");
        assert_eq!(host.log.lock().iter().filter(|l| l.starts_with("approval")).count(), 1, "asked once");
        assert_eq!(ask(&s, "npm run build").await["behavior"], "allow");
        assert_eq!(host.log.lock().iter().filter(|l| l.starts_with("approval")).count(), 2, "a different command asks again");
    }

    #[tokio::test]
    async fn stops_when_dropped() {
        let (s, _, _) = serve(None).await;
        let url = s.url();
        drop(s);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(reqwest::Client::new().post(url).send().await.is_err(), "the port is closed");
    }

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }
}
