//! The talk loop: button → record → transcribe → deliver (type it, or hand it to an agent).
use crate::audio::{Captured, Recording};
use crate::error::{msg, Error, Result};
use crate::input::ButtonEvent;
use crate::settings::{Delivery, Settings, SttEngine, TalkMode};
use crate::i18n::{tr, trf};
use crate::{agents, dictation, inject, sanitize, speech, stt};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};

const LIVE_EVERY: Duration = Duration::from_millis(900);
const LIVE_WINDOW_SECONDS: f32 = 15.0;
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
    /// After "Hey Xpu": ends when the user stops talking.
    Wake,
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
/// "Make that shorter" edits the last dictation if it was typed within this time.
const EDIT_WINDOW: Duration = Duration::from_secs(10 * 60);
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
        "para" | "detente" | "alto" | "stopp" | "halt" | "hör auf" | "arrête" | "arrete" | "停" | "停止" | "止めて" | "とめて" | "ストップ" => Some(VoiceCommand::Stop),
        "nueva tarea" | "neue aufgabe" | "nouvelle tâche" | "nouvelle tache" | "مهمة جديدة" | "新任务" | "新しいタスク" => Some(VoiceCommand::NewTask),
        "eso es todo" | "nada más" | "nada mas" | "das wars" | "das war es" | "cest tout" | "cest bon" | "هذا كل شيء" | "就这样" | "没了" | "以上です" | "以上" => Some(VoiceCommand::Done),
        "deshaz eso" | "deshacer" | "rückgängig" | "mach das rückgängig" | "annule ça" | "annule ca" | "annuler" | "تراجع" | "تراجع عن ذلك" | "撤销" | "元に戻して" | "取り消して" => Some(VoiceCommand::Undo),
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
    /// The plan's progress ("2/3") once the assistant has shown one.
    step: Mutex<Option<String>>,
}

impl MacHost {
    fn new(ctl: &Arc<Controller>, app: &AppHandle, speak: bool) -> Arc<Self> {
        Arc::new(Self { ctl: ctl.clone(), app: app.clone(), speak, step: Mutex::new(None) })
    }

    fn label(&self) -> String {
        match self.step.lock().as_deref() {
            Some(p) => format!("Mac · {p}"),
            None => "Mac".into(),
        }
    }
}

