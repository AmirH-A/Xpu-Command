//! The talk loop: button → record → transcribe → deliver (type it, or hand it to an agent).
use crate::audio::{Captured, Recording};
use crate::error::{msg, Error, Result};
use crate::input::ButtonEvent;
use crate::settings::{Delivery, Settings, SttEngine, TalkMode};
use crate::{agents, inject, sanitize, speech, stt};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};

const MIN_HOLD: Duration = Duration::from_millis(250);
/// A double knock that ends a recording is itself recorded: its span plus this margin (microphone
/// latency, the last tap's ring) is dropped from the end.
const KNOCK_TAIL_MARGIN_MS: u32 = 200;
/// Below this peak and with no detected speech, nothing was said (don't run speech-to-text).
const NOTHING_SAID_PEAK: f32 = 0.05;

/// How a recording was started: it decides how it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// Held button or shortcut: ends on release.
    Hold,
    /// A click in toggle mode: ends on the next click, or when the user stops talking.
    Click,
    /// A double knock: ends on the next double knock, or when the user stops talking.
    Knock,
    Headset,
    /// Opened by the app after an agent's answer: ends when the user stops talking; silence
    /// closes it quietly.
    Reply,
}

struct Rec {
    rec: Recording,
    started: Instant,
    id: u64,
    trigger: Trigger,
    vad: Arc<Mutex<crate::vad::Endpoint>>,
}

/// What the listening logic learned about a recording.
#[derive(Debug, Clone, Copy)]
struct Heard {
    spoke: bool,
    voiced_ms: u32,
    /// The app opened the microphone (a reply window): fail silently, not with an error.
    quiet: bool,
}
const AGENT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// A Mac request is short: open, play, remind. Anything longer is stuck.
const MAC_TIMEOUT: Duration = Duration::from_secs(300);
/// A follow-up within this time continues the agent's conversation; later ones start fresh.
const SESSION_IDLE: Duration = Duration::from_secs(30 * 60);
/// Below this peak level the microphone delivered silence (often: access denied, so macOS feeds zeros).
const SILENT_PEAK: f32 = 1e-4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Nothing,
}

/// What a button event means, given the mode and whether we are recording. Pure, so it is tested.
pub fn decide(mode: TalkMode, ev: ButtonEvent, recording: bool) -> Action {
    match (mode, ev, recording) {
        (_, ButtonEvent::Pressed, false) => Action::Start,
        (TalkMode::Hold, ButtonEvent::Released, true) | (TalkMode::Toggle, ButtonEvent::Pressed, true) => Action::Stop,
        _ => Action::Nothing,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCommand {
    Stop,
    NewTask,
    /// "That's all": nothing more, close the conversation.
    Done,
    Undo,
}

/// Short spoken commands. Only an utterance that is *just* the command counts.
pub fn voice_command(text: &str) -> Option<VoiceCommand> {
    let t: String = crate::sanitize::fold_persian(&text.to_lowercase()).chars().filter(|c| c.is_alphanumeric() || *c == ' ').collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    match t.as_str() {
        "بس کن" | "وایسا" | "توقف" | "متوقف شو" | "استاپ" => Some(VoiceCommand::Stop),
        "کار جدید" | "از اول" | "شروع دوباره" | "مکالمه جدید" => Some(VoiceCommand::NewTask),
        "کافیه" | "تمام" | "تموم" | "همین" | "همین بود" | "نه ممنون" | "نه مرسی" | "ولش کن" => Some(VoiceCommand::Done),
        "برگردون" | "برگردونش" | "آندو" | "اندو" | "قبلی رو برگردون" => Some(VoiceCommand::Undo),
        "stop" | "stop it" | "cancel" | "stop the agent" => Some(VoiceCommand::Stop),
        "new task" | "new session" | "start over" | "new conversation" => Some(VoiceCommand::NewTask),
        "undo" | "undo that" | "undo it" | "undo the last one" | "take that back" | "revert that" => Some(VoiceCommand::Undo),
        "that's all" | "thats all" | "that is all" | "that's it" | "thats it" | "no thanks" | "nothing" | "never mind" | "nevermind" | "all good" | "i'm done" | "im done" | "done" => {
            Some(VoiceCommand::Done)
        }
        _ => None,
    }
}

/// The agent conversation to continue, if the same agent and project were used recently.
#[derive(Debug, Clone)]
struct Session {
    agent: crate::settings::AgentKind,
    profile: Option<String>,
    project: std::path::PathBuf,
    id: String,
    at: Instant,
}

struct Follow {
    name: String,
    project: std::path::PathBuf,
    continuing: bool,
    speak: bool,
    agent: crate::settings::AgentKind,
    profile: Option<String>,
}

/// The app side of the Mac tool server: the pill, the question, the log.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct MacHost {
    ctl: Arc<Controller>,
    app: AppHandle,
    speak: bool,
}

impl crate::mac::server::Host for MacHost {
    fn status(&self, text: &str) {
        self.ctl.overlay(&self.app, Phase::Working, text, Some("Mac"));
    }
    fn ask(&self, ask: crate::mac::server::Ask, text: &str) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.ctl.pending.lock() = Some(tx);
        match ask {
            crate::mac::server::Ask::Cancel => self.ctl.overlay(&self.app, Phase::Pending, text, Some("Press or knock to cancel")),
            crate::mac::server::Ask::Approve => {
                self.ctl.overlay(&self.app, Phase::Confirm, &format!("{text}?"), Some("Press or knock to allow"));
                if self.speak {
                    speech::speak(format!("{text}? Press or knock to allow."));
                }
            }
        }
        rx
    }
    fn end_ask(&self) {
        self.ctl.pending.lock().take();
    }
    fn record(&self, tool: &str, summary: &str, outcome: crate::mac::server::Outcome, detail: &str) {
        crate::activity::record(tool, summary, outcome, detail);
    }
    fn highlight(&self, frame: Option<(f64, f64, f64, f64)>) {
        match frame {
            Some(f) => crate::frame::show(&self.app, f),
            None => crate::frame::hide(&self.app),
        }
    }
}

