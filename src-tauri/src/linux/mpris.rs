//! Media players over MPRIS (D-Bus): Spotify, browsers, Rhythmbox, Elisa, VLC… all speak it.
//!
//! * As a client: play, pause, next, previous, "now playing", open a Spotify link in Spotify.
//! * As a player ("headset button"): Xpu Command shows up as a player that is playing, so the
//!   desktop sends the headset's play/pause press to it, the Linux counterpart of AirPods on the
//!   Mac. Next and previous still reach the music.
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const OURS: &str = "org.mpris.MediaPlayer2.xpucommand";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

fn session() -> Result<Connection, String> {
    static S: Mutex<Option<Connection>> = Mutex::new(None);
    let mut s = S.lock();
    if let Some(c) = s.as_ref() {
        return Ok(c.clone());
    }
    let c = zbus::blocking::connection::Builder::session()
        .map_err(|e| e.to_string())?
        .method_timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| format!("no session bus: {e}"))?;
    *s = Some(c.clone());
    Ok(c)
}

/// Bus names of the running players (not ours).
pub fn players() -> Vec<String> {
    let Ok(c) = session() else { return Vec::new() };
    let Ok(r) = c.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "ListNames", &()) else { return Vec::new() };
    let names: Vec<String> = r.body().deserialize().unwrap_or_default();
    names.into_iter().filter(|n| n.starts_with(PREFIX) && n != OURS && !n.starts_with(&format!("{OURS}."))).collect()
}

fn prop(name: &str, p: &str) -> Option<OwnedValue> {
    let c = session().ok()?;
    let r = c.call_method(Some(name), PATH, Some("org.freedesktop.DBus.Properties"), "Get", &(PLAYER, p)).ok()?;
    r.body().deserialize::<OwnedValue>().ok()
}

pub fn status(name: &str) -> Option<String> {
    prop(name, "PlaybackStatus").and_then(|v| String::try_from(v).ok())
}

/// The player's display name ("Spotify", "Firefox").
pub fn identity(name: &str) -> String {
    let c = session().ok();
    let v = c.and_then(|c| c.call_method(Some(name), PATH, Some("org.freedesktop.DBus.Properties"), "Get", &("org.mpris.MediaPlayer2", "Identity")).ok());
    v.and_then(|r| r.body().deserialize::<OwnedValue>().ok()).and_then(|v| String::try_from(v).ok()).unwrap_or_else(|| name.trim_start_matches(PREFIX).split('.').next().unwrap_or("the player").to_string())
}

/// The player to control: the one asked for (by part of its name, e.g. "spotify"), else the one
/// playing, else the one paused, else any.
pub fn pick(prefer: Option<&str>) -> Option<String> {
    let all = players();
    if let Some(p) = prefer {
        return all.into_iter().find(|n| n.to_lowercase().contains(p));
    }
    let with: Vec<(String, String)> = all.iter().map(|n| (n.clone(), status(n).unwrap_or_default())).collect();
    with.iter().find(|(_, s)| s == "Playing").or_else(|| with.iter().find(|(_, s)| s == "Paused")).or(with.first()).map(|(n, _)| n.clone())
}

/// Calls Play, Pause, PlayPause, Next or Previous.
pub fn command(name: &str, method: &str) -> Result<(), String> {
    let c = session()?;
    c.call_method(Some(name), PATH, Some(PLAYER), method, &()).map(drop).map_err(|e| format!("{} didn't respond: {e}", identity(name)))
}

pub fn open_uri(name: &str, uri: &str) -> Result<(), String> {
    let c = session()?;
    c.call_method(Some(name), PATH, Some(PLAYER), "OpenUri", &(uri,)).map(drop).map_err(|e| e.to_string())
}

/// "Title by Artist", or "" when the player doesn't say.
pub fn now_playing(name: &str) -> String {
    let Some(v) = prop(name, "Metadata") else { return String::new() };
    let Ok(m) = HashMap::<String, OwnedValue>::try_from(v) else { return String::new() };
    let title = m.get("xesam:title").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
    let artist = m.get("xesam:artist").and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok()).map(|a| a.join(", ")).unwrap_or_default();
    match (title.is_empty(), artist.is_empty()) {
        (true, _) => String::new(),
        (false, true) => title,
        (false, false) => format!("{title} by {artist}"),
    }
}

