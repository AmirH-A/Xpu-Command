use crate::settings::{AgentKind, Delivery, Settings, SttEngine};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub ok: Option<bool>,
    pub title: String,
    pub detail: String,
    pub fix: Option<&'static str>,
}

fn check(id: &'static str, ok: Option<bool>, title: &str, detail: &str, fix: Option<&'static str>) -> Check {
    Check { id, ok, title: title.into(), detail: detail.into(), fix: if ok == Some(false) { fix } else { None } }
}

pub fn run(s: &Settings) -> Vec<Check> {
    let p = crate::permissions::status();
    let mut out = vec![
        check("location", Some(!p.needs_move), "Installed in Applications", if p.needs_move { "Running from the download, so macOS can't keep permissions." } else { "Permissions stay after restarts." }, Some("move")),
        check(
            "accessibility",
            Some(!p.accessibility_required || p.accessibility),
            "Accessibility",
            if p.accessibility { "The talk button and typing work." } else { "Needed for the talk button and typing." },
            Some("accessibility"),
        ),
        check(
            "microphone",
            Some(p.microphone == crate::permissions::Mic::Allowed),
            "Microphone",
            match p.microphone {
                crate::permissions::Mic::Allowed => "Allowed.",
                crate::permissions::Mic::NotAsked => "Not allowed yet.",
                crate::permissions::Mic::Denied => "Turned off in System Settings.",
            },
            Some("microphone"),
        ),
    ];
    out.push(match s.stt {
        SttEngine::Local => {
            let ready = crate::stt::model::find(&s.local_model).is_some_and(crate::stt::model::is_downloaded);
            check("speech", Some(ready), "Speech model", if ready { "Downloaded and checked." } else { "Not downloaded yet." }, Some("model"))
        }
        SttEngine::Cloud => {
            let key = crate::secrets::get(crate::secrets::OPENAI_KEY).ok().flatten().is_some();
            check("speech", Some(key), "OpenAI key for cloud speech", if key { "Saved in your keychain." } else { "No key saved." }, Some("model"))
        }
    });
    if s.stt == SttEngine::Local && s.local_model.ends_with(".en") && !matches!(s.language.as_str(), "en" | "auto") {
        out.push(check("language", Some(false), "Speech model language", "This model only understands English, but your language isn't English.", Some("model")));
    }
    let uses_claude = s.delivery == Delivery::ControlMac || (s.delivery == Delivery::Agent && s.agent == AgentKind::Claude);
    if uses_claude {
        let found = crate::agents::find_binary(AgentKind::Claude);
        out.push(check("claude", Some(found.is_some()), "Claude Code installed", if found.is_some() { "Found." } else { "Needed for Agent and Control Mac modes." }, Some("claude-install")));
        if found.is_some() {
            let dir = s.claude_profile.as_deref().and_then(|p| crate::settings::claude_profile_dir(p).ok());
            let signed = claude_signed_in(dir.as_deref());
            out.push(check(
                "claude-login",
                signed,
                "Claude Code signed in",
                match signed {
                    Some(true) => "Signed in.",
                    Some(false) => "Open Terminal, run claude and use /login.",
                    None => "Couldn't tell; it may use an API key.",
                },
                Some("claude-login"),
            ));
        }
    }
    if s.knock {
        let err = crate::knock_error();
        out.push(check("knock", Some(err.is_none()), "Double knock sensor", err.as_deref().unwrap_or("Listening for knocks."), None));
    }
    out
}

fn claude_signed_in(profile: Option<&Path>) -> Option<bool> {
    let file = match profile {
        Some(dir) => dir.join(".claude.json"),
        None => dirs::home_dir()?.join(".claude.json"),
    };
    let text = std::fs::read_to_string(file).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(v.get("oauthAccount").is_some_and(|a| !a.is_null()))
}

pub fn recovery(error: &str) -> String {
    let e = error.to_lowercase();
    let fix = if e.contains("cli not found") || e.contains("install it first") {
        "Claude Code isn't installed. Install it and sign in, then try again. Settings › Health shows how."
    } else if e.contains("no microphone found") {
        "No microphone found. Connect one, or pick another in Settings › Speech."
    } else if e.contains("not downloaded yet") {
        "The speech model isn't downloaded yet. Download it in Settings › Get started."
    } else if e.contains("timed out") || e.contains("took too long") {
        "That took too long and was stopped. Try a shorter request."
    } else if e.contains("error sending request") || e.contains("dns") || e.contains("connection") || e.contains("network") {
        "Can't reach the internet. Check your connection, or switch Speech to On this Mac."
    } else if e.contains("no api key") {
        "Cloud speech needs an OpenAI key. Add one in Settings › Speech, or switch to On this Mac."
    } else {
        return error.to_string();
    };
    fix.to_string()
}

pub fn logs_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join("Library/Logs/Xpu Command"))
}

pub mod crash {
    use super::logs_dir;
    use serde::Serialize;
    use std::path::PathBuf;

