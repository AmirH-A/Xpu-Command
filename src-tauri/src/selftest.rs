//! Debug builds only: run the pipeline without the GUI.
//!   xpu-command --selftest-stt <file.wav> [model]       transcribe a 16-bit PCM WAV with the local engine
//!   xpu-command --selftest-agent <claude|codex> <dir> <prompt…>   run an agent headless, print events
//!   xpu-command --selftest-mac-tool <tool> '<json>'    validate and run one Mac tool directly (no model, no gate)
//!   xpu-command --selftest-mac-tools '[["tool",{…}],…]' several tools in one session (no gate)
//!   xpu-command --selftest-mac "<request>"             the whole Mac pipeline with a console "pill";
//!                                                       XPU_PRESS=1 presses (cancel/approve) after 1 s;
//!                                                       XPU_ALLOW="com.apple.calculator=control,…" app access
//!   xpu-command --selftest-knock [low|medium|high|<g>] print taps and double knocks for 60 s (calibration)
use crate::{agents, audio, knock, sanitize, settings, stt};

pub fn maybe_run() -> bool {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--selftest-stt") => {
            let model = args.get(3).map(String::as_str).unwrap_or("base.en");
            match stt_file(&args[2], model) {
                Ok(t) => println!("TRANSCRIPT: {t}"),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
            true
        }
        Some("--selftest-agent") => {
            let kind = if args[2] == "codex" { settings::AgentKind::Codex } else { settings::AgentKind::Claude };
            let prompt = args[4..].join(" ");
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let req = agents::Request { kind, prompt: &prompt, project: std::path::Path::new(&args[3]), safety: settings::Safety::Edit, resume: std::env::var("XPU_RESUME").ok().as_deref().map(|s| Box::leak(s.to_string().into_boxed_str()) as &str), timeout: std::time::Duration::from_secs(600), claude_config_dir: None, mac: None, approvals: None };
                let profile = std::env::var("XPU_CLAUDE_PROFILE").ok().map(|p| settings::claude_profile_dir(&p).expect("bad profile"));
                let req = agents::Request { claude_config_dir: profile.as_deref(), ..req };
                let ask = std::env::var("XPU_SAFETY").as_deref() == Ok("ask");
                let approvals = if ask {
                    let server = crate::mac::server::start_approvals(std::sync::Arc::new(ConsoleHost), Default::default()).await.unwrap();
                    let config = crate::mac::McpConfigFile::create(&settings::config_dir().unwrap(), &server.mcp_config()).unwrap();
                    Some((server, config))
                } else {
                    None
                };
                let req = agents::Request { safety: if ask { settings::Safety::AskCommands } else { req.safety }, approvals: approvals.as_ref().map(|(_, c)| c.path()), ..req };
                match agents::start(req) {
                    Ok(mut run) => {
                        while let Some(ev) = run.events.recv().await {
                            println!("EVENT: {}", serde_json::to_string(&ev).unwrap());
                        }
                    }
                    Err(e) => eprintln!("error: {e}"),
                }
            });
            true
        }
        Some("--selftest-model") => {
            let id = args.get(2).map(String::as_str).unwrap_or("base.en");
            let m = stt::model::find(id).expect("unknown model");
            let rt = tokio::runtime::Runtime::new().unwrap();
            match rt.block_on(stt::model::download(m, |d, t| if d == t { println!("downloaded {d} bytes") })) {
                Ok(p) => println!("verified: {}", p.display()),
                Err(e) => eprintln!("error: {e}"),
            }
            true
        }
        #[cfg(target_os = "macos")]
        Some("--selftest-mac-tool") => {
                        let v: serde_json::Value = serde_json::from_str(args.get(3).map(String::as_str).unwrap_or("{}")).expect("arguments must be JSON");
            let exec = crate::mac::exec::MacExec::new(Default::default(), crate::mac::workspace().unwrap());
            run_tool(&exec, &args[2], &v);
            true
        }
        #[cfg(target_os = "macos")]
        Some("--selftest-mac-tools") => {
            let steps: Vec<(String, serde_json::Value)> = serde_json::from_str(&args[2]).expect("a JSON list of [tool, args]");
            let exec = crate::mac::exec::MacExec::new(Default::default(), crate::mac::workspace().unwrap());
            for (tool, v) in steps {
                println!("── {tool} {v}");
                run_tool(&exec, &tool, &v);
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
            if std::env::var("XPU_UNDO").is_ok_and(|v| v == "1") {
                println!("── undo");
                println!("{}", crate::mac::exec::undo_last());
            }
            true
        }
        #[cfg(target_os = "macos")]
        Some("--selftest-mac") => {
            mac_request(&args[2..].join(" "));
            true
        }
        Some("--selftest-vad") => {
            let samples = read_wav_16k(&args[2]).expect("a 16-bit PCM WAV");
            let mut e = crate::vad::Endpoint::default();
            let step = (audio::TARGET_RATE / 20) as usize;
            for (i, chunk) in samples.chunks(step).enumerate() {
                let rms = (chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len() as f32).sqrt();
                let v = e.push(rms);
                if v != crate::vad::Verdict::Continue {
                    println!("{v:?} at {:.2}s (spoke {}, voiced {} ms, threshold {:.4})", (i + 1) as f32 * 0.05, e.spoke(), e.voiced_ms(), e.threshold());
                    break;
                }
            }
            let r = crate::vad::trim(&samples, audio::TARGET_RATE, e.threshold(), 250, 0);
            println!("trim keeps {:.2}s..{:.2}s of {:.2}s", r.start as f32 / 16000.0, r.end as f32 / 16000.0, samples.len() as f32 / 16000.0);
            true
        }
        #[cfg(target_os = "macos")]
        Some("--selftest-airpods") => {
            println!("listening for 10 s: press your AirPods");
            for e in crate::airpods::listen_for(10) {
                println!("EVENT {e:?}");
            }
            true
        }
        Some("--selftest-knock") => {
            let sens = match args.get(2).map(String::as_str) {
                Some("low") => settings::KnockSensitivity::Low,
                Some("high") => settings::KnockSensitivity::High,
                _ => settings::KnockSensitivity::Medium,
            };
            let cfg: &'static knock::KnockConfig = Box::leak(Box::new(knock::KnockConfig::new(true, sens)));
            if let Some(g) = args.get(2).and_then(|a| a.parse::<f32>().ok()) {
                cfg.set_threshold_g(g);
            }
            let (tx, rx) = std::sync::mpsc::channel();
            if let Err(e) = knock::start(cfg, tx) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
            println!("listening for 60 s at {sens:?} ({} g). Knock twice on the palm rest; also try typing.", cfg.threshold());
            let start = std::time::Instant::now();
            let mut next = 5;
            while start.elapsed().as_secs() < 60 {
                match rx.recv_timeout(std::time::Duration::from_millis(250)) {
                    Ok(knock::Event::Tap(g)) => println!("  tap  {g:.3} g"),
                    Ok(knock::Event::DoubleKnock { span_ms }) => println!("DOUBLE KNOCK ({span_ms} ms)"),
                    Err(_) => {}
                }
                if start.elapsed().as_secs() >= next {
                    println!("  ({} sensor readings so far)", knock::SAMPLES.load(std::sync::atomic::Ordering::Relaxed));
                    next += 5;
                }
            }
            true
        }
        _ => false,
    }
}

