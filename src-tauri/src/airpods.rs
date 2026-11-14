use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Press {
    Single,
    Double,
    Triple,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Talk,
    Music(MusicKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicKey {
    PlayPause,
    Next,
    Previous,
}

pub fn route(press: Press, gesture: Press) -> Route {
    if press == gesture {
        return Route::Talk;
    }
    Route::Music(match press {
        Press::Single => MusicKey::PlayPause,
        Press::Double => MusicKey::Next,
        Press::Triple => MusicKey::Previous,
    })
}

#[cfg(target_os = "macos")]
pub use mac::{apply, Event};

#[cfg(not(target_os = "macos"))]
pub fn apply(_app: &tauri::AppHandle, _gesture: Option<Press>) {}

#[cfg(target_os = "macos")]
mod mac {
    use super::{route, MusicKey, Press, Route};
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSString};
    use objc2_media_player::{
        MPMediaItemPropertyArtist, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingPlaybackState, MPRemoteCommand, MPRemoteCommandCenter,
        MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
    };
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::OnceLock;
    use std::time::Duration;
    use tauri::AppHandle;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Event {
        Talk,
        Music(MusicKey),
    }

    static GESTURE: AtomicU8 = AtomicU8::new(0);
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    static EVENTS: OnceLock<Sender<Event>> = OnceLock::new();

    fn encode(g: Option<Press>) -> u8 {
        match g {
            None => 0,
            Some(Press::Single) => 1,
            Some(Press::Double) => 2,
            Some(Press::Triple) => 3,
        }
    }

    fn gesture() -> Option<Press> {
        match GESTURE.load(Ordering::Relaxed) {
            1 => Some(Press::Single),
            2 => Some(Press::Double),
            3 => Some(Press::Triple),
            _ => None,
        }
    }

    fn hint(g: Press) -> &'static str {
        match g {
            Press::Single => "Press your AirPods to talk",
            Press::Double => "Double-press your AirPods to talk",
            Press::Triple => "Triple-press your AirPods to talk",
        }
    }

    pub fn init(events: Sender<Event>) {
        let _ = EVENTS.set(events);
    }

    pub fn apply(app: &AppHandle, g: Option<Press>) {
        GESTURE.store(encode(g), Ordering::Relaxed);
        let _ = app.run_on_main_thread(move || unsafe {
            install();
            match g {
                Some(g) => claim(g),
                None => release(),
            }
        });
        static KEEPER: OnceLock<()> = OnceLock::new();
        let app = app.clone();
        KEEPER.get_or_init(move || {
            let _ = std::thread::Builder::new().name("xpu-airpods".into()).spawn(move || loop {
                std::thread::sleep(Duration::from_secs(4));
                if let Some(g) = gesture() {
                    let _ = app.run_on_main_thread(move || unsafe { claim(g) });
                }
            });
        });
    }

    pub(crate) unsafe fn claim(g: Press) {
        let center = MPNowPlayingInfoCenter::defaultCenter();
        let title = NSString::from_str("Xpu Command");
        let artist = NSString::from_str(hint(g));
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
        let commands: [(Retained<MPRemoteCommand>, Press); 5] = [
            (c.togglePlayPauseCommand(), Press::Single),
            (c.playCommand(), Press::Single),
            (c.pauseCommand(), Press::Single),
            (c.nextTrackCommand(), Press::Double),
            (c.previousTrackCommand(), Press::Triple),
        ];
        for (command, press) in commands {
            command.setEnabled(true);
            let handler = RcBlock::new(move |_e: NonNull<MPRemoteCommandEvent>| -> MPRemoteCommandHandlerStatus {
                let Some(g) = gesture() else { return MPRemoteCommandHandlerStatus::CommandFailed };
                let event = match route(press, g) {
                    Route::Talk => Event::Talk,
                    Route::Music(k) => Event::Music(k),
                };
                if let Some(tx) = EVENTS.get() {
                    let _ = tx.send(event);
                }
                MPRemoteCommandHandlerStatus::Success
            });
            let _target = command.addTargetWithHandler(&handler);
        }
    }

    #[cfg(debug_assertions)]
    pub fn listen_for(g: Press, secs: u64) -> Vec<Event> {
        let (tx, rx) = std::sync::mpsc::channel();
        init(tx);
        GESTURE.store(encode(Some(g)), Ordering::Relaxed);
        unsafe {
            install();
            claim(g);
        }
        core_foundation::runloop::CFRunLoop::run_in_mode(unsafe { core_foundation::runloop::kCFRunLoopDefaultMode }, Duration::from_secs(secs), false);
        rx.try_iter().collect()
    }

    pub fn reclaim_soon(app: &AppHandle) {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            if let Some(g) = gesture() {
                let _ = app.run_on_main_thread(move || unsafe { claim(g) });
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use mac::{init, reclaim_soon};
#[cfg(all(target_os = "macos", debug_assertions))]
pub use mac::listen_for;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chosen_press_talks_and_the_others_control_music() {
        assert_eq!(route(Press::Triple, Press::Triple), Route::Talk);
        assert_eq!(route(Press::Single, Press::Triple), Route::Music(MusicKey::PlayPause));
        assert_eq!(route(Press::Double, Press::Triple), Route::Music(MusicKey::Next));
        assert_eq!(route(Press::Single, Press::Single), Route::Talk);
        assert_eq!(route(Press::Triple, Press::Single), Route::Music(MusicKey::Previous));
        assert_eq!(route(Press::Double, Press::Double), Route::Talk);
    }
}
