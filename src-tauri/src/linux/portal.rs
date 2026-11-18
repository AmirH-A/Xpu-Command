//! XDG desktop portals: the sanctioned way for apps to ask the desktop for things Wayland doesn't
//! let them do themselves. Used for the keyboard shortcut (GlobalShortcuts: GNOME 48+, KDE,
//! Hyprland), screenshots (Screenshot) and the light/dark setting (Settings). The desktop shows
//! its own consent dialog where it wants one.
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";

fn token() -> String {
    format!("xpu{}", uuid::Uuid::new_v4().simple())
}

/// Where the portal will answer a request we make with `token` (the documented request path).
fn request_path(c: &Connection, token: &str) -> Result<String, String> {
    let me = c.unique_name().ok_or("no bus name")?.as_str().trim_start_matches(':').replace('.', "_");
    Ok(format!("{PATH}/request/{me}/{token}"))
}

/// Makes a portal request and waits for its Response: Ok(results) when the user (or desktop)
/// agreed, Err when cancelled, refused or not answered in `wait`.
fn request<B>(c: &Connection, iface: &str, method: &str, token: &str, body: &B, wait: Duration) -> Result<HashMap<String, OwnedValue>, String>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    let path = request_path(c, token)?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .and_then(|b| b.member("Response"))
        .and_then(|b| b.path(path.as_str()))
        .map_err(|e| e.to_string())?
        .build()
        .to_owned();
    // Subscribe before asking, so a quick answer isn't missed.
    let it = zbus::blocking::MessageIterator::for_match_rule(rule, c, Some(8)).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Some(m) = it.flatten().next() {
            let _ = tx.send(m.body().deserialize::<(u32, HashMap<String, OwnedValue>)>().map_err(|e| e.to_string()));
        }
    });
    c.call_method(Some(DEST), PATH, Some(iface), method, body).map_err(|e| {
        let s = e.to_string();
        if s.contains("UnknownInterface") || s.contains("UnknownMethod") || s.contains("ServiceUnknown") {
            format!("this desktop doesn't offer {}", iface.trim_start_matches("org.freedesktop.portal."))
        } else {
            s
        }
    })?;
    match rx.recv_timeout(wait) {
        Ok(Ok((0, results))) => Ok(results),
        Ok(Ok((1, _))) => Err("it was cancelled".into()),
        Ok(Ok(_)) => Err("the desktop refused".into()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the desktop didn't answer".into()),
    }
}

fn options(token: &str) -> HashMap<&'static str, Value<'static>> {
    HashMap::from([("handle_token", Value::from(token.to_string()))])
}

/// A screenshot of the whole screen, as a PNG file the portal wrote (the caller deletes it).
pub fn screenshot() -> Result<std::path::PathBuf, String> {
    let c = Connection::session().map_err(|e| e.to_string())?;
    let t = token();
    let mut o = options(&t);
    o.insert("interactive", Value::from(false));
    let r = request(&c, "org.freedesktop.portal.Screenshot", "Screenshot", &t, &("", o), Duration::from_secs(60))?;
    let uri = r.get("uri").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).ok_or("the screenshot has no file")?;
    let path = uri.strip_prefix("file://").ok_or("the screenshot isn't a local file")?;
    let path = percent_encoding::percent_decode_str(path).decode_utf8_lossy().to_string();
    Ok(std::path::PathBuf::from(path))
}

