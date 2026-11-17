//! "Hey Xpu": an optional wake phrase, so no button is needed. Off by default.
//!
//! The microphone stays open while it is on (macOS shows its indicator); audio lives only in a
//! few seconds of memory, is never written anywhere and never leaves the Mac. A short burst of
//! speech (a phrase, not a conversation) is checked with the speech model already on this Mac; only
//! a match does anything. Paused while the app records, asks, or speaks itself.
use crate::controller::Controller;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How much audio is kept: enough for the phrase plus a short request after it.
const KEEP_SECONDS: u32 = 6;
/// Speech shorter than this is a cough or a click; longer than this is talking, not a wake phrase
/// (a request said in one breath, "Hey Xpu, open Notes", fits).
const MIN_VOICED_MS: u32 = 300;
const MAX_SEGMENT_MS: u32 = 5000;
/// Quiet that ends a phrase.
const END_QUIET_MS: u32 = 450;
const LOUD_MIN_MS: u32 = 150;
/// At most one check in this time, so a TV or music in the room can't keep the model busy.
const MIN_GAP: Duration = Duration::from_millis(1200);
/// Whisper gets no prompt for the check: priming it with the name made it hear "Hey Xpu" in "Hey
/// Sue" and "Hey Expo", and drop the phrase when a request followed it (measured with `say` voices:
/// without a prompt, 12 of 12 phrases matched and 0 of 16 other sentences did).
pub const PROMPT: &str = "";

static RUNNING: AtomicBool = AtomicBool::new(false);
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

/// Why the wake phrase isn't listening, if it should be.
pub fn problem() -> Option<String> {
    PROBLEM.lock().clone()
}

/// Finds short bursts of speech in the microphone level (RMS every ~50 ms).
#[derive(Debug, Clone)]
pub struct Segmenter {
    floor: f32,
    loud_ms: u32,
    quiet_ms: u32,
    voiced_ms: u32,
    len_ms: u32,
    in_speech: bool,
    too_long: bool,
}

impl Default for Segmenter {
    fn default() -> Self {
        Self { floor: f32::MAX, loud_ms: 0, quiet_ms: 0, voiced_ms: 0, len_ms: 0, in_speech: false, too_long: false }
    }
}

impl Segmenter {
    fn threshold(&self) -> f32 {
        let floor = if self.floor == f32::MAX { 0.003 } else { self.floor };
        (floor * 3.0).clamp(0.01, 0.06)
    }

    pub fn reset(&mut self) {
        *self = Self { floor: self.floor, ..Self::default() };
    }

    /// Some(ms of audio to check, ending now) when a short phrase has just ended.
    pub fn push(&mut self, rms: f32) -> Option<u32> {
        let step = crate::vad::STEP_MS;
        let rms = if rms.is_finite() { rms.max(0.0) } else { 0.0 };
        if rms < self.floor {
            self.floor = rms;
        } else if self.floor != f32::MAX && !self.in_speech {
            self.floor += (rms.min(0.02) - self.floor) * 0.01;
        }
        let loud = rms > self.threshold();
        if loud {
            self.loud_ms += step;
            self.quiet_ms = 0;
        } else {
            self.loud_ms = 0;
            self.quiet_ms += step;
        }
        if !self.in_speech {
            if self.loud_ms >= LOUD_MIN_MS {
                self.in_speech = true;
                self.len_ms = self.loud_ms;
                self.voiced_ms = self.loud_ms;
            }
            return None;
        }
        self.len_ms += step;
        if loud {
            self.voiced_ms += step;
        }
        if self.len_ms > MAX_SEGMENT_MS {
            self.too_long = true;
        }
        if self.quiet_ms < END_QUIET_MS {
            return None;
        }
        let (len, voiced, too_long) = (self.len_ms, self.voiced_ms, self.too_long);
        self.reset();
        (!too_long && voiced >= MIN_VOICED_MS).then_some(len + 250)
    }
}

