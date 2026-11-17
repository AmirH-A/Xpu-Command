//! User settings: a small JSON file in the app's config directory (mode 600 on Unix).
//! Secrets are never here: the cloud speech key lives in the OS keychain (see secrets.rs).
use crate::error::{msg, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TalkButton {
    Back,
    Forward,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextSize {
    #[default]
    Normal,
    Large,
    Larger,
}

/// Keyboard shortcut, a second trigger besides the mouse button (trackpads, laptops).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
pub enum Hotkey {
    CtrlAltSpace,
    AltShiftSpace,
    CtrlShiftSpace,
}

/// How hard a double knock must be. Higher sensitivity = lighter taps count (and more false alarms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnockSensitivity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TalkMode {
    Hold,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttEngine {
    Local,
    Cloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    TypeIntoFocused,
    Agent,
    ControlMac,
}

/// The Claude model for "Control my Mac": Haiku is fast; Sonnet handles longer requests better.
/// Auto picks per request: Haiku for short one-step requests, Sonnet for work inside apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacModel {
    Auto,
    Haiku,
    Sonnet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Claude,
    Codex,
    Custom,
}

/// What an agent may do. There is deliberately no "bypass" level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Safety {
    ReadOnly,
    Edit,
    AskCommands,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub button: TalkButton,
    /// None = no keyboard shortcut.
    pub hotkey: Option<Hotkey>,
    pub mode: TalkMode,
    /// Experimental: double-knock on the laptop to start, again to send. Off by default.
    pub knock: bool,
    pub knock_sensitivity: KnockSensitivity,
    /// After a click (toggle mode) or a double knock: send when the user stops talking.
    pub auto_stop: bool,
    /// Soft sounds when listening starts, is sent, or is cancelled.
    pub sound_cues: bool,
    pub live_words: bool,
    pub instant: bool,
    pub text_size: TextSize,
    pub tutorial_done: bool,
    pub ui_language: String,
    /// Say "Hey Xpu" to talk: the microphone stays on, checked on this Mac. Off by default.
    pub wake_word: bool,
    /// Press AirPods (or any headset play button) to talk.
    #[serde(alias = "airpods_press", deserialize_with = "airpods_on")]
    pub airpods: bool,
    /// After an agent's answer is read aloud, listen briefly for a reply that continues it.
    pub conversation: bool,
    pub memory: bool,
    pub stt: SttEngine,
    pub local_model: String,
    pub language: String,
    pub delivery: Delivery,
    pub agent: AgentKind,
    pub custom_agent: Option<PathBuf>,
    pub project_dir: Option<PathBuf>,
    pub safety: Safety,
    pub speak_summary: bool,
    /// Mode A only: press Enter after typing. Off by default: in a terminal, Enter runs the line.
    pub auto_submit: bool,
    /// Mode A: clean what is typed (fillers, punctuation) in the style of the app it goes into.
    pub polish: bool,
    pub max_record_seconds: u32,
    /// Microphone by name; None = system default. Useful to talk into the laptop mic while
    /// listening on Bluetooth headphones (their mic switches them to low-quality call audio).
    pub input_device: Option<String>,
    /// Claude Code profile: the name of a `~/.claude*` folder (passed as CLAUDE_CONFIG_DIR), for
    /// people with several accounts (`claude-work`, `claude-startup`, …). None = Claude's default.
    pub claude_profile: Option<String>,
    pub mac_model: MacModel,
    /// App access for "Control my Mac", by bundle id. Apps not listed are Off.
    pub app_access: std::collections::BTreeMap<String, crate::mac::policy::Access>,
    /// Every normal app at Control (excluded apps and terminals still need their own setting).
    pub allow_all_apps: bool,
    /// Every action on the Mac waits for the user's press.
    pub step_by_step: bool,
    /// Send the front app, and for apps the assistant may see its window, page and selected text,
    /// with each request ("reply to this").
    pub share_context: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            button: TalkButton::Forward,
            hotkey: Some(Hotkey::CtrlAltSpace),
            mode: TalkMode::Hold,
            knock: false,
            knock_sensitivity: KnockSensitivity::Medium,
            auto_stop: true,
            sound_cues: true,
            live_words: true,
            instant: true,
            text_size: TextSize::Normal,
            tutorial_done: false,
            ui_language: "system".into(),
            wake_word: false,
            airpods: false,
            conversation: true,
            memory: true,
            stt: SttEngine::Local,
            local_model: "base.en".into(),
            language: "en".into(),
            delivery: Delivery::TypeIntoFocused,
            agent: AgentKind::Claude,
            custom_agent: None,
            project_dir: None,
            safety: Safety::AskCommands,
            speak_summary: true,
            auto_submit: false,
            polish: true,
            max_record_seconds: 120,
            input_device: None,
            claude_profile: None,
            mac_model: MacModel::Auto,
            app_access: Default::default(),
            allow_all_apps: false,
            step_by_step: false,
            share_context: true,
        }
    }
}