/// A 16-bit PCM WAV as mono 16 kHz samples.
fn read_wav_16k(path: &str) -> crate::error::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    let find = |tag: &[u8]| bytes.windows(4).position(|w| w == tag);
    let fmt = find(b"fmt ").ok_or_else(|| crate::error::msg("not a WAV"))?;
    let channels = u16::from_le_bytes([bytes[fmt + 10], bytes[fmt + 11]]) as usize;
    let rate = u32::from_le_bytes(bytes[fmt + 12..fmt + 16].try_into().unwrap());
    let data = find(b"data").ok_or_else(|| crate::error::msg("no data chunk"))? + 8;
    let mono: Vec<f32> = bytes[data..].chunks_exact(2 * channels).map(|f| f.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).sum::<f32>() / channels as f32).collect();
    Ok(audio::resample(&mono, rate, audio::TARGET_RATE))
}

fn stt_file(path: &str, model: &str) -> crate::error::Result<String> {
    let bytes = std::fs::read(path)?;
    let find = |tag: &[u8]| bytes.windows(4).position(|w| w == tag);
    let fmt = find(b"fmt ").ok_or_else(|| crate::error::msg("not a WAV"))?;
    let channels = u16::from_le_bytes([bytes[fmt + 10], bytes[fmt + 11]]) as usize;
    let rate = u32::from_le_bytes(bytes[fmt + 12..fmt + 16].try_into().unwrap());
    let data = find(b"data").ok_or_else(|| crate::error::msg("no data chunk"))? + 8;
    let samples: Vec<f32> = bytes[data..].chunks_exact(2 * channels).map(|f| i16::from_le_bytes([f[0], f[1]]) as f32 / 32768.0).collect();
    let audio = audio::Captured(audio::resample(&samples, rate, audio::TARGET_RATE));
    let engine = stt::LocalEngine::default();
    let raw = engine.transcribe(model, &stt::Request { audio: &audio, language: "en", vocabulary: "" })?;
    Ok(sanitize::clean_transcript(&raw))
}