/// The app side of a coding agent's permission questions: show, speak, listen for yes or no.
struct AgentHost {
    ctl: Arc<Controller>,
    app: AppHandle,
    speak: bool,
    agent: String,
}

impl crate::mac::server::Host for AgentHost {
    fn status(&self, text: &str) {
        self.ctl.overlay(&self.app, Phase::Working, text, Some(&self.agent));
    }
    fn ask(&self, _ask: crate::mac::server::Ask, _text: &str) -> tokio::sync::oneshot::Receiver<()> {
        tokio::sync::oneshot::channel().1
    }
    fn end_ask(&self) {
        self.ctl.pending.lock().take();
        self.ctl.overlay(&self.app, Phase::Working, "Working…", Some(&self.agent));
    }
    fn record(&self, tool: &str, summary: &str, outcome: crate::mac::server::Outcome, detail: &str) {
        crate::activity::record(tool, summary, outcome, detail);
    }
    fn highlight(&self, _frame: Option<(f64, f64, f64, f64)>) {}

    fn approval(&self, q: &crate::agents::approvals::Question) -> tokio::sync::oneshot::Receiver<(crate::agents::approvals::Answer, crate::mac::server::Via)> {
        use crate::agents::approvals::{parse_answer, Answer};
        use crate::mac::server::Via;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let shown = format!("{} wants to {}", self.agent, q.what);
        let hint = match q.danger {
            Some(d) => format!("This {d}: say “yes” to allow"),
            None => "Say yes or no, or press to allow".to_string(),
        };
        let spoken = {
            let what = q.what.replace('`', "");
            match q.danger {
                Some(d) => format!("{} wants to {what}. This {d}. Say yes to allow, or no.", self.agent),
                None => format!("{} wants to {what}. Say yes or no.", self.agent),
            }
        };
        let (ctl, app, speak, danger) = (self.ctl.clone(), self.app.clone(), self.speak, q.danger.is_some());
        let arm = move |ctl: &Arc<Controller>| {
            let (ptx, prx) = tokio::sync::oneshot::channel::<()>();
            *ctl.pending.lock() = Some(ptx);
            prx
        };
        let mut press = arm(&ctl);
        ctl.overlay(&app, Phase::Confirm, &shown, Some(&hint));
        tauri::async_runtime::spawn(async move {
            if speak {
                speech::speak_and_wait(spoken).await;
            } else if ctl.settings.read().sound_cues {
                crate::cues::play(crate::cues::Cue::Start);
            }
            let s = ctl.settings.read().clone();
            for _ in 0..8 {
                if tx.is_closed() {
                    return;
                }
                let listen = ctl.recording.lock().is_none();
                tokio::select! {
                    r = &mut press => {
                        if r.is_err() {
                            return;
                        }
                        if danger {
                            ctl.overlay(&app, Phase::Confirm, &shown, Some("A press can't allow this one: say “yes”"));
                            press = arm(&ctl);
                            continue;
                        }
                        let _ = tx.send((Answer::Yes, Via::Press));
                        return;
                    }
                    heard = ctl.capture_answer(&app, Duration::from_secs(9)), if listen => {
                        let audio = match heard {
                            Ok(Some(a)) => a,
                            Ok(None) => continue,
                            Err(e) => {
                                log::warn!("listening for an answer: {e}");
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                continue;
                            }
                        };
                        let text = ctl.transcribe(&s, audio, String::new()).await.unwrap_or_default();
                        match parse_answer(&text) {
                            Some(a) => {
                                let _ = tx.send((a, Via::Voice));
                                return;
                            }
                            None if !text.is_empty() => {
                                ctl.overlay(&app, Phase::Confirm, &shown, Some("Sorry: yes or no?"));
                                if speak {
                                    speech::speak_and_wait("Sorry, yes or no?".to_string()).await;
                                }
                            }
                            None => {}
                        }
                    }
                }
            }
        });
        rx
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Listening,
    Transcribing,
    Typing,
    Working,
    Done,
    Error,
    /// A Mac action is about to happen; a press cancels it.
    Pending,
    /// A Mac action waits for the user's yes; a press approves it.
    Confirm,
    Hidden,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayState {
    pub phase: Phase,
    pub text: String,
    pub agent: Option<String>,
}

pub struct Controller {
    pub settings: RwLock<Settings>,
    pub hook: &'static crate::input::HookConfig,
    pub knock: &'static crate::knock::KnockConfig,
    engine: Arc<stt::LocalEngine>,
    recording: Mutex<Option<Rec>>,
    rec_seq: std::sync::atomic::AtomicU64,
    agent_cancel: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    session: Mutex<Option<Session>>,
    /// While a Mac action is shown for cancel or approval: the next press answers it.
    pending: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    generation: std::sync::atomic::AtomicU64,
}

impl Controller {
    pub fn new(settings: Settings, hook: &'static crate::input::HookConfig, knock: &'static crate::knock::KnockConfig) -> Self {
        Self {
            settings: RwLock::new(settings),
            hook,
            knock,
            engine: Arc::new(stt::LocalEngine::default()),
            recording: Mutex::new(None),
            rec_seq: Default::default(),
            agent_cancel: Mutex::new(None),
            session: Mutex::new(None),
            pending: Mutex::new(None),
            generation: Default::default(),
        }
    }

    /// A press while a Mac action is shown answers it (cancel or approve) instead of starting to talk.
    fn answer_pending(&self) -> bool {
        match self.pending.lock().take() {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    pub fn on_button(self: &Arc<Self>, app: &AppHandle, ev: ButtonEvent) {
        if ev == ButtonEvent::Pressed && self.answer_pending() {
            return;
        }
        let mode = self.settings.read().mode;
        let recording = self.recording.lock().is_some();
        match decide(mode, ev, recording) {
            Action::Start => self.start_recording(app, if mode == TalkMode::Hold { Trigger::Hold } else { Trigger::Click }),
            Action::Stop => self.stop_recording(app, 0),
            Action::Nothing => {}
        }
    }

    /// A double knock works like a toggle click whatever the talk mode: knock to start, knock to send.
    pub fn on_knock(self: &Arc<Self>, app: &AppHandle, span_ms: u32) {
        if !self.hook.enabled.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if self.answer_pending() {
            return;
        }
        let recording = self.recording.lock().is_some();
        match decide(TalkMode::Toggle, ButtonEvent::Pressed, recording) {
            Action::Start => self.start_recording(app, Trigger::Knock),
            Action::Stop => self.stop_recording(app, span_ms.min(1500) + KNOCK_TAIL_MARGIN_MS),
            Action::Nothing => {}
        }
    }

    pub fn on_headset(self: &Arc<Self>, app: &AppHandle) {
        if !self.hook.enabled.load(std::sync::atomic::Ordering::Relaxed) || self.answer_pending() {
            return;
        }
        let recording = self.recording.lock().is_some();
        match decide(TalkMode::Toggle, ButtonEvent::Pressed, recording) {
            Action::Start => self.start_recording(app, Trigger::Headset),
            Action::Stop => self.stop_recording(app, 0),
            Action::Nothing => {}
        }
    }

    /// Loads the speech model in the background so the first talk isn't slowed by it.
    pub fn preload(self: &Arc<Self>) {
        let s = self.settings.read().clone();
        if s.stt != SttEngine::Local {
            return;
        }
        let engine = self.engine.clone();
        std::thread::spawn(move || {
            if let Err(e) = engine.preload(&s.local_model) {
                log::info!("speech model not preloaded: {e}");
            }
        });
    }

    fn start_recording(self: &Arc<Self>, app: &AppHandle, trigger: Trigger) {
        let (max, device, auto_stop, cues) = {
            let s = self.settings.read();
            (s.max_record_seconds, s.input_device.clone(), s.auto_stop, s.sound_cues)
        };
        let auto = (auto_stop && trigger != Trigger::Hold) || trigger == Trigger::Reply;
        let id = self.rec_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let vad = Arc::new(Mutex::new(crate::vad::Endpoint::default()));
        let (a, v, weak) = (app.clone(), vad.clone(), Arc::downgrade(self));
        let last = Arc::new(Mutex::new(Instant::now()));
        let fired = std::sync::atomic::AtomicBool::new(false);
        let level = move |rms: f32| {
            let verdict = v.lock().push(rms);
            {
                let mut l = last.lock();
                if l.elapsed() >= Duration::from_millis(50) {
                    *l = Instant::now();
                    let _ = a.emit_to("overlay", "level", (rms * 6.0).min(1.0));
                }
            }
            if auto && verdict != crate::vad::Verdict::Continue && !fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                if let Some(this) = weak.upgrade() {
                    let a = a.clone();
                    tauri::async_runtime::spawn(async move {
                        match verdict {
                            crate::vad::Verdict::Done => this.stop_if(&a, id),
                            _ if trigger == Trigger::Reply => this.cancel_if(&a, id, ""),
                            _ => this.cancel_if(&a, id, "Didn't hear anything"),
                        }
                    });
                }
            }
        };
        match Recording::start(device.as_deref(), max, level) {
            Ok(r) => {
                *self.recording.lock() = Some(Rec { rec: r, started: Instant::now(), id, trigger, vad });
                if cues {
                    crate::cues::play(crate::cues::Cue::Start);
                }
                let hint = match (trigger, auto) {
                    (Trigger::Reply, _) => "Listening for your reply… (or say “that's all”)",
                    (Trigger::Hold, _) => "Listening… release to send",
                    (Trigger::Click, true) => "Listening… pause or click to send",
                    (Trigger::Click, false) => "Listening… click to send",
                    (Trigger::Knock, true) => "Listening… pause or knock to send",
                    (Trigger::Knock, false) => "Listening… knock twice to send",
                    (Trigger::Headset, true) => "Listening… pause or press your AirPods to send",
                    (Trigger::Headset, false) => "Listening… press your AirPods to send",
                };
                self.overlay(app, Phase::Listening, hint, None);
            }
            Err(e) => self.fail(app, e),
        }
    }

    /// Auto-stop for recording `id` (the user stopped talking), unless it already ended.
    fn stop_if(self: &Arc<Self>, app: &AppHandle, id: u64) {
        if self.recording.lock().as_ref().is_some_and(|r| r.id == id) {
            self.stop_recording(app, 0);
        }
    }

    /// Cancels recording `id` (nothing was said), unless it already ended.
    fn cancel_if(self: &Arc<Self>, app: &AppHandle, id: u64, why: &str) {
        let taken = {
            let mut slot = self.recording.lock();
            if slot.as_ref().is_some_and(|r| r.id == id) { slot.take() } else { None }
        };
        if let Some(r) = taken {
            drop(r);
            if why.is_empty() {
                self.overlay(app, Phase::Hidden, "", None);
                return;
            }
            if self.settings.read().sound_cues {
                crate::cues::play(crate::cues::Cue::Cancel);
            }
            self.overlay(app, Phase::Error, why, None);
            self.hide_later(app, Duration::from_millis(1500));
        }
    }

    /// Ends the recording and sends it on. `cut_tail_ms`: drop the end (the knock that ended it).
    fn stop_recording(self: &Arc<Self>, app: &AppHandle, cut_tail_ms: u32) {
        let Some(r) = self.recording.lock().take() else { return };
        if r.trigger == Trigger::Hold && r.started.elapsed() < MIN_HOLD {
            drop(r);
            self.overlay(app, Phase::Hidden, "", None);
            return;
        }
        if self.settings.read().sound_cues {
            crate::cues::play(crate::cues::Cue::Send);
        }
        let (threshold, heard) = {
            let v = r.vad.lock();
            (v.threshold(), Heard { spoke: v.spoke(), voiced_ms: v.voiced_ms(), quiet: r.trigger == Trigger::Reply })
        };
        let rec = r.rec;
        let this = self.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            this.overlay(&app, Phase::Transcribing, "Transcribing…", None);
            let result = async {
                let audio = tauri::async_runtime::spawn_blocking(move || -> Result<Captured> {
                    let full = rec.finish()?;
                    let keep = crate::vad::trim(&full.0, crate::audio::TARGET_RATE, threshold, 250, cut_tail_ms);
                    Ok(Captured(full.0[keep].to_vec()))
                })
                .await
                .map_err(|_| msg("audio task failed"))??;
                this.process(&app, audio, heard).await
            }
            .await;
            if let Err(e) = result {
                this.fail(&app, e);
            }
        });
    }

    async fn process(self: &Arc<Self>, app: &AppHandle, audio: Captured, heard: Heard) -> Result<()> {
        if !stt::min_audio_ok(&audio) {
            self.overlay(app, Phase::Hidden, "", None);
            return Ok(());
        }
        let peak = audio.peak();
        if heard.quiet && !heard.spoke {
            self.overlay(app, Phase::Hidden, "", None);
            return Ok(());
        }
        if peak >= SILENT_PEAK && !heard.spoke && peak < NOTHING_SAID_PEAK {
            self.overlay(app, Phase::Error, "Didn't hear anything", None);
            self.hide_later(app, Duration::from_millis(1500));
            return Ok(());
        }
        if peak < SILENT_PEAK {
            return Err(msg(if cfg!(target_os = "macos") {
                "No sound from the microphone. Allow microphone access (System Settings › Privacy & Security › Microphone) and check the input device in Settings."
            } else {
                "No sound from the microphone. Check microphone access and the input device in Settings."
            }));
        }
        let s = self.settings.read().clone();
        let vocabulary = project_vocabulary(s.project_dir.as_deref());
        let text = self.transcribe(&s, audio, vocabulary).await?;
        if heard.quiet && (text.is_empty() || crate::vad::is_phantom(&text, heard.voiced_ms) || voice_command(&text) == Some(VoiceCommand::Done)) {
            self.overlay(app, Phase::Hidden, "", None);
            return Ok(());
        }
        if text.is_empty() || crate::vad::is_phantom(&text, heard.voiced_ms) {
            self.overlay(app, Phase::Error, "Didn't catch that", None);
            self.hide_later(app, Duration::from_secs(2));
            return Ok(());
        }
        if matches!(s.delivery, Delivery::Agent | Delivery::ControlMac) {
            match voice_command(&text) {
                Some(VoiceCommand::Stop) => {
                    let stopped = self.agent_cancel.lock().take().map(|c| c.send(()).is_ok()).unwrap_or(false);
                    self.overlay(app, Phase::Done, if stopped { "Stopped the agent" } else { "Nothing to stop" }, None);
                    self.hide_later(app, Duration::from_secs(2));
                    return Ok(());
                }
                Some(VoiceCommand::Done) => {
                    self.overlay(app, Phase::Hidden, "", None);
                    return Ok(());
                }
                Some(VoiceCommand::Undo) => {
                    let said = self.undo().await;
                    self.overlay(app, Phase::Done, &said, Some("Undo"));
                    if s.speak_summary {
                        speech::speak(said);
                    }
                    self.hide_later(app, Duration::from_secs(3));
                    return Ok(());
                }
                Some(VoiceCommand::NewTask) => {
                    self.session.lock().take();
                    self.overlay(app, Phase::Done, "New conversation: the next request starts fresh", None);
                    self.hide_later(app, Duration::from_secs(2));
                    return Ok(());
                }
                None => {}
            }
        }
        if matches!(s.delivery, Delivery::Agent | Delivery::ControlMac) {
            if let Some(r) = crate::routines::match_spoken(&text, &crate::routines::list()).cloned() {
                return self.run_routine(app, &s, r).await;
            }
        }
        match s.delivery {
            Delivery::TypeIntoFocused => {
                self.overlay(app, Phase::Typing, &text, None);
                let (t, submit) = (text.clone(), s.auto_submit);
                let (tx, rx) = tokio::sync::oneshot::channel();
                app.run_on_main_thread(move || {
                    let _ = tx.send(inject::type_text(&t, submit));
                })
                .map_err(|e| msg(e.to_string()))?;
                rx.await.map_err(|_| msg("typing was interrupted"))??;
                self.hide_later(app, Duration::from_millis(700));
            }
            Delivery::Agent => self.run_agent(app, &s, &text).await?,
            Delivery::ControlMac => self.run_mac(app, &s, &text).await?,
        }
        Ok(())
    }

    /// Speech to text with the configured engine; the result is cleaned for display and use.
    async fn transcribe(&self, s: &Settings, audio: Captured, vocabulary: String) -> Result<String> {
        let raw = match s.stt {
            SttEngine::Local if s.local_model.ends_with(".en") && !matches!(s.language.as_str(), "en" | "auto") => {
                return Err(msg("This speech model only understands English. In Settings › Speech to text, pick a multilingual model (Small is best for Persian)."));
            }
            SttEngine::Local => {
                let engine = self.engine.clone();
                let (model, lang) = (s.local_model.clone(), s.language.clone());
                tauri::async_runtime::spawn_blocking(move || engine.transcribe(&model, &stt::Request { audio: &audio, language: &lang, vocabulary: &vocabulary }))
                    .await
                    .map_err(|_| msg("speech task failed"))??
            }
            SttEngine::Cloud => stt::transcribe_cloud(&stt::Request { audio: &audio, language: &s.language, vocabulary: &vocabulary }).await?,
        };
        Ok(sanitize::clean_transcript(&raw))
    }

    /// Records one short spoken answer: stops when the user stops talking, or after `wait`.
    /// None when nothing was said.
    async fn capture_answer(self: &Arc<Self>, app: &AppHandle, wait: Duration) -> Result<Option<Captured>> {
        let device = self.settings.read().input_device.clone();
        let vad = Arc::new(Mutex::new(crate::vad::Endpoint::default()));
        let (vtx, vrx) = tokio::sync::oneshot::channel::<crate::vad::Verdict>();
        let vtx = Mutex::new(Some(vtx));
        let (v2, a) = (vad.clone(), app.clone());
        let level = move |rms: f32| {
            let verdict = v2.lock().push(rms);
            let _ = a.emit_to("overlay", "level", (rms * 6.0).min(1.0));
            if verdict != crate::vad::Verdict::Continue {
                if let Some(t) = vtx.lock().take() {
                    let _ = t.send(verdict);
                }
            }
        };
        let rec = Recording::start(device.as_deref(), 20, level)?;
        let _ = tokio::time::timeout(wait, vrx).await;
        let (threshold, spoke) = {
            let v = vad.lock();
            (v.threshold(), v.spoke())
        };
        let full = tauri::async_runtime::spawn_blocking(move || rec.finish()).await.map_err(|_| msg("audio task failed"))??;
        if !spoke {
            return Ok(None);
        }
        let keep = crate::vad::trim(&full.0, crate::audio::TARGET_RATE, threshold, 250, 0);
        Ok(Some(Captured(full.0[keep].to_vec())))
    }

    async fn run_agent(self: &Arc<Self>, app: &AppHandle, s: &Settings, text: &str) -> Result<()> {
        let project = s.project_dir.clone().ok_or_else(|| msg("choose a project folder in settings first"))?;
        if self.agent_cancel.lock().is_some() {
            return Err(msg("the agent is still working; say \"stop\" to stop it"));
        }
        let name = agents::name(s.agent).to_string();
        let project = crate::settings::validate_project_dir(&project)?;
        let resume = {
            let mut slot = self.session.lock();
            if slot.as_ref().is_some_and(|x| x.agent != s.agent || x.project != project || x.profile != s.claude_profile || x.at.elapsed() > SESSION_IDLE) {
                *slot = None;
            }
            slot.as_ref().map(|x| x.id.clone())
        };
        let continuing = resume.is_some();
        let profile = s.claude_profile.as_deref().map(crate::settings::claude_profile_dir).transpose()?;
        let approvals = if s.agent == crate::settings::AgentKind::Claude && s.safety == crate::settings::Safety::AskCommands {
            let host = Arc::new(AgentHost { ctl: self.clone(), app: app.clone(), speak: s.speak_summary, agent: name.clone() });
            let server = crate::mac::server::start_approvals(host, Default::default()).await?;
            let config = crate::mac::McpConfigFile::create(&crate::settings::config_dir()?, &server.mcp_config())?;
            Some((server, config))
        } else {
            None
        };
        let with_memory;
        let prompt = match (s.memory, resume.is_some()) {
            (true, false) => {
                let block = crate::memory::prompt_block(&crate::memory::list());
                with_memory = if block.is_empty() { text.to_string() } else { format!("{block}\nRequest:\n{text}") };
                with_memory.as_str()
            }
            _ => text,
        };
        let run = agents::start(agents::Request {
            kind: s.agent,
            prompt,
            project: &project,
            safety: s.safety,
            resume: resume.as_deref(),
            timeout: AGENT_TIMEOUT,
            claude_config_dir: profile.as_deref(),
            mac: None,
            custom_bin: s.custom_agent.as_deref(),
            approvals: approvals.as_ref().map(|(_, c)| c.path()),
        })?;
        self.follow(app, run, Follow { name, project, continuing, speak: s.speak_summary, agent: s.agent, profile: s.claude_profile.clone() }, text, approvals);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub async fn run_routine(self: &Arc<Self>, app: &AppHandle, s: &Settings, r: crate::routines::Routine) -> Result<()> {
        if self.agent_cancel.lock().is_some() {
            return Err(msg("still working on the last request; say \"stop\" to stop it"));
        }
        let workspace = crate::mac::workspace()?;
        let host = Arc::new(MacHost { ctl: self.clone(), app: app.clone(), speak: s.speak_summary });
        let policy = s.mac_policy();
        let exec = Arc::new(crate::mac::exec::MacExec::new(policy.clone(), workspace).with_memory(s.memory).without_learning());
        let steps: Vec<(String, serde_json::Value)> = r.steps.iter().map(|st| (st.tool.clone(), st.args.clone())).collect();
        self.overlay(app, Phase::Working, &format!("Routine: {}", r.name), Some("Routine"));
        let (done, stopped) = crate::mac::server::run_steps(host, exec, policy, Default::default(), &steps).await;
        crate::frame::hide(app);
        let said = match stopped {
            None => format!("Ran {}: {} step{}", r.name, done.len(), if done.len() == 1 { "" } else { "s" }),
            Some(why) => format!("Stopped {} after {} of {} steps: {}", r.name, done.len(), steps.len(), crate::agents::clip(&why, 120)),
        };
        self.overlay(app, Phase::Done, &said, Some("Routine"));
        if s.speak_summary {
            speech::speak(said);
        }
        self.hide_later(app, Duration::from_secs(3));
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub async fn run_routine(self: &Arc<Self>, _app: &AppHandle, _s: &Settings, _r: crate::routines::Routine) -> Result<()> {
        Err(crate::mac::not_supported())
    }

    pub async fn undo(&self) -> String {
        if self.agent_cancel.lock().is_some() {
            return "Still working on the last request; say \"stop\" first".into();
        }
        #[cfg(target_os = "macos")]
        {
            let said = tokio::task::spawn_blocking(crate::mac::exec::undo_last).await.unwrap_or_else(|_| "Undo failed".into());
            if said != crate::mac::undo::summary(&[], &[]) {
                crate::activity::record("undo", &said, crate::mac::server::Outcome::Done, "");
            }
            said
        }
        #[cfg(not(target_os = "macos"))]
        {
            crate::mac::undo::summary(&[], &[])
        }
    }

    /// "Control my Mac": start this request's tool server, then Claude Code with only those tools.
    #[cfg(target_os = "macos")]
    async fn run_mac(self: &Arc<Self>, app: &AppHandle, s: &Settings, text: &str) -> Result<()> {
        if self.agent_cancel.lock().is_some() {
            return Err(msg("still working on the last request; say \"stop\" to stop it"));
        }
        let workspace = crate::mac::workspace()?;
        let resume = {
            let mut slot = self.session.lock();
            if slot.as_ref().is_some_and(|x| x.agent != crate::settings::AgentKind::Claude || x.project != workspace || x.profile != s.claude_profile || x.at.elapsed() > SESSION_IDLE) {
                *slot = None;
            }
            slot.as_ref().map(|x| x.id.clone())
        };
        let host = Arc::new(MacHost { ctl: self.clone(), app: app.clone(), speak: s.speak_summary });
        let policy = s.mac_policy();
        let exec = Arc::new(crate::mac::exec::MacExec::new(policy.clone(), workspace.clone()).with_memory(s.memory));
        let server = crate::mac::server::start(host, exec, policy, Default::default()).await?;
        let config = crate::mac::McpConfigFile::create(&crate::settings::config_dir()?, &server.mcp_config())?;
        let memory = if s.memory { crate::memory::prompt_block(&crate::memory::list()) } else { String::new() };
        let prompt = crate::mac::system_prompt(chrono::Local::now(), &memory);
        let profile = s.claude_profile.as_deref().map(crate::settings::claude_profile_dir).transpose()?;
        let run = agents::start(agents::Request {
            kind: crate::settings::AgentKind::Claude,
            prompt: text,
            project: &workspace,
            safety: crate::settings::Safety::ReadOnly,
            resume: resume.as_deref(),
            timeout: MAC_TIMEOUT,
            claude_config_dir: profile.as_deref(),
            mac: Some(agents::MacArgs { mcp_config: config.path(), model: crate::mac::model_name(s.mac_model), system_prompt: &prompt }),
            approvals: None,
            custom_bin: None,
        })?;
        let f = Follow { name: "Mac".into(), project: workspace, continuing: resume.is_some(), speak: s.speak_summary, agent: crate::settings::AgentKind::Claude, profile: s.claude_profile.clone() };
        self.follow(app, run, f, text, (server, config));
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    async fn run_mac(self: &Arc<Self>, _app: &AppHandle, _s: &Settings, _text: &str) -> Result<()> {
        Err(crate::mac::not_supported())
    }

    /// Follows an agent run: the pill, the session for follow-ups, the spoken summary. `keep` is
    /// dropped when the run ends (the Mac tool server and its config file).
    fn follow<K: Send + 'static>(self: &Arc<Self>, app: &AppHandle, mut run: agents::Run, f: Follow, text: &str, keep: K) {
        let Follow { name, project, continuing, speak, agent, profile } = f;
        let profile_name = profile;
        *self.agent_cancel.lock() = Some(run.cancel);
        let label = if continuing { format!("{name} · continuing") } else { name.clone() };
        self.overlay(app, Phase::Working, &format!("“{}”", agents::clip(text, 90)), Some(&label));
        let (this, app) = (self.clone(), app.clone());
        tauri::async_runtime::spawn(async move {
            let keep = keep;
            let mut reply_after = false;
            while let Some(ev) = run.events.recv().await {
                match ev {
                    agents::AgentEvent::Session { id } => {
                        *this.session.lock() = Some(Session { agent, profile: profile_name.clone(), project: project.clone(), id, at: Instant::now() });
                    }
                    agents::AgentEvent::Status { text } => this.overlay(&app, Phase::Working, &text, Some(&name)),
                    agents::AgentEvent::Done { summary } => {
                        if let Some(x) = this.session.lock().as_mut() {
                            x.at = Instant::now();
                        }
                        let shown = if summary.is_empty() { "Done".to_string() } else { summary.clone() };
                        this.overlay(&app, Phase::Done, &shown, Some(&name));
                        reply_after = this.settings.read().conversation;
                        if speak && !summary.is_empty() {
                            if reply_after {
                                speech::speak_and_wait(summary).await;
                            } else {
                                speech::speak(summary);
                            }
                        }
                        if !reply_after {
                            this.hide_later(&app, Duration::from_secs(8));
                        }
                        break;
                    }
                    agents::AgentEvent::Failed { error } => {
                        if continuing && error != "stopped" {
                            this.session.lock().take();
                        }
                        this.overlay(&app, Phase::Error, &error, Some(&name));
                        this.hide_later(&app, Duration::from_secs(6));
                        break;
                    }
                }
            }
            drop(keep);
            this.agent_cancel.lock().take();
            this.pending.lock().take();
            crate::frame::hide(&app);
            if reply_after && this.recording.lock().is_none() {
                tokio::time::sleep(Duration::from_millis(250)).await;
                this.start_recording(&app, Trigger::Reply);
            } else if reply_after {
                this.hide_later(&app, Duration::from_secs(8));
            }
        });
    }

    fn fail(self: &Arc<Self>, app: &AppHandle, e: Error) {
        log::warn!("{e}");
        if matches!(e, Error::Permission(_)) {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
            }
        }
        self.overlay(app, Phase::Error, &e.to_string(), None);
        self.hide_later(app, Duration::from_secs(4));
    }

    pub fn overlay(&self, app: &AppHandle, phase: Phase, text: &str, agent: Option<&str>) {
        self.generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(w) = app.get_webview_window("overlay") else { return };
        if matches!(phase, Phase::Hidden) {
            let _ = w.hide();
        } else {
            place_overlay(&w);
            let _ = w.set_ignore_cursor_events(true);
            crate::overlay_window::show_without_focus(&w);
        }
        let _ = app.emit_to("overlay", "overlay", OverlayState { phase, text: text.to_string(), agent: agent.map(str::to_string) });
    }

    /// Hides the overlay after `d`, unless something newer was shown meanwhile.
    fn hide_later(self: &Arc<Self>, app: &AppHandle, d: Duration) {
        let gen = self.generation.load(std::sync::atomic::Ordering::Relaxed);
        let (this, app) = (self.clone(), app.clone());
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(d).await;
            if this.generation.load(std::sync::atomic::Ordering::Relaxed) == gen {
                this.overlay(&app, Phase::Hidden, "", None);
            }
        });
    }
}

