//! Short sound cues, so the user knows without looking: listening started, sent, cancelled.
//! Built-in macOS sounds, played quietly by /usr/bin/afplay (no shell, fixed paths); on Linux,
//! short tones made in memory.
#[derive(Debug, Clone, Copy)]
pub enum Cue {
    Start,
    Send,
    Cancel,
}

pub fn play(cue: Cue) {
    #[cfg(target_os = "macos")]
    {
        let file = match cue {
            Cue::Start => "/System/Library/Sounds/Tink.aiff",
            Cue::Send => "/System/Library/Sounds/Pop.aiff",
            Cue::Cancel => "/System/Library/Sounds/Bottle.aiff",
        };
        let _ = std::thread::Builder::new().name("xpu-cue".into()).spawn(move || {
            let child = {
                let _g = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
                std::process::Command::new("/usr/bin/afplay")
                    .args(["-v", "0.35", file])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
            };
            if let Ok(mut c) = child {
                let _ = c.wait();
            }
        });
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::thread::Builder::new().name("xpu-cue".into()).spawn(move || {
            if let Err(e) = crate::linux::sound::play(cue) {
                log::debug!("cue: {e}");
            }
        });
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = cue;
}