impl crate::mac::server::Host for MacHost {
    fn status(&self, text: &str) {
        self.ctl.overlay(&self.app, Phase::Working, text, Some(&self.label()));
    }
    fn plan(&self, plan: &crate::mac::plan::Plan) {
        *self.step.lock() = Some(plan.progress());
        self.ctl.overlay(&self.app, Phase::Working, plan.current(), Some(&self.label()));
    }
    fn ask(&self, ask: crate::mac::server::Ask, text: &str) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.ctl.pending.lock() = Some(tx);
        match ask {
            crate::mac::server::Ask::Cancel => self.ctl.overlay(&self.app, Phase::Pending, text, Some(tr("Press or knock to cancel"))),
            crate::mac::server::Ask::Approve => {
                self.ctl.overlay(&self.app, Phase::Confirm, &format!("{text}?"), Some(tr("Press or knock to allow")));
                if self.speak {
                    speech::speak(trf("{what}? Press or knock to allow.", &[("what", text)]));
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
        self.ctl.overlay(&self.app, Phase::Working, tr("Working…"), Some(&self.agent));
    }
    fn record(&self, tool: &str, summary: &str, outcome: crate::mac::server::Outcome, detail: &str) {
        crate::activity::record(tool, summary, outcome, detail);
    }
    fn highlight(&self, _frame: Option<(f64, f64, f64, f64)>) {}

    fn approval(&self, q: &crate::agents::approvals::Question) -> tokio::sync::oneshot::Receiver<(crate::agents::approvals::Answer, crate::mac::server::Via)> {
        use crate::agents::approvals::{parse_answer, Answer};
        use crate::mac::server::Via;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let shown = trf("{agent} wants to {what}", &[("agent", &self.agent), ("what", &q.what)]);
        let hint = match q.danger {
            Some(d) => trf("This {danger}: say “yes” to allow", &[("danger", tr(d))]),
            None => tr("Say yes or no, or press to allow").to_string(),
        };
        let spoken = {
            let what = q.what.replace('`', "");
            match q.danger {
                Some(d) => trf("{agent} wants to {what}. This {danger}. Say yes to allow, or no.", &[("agent", &self.agent), ("what", &what), ("danger", tr(d))]),
                None => trf("{agent} wants to {what}. Say yes or no.", &[("agent", &self.agent), ("what", &what)]),
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
                            ctl.overlay(&app, Phase::Confirm, &shown, Some(tr("A press can't allow this one: say “yes”")));
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
                                ctl.overlay(&app, Phase::Confirm, &shown, Some(tr("Sorry: yes or no?")));
                                if speak {
                                    speech::speak_and_wait(tr("Sorry, yes or no?").to_string()).await;
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

impl Phase {
    /// The pill's label, in the app's language.
    fn label(&self) -> &'static str {
        tr(match self {
            Phase::Listening => "Listening",
            Phase::Transcribing => "Transcribing",
            Phase::Typing => "Typing",
            Phase::Working => "Working",
            Phase::Done => "Done",
            Phase::Error => "Problem",
            Phase::Pending => "About to",
            Phase::Confirm => "Allow?",
            Phase::Hidden => return "",
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayState {
    pub phase: Phase,
    pub label: &'static str,
    pub text: String,
    pub agent: Option<String>,
    /// The text is the user's words as they speak (not a hint).
    pub live: bool,
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
    tray_live: std::sync::atomic::AtomicBool,
    timing: Mutex<Option<Timing>>,
    /// The last dictation typed, for spoken edits ("scratch that", "make that shorter").
    typed: Mutex<Option<inject::Typed>>,
}

#[derive(Debug, Clone, Copy)]
struct Timing {
    ended: Instant,
    text_ms: u128,
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
            tray_live: std::sync::atomic::AtomicBool::new(false),
            timing: Mutex::new(None),
            typed: Mutex::new(None),
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
        let auto = (auto_stop && trigger != Trigger::Hold) || matches!(trigger, Trigger::Reply | Trigger::Wake);
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
                            _ => this.cancel_if(&a, id, tr("Didn't hear anything")),
                        }
                    });
                }
            }
        };
        match Recording::start(device.as_deref(), max, level) {
            Ok(r) => {
                *self.recording.lock() = Some(Rec { rec: r, started: Instant::now(), id, trigger, vad });
                self.live_words(app, id);
                if cues {
                    crate::cues::play(crate::cues::Cue::Start);
                }
                let hint = tr(match (trigger, auto) {
                    (Trigger::Reply, _) => "Listening for your reply… (or say “that's all”)",
                    (Trigger::Hold, _) => "Listening… release to send",
                    (Trigger::Click, true) => "Listening… pause or click to send",
                    (Trigger::Click, false) => "Listening… click to send",
                    (Trigger::Knock, true) => "Listening… pause or knock to send",
                    (Trigger::Knock, false) => "Listening… knock twice to send",
                    (Trigger::Headset, true) => "Listening… pause or press your AirPods to send",
                    (Trigger::Headset, false) => "Listening… press your AirPods to send",
                    (Trigger::Wake, _) => "Listening… pause to send",
                });
                self.overlay(app, Phase::Listening, hint, None);
            }
            Err(e) => self.fail(app, e),
        }
    }

    fn live_words(self: &Arc<Self>, app: &AppHandle, id: u64) {
        let (model, language, instant) = {
            let s = self.settings.read();
            if !s.live_words || s.stt != SttEngine::Local || (s.local_model.ends_with(".en") && !matches!(s.language.as_str(), "en" | "auto")) {
                return;
            }
            (s.local_model.clone(), s.language.clone(), s.instant && s.delivery == Delivery::ControlMac)
        };
        let (this, app) = (self.clone(), app.clone());
        tauri::async_runtime::spawn(async move {
            let mut shown = String::new();
            loop {
                tokio::time::sleep(LIVE_EVERY).await;
                let snap = {
                    let rec = this.recording.lock();
                    let Some(r) = rec.as_ref().filter(|r| r.id == id) else { break };
                    let voiced = r.vad.lock().voiced_ms();
                    if voiced < 300 {
                        continue;
                    }
                    match r.rec.snapshot(LIVE_WINDOW_SECONDS) {
                        Some(a) => (a, voiced),
                        None => continue,
                    }
                };
                let (audio, voiced) = snap;
                let (engine, model, lang) = (this.engine.clone(), model.clone(), language.clone());
                let heard = tauri::async_runtime::spawn_blocking(move || engine.transcribe(&model, &stt::Request { audio: &audio, language: &lang, vocabulary: "" })).await;
                let text = match heard {
                    Ok(Ok(t)) => sanitize::clean_transcript(&t),
                    _ => break,
                };
                let complete = voice_command(&text).is_some() || (instant && crate::mac::quick::parse(&text).is_some());
                {
                    let rec = this.recording.lock();
                    let Some(r) = rec.as_ref().filter(|r| r.id == id) else { break };
                    r.vad.lock().set_end_silence(if complete { crate::vad::QUICK_END_MS } else { crate::vad::END_SILENCE_MS });
                }
                if !text.is_empty() && !crate::vad::is_phantom(&text, voiced) && text != shown {
                    this.show(&app, Phase::Listening, &text, None, true);
                    shown = text;
                }
            }
        });
    }

    fn record_timing(&self, what: &str) {
        let Some(t) = self.timing.lock().take() else { return };
        let total = t.ended.elapsed().as_millis();
        let after = total.saturating_sub(t.text_ms);
        crate::activity::record("timing", &format!("{what}: speech to text {} ms · then {} ms", t.text_ms, after), crate::mac::server::Outcome::Done, &format!("{:.1} s", total as f64 / 1000.0));
    }

    fn set_tray_live(&self, app: &AppHandle, on: bool) {
        if self.tray_live.swap(on, std::sync::atomic::Ordering::Relaxed) == on {
            return;
        }
        let Some(tray) = app.tray_by_id("tray") else { return };
        let icon = if on { tauri::image::Image::from_bytes(include_bytes!("../icons/tray-live.png")).ok() } else { app.default_window_icon().cloned() };
        if let Err(e) = tray.set_icon(icon) {
            log::warn!("tray icon: {e}");
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
            this.overlay(&app, Phase::Transcribing, tr("Transcribing…"), None);
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
            self.overlay(app, Phase::Error, tr("Didn't hear anything"), None);
            self.hide_later(app, Duration::from_millis(1500));
            return Ok(());
        }
        if peak < SILENT_PEAK {
            return Err(msg(if cfg!(target_os = "macos") {
                tr("No sound from the microphone. Turn on Xpu Command in System Settings › Privacy & Security › Microphone.")
            } else {
                tr("No sound from the microphone. Check microphone access and the input device in Settings.")
            }));
        }
        let s = self.settings.read().clone();
        let vocabulary = project_vocabulary(s.project_dir.as_deref());
        let ended = Instant::now();
        let text = self.transcribe(&s, audio, vocabulary).await?;
        *self.timing.lock() = Some(Timing { ended, text_ms: ended.elapsed().as_millis() });
        if heard.quiet && (text.is_empty() || crate::vad::is_phantom(&text, heard.voiced_ms) || voice_command(&text) == Some(VoiceCommand::Done)) {
            self.overlay(app, Phase::Hidden, "", None);
            return Ok(());
        }
        if text.is_empty() || crate::vad::is_phantom(&text, heard.voiced_ms) {
            self.overlay(app, Phase::Error, tr("Didn't catch that"), None);
            self.hide_later(app, Duration::from_secs(2));
            return Ok(());
        }
        self.deliver(app, &s, text).await
    }

    /// Acts on what was said: a voice command, a routine, or the request itself (typed, to the
    /// agent, or on the Mac).
    async fn deliver(self: &Arc<Self>, app: &AppHandle, s: &Settings, text: String) -> Result<()> {
        if matches!(s.delivery, Delivery::Agent | Delivery::ControlMac) {
            match voice_command(&text) {
                Some(VoiceCommand::Stop) => {
                    let stopped = self.agent_cancel.lock().take().map(|c| c.send(()).is_ok()).unwrap_or(false);
                    self.overlay(app, Phase::Done, tr(if stopped { "Stopped the agent" } else { "Nothing to stop" }), None);
                    self.hide_later(app, Duration::from_secs(2));
                    return Ok(());
                }
                Some(VoiceCommand::Done) => {
                    self.overlay(app, Phase::Hidden, "", None);
                    return Ok(());
                }
                Some(VoiceCommand::Undo) => {
                    let said = self.undo().await;
                    self.overlay(app, Phase::Done, &said, Some(tr("Undo")));
                    if s.speak_summary {
                        speech::speak(said);
                    }
                    self.hide_later(app, Duration::from_secs(3));
                    return Ok(());
                }
                Some(VoiceCommand::NewTask) => {
                    self.session.lock().take();
                    self.overlay(app, Phase::Done, tr("New conversation: the next request starts fresh"), None);
                    self.hide_later(app, Duration::from_secs(2));
                    return Ok(());
                }
                None => {}
            }
        }
        if matches!(s.delivery, Delivery::Agent | Delivery::ControlMac) {
            if let Some(r) = crate::routines::match_spoken(&text, &crate::routines::list()).cloned() {
                return self.run_routine(app, s, r).await;
            }
        }
        match s.delivery {
            Delivery::TypeIntoFocused => self.dictate(app, s, text).await?,
            Delivery::Agent => self.run_agent(app, s, &text).await?,
            Delivery::ControlMac => {
                if s.instant && self.try_instant(app, s, &text).await {
                    return Ok(());
                }
                self.run_mac(app, s, &text).await?
            }
        }
        Ok(())
    }

    /// Types a dictation into the app in front, polished for that app; or, if it is a spoken edit
    /// ("make that shorter", "scratch that") of the last dictation there, applies it.
    async fn dictate(self: &Arc<Self>, app: &AppHandle, s: &Settings, text: String) -> Result<()> {
        let (pid, bundle) = front_app();
        let style = bundle.as_deref().map_or(dictation::Style::Prose, dictation::style_for);
        let last = self.typed.lock().clone().filter(|t| Some(t.pid) == pid && t.at.elapsed() < EDIT_WINDOW);
        if let (Some(edit), Some(last)) = (dictation::edit_command(&text), last.clone()) {
            return self.edit_typed(app, s, last, edit, style).await;
        }
        let shown = if s.polish { dictation::polish(&text, style) } else { text };
        if shown.is_empty() {
            self.overlay(app, Phase::Hidden, "", None);
            return Ok(());
        }
        self.overlay(app, Phase::Typing, &shown, None);
        let typed = self.on_main(app, {
            let (t, submit) = (shown.clone(), s.auto_submit);
            move || inject::type_after(&t, submit, last.as_ref())
        })
        .await?;
        *self.typed.lock() = match pid {
            Some(pid) if !s.auto_submit => Some(inject::Typed { text: typed.trim_start().to_string(), pid, at: Instant::now() }),
            _ => None,
        };
        self.record_timing("Typed");
        self.hide_later(app, Duration::from_millis(700));
        Ok(())
    }

    async fn edit_typed(self: &Arc<Self>, app: &AppHandle, s: &Settings, last: inject::Typed, edit: dictation::Edit, style: dictation::Style) -> Result<()> {
        let new = match edit {
            dictation::Edit::Delete => String::new(),
            dictation::Edit::Rewrite(how) => {
                self.overlay(app, Phase::Working, tr("Rewriting…"), Some(tr("Dictation")));
                let profile = s.claude_profile.as_deref().map(crate::settings::claude_profile_dir).transpose()?;
                let out = dictation::rewrite(&last.text, &how, profile.as_deref()).await?;
                if s.polish { dictation::polish(&out, style) } else { out }
            }
        };
        let fix = (last.clone(), new.clone());
        self.on_main(app, move || inject::replace_typed(&fix.0, &fix.1)).await?;
        crate::activity::record("dictation", if new.is_empty() { "Removed the last dictation" } else { "Rewrote the last dictation" }, crate::mac::server::Outcome::Done, "");
        *self.typed.lock() = (!new.is_empty()).then(|| inject::Typed { text: new.clone(), pid: last.pid, at: Instant::now() });
        self.record_timing("Edit");
        if new.is_empty() {
            self.overlay(app, Phase::Done, tr("Removed it"), Some(tr("Dictation")));
            self.hide_later(app, Duration::from_millis(1200));
        } else {
            self.overlay(app, Phase::Done, &new, Some(tr("Dictation")));
            self.hide_later(app, Duration::from_secs(3));
        }
        Ok(())
    }

    /// Runs `f` on the main thread (keyboard and Accessibility calls) and waits for its result.
    async fn on_main<T: Send + 'static>(&self, app: &AppHandle, f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| msg(e.to_string()))?;
        rx.await.map_err(|_| msg("typing was interrupted"))?
    }

    /// Something else is using the microphone or waits for the user's answer.
    pub fn busy(&self) -> bool {
        self.recording.lock().is_some() || self.pending.lock().is_some()
    }

    /// Whisper on this Mac with the shared engine (blocking): for the wake phrase.
    pub fn transcribe_local(&self, model: &str, language: &str, audio: &Captured, prompt: &str) -> Result<String> {
        self.engine.transcribe(model, &stt::Request { audio, language, vocabulary: prompt })
    }

    /// "Hey Xpu" was heard. With a request in the same breath, it runs now; otherwise listening
    /// starts for it.
    pub fn on_wake(self: &Arc<Self>, app: &AppHandle, rest: String) {
        if !self.hook.enabled.load(std::sync::atomic::Ordering::Relaxed) || self.busy() {
            return;
        }
        if rest.split_whitespace().count() >= 2 || voice_command(&rest).is_some() {
            if self.settings.read().sound_cues {
                crate::cues::play(crate::cues::Cue::Send);
            }
            *self.timing.lock() = Some(Timing { ended: Instant::now(), text_ms: 0 });
            let (this, app) = (self.clone(), app.clone());
            tauri::async_runtime::spawn(async move {
                let s = this.settings.read().clone();
                if let Err(e) = this.deliver(&app, &s, rest).await {
                    this.fail(&app, e);
                }
            });
        } else {
            self.start_recording(app, Trigger::Wake);
        }
    }

    /// Speech to text with the configured engine; the result is cleaned for display and use.
    async fn transcribe(&self, s: &Settings, audio: Captured, vocabulary: String) -> Result<String> {
        let raw = match s.stt {
            SttEngine::Local if s.local_model.ends_with(".en") && !matches!(s.language.as_str(), "en" | "auto") => {
                return Err(msg(tr("This speech model only understands English. In Settings › Speech to text, pick a multilingual model (Small is best for Persian).")));
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
        let project = s.project_dir.clone().ok_or_else(|| msg(tr("Choose a project folder in Settings first")))?;
        if self.agent_cancel.lock().is_some() {
            return Err(msg(tr("The agent is still working; say “stop” to stop it")));
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
            return Err(msg(tr("Still working on the last request; say “stop” to stop it")));
        }
        let workspace = crate::mac::workspace()?;
        let host = MacHost::new(self, app, s.speak_summary);
        let policy = s.mac_policy();
        let exec = Arc::new(crate::mac::exec::MacExec::new(policy.clone(), workspace).with_memory(s.memory).without_learning());
        let steps: Vec<(String, serde_json::Value)> = r.steps.iter().map(|st| (st.tool.clone(), st.args.clone())).collect();
        self.overlay(app, Phase::Working, &trf("Routine: {name}", &[("name", &r.name)]), Some(tr("Routine")));
        let (done, stopped) = crate::mac::server::run_steps(host, exec, policy, Default::default(), &steps).await;
        crate::frame::hide(app);
        let said = match stopped {
            None if done.len() == 1 => trf("Ran {name}: 1 step", &[("name", &r.name)]),
            None => trf("Ran {name}: {count} steps", &[("name", &r.name), ("count", &done.len().to_string())]),
            Some(why) => trf("Stopped {name} after {done} of {total} steps: {why}", &[("name", &r.name), ("done", &done.len().to_string()), ("total", &steps.len().to_string()), ("why", &crate::agents::clip(&why, 120))]),
        };
        self.record_timing("Routine");
        self.overlay(app, Phase::Done, &said, Some(tr("Routine")));
        if s.speak_summary {
            speech::speak(said);
        }
        self.hide_later(app, Duration::from_secs(3));
        Ok(())
    }

    #[cfg(target_os = "macos")]
    async fn try_instant(self: &Arc<Self>, app: &AppHandle, s: &Settings, text: &str) -> bool {
        let Some(steps) = crate::mac::quick::parse(text) else { return false };
        if self.agent_cancel.lock().is_some() {
            return false;
        }
        let names_ok = {
            let installed = crate::mac::apps::installed();
            steps.iter().all(|(tool, args)| match tool.as_str() {
                "open_app" | "quit_app" => args["name"].as_str().is_some_and(|n| crate::mac::apps::find(&installed, n).is_ok()),
                _ => true,
            })
        };
        if !names_ok {
            return false;
        }
        let started = Instant::now();
        let Ok(workspace) = crate::mac::workspace() else { return false };
        let host = MacHost::new(self, app, s.speak_summary);
        let policy = s.mac_policy();
        let exec = Arc::new(crate::mac::exec::MacExec::new(policy.clone(), workspace).with_memory(s.memory));
        let (done, stopped) = crate::mac::server::run_steps(host, exec, policy, Default::default(), &steps).await;
        crate::frame::hide(app);
        match (stopped, done.first()) {
            (None, Some(said)) => {
                log::info!("instant command in {} ms", started.elapsed().as_millis());
                self.record_timing("Instant command");
                self.overlay(app, Phase::Done, said, Some("Mac"));
                if s.speak_summary {
                    speech::speak(said.clone());
                }
                self.hide_later(app, Duration::from_millis(2200));
                true
            }
            (Some(why), _) if why.contains("cancelled") || why.contains("did not") => {
                self.overlay(app, Phase::Done, tr("Cancelled"), Some("Mac"));
                self.hide_later(app, Duration::from_millis(1500));
                true
            }
            _ => false,
        }
    }

    #[cfg(not(target_os = "macos"))]
    async fn try_instant(self: &Arc<Self>, _app: &AppHandle, _s: &Settings, _text: &str) -> bool {
        false
    }

    #[cfg(not(target_os = "macos"))]
    pub async fn run_routine(self: &Arc<Self>, _app: &AppHandle, _s: &Settings, _r: crate::routines::Routine) -> Result<()> {
        Err(crate::mac::not_supported())
    }

    pub async fn undo(&self) -> String {
        if self.agent_cancel.lock().is_some() {
            return tr("Still working on the last request; say “stop” first").into();
        }
        #[cfg(target_os = "macos")]
        {
            let said = tokio::task::spawn_blocking(crate::mac::exec::undo_last).await.unwrap_or_else(|_| tr("Undo failed").into());
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
            return Err(msg(tr("Still working on the last request; say “stop” to stop it")));
        }
        let workspace = crate::mac::workspace()?;
        let resume = {
            let mut slot = self.session.lock();
            if slot.as_ref().is_some_and(|x| x.agent != crate::settings::AgentKind::Claude || x.project != workspace || x.profile != s.claude_profile || x.at.elapsed() > SESSION_IDLE) {
                *slot = None;
            }
            slot.as_ref().map(|x| x.id.clone())
        };
        let host = MacHost::new(self, app, s.speak_summary);
        let policy = s.mac_policy();
        let context = if s.share_context {
            let p = policy.clone();
            tokio::task::spawn_blocking(move || crate::mac::ui::context(&p)).await.ok().flatten()
        } else {
            None
        };
        if let Some(c) = &context {
            crate::activity::record("context", &c.summary(), crate::mac::server::Outcome::Done, "");
        }
        let seen: Vec<String> = context.iter().filter(|c| c.selection.is_some()).map(|c| c.bundle.clone()).collect();
        let exec = Arc::new(crate::mac::exec::MacExec::new(policy.clone(), workspace.clone()).with_memory(s.memory));
        let server = crate::mac::server::start(host, exec, policy, Default::default(), seen).await?;
        let config = crate::mac::McpConfigFile::create(&crate::settings::config_dir()?, &server.mcp_config())?;
        let memory = if s.memory { crate::memory::prompt_block(&crate::memory::list()) } else { String::new() };
        let lang = speech::language_of(text, &s.language).unwrap_or("en");
        let prompt = crate::mac::system_prompt(chrono::Local::now(), &memory, lang);
        let model = crate::mac::pick_model(s.mac_model, text, context.as_ref().is_some_and(|c| c.selection.is_some()));
        if s.mac_model == crate::settings::MacModel::Auto {
            crate::activity::record("model", &format!("Chose {} for this request", crate::mac::model_label(model)), crate::mac::server::Outcome::Done, "automatic");
        }
        let profile = s.claude_profile.as_deref().map(crate::settings::claude_profile_dir).transpose()?;
        let full = crate::mac::with_context(text, context.as_ref());
        let run = agents::start(agents::Request {
            kind: crate::settings::AgentKind::Claude,
            prompt: &full,
            project: &workspace,
            safety: crate::settings::Safety::ReadOnly,
            resume: resume.as_deref(),
            timeout: MAC_TIMEOUT,
            claude_config_dir: profile.as_deref(),
            mac: Some(agents::MacArgs { mcp_config: config.path(), model: crate::mac::model_name(model), system_prompt: &prompt }),
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
        let label = if continuing { trf("{name} · continuing", &[("name", &name)]) } else { name.clone() };
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
                        this.record_timing(&name);
                        if let Some(x) = this.session.lock().as_mut() {
                            x.at = Instant::now();
                        }
                        let shown = if summary.is_empty() { tr("Done").to_string() } else { summary.clone() };
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
                        this.overlay(&app, Phase::Error, &crate::health::recovery(&error), Some(&name));
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
        self.overlay(app, Phase::Error, &crate::health::recovery(&e.to_string()), None);
        self.hide_later(app, Duration::from_secs(5));
    }

    pub fn overlay(&self, app: &AppHandle, phase: Phase, text: &str, agent: Option<&str>) {
        self.show(app, phase, text, agent, false);
    }

    fn show(&self, app: &AppHandle, phase: Phase, text: &str, agent: Option<&str>, live: bool) {
        self.generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.set_tray_live(app, matches!(phase, Phase::Listening | Phase::Confirm | Phase::Pending));
        let Some(w) = app.get_webview_window("overlay") else { return };
        if matches!(phase, Phase::Hidden) {
            let _ = w.hide();
        } else {
            place_overlay(&w);
            let _ = w.set_ignore_cursor_events(true);
            crate::overlay_window::show_without_focus(&w);
        }
        let _ = app.emit_to("overlay", "overlay", OverlayState { label: phase.label(), phase, text: text.to_string(), agent: agent.map(str::to_string), live });
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

/// The app in front (its process and bundle id), where a dictation will be typed.
#[cfg(target_os = "macos")]
fn front_app() -> (Option<i32>, Option<String>) {
    let pid = crate::mac::ui::front_pid().filter(|p| *p != std::process::id() as i32);
    (pid, pid.and_then(crate::mac::ui::app_of_pid).map(|a| a.bundle))
}

#[cfg(not(target_os = "macos"))]
fn front_app() -> (Option<i32>, Option<String>) {
    (None, None)
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
        assert_eq!(voice_command("¡Para!"), Some(VoiceCommand::Stop));
        assert_eq!(voice_command("Mach das rückgängig."), Some(VoiceCommand::Undo));
        assert_eq!(voice_command("C'est tout."), Some(VoiceCommand::Done));
        assert_eq!(voice_command("撤销"), Some(VoiceCommand::Undo));
        assert_eq!(voice_command("新しいタスク"), Some(VoiceCommand::NewTask));
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