/// Validates and runs one tool directly (the gate is tested separately; this checks the system side).
#[cfg(target_os = "macos")]
fn run_tool(exec: &crate::mac::exec::MacExec, tool: &str, v: &serde_json::Value) {
    use crate::mac::server::{Exec, Output};
    match crate::mac::tools::parse(tool, v, chrono::Local::now().naive_local()) {
        Ok(c) => {
            let target = match exec.target(&c) {
                Ok(t) => t,
                Err(e) => return println!("ERROR: {e}"),
            };
            if let Some(t) = &target {
                println!("TARGET {} ({}) pid {} label {:?}", t.name, t.bundle, t.pid, t.label);
            }
            println!("{:?} · {}", c.risk(), c.describe());
            match exec.run(&c, target.as_ref()) {
                Ok(Output::Text(m)) => println!("OK: {m}"),
                Ok(Output::Image { text, jpeg }) => println!("OK: {text} ({} bytes of JPEG)", jpeg.len()),
                Err(e) => println!("ERROR: {e}"),
            }
        }
        Err(e) => println!("REFUSED: {e}"),
    }
}

/// Prints what the pill would show; answers questions from environment variables.
struct ConsoleHost;

impl crate::mac::server::Host for ConsoleHost {
    fn status(&self, t: &str) {
        println!("PILL   {t}");
    }
    fn ask(&self, a: crate::mac::server::Ask, t: &str) -> tokio::sync::oneshot::Receiver<()> {
        let press = std::env::var("XPU_PRESS").is_ok();
        println!("ASK    {a:?}: {t}{}", if press { "  (pressing in 1 s)" } else { "  (no press)" });
        let (tx, rx) = tokio::sync::oneshot::channel();
        if press {
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = tx.send(());
            });
        } else {
            std::mem::forget(tx);
        }
        rx
    }
    fn end_ask(&self) {}
    fn record(&self, tool: &str, summary: &str, o: crate::mac::server::Outcome, detail: &str) {
        println!("LOG    {tool} {o:?} · {summary} · {}", crate::agents::clip(detail, 100));
    }
    fn highlight(&self, f: Option<(f64, f64, f64, f64)>) {
        if let Some(f) = f {
            println!("FRAME  {f:?}");
        }
    }
    fn approval(&self, q: &crate::agents::approvals::Question) -> tokio::sync::oneshot::Receiver<(crate::agents::approvals::Answer, crate::mac::server::Via)> {
        use crate::agents::approvals::Answer;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let answer = std::env::var("XPU_ANSWER").unwrap_or_default();
        println!("ASK    wants to {} (danger: {:?}) -> {answer:?}", q.what, q.danger);
        match answer.as_str() {
            "yes" => drop(tx.send((Answer::Yes, crate::mac::server::Via::Voice))),
            "no" => drop(tx.send((Answer::No, crate::mac::server::Via::Voice))),
            "always" => drop(tx.send((Answer::Always, crate::mac::server::Via::Voice))),
            _ => std::mem::forget(tx),
        }
        rx
    }
}

#[cfg(target_os = "macos")]
fn mac_request(text: &str) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut policy = crate::mac::policy::Policy::default();
        for pair in std::env::var("XPU_ALLOW").unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
            let (b, l) = pair.split_once('=').expect("bundle=level");
            let level = match l {
                "see" => crate::mac::policy::Access::See,
                "control" => crate::mac::policy::Access::Control,
                "ask" => crate::mac::policy::Access::Ask,
                _ => crate::mac::policy::Access::Off,
            };
            policy.levels.insert(b.to_string(), level);
        }
        let exec = std::sync::Arc::new(crate::mac::exec::MacExec::new(policy.clone(), crate::mac::workspace().unwrap()));
        let server = crate::mac::server::start(std::sync::Arc::new(ConsoleHost), exec, policy, Default::default()).await.unwrap();
        let config = crate::mac::McpConfigFile::create(&crate::settings::config_dir().unwrap(), &server.mcp_config()).unwrap();
        let workspace = crate::mac::workspace().unwrap();
        let prompt = crate::mac::system_prompt(chrono::Local::now(), &crate::memory::prompt_block(&crate::memory::list()));
        let profile = std::env::var("XPU_CLAUDE_PROFILE").ok().map(|p| settings::claude_profile_dir(&p).expect("bad profile"));
        let model = std::env::var("XPU_MODEL").unwrap_or_else(|_| "haiku".into());
        let started = std::time::Instant::now();
        let req = agents::Request {
            kind: settings::AgentKind::Claude,
            prompt: text,
            project: &workspace,
            safety: settings::Safety::ReadOnly,
            resume: None,
            timeout: std::time::Duration::from_secs(300),
            claude_config_dir: profile.as_deref(),
            mac: Some(agents::MacArgs { mcp_config: config.path(), model: &model, system_prompt: &prompt }),
            approvals: None,
        };
        match agents::start(req) {
            Ok(mut run) => {
                while let Some(ev) = run.events.recv().await {
                    println!("EVENT  {} (+{:.1}s)", serde_json::to_string(&ev).unwrap(), started.elapsed().as_secs_f32());
                }
            }
            Err(e) => eprintln!("error: {e}"),
        }
        drop(server);
    });
}
