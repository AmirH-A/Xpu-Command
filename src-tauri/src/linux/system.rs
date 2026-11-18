//! System settings the assistant may change: output volume (PipeWire's wpctl, PulseAudio's
//! pactl, or ALSA's amixer, whichever is there) and dark mode (GNOME-family and KDE desktops).
use std::time::Duration;

const T: Duration = Duration::from_secs(5);

/// "Volume: 0.45 [MUTED]" (wpctl) → (45, true).
pub fn parse_wpctl(s: &str) -> Option<(u8, bool)> {
    let v: f64 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(((v * 100.0).round().clamp(0.0, 100.0) as u8, s.contains("[MUTED]")))
}

/// The first "NN%" in pactl or amixer output.
pub fn first_percent(s: &str) -> Option<u8> {
    s.split(|c: char| c.is_whitespace() || c == '[' || c == ']' || c == '/')
        .find_map(|w| w.strip_suffix('%').and_then(|n| n.parse::<u32>().ok()))
        .map(|n| n.min(100) as u8)
}

/// (level 0-100, muted).
pub fn volume() -> Result<(u8, bool), String> {
    if super::program("wpctl").is_some() {
        if let Some(v) = super::run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"], T).ok().as_deref().and_then(parse_wpctl) {
            return Ok(v);
        }
    }
    if super::program("pactl").is_some() {
        let level = super::run("pactl", &["get-sink-volume", "@DEFAULT_SINK@"], T).ok().as_deref().and_then(first_percent);
        let muted = super::run("pactl", &["get-sink-mute", "@DEFAULT_SINK@"], T).is_ok_and(|s| s.contains("yes"));
        if let Some(l) = level {
            return Ok((l, muted));
        }
    }
    if super::program("amixer").is_some() {
        let s = super::run("amixer", &["-D", "default", "sget", "Master"], T)?;
        let l = first_percent(&s).ok_or("couldn't read the volume")?;
        return Ok((l, s.contains("[off]")));
    }
    Err("no volume control found (wpctl, pactl or amixer)".into())
}

/// Sets the level and/or mute; returns the new level.
pub fn set_volume(level: Option<u8>, mute: Option<bool>) -> Result<u8, String> {
    let lvl = level.map(|l| l.min(100));
    if super::program("wpctl").is_some() {
        if let Some(l) = lvl {
            super::run("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", f64::from(l) / 100.0)], T)?;
        }
        let m = match (mute, lvl) {
            (Some(m), _) => Some(m),
            (None, Some(_)) => Some(false),
            _ => None,
        };
        if let Some(m) = m {
            super::run("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", if m { "1" } else { "0" }], T)?;
        }
    } else if super::program("pactl").is_some() {
        if let Some(l) = lvl {
            super::run("pactl", &["set-sink-volume", "@DEFAULT_SINK@", &format!("{l}%")], T)?;
        }
        if let Some(m) = mute.or(lvl.map(|_| false)) {
            super::run("pactl", &["set-sink-mute", "@DEFAULT_SINK@", if m { "1" } else { "0" }], T)?;
        }
    } else if super::program("amixer").is_some() {
        if let Some(l) = lvl {
            super::run("amixer", &["-D", "default", "-q", "sset", "Master", &format!("{l}%")], T)?;
        }
        if let Some(m) = mute.or(lvl.map(|_| false)) {
            super::run("amixer", &["-D", "default", "-q", "sset", "Master", if m { "mute" } else { "unmute" }], T)?;
        }
    } else {
        return Err("no volume control found (wpctl, pactl or amixer)".into());
    }
    Ok(volume().map(|v| v.0).unwrap_or(lvl.unwrap_or(0)))
}

fn gnome_like() -> bool {
    ["gnome", "unity", "budgie", "pantheon", "cinnamon", "x-cinnamon", "cosmic"].iter().any(|d| super::is_desktop(d))
}

pub fn dark_mode() -> Result<bool, String> {
    if let Some(d) = super::portal::prefers_dark() {
        return Ok(d);
    }
    if super::program("gsettings").is_some() {
        if let Ok(s) = super::run("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"], T) {
            return Ok(s.contains("dark"));
        }
    }
    Err("couldn't read the light/dark setting".into())
}

pub fn set_dark_mode(on: bool) -> Result<(), String> {
    if super::is_desktop("kde") {
        let scheme = if on { "BreezeDark" } else { "BreezeLight" };
        if super::program("plasma-apply-colorscheme").is_some() {
            return super::run("plasma-apply-colorscheme", &[scheme], T).map(drop);
        }
        return Err("plasma-apply-colorscheme isn't installed".into());
    }
    if gnome_like() || super::program("gsettings").is_some() {
        super::run("gsettings", &["set", "org.gnome.desktop.interface", "color-scheme", if on { "prefer-dark" } else { "default" }], T)?;
        // Older GTK 3 apps follow the theme name, not the color scheme: switch Adwaita too.
        if let Ok(theme) = super::run("gsettings", &["get", "org.gnome.desktop.interface", "gtk-theme"], T) {
            let t = theme.trim_matches('\'');
            if t == "Adwaita" || t == "Adwaita-dark" {
                let _ = super::run("gsettings", &["set", "org.gnome.desktop.interface", "gtk-theme", if on { "Adwaita-dark" } else { "Adwaita" }], T);
            }
        }
        if super::is_desktop("x-cinnamon") || super::is_desktop("cinnamon") {
            let _ = super::run("gsettings", &["set", "org.x.apps.portal", "color-scheme", if on { "prefer-dark" } else { "prefer-light" }], T);
        }
        return Ok(());
    }
    Err("dark mode can't be switched on this desktop".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_outputs_are_parsed() {
        assert_eq!(parse_wpctl("Volume: 0.45"), Some((45, false)));
        assert_eq!(parse_wpctl("Volume: 0.30 [MUTED]"), Some((30, true)));
        assert_eq!(parse_wpctl("Volume: 1.20"), Some((100, false)), "boosted volume is shown as 100");
        assert_eq!(parse_wpctl("nothing"), None);
        assert_eq!(first_percent("Volume: front-left: 29491 /  45% / -20.81 dB,   front-right: 29491 /  45% / -20.81 dB"), Some(45));
        assert_eq!(first_percent("  Front Left: Playback 39321 [60%] [on]"), Some(60));
        assert_eq!(first_percent("no level"), None);
    }
}
