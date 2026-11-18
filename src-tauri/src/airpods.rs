#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicKey {
    Next,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Talk,
    Music(MusicKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    PlayPause,
    Next,
    Previous,
}

pub fn route(button: Button) -> Event {
    match button {
        Button::PlayPause => Event::Talk,
        Button::Next => Event::Music(MusicKey::Next),
        Button::Previous => Event::Music(MusicKey::Previous),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn apply(_app: &tauri::AppHandle, _on: bool) {}

/// Linux: the headset's play/pause press arrives as an MPRIS command (see `linux::mpris`).
#[cfg(target_os = "linux")]
mod linux {
    pub fn init(events: std::sync::mpsc::Sender<super::Event>) {
        crate::linux::mpris::init(events);
    }

    pub fn apply(_app: &tauri::AppHandle, on: bool) {
        std::thread::spawn(move || crate::linux::mpris::apply(on));
    }

    pub fn reclaim_soon(_app: &tauri::AppHandle) {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(600));
            crate::linux::mpris::reclaim();
        });
    }
}

#[cfg(target_os = "linux")]
pub use linux::{apply, init, reclaim_soon};

#[cfg(target_os = "macos")]
mod mac {
    use super::{route, Button, Event};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSString};
    use objc2_media_player::{
        MPMediaItemPropertyArtist, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingPlaybackState, MPRemoteCommand, MPRemoteCommandCenter,
        MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
    };
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::OnceLock;
    use std::time::Duration;
    use tauri::AppHandle;

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    static EVENTS: OnceLock<Sender<Event>> = OnceLock::new();

    pub fn init(events: Sender<Event>) {
        let _ = EVENTS.set(events);
    }

    pub fn apply(app: &AppHandle, on: bool) {
        ENABLED.store(on, Ordering::Relaxed);
        let _ = app.run_on_main_thread(move || unsafe {
            install();
            if on {
                claim();
            } else {
                release();
            }
        });
        static KEEPER: OnceLock<()> = OnceLock::new();
        let app = app.clone();
        KEEPER.get_or_init(move || {
            let _ = std::thread::Builder::new().name("xpu-airpods".into()).spawn(move || loop {
                std::thread::sleep(Duration::from_secs(4));
                if ENABLED.load(Ordering::Relaxed) {
                    let _ = app.run_on_main_thread(|| unsafe { claim() });
                }
            });
        });
    }

    unsafe fn claim() {
        let center = MPNowPlayingInfoCenter::defaultCenter();
        let title = NSString::from_str("Xpu Command");
        let artist = NSString::from_str("Press your AirPods to talk");
        let info: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_retained_objects(&[MPMediaItemPropertyTitle, MPMediaItemPropertyArtist], &[Retained::into_super(Retained::into_super(title)), Retained::into_super(Retained::into_super(artist))]);
        center.setNowPlayingInfo(Some(&info));
        center.setPlaybackState(MPNowPlayingPlaybackState::Playing);
    }

    unsafe fn release() {
        let center = MPNowPlayingInfoCenter::defaultCenter();
        center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
        center.setNowPlayingInfo(None);
    }

    unsafe fn install() {
        if INSTALLED.swap(true, Ordering::SeqCst) {
            return;
        }
        let c = MPRemoteCommandCenter::sharedCommandCenter();
        let commands: [(Retained<MPRemoteCommand>, Button); 5] = [
            (c.togglePlayPauseCommand(), Button::PlayPause),
            (c.playCommand(), Button::PlayPause),
            (c.pauseCommand(), Button::PlayPause),
            (c.nextTrackCommand(), Button::Next),
            (c.previousTrackCommand(), Button::Previous),
        ];
        for (command, button) in commands {
            command.setEnabled(true);
            let handler = RcBlock::new(move |_e: NonNull<MPRemoteCommandEvent>| -> MPRemoteCommandHandlerStatus {
                if !ENABLED.load(Ordering::Relaxed) {
                    return MPRemoteCommandHandlerStatus::CommandFailed;
                }
                if let Some(tx) = EVENTS.get() {
                    let _ = tx.send(route(button));
                }
                MPRemoteCommandHandlerStatus::Success
            });
            let _target = command.addTargetWithHandler(&handler);
        }
    }

    #[cfg(debug_assertions)]
    pub fn listen_for(secs: u64) -> Vec<Event> {
        let (tx, rx) = std::sync::mpsc::channel();
        init(tx);
        ENABLED.store(true, Ordering::Relaxed);
        unsafe {
            install();
            claim();
        }
        core_foundation::runloop::CFRunLoop::run_in_mode(unsafe { core_foundation::runloop::kCFRunLoopDefaultMode }, Duration::from_secs(secs), false);
        rx.try_iter().collect()
    }

    pub fn reclaim_soon(app: &AppHandle) {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            if ENABLED.load(Ordering::Relaxed) {
                let _ = app.run_on_main_thread(|| unsafe { claim() });
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use mac::{apply, init, reclaim_soon};
#[cfg(all(target_os = "macos", debug_assertions))]
pub use mac::listen_for;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_talks_and_skips_still_control_music() {
        assert_eq!(route(Button::PlayPause), Event::Talk);
        assert_eq!(route(Button::Next), Event::Music(MusicKey::Next));
        assert_eq!(route(Button::Previous), Event::Music(MusicKey::Previous));
    }
}
