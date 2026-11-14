//! Spoken summaries and questions in the headset. The synthesizer lives on one thread; requests
//! queue to it. `speak_and_wait` tells the caller when speaking has finished (so listening for a
//! reply doesn't hear our own voice).
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

type Job = (String, Option<tokio::sync::oneshot::Sender<()>>);
static TX: OnceLock<Sender<Job>> = OnceLock::new();

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
            for (text, done) in rx {
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
    let _ = queue().send((text, None));
}

/// Speaks and resolves when finished (or on failure).
pub async fn speak_and_wait(text: String) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if queue().send((text, Some(tx))).is_ok() {
        let _ = rx.await;
    }
}