// --- Xpu Command as a player, for the headset button ---

static ENABLED: AtomicBool = AtomicBool::new(false);
static EVENTS: Mutex<Option<Sender<crate::airpods::Event>>> = Mutex::new(None);
static SERVER: Mutex<Option<Connection>> = Mutex::new(None);

fn send(b: crate::airpods::Button) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if let Some(tx) = EVENTS.lock().as_ref() {
        let _ = tx.send(crate::airpods::route(b));
    }
}

struct Root;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}
    fn quit(&self) {}
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> String {
        "Xpu Command".into()
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player;

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play_pause(&self) {
        send(crate::airpods::Button::PlayPause);
    }
    fn play(&self) {
        send(crate::airpods::Button::PlayPause);
    }
    fn pause(&self) {
        send(crate::airpods::Button::PlayPause);
    }
    fn stop(&self) {}
    fn next(&self) {
        send(crate::airpods::Button::Next);
    }
    fn previous(&self) {
        send(crate::airpods::Button::Previous);
    }
    #[zbus(property)]
    fn playback_status(&self) -> String {
        if ENABLED.load(Ordering::Relaxed) { "Playing".into() } else { "Stopped".into() }
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        let put = |m: &mut HashMap<String, OwnedValue>, k: &str, v: Value| {
            if let Ok(v) = OwnedValue::try_from(v) {
                m.insert(k.to_string(), v);
            }
        };
        put(&mut m, "mpris:trackid", Value::from(zbus::zvariant::ObjectPath::from_static_str_unchecked("/app/xpucommand/talk")));
        put(&mut m, "xesam:title", Value::from("Xpu Command"));
        put(&mut m, "xesam:artist", Value::from(vec!["Press your headset to talk"]));
        m
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn position(&self) -> i64 {
        0
    }
}

pub fn init(events: Sender<crate::airpods::Event>) {
    *EVENTS.lock() = Some(events);
}

/// Tells the desktop we (re)started playing, so the headset's press comes to us.
fn announce(c: &Connection) {
    let changed: HashMap<&str, Value> = HashMap::from([("PlaybackStatus", Value::from(if ENABLED.load(Ordering::Relaxed) { "Playing" } else { "Stopped" }))]);
    let _ = c.emit_signal(None::<&str>, PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged", &(PLAYER, changed, Vec::<String>::new()));
}

/// Turns the headset button on or off. On: Xpu Command is a playing player, reclaimed now and
/// then (a music app that starts playing takes the press until then); off: it leaves the bus.
pub fn apply(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    let mut slot = SERVER.lock();
    if !on {
        if let Some(c) = slot.take() {
            announce(&c);
        }
        return;
    }
    if slot.is_none() {
        let built = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name(OURS))
            .and_then(|b| b.serve_at(PATH, Root))
            .and_then(|b| b.serve_at(PATH, Player))
            .and_then(|b| b.build());
        match built {
            Ok(c) => *slot = Some(c),
            Err(e) => {
                log::warn!("headset button: couldn't register as a media player: {e}");
                return;
            }
        }
    }
    if let Some(c) = slot.as_ref() {
        announce(c);
    }
    static KEEPER: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    KEEPER.get_or_init(|| {
        let _ = std::thread::Builder::new().name("xpu-headset-claim".into()).spawn(|| loop {
            std::thread::sleep(Duration::from_secs(4));
            reclaim();
        });
    });
}

/// Claims the headset press again (after we forwarded next/previous to the music).
pub fn reclaim() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if let Some(c) = SERVER.lock().as_ref() {
        announce(c);
    }
}

/// Next or previous on the music that is playing (not us).
pub fn music_key(key: crate::airpods::MusicKey) -> Result<String, String> {
    let p = pick(None).ok_or("no music app is running")?;
    command(&p, match key {
        crate::airpods::MusicKey::Next => "Next",
        crate::airpods::MusicKey::Previous => "Previous",
    })?;
    std::thread::sleep(Duration::from_millis(300));
    Ok(now_playing(&p))
}