    #[derive(Debug, Clone, Serialize)]
    pub struct Crash {
        pub at: String,
        pub summary: String,
    }

    pub fn install() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into());
            let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
            let report = format!(
                "Xpu Command {}\nmacOS {}\nthread: {thread}\nat: {place}\n\n{message}\n\n{}",
                env!("CARGO_PKG_VERSION"),
                os_version(),
                std::backtrace::Backtrace::force_capture()
            );
            if let Some(dir) = logs_dir() {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join(format!("crash-{}.txt", chrono::Local::now().format("%Y%m%d-%H%M%S"))), report);
            }
            log::error!("crashed at {place}: {message}");
            previous(info);
        }));
    }

    fn os_version() -> String {
        std::process::Command::new("/usr/bin/sw_vers").arg("-productVersion").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
    }

    fn unseen() -> Vec<PathBuf> {
        let Some(dir) = logs_dir() else { return Vec::new() };
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt") && !n.ends_with(".seen.txt"))).collect();
        v.sort();
        v
    }

    pub fn pending() -> Option<Crash> {
        let last = unseen().pop()?;
        let text = std::fs::read_to_string(&last).ok()?;
        let summary = text.split("\n\n").nth(1).unwrap_or("").lines().next().unwrap_or("").chars().take(160).collect();
        let at = last.file_stem()?.to_str()?.trim_start_matches("crash-").to_string();
        Some(Crash { at, summary })
    }

    pub fn dismiss() {
        for p in unseen() {
            let _ = std::fs::rename(&p, p.with_extension("seen.txt"));
        }
    }

    pub fn report_url() -> Option<String> {
        use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
        let last = unseen().pop()?;
        let text = std::fs::read_to_string(last).ok()?;
        let head: String = text.chars().take(3500).collect();
        let title = format!("Crash: {}", pending().map(|c| c.summary).unwrap_or_default());
        let body = format!("What were you doing when it closed?\n\n\n<details><summary>Crash report</summary>\n\n```\n{head}\n```\n</details>\n");
        Some(format!(
            "https://github.com/XPU-LAB/xpu-command/issues/new?title={}&body={}&labels=crash",
            utf8_percent_encode(&title, NON_ALPHANUMERIC),
            utf8_percent_encode(&body, NON_ALPHANUMERIC)
        ))
    }
}

pub fn diagnostics_zip(s: &Settings) -> crate::error::Result<PathBuf> {
    use crate::error::msg;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let stage = std::env::temp_dir().join(format!("xpu-diagnostics-{}", uuid::Uuid::new_v4().simple()));
    let root = stage.join(format!("Xpu-Command-diagnostics-{stamp}"));
    std::fs::create_dir_all(&root)?;
    if let Some(logs) = logs_dir() {
        if let Ok(rd) = std::fs::read_dir(logs) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() {
                    let _ = std::fs::copy(&p, root.join(p.file_name().unwrap_or_default()));
                }
            }
        }
    }
    let config = crate::settings::config_dir()?;
    for name in ["settings.json", "activity.jsonl"] {
        let _ = std::fs::copy(config.join(name), root.join(name));
    }
    let report = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "health": run(s),
        "bundle": crate::permissions::running_bundle(),
    });
    std::fs::write(root.join("health.json"), serde_json::to_vec_pretty(&report)?)?;
    let desktop = dirs::desktop_dir().or_else(dirs::home_dir).ok_or_else(|| msg("no Desktop folder"))?;
    let zip = desktop.join(format!("Xpu-Command-diagnostics-{stamp}.zip"));
    #[cfg(target_os = "macos")]
    crate::mac::exec::run_program("/usr/bin/ditto", &["-c".as_ref(), "-k".as_ref(), "--keepParent".as_ref(), root.as_os_str(), zip.as_os_str()], std::time::Duration::from_secs(60)).map_err(msg)?;
    let _ = std::fs::remove_dir_all(&stage);
    Ok(zip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_messages_say_how_to_fix() {
        assert!(recovery("Claude Code CLI not found; install it first").contains("Settings › Health"));
        assert!(recovery("no microphone found").contains("pick another"));
        assert!(recovery("speech model base is not downloaded yet").contains("Get started"));
        assert!(recovery("timed out").contains("shorter request"));
        assert!(recovery("error sending request for url").contains("internet"));
        assert_eq!(recovery("something specific happened"), "something specific happened");
    }

    #[test]
    fn signed_in_is_read_from_the_profile() {
        let dir = std::env::temp_dir().join(format!("xpu-health-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(claude_signed_in(Some(&dir)), None, "no file: unknown");
        std::fs::write(dir.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"a@b.c"}}"#).unwrap();
        assert_eq!(claude_signed_in(Some(&dir)), Some(true));
        std::fs::write(dir.join(".claude.json"), r#"{"numStartups":3}"#).unwrap();
        assert_eq!(claude_signed_in(Some(&dir)), Some(false));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