impl Settings {
    /// The App access snapshot a "Control my Mac" request runs with.
    pub fn mac_policy(&self) -> crate::mac::policy::Policy {
        crate::mac::policy::Policy { levels: self.app_access.clone(), allow_all: self.allow_all_apps, step_by_step: self.step_by_step }
    }

    pub fn validate(&self) -> Result<()> {
        if crate::stt::model::find(&self.local_model).is_none() {
            return Err(msg(format!("unknown speech model {:?}", self.local_model)));
        }
        let lang_ok = self.language == "auto" || (self.language.len() == 2 && self.language.chars().all(|c| c.is_ascii_lowercase()));
        if !lang_ok {
            return Err(msg("language must be \"auto\" or a two-letter code"));
        }
        if !["system", "en", "fa", "es", "de", "fr", "ar", "zh", "ja"].contains(&self.ui_language.as_str()) {
            return Err(msg("unknown app language"));
        }
        if !(5..=600).contains(&self.max_record_seconds) {
            return Err(msg("max recording length must be 5–600 seconds"));
        }
        if let Some(d) = &self.input_device {
            if d.is_empty() || d.len() > 200 || d.chars().any(char::is_control) {
                return Err(msg("invalid input device name"));
            }
        }
        if let Some(dir) = &self.project_dir {
            validate_project_dir(dir)?;
        }
        if self.custom_agent.as_ref().is_some_and(|p| !p.is_absolute()) {
            return Err(msg("the custom agent must be an absolute path"));
        }
        if let Some(p) = &self.claude_profile {
            claude_profile_dir(p)?;
        }
        if self.app_access.len() > 1000 || self.app_access.keys().any(|b| !crate::mac::policy::valid_bundle_id(b)) {
            return Err(msg("invalid app access list"));
        }
        Ok(())
    }
}

/// A project folder must be an existing, absolute directory and must not be a system or home root:
/// an agent with edit rights there could change anything below it.
pub fn validate_project_dir(dir: &Path) -> Result<PathBuf> {
    if !dir.is_absolute() {
        return Err(msg("project folder must be an absolute path"));
    }
    let canon = dir.canonicalize().map_err(|_| msg("project folder does not exist"))?;
    if !canon.is_dir() {
        return Err(msg("project folder is not a directory"));
    }
    let home = dirs::home_dir().and_then(|h| h.canonicalize().ok());
    let too_broad = canon.parent().is_none()
        || Some(&canon) == home.as_ref()
        || ["/System", "/usr", "/bin", "/sbin", "/etc", "/Library", "/Applications", "/private", "C:\\Windows", "C:\\Program Files"]
            .iter()
            .any(|p| canon.starts_with(p) && canon.components().count() <= Path::new(p).components().count() + 1);
    if too_broad {
        return Err(msg("pick a specific project folder, not a system or home root"));
    }
    Ok(canon)
}