/// If `text` starts with the wake phrase, what was said after it ("" for nothing).
/// Whisper writes "Xpu" many ways: "XPU", "X.P.U.", "ex pee you", "Expu", "ایکس پی یو"…
pub fn heard(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let norm: Vec<String> = words.iter().map(|w| crate::sanitize::fold_persian(&w.to_lowercase()).chars().filter(|c| c.is_alphanumeric()).collect()).collect();
    const GREETING: &[&str] = &["hey", "hi", "hay", "hei", "heyy", "ok", "okay", "oh", "هی", "های", "hej", "oye", "he", "hallo", "salut", "يا"];
    let piece = |w: &str| -> Option<&'static str> {
        Some(match w {
            "x" | "ex" | "eks" | "ix" | "ایکس" | "اکس" | "إكس" => "x",
            "p" | "pee" | "pi" | "pea" | "pe" | "پی" | "بي" => "p",
            "u" | "you" | "yu" | "yo" | "ou" | "یو" | "يو" => "u",
            "xp" | "expee" | "xpee" => "xp",
            "pu" | "pyou" | "peeyou" | "pew" => "pu",
            "xpu" | "xpus" | "expu" | "xpew" | "xbu" | "xpou" | "xpyou" | "xpyu" | "expew" | "ekspu" | "ایکسپیو" => "xpu",
            _ => return None,
        })
    };
    // Run together: "HeyXpu", "HXPU", "KXPU".
    if let Some(k) = norm.iter().take(2).position(|w| w.strip_suffix("xpu").is_some_and(|g| GREETING.contains(&g) || matches!(g, "h" | "k" | "hai"))) {
        let rest = words[k + 1..].join(" ");
        return Some(rest.trim_start_matches(|c: char| !c.is_alphanumeric()).trim().to_string());
    }
    let start = norm.iter().take(2).position(|w| GREETING.contains(&w.as_str()))? + 1;
    let mut joined = String::new();
    for (k, w) in norm.iter().enumerate().skip(start).take(3) {
        joined.push_str(piece(w)?);
        if joined == "xpu" {
            let rest = words[k + 1..].join(" ");
            return Some(rest.trim_start_matches(|c: char| !c.is_alphanumeric()).trim().to_string());
        }
        if !"xpu".starts_with(&joined) {
            return None;
        }
    }
    None
}

