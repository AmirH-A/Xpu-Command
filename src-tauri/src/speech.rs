//! Spoken summaries and questions in the headset. The synthesizer lives on one thread; requests
//! queue to it. `speak_and_wait` tells the caller when speaking has finished (so listening for a
//! reply doesn't hear our own voice).
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

type Job = (String, Option<tokio::sync::oneshot::Sender<()>>);
static TX: OnceLock<Sender<Job>> = OnceLock::new();
static LANGUAGE: parking_lot::Mutex<String> = parking_lot::Mutex::new(String::new());

pub fn set_language(code: &str) {
    *LANGUAGE.lock() = code.to_string();
}

pub fn language_of(text: &str, preferred: &str) -> Option<&'static str> {
    let has = |f: &dyn Fn(char) -> bool| text.chars().any(f);
    if crate::sanitize::is_mostly_arabic_script(text) {
        return Some(if preferred == "fa" || has(&|c| matches!(c, 'پ' | 'چ' | 'ژ' | 'گ')) { "fa" } else { "ar" });
    }
    if has(&|c| matches!(c as u32, 0x3040..=0x30FF)) {
        return Some("ja");
    }
    if has(&|c| matches!(c as u32, 0x4E00..=0x9FFF)) {
        return Some(if preferred == "ja" { "ja" } else { "zh" });
    }
    let lower = format!(" {} ", text.to_lowercase());
    let words = |ws: &[&str]| ws.iter().any(|w| lower.contains(&format!(" {w} ")));
    let looks = match preferred {
        "es" => has(&|c| "ñáíóú¿¡".contains(c)) || words(&["el", "la", "los", "las", "de", "que", "y", "es", "con", "para"]),
        "de" => has(&|c| "äöüß".contains(c)) || words(&["der", "die", "das", "und", "ist", "nicht", "ich", "mit"]),
        "fr" => has(&|c| "éèêàçùâîô".contains(c)) || words(&["le", "la", "les", "est", "et", "des", "une", "pour"]),
        _ => false,
    };
    looks.then(|| ["es", "de", "fr"].into_iter().find(|l| *l == preferred)).flatten()
}

fn pick_voice(tts: &tts::Tts, code: &str) -> Option<tts::Voice> {
    let voices = tts.voices().ok()?;
    voices.into_iter().find(|v| v.language().primary_language().eq_ignore_ascii_case(code))
}

fn queue() -> &'static Sender<Job> {
    TX.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let _ = std::thread::Builder::new().name("xpu-tts".into()).spawn(move || {
            let mut tts = match tts::Tts::default() {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("text-to-speech unavailable: {e}");
                    for (_, done) in rx {
                        if let Some(d) = done {
                            let _ = d.send(());
                        }
                    }
                    return;
                }
            };
            let default_voice = tts.voice().ok().flatten();
            let mut current: Option<&'static str> = None;
            for (text, done) in rx {
                let want = language_of(&text, &LANGUAGE.lock());
                if want != current {
                    let voice = match want {
                        Some(code) => pick_voice(&tts, code),
                        None => default_voice.clone(),
                    };
                    if let Some(v) = voice {
                        if let Err(e) = tts.set_voice(&v) {
                            log::warn!("voice for {want:?}: {e}");
                        }
                    }
                    current = want;
                }
                let max = Duration::from_millis(1500 + 90 * text.chars().count() as u64).min(Duration::from_secs(60));
                if let Err(e) = tts.speak(text, true) {
                    log::warn!("text-to-speech failed: {e}");
                } else if done.is_some() {
                    let start = Instant::now();
                    std::thread::sleep(Duration::from_millis(150));
                    while start.elapsed() < max && tts.is_speaking().unwrap_or(false) {
                        std::thread::sleep(Duration::from_millis(80));
                    }
                }
                if let Some(d) = done {
                    let _ = d.send(());
                }
            }
        });
        tx
    })
}

pub fn speak(text: String) {
    if !has_voice_for(&text) {
        return;
    }
    let _ = queue().send((text, None));
}

/// Speaks and resolves when finished (or on failure).
pub async fn speak_and_wait(text: String) {
    if !has_voice_for(&text) {
        return;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    if queue().send((text, Some(tx))).is_ok() {
        let _ = rx.await;
    }
}

fn has_voice_for(text: &str) -> bool {
    if language_of(text, &LANGUAGE.lock()) == Some("fa") {
        log::info!("no Persian voice on this Mac: the answer is shown, not spoken");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::language_of;

    #[test]
    fn the_voice_follows_the_language() {
        assert_eq!(language_of("اسپاتیفای رو باز کردم", "en"), Some("fa"));
        assert_eq!(language_of("تم فتح التطبيق", "en"), Some("ar"));
        assert_eq!(language_of("تم فتح التطبيق", "fa"), Some("fa"));
        assert_eq!(language_of("Spotify を開きました", "en"), Some("ja"));
        assert_eq!(language_of("已打开 Spotify", "en"), Some("zh"));
        assert_eq!(language_of("已打开", "ja"), Some("ja"));
        assert_eq!(language_of("Abrí Spotify", "es"), Some("es"));
        assert_eq!(language_of("Opened Spotify", "en"), None);
        assert_eq!(language_of("Opened Spotify.", "es"), None, "English text keeps the default voice");
        assert_eq!(language_of("Die Lautstärke ist 30 %.", "de"), Some("de"));
        assert_eq!(language_of("J’ai ouvert Spotify et lancé la musique.", "fr"), Some("fr"));
    }
}
