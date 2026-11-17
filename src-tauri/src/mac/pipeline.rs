//! Starting one "Control my Mac" request: what is on screen, what the user's words already allow,
//! the tool server, the prompt and the model, then Claude Code. The app and the benchmark both start
//! requests here, so the benchmark measures exactly what people use.
use super::server::{Host, Limits, Outcome, Server};
use super::{consent, policy::Policy, McpConfigFile};
use crate::error::Result;
use crate::settings::MacModel;
use std::sync::Arc;
use std::time::Duration;

/// A Mac request is short: open, play, remind. Anything longer is stuck.
pub const TIMEOUT: Duration = Duration::from_secs(300);

pub struct Options {
    pub text: String,
    pub policy: Policy,
    pub share_context: bool,
    pub memory: bool,
    pub model: MacModel,
    /// The language setting, for the language of the spoken answer.
    pub language: String,
    pub profile: Option<std::path::PathBuf>,
    /// A session to continue.
    pub resume: Option<String>,
    pub limits: Limits,
}

/// A started request. Dropping `keep` stops its tool server and removes its config file.
pub struct Started {
    pub run: crate::agents::Run,
    pub keep: (Server, McpConfigFile),
    pub model: MacModel,
}

pub async fn start(host: Arc<dyn Host>, o: Options) -> Result<Started> {
    let workspace = super::workspace()?;
    let context = if o.share_context {
        let p = o.policy.clone();
        tokio::task::spawn_blocking(move || super::ui::context(&p)).await.ok().flatten()
    } else {
        None
    };
    if let Some(c) = &context {
        host.record("context", &c.summary(), Outcome::Done, "");
    }
    let seen: Vec<String> = context.iter().filter(|c| c.selection.is_some()).map(|c| c.bundle.clone()).collect();
    let named = {
        let (t, front) = (o.text.clone(), context.as_ref().map(|c| (c.bundle.clone(), c.app.clone())));
        tokio::task::spawn_blocking(move || consented(&t, front)).await.unwrap_or_default()
    };
    if !named.is_empty() {
        let names: Vec<&str> = named.iter().map(|(_, n)| n.as_str()).collect();
        host.record("consent", &format!("Allowed for this request because you asked: {}", names.join(", ")), Outcome::Done, "");
    }
    let named: Vec<String> = named.into_iter().map(|(b, _)| b).collect();
    let model = super::pick_model(o.model, &o.text, context.as_ref().is_some_and(|c| c.selection.is_some()));
    if o.model == MacModel::Auto {
        host.record("model", &format!("Chose {} for this request", super::model_label(model)), Outcome::Done, "automatic");
    }
    let exec = Arc::new(super::exec::MacExec::new(o.policy.clone(), workspace.clone()).with_memory(o.memory));
    let server = super::server::start(host, exec, o.policy, o.limits, seen, named).await?;
    let config = McpConfigFile::create(&crate::settings::config_dir()?, &server.mcp_config())?;
    let memory = if o.memory { crate::memory::prompt_block(&crate::memory::list()) } else { String::new() };
    let lang = crate::speech::language_of(&o.text, &o.language).unwrap_or("en");
    let prompt = super::system_prompt(chrono::Local::now(), &memory, lang);
    let full = super::with_context(&o.text, context.as_ref());
    let run = crate::agents::start(crate::agents::Request {
        kind: crate::settings::AgentKind::Claude,
        prompt: &full,
        project: &workspace,
        safety: crate::settings::Safety::ReadOnly,
        resume: o.resume.as_deref(),
        timeout: TIMEOUT,
        claude_config_dir: o.profile.as_deref(),
        mac: Some(crate::agents::MacArgs { mcp_config: config.path(), model: super::model_name(model), system_prompt: &prompt }),
        approvals: None,
        custom_bin: None,
    })?;
    Ok(Started { run, keep: (server, config), model })
}

/// The apps (bundle id, name) the request itself asks for: named installed or running apps, the
/// app in front when it says "this", and the user's files when it mentions one.
pub fn consented(text: &str, front: Option<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |b: String, n: String| {
        if !out.iter().any(|(x, _)| *x == b) {
            out.push((b, n));
        }
    };
    let running = super::ui::apps_on_screen();
    for name in consent::mentioned(text, running.iter().map(|a| a.name.as_str())) {
        if let Some(a) = running.iter().find(|a| a.name == name) {
            add(a.bundle.clone(), a.name.clone());
        }
    }
    let installed = super::apps::installed();
    for name in consent::mentioned(text, installed.iter().map(|a| a.name.as_str())) {
        if let Some((bundle, shown)) = installed.iter().find(|a| a.name == name).and_then(|a| super::ui::bundle_info(&a.path)) {
            add(bundle, shown);
        }
    }
    if let Some((bundle, name)) = front.filter(|_| consent::points_at_front(text)) {
        add(bundle, name);
    }
    if consent::mentions_files(text) {
        add(super::policy::FILES.to_string(), "Your files".to_string());
    }
    out
}