/// The desktop's light/dark preference: Some(true) dark, Some(false) light, None unknown.
pub fn prefers_dark() -> Option<bool> {
    let c = Connection::session().ok()?;
    let r = c.call_method(Some(DEST), PATH, Some("org.freedesktop.portal.Settings"), "ReadOne", &("org.freedesktop.appearance", "color-scheme")).ok();
    let v: OwnedValue = match r {
        Some(r) => r.body().deserialize().ok()?,
        None => {
            // Older portals: Read wraps the value in a second variant.
            let r = c.call_method(Some(DEST), PATH, Some("org.freedesktop.portal.Settings"), "Read", &("org.freedesktop.appearance", "color-scheme")).ok()?;
            let outer: OwnedValue = r.body().deserialize().ok()?;
            match &*outer {
                Value::Value(inner) => OwnedValue::try_from(&**inner).ok()?,
                _ => outer,
            }
        }
    };
    match u32::try_from(v).ok()? {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// A global shortcut bound through the portal. Its events arrive while this lives; dropping it
/// closes the session (the desktop forgets the binding).
pub struct Shortcut {
    conn: Connection,
    session: String,
}

impl Drop for Shortcut {
    fn drop(&mut self) {
        let _ = self.conn.call_method(Some(DEST), self.session.as_str(), Some("org.freedesktop.portal.Session"), "Close", &());
    }
}

/// The trigger the way the shortcuts spec writes it, e.g. "CTRL+ALT+space".
pub fn trigger(h: crate::settings::Hotkey) -> &'static str {
    match h {
        crate::settings::Hotkey::CtrlAltSpace => "CTRL+ALT+space",
        crate::settings::Hotkey::AltShiftSpace => "ALT+SHIFT+space",
        crate::settings::Hotkey::CtrlShiftSpace => "CTRL+SHIFT+space",
    }
}

/// Binds the talk shortcut. `on` gets true when it is pressed and false when released. The
/// desktop may ask the user to confirm (or change) the keys the first time.
pub fn bind_shortcut(h: crate::settings::Hotkey, on: impl Fn(bool) + Send + Sync + 'static) -> Result<Shortcut, String> {
    let conn = Connection::session().map_err(|e| e.to_string())?;
    let t = token();
    let mut o = options(&t);
    o.insert("session_handle_token", Value::from(token()));
    let r = request(&conn, "org.freedesktop.portal.GlobalShortcuts", "CreateSession", &t, &(o,), Duration::from_secs(30))?;
    let session = r
        .get("session_handle")
        .and_then(|v| match &**v {
            Value::Str(s) => Some(s.to_string()),
            Value::ObjectPath(p) => Some(p.to_string()),
            _ => None,
        })
        .ok_or("the desktop made no shortcut session")?;
    let session_path = ObjectPath::try_from(session.as_str()).map_err(|e| e.to_string())?;
    let shortcut: Vec<(&str, HashMap<&str, Value>)> =
        vec![("talk", HashMap::from([("description", Value::from("Hold to talk to Xpu Command")), ("preferred_trigger", Value::from(trigger(h)))]))];
    // Listen before binding, so no press is missed.
    let mut rules = Vec::new();
    for member in ["Activated", "Deactivated"] {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.GlobalShortcuts")
            .and_then(|b| b.member(member))
            .map_err(|e| e.to_string())?
            .build()
            .to_owned();
        rules.push(zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(64)).map_err(|e| e.to_string())?);
    }
    let bt = token();
    request(&conn, "org.freedesktop.portal.GlobalShortcuts", "BindShortcuts", &bt, &(session_path, shortcut, "", options(&bt)), Duration::from_secs(120))?;
    let on = std::sync::Arc::new(on);
    for (i, it) in rules.into_iter().enumerate() {
        let (on, mine) = (on.clone(), session.clone());
        let pressed = i == 0;
        std::thread::Builder::new()
            .name("xpu-portal-shortcut".into())
            .spawn(move || {
                for m in it.flatten() {
                    let Ok((s, id, _, _)) = m.body().deserialize::<(zbus::zvariant::OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>() else { continue };
                    if s.as_str() == mine && id == "talk" {
                        on(pressed);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(Shortcut { conn, session })
}

#[cfg(test)]
mod tests {
    #[test]
    fn triggers_follow_the_shortcuts_spec() {
        use crate::settings::Hotkey::*;
        assert_eq!(super::trigger(CtrlAltSpace), "CTRL+ALT+space");
        assert_eq!(super::trigger(AltShiftSpace), "ALT+SHIFT+space");
        assert_eq!(super::trigger(CtrlShiftSpace), "CTRL+SHIFT+space");
    }
}