/// Bottom-center of the monitor the cursor is on.
fn place_overlay(w: &tauri::WebviewWindow) {
    let monitor = w.cursor_position().ok().and_then(|p| w.monitor_from_point(p.x, p.y).ok().flatten()).or_else(|| w.primary_monitor().ok().flatten());
    if let (Some(m), Ok(size)) = (monitor, w.outer_size()) {
        let (mp, ms) = (m.position(), m.size());
        let x = mp.x + (ms.width as i32 - size.width as i32) / 2;
        let y = mp.y + ms.height as i32 - size.height as i32 - (ms.height as i32 / 10);
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
}

/// File and folder names from the top of the project, to help the recogniser with identifiers.
/// Names only, a bounded number, never file contents.
pub fn project_vocabulary(dir: Option<&std::path::Path>) -> String {
    let Some(dir) = dir else { return String::new() };
    let mut words: Vec<String> = Vec::new();
    let skip = ["node_modules", "target", "dist", "build", ".git", "vendor", "venv", ".venv", "__pycache__"];
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten().take(200) {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || skip.contains(&name.as_str()) {
                continue;
            }
            let stem = std::path::Path::new(&name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if stem.len() > 2 && stem.len() < 40 && !words.contains(&stem) {
                words.push(stem);
            }
            if depth < 2 && e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) {
                stack.push((e.path(), depth + 1));
            }
            if words.len() >= 60 {
                return words.join(", ");
            }
        }
    }
    words.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ButtonEvent::*;

    #[test]
    fn hold_mode() {
        assert_eq!(decide(TalkMode::Hold, Pressed, false), Action::Start);
        assert_eq!(decide(TalkMode::Hold, Released, true), Action::Stop);
        assert_eq!(decide(TalkMode::Hold, Released, false), Action::Nothing, "a release without a press does nothing");
        assert_eq!(decide(TalkMode::Hold, Pressed, true), Action::Nothing, "a second press while held does nothing");
    }

    #[test]
    fn toggle_mode() {
        assert_eq!(decide(TalkMode::Toggle, Pressed, false), Action::Start);
        assert_eq!(decide(TalkMode::Toggle, Released, true), Action::Nothing, "releasing doesn't stop");
        assert_eq!(decide(TalkMode::Toggle, Pressed, true), Action::Stop);
    }

    #[test]
    fn voice_commands_only_on_their_own() {
        assert_eq!(voice_command("Stop."), Some(VoiceCommand::Stop));
        assert_eq!(voice_command(" cancel! "), Some(VoiceCommand::Stop));
        assert_eq!(voice_command("New task."), Some(VoiceCommand::NewTask));
        assert_eq!(voice_command("Start over"), Some(VoiceCommand::NewTask));
        assert_eq!(voice_command("Undo that."), Some(VoiceCommand::Undo));
        assert_eq!(voice_command("بس کن."), Some(VoiceCommand::Stop));
        assert_eq!(voice_command(" کار  جدید "), Some(VoiceCommand::NewTask));
        assert_eq!(voice_command("کافیه"), Some(VoiceCommand::Done));
        assert_eq!(voice_command("برگردونش!"), Some(VoiceCommand::Undo));
        assert_eq!(voice_command("اسپاتیفای رو باز کن"), None);
        assert_eq!(voice_command("undo the reminder and open notes"), None);
        assert_eq!(voice_command("stop using the old API and add a test"), None, "a request mentioning stop is a request");
        assert_eq!(voice_command(""), None);
    }

    #[test]
    fn vocabulary_is_names_only_and_bounded() {
        let dir = std::env::temp_dir().join(format!("xpu-vocab-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("src/auth")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/junk")).unwrap();
        std::fs::write(dir.join("src/auth/useAuthStore.ts"), "secret contents").unwrap();
        std::fs::write(dir.join(".env"), "KEY=1").unwrap();
        let v = project_vocabulary(Some(&dir));
        assert!(v.contains("useAuthStore"));
        assert!(!v.contains("secret") && !v.contains("junk") && !v.contains("env"), "{v}");
        for i in 0..200 {
            std::fs::write(dir.join(format!("file{i}.rs")), "").unwrap();
        }
        assert!(project_vocabulary(Some(&dir)).split(", ").count() <= 60);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