/// Starts listening if it is on and not already running. Stops by itself when turned off.
pub fn start(app: &tauri::AppHandle, ctl: &Arc<Controller>) {
    if !ctl.settings.read().wake_word || RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let (app, ctl) = (app.clone(), ctl.clone());
    let spawned = std::thread::Builder::new().name("xpu-wake".into()).spawn(move || {
        loop {
            match run(&app, &ctl) {
                Ok(Next::Reopen) => continue,
                Ok(Next::Stop) => break,
                Err(e) => {
                    let mut p = PROBLEM.lock();
                    if p.as_deref() != Some(e.as_str()) {
                        log::warn!("wake phrase stopped: {e}");
                    }
                    *p = Some(e);
                    break;
                }
            }
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

enum Next {
    Stop,
    /// The microphone, model or language changed: listen again with the new ones.
    Reopen,
}

fn run(app: &tauri::AppHandle, ctl: &Arc<Controller>) -> Result<Next, String> {
    let s = ctl.settings.read().clone();
    let model = crate::stt::model::find(&s.local_model).filter(|m| crate::stt::model::is_downloaded(m)).ok_or("it needs a speech model on this Mac: download one in Speech")?;
    let model_id = model.id.to_string();
    let language = if s.local_model.ends_with(".en") { "en".to_string() } else { s.language.clone() };
    let device = s.input_device.clone();
    let (tx, rx) = std::sync::mpsc::channel::<f32>();
    let tx = Mutex::new(tx);
    let rec = crate::audio::Recording::listen(device.as_deref(), KEEP_SECONDS, move |rms| {
        let _ = tx.lock().send(rms);
    })
    .map_err(|e| e.to_string())?;
    *PROBLEM.lock() = None;
    log::info!("wake phrase listening");
    let mut seg = Segmenter::default();
    let mut last_check = Instant::now() - MIN_GAP;
    loop {
        let rms = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(r) => r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => 0.0,
            Err(_) => return Err("the microphone stopped".into()),
        };
        {
            let now = ctl.settings.read();
            if !now.wake_word {
                break;
            }
            if now.input_device != device || now.local_model != s.local_model || now.language != s.language {
                return Ok(Next::Reopen);
            }
        }
        if !ctl.hook.enabled.load(Ordering::Relaxed) || ctl.busy() || crate::speech::speaking() {
            seg.reset();
            continue;
        }
        let Some(ms) = seg.push(rms) else { continue };
        if last_check.elapsed() < MIN_GAP {
            continue;
        }
        last_check = Instant::now();
        let Some(audio) = rec.snapshot(ms as f32 / 1000.0) else { continue };
        let started = Instant::now();
        let text = match ctl.transcribe_local(&model_id, &language, &audio, PROMPT) {
            Ok(t) => crate::sanitize::clean_transcript(&t),
            Err(e) => {
                log::warn!("wake check failed: {e}");
                continue;
            }
        };
        while rx.try_recv().is_ok() {}
        log::debug!("wake check in {} ms", started.elapsed().as_millis());
        if let Some(rest) = heard(&text) {
            log::info!("wake phrase heard");
            ctl.on_wake(app, rest);
            seg.reset();
        }
    }
    drop(rec);
    log::info!("wake phrase off");
    Ok(Next::Stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(seg: &mut Segmenter, levels: &[(f32, u32)]) -> Vec<u32> {
        let mut out = Vec::new();
        for &(rms, ms) in levels {
            for _ in 0..ms / crate::vad::STEP_MS {
                if let Some(x) = seg.push(rms) {
                    out.push(x);
                }
            }
        }
        out
    }

    #[test]
    fn a_short_phrase_is_found_and_talking_is_not() {
        let mut s = Segmenter::default();
        let got = feed(&mut s, &[(0.002, 1000), (0.05, 900), (0.002, 600)]);
        assert_eq!(got.len(), 1);
        assert!((1300..=1800).contains(&got[0]), "the phrase plus its quiet tail: {got:?}");
        assert!(feed(&mut s, &[(0.05, 100), (0.002, 800)]).is_empty(), "a click is not speech");
        assert!(feed(&mut s, &[(0.05, 7000), (0.002, 800)]).is_empty(), "a long talk is not a wake phrase");
        assert_eq!(feed(&mut s, &[(0.05, 800), (0.002, 600)]).len(), 1, "and it recovers after one");
    }

    #[test]
    fn the_phrase_in_its_many_spellings() {
        for t in ["Hey Xpu.", "HeyXpu.", "HXPU", "Hey, XPU!", "hey x p u", "Hey ex pee you.", "Hey Expu", "hi xpu", "Okay, X.P.U.", "هی ایکس پی یو", "Hey XP, you"] {
            assert_eq!(heard(t).as_deref(), Some(""), "{t}");
        }
        assert_eq!(heard("Hey Xpu, open Notes.").as_deref(), Some("open Notes."));
        assert_eq!(heard("Hey XPU what's the weather").as_deref(), Some("what's the weather"));
        assert_eq!(heard("KXPU, Open Notes.").as_deref(), Some("Open Notes."));
        assert_eq!(heard("هی ایکس پی یو اسپاتیفای رو باز کن").as_deref(), Some("اسپاتیفای رو باز کن"));
        for t in ["Hey you", "Hey, how are you?", "the XPU is fast", "Hey X", "Hey expo", "", "Xpu", "Hey Sue, open the door", "the cpu"] {
            assert_eq!(heard(t), None, "{t}");
        }
    }
}