/// A Claude profile folder: a directory directly in the home folder named `.claude` or `.claude-…`.
/// Anything else (paths, `..`, symlinks out of home) is refused.
pub fn claude_profile_dir(name: &str) -> Result<PathBuf> {
    let ok_name = (name == ".claude" || name.starts_with(".claude-"))
        && name.len() <= 64
        && name[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok_name {
        return Err(msg("a Claude profile must be a ~/.claude or ~/.claude-<name> folder"));
    }
    let home = dirs::home_dir().ok_or_else(|| msg("no home folder"))?.canonicalize()?;
    let dir = home.join(name).canonicalize().map_err(|_| msg(format!("Claude profile ~/{name} does not exist")))?;
    if !dir.is_dir() || dir.parent() != Some(home.as_path()) {
        return Err(msg("a Claude profile must be a folder directly in your home folder"));
    }
    Ok(dir)
}

/// The `~/.claude*` folders on this machine, for the settings window.
pub fn claude_profiles() -> Vec<String> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    let mut v: Vec<String> = std::fs::read_dir(home)
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).filter(|n| claude_profile_dir(n).is_ok()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

pub fn config_dir() -> Result<PathBuf> {
    let d = dirs::config_dir().ok_or_else(|| msg("no config directory"))?.join("xpu-command");
    std::fs::create_dir_all(&d)?;
    restrict(&d, 0o700)?;
    Ok(d)
}

fn file() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.json"))
}

/// True until settings have been saved once: the first launch opens the settings window.
pub fn first_run() -> bool {
    file().map(|f| !f.exists()).unwrap_or(true)
}

/// Accepts the old `airpods_press` value ("single", "double", "triple" or null) as on or off.
fn airpods_on<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        On(bool),
        Gesture(Option<String>),
    }
    Ok(match Value::deserialize(d)? {
        Value::On(on) => on,
        Value::Gesture(g) => g.is_some(),
    })
}

pub fn load() -> Settings {
    let read = || -> Result<Settings> {
        let s: Settings = serde_json::from_str(&std::fs::read_to_string(file()?)?)?;
        s.validate()?;
        Ok(s)
    };
    read().unwrap_or_else(|e| {
        log::info!("using default settings ({e})");
        Settings::default()
    })
}

pub fn save(s: &Settings) -> Result<()> {
    s.validate()?;
    let path = file()?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    restrict(&tmp, 0o600)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn restrict(p: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}
#[cfg(not(unix))]
fn restrict(_: &Path, _: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe_and_valid() {
        let s = Settings::default();
        assert_eq!(s.stt, SttEngine::Local, "audio stays on the machine by default");
        assert!(!s.auto_submit, "never press Enter by default");
        assert!(!s.wake_word, "the microphone isn't always on unless the user turns it on");
        assert_eq!(s.safety, Safety::AskCommands, "commands need the user's yes by default");
        s.validate().unwrap();
    }

    #[test]
    fn old_airpods_gesture_turns_airpods_on() {
        let on: Settings = serde_json::from_str(r#"{"airpods_press":"triple"}"#).unwrap();
        assert!(on.airpods);
        let off: Settings = serde_json::from_str(r#"{"airpods_press":null}"#).unwrap();
        assert!(!off.airpods);
        let new: Settings = serde_json::from_str(r#"{"airpods":true}"#).unwrap();
        assert!(new.airpods);
    }

    #[test]
    fn unknown_fields_and_bad_values_are_rejected() {
        assert!(serde_json::from_str::<Settings>(r#"{"safety":"bypass"}"#).is_err());
        assert!(serde_json::from_str::<Settings>(r#"{"surprise":1}"#).is_err());
        let s = Settings { language: "english".into(), ..Default::default() };
        assert!(s.validate().is_err());
        let s = Settings { local_model: "../../evil".into(), ..Default::default() };
        assert!(s.validate().is_err());
    }

    #[test]
    fn claude_profile_must_be_a_home_dot_claude_folder() {
        for bad in ["../.claude", "/etc", ".claude/../../etc", ".bashrc", ".claude-x/y", ".claude-a b", ""] {
            assert!(claude_profile_dir(bad).is_err(), "{bad:?} must be refused");
        }
        let name = format!(".claude-xputest{}", std::process::id());
        let dir = dirs::home_dir().unwrap().join(&name);
        std::fs::create_dir_all(&dir).unwrap();
        let got = claude_profile_dir(&name);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(got.is_ok());
    }

    #[test]
    fn project_dir_must_be_specific() {
        assert!(validate_project_dir(Path::new("relative/dir")).is_err());
        assert!(validate_project_dir(Path::new("/")).is_err());
        if let Some(h) = dirs::home_dir() {
            assert!(validate_project_dir(&h).is_err());
        }
        let tmp = std::env::temp_dir().join(format!("xpu-proj-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(validate_project_dir(&tmp).is_ok());
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
