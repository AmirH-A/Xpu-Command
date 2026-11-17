//! App access: which apps the assistant may see or control, and the gate decision for every call.
//! Pure (no I/O): every rule here is unit-tested. The app enforces it on each action, against the
//! app that really owns the window, so a fooled or mistaken model still can't touch other apps.
use super::tools::Risk;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The user's choice per app. `Off` is the default for every app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Off,
    /// Read the window (text, controls, a screenshot of that app only).
    See,
    /// See, plus click, type, menus, keys.
    Control,
    /// Control, but every action waits for the user's press.
    Ask,
}

/// What a call needs from the app it touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Need {
    See,
    Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Never seen or controlled, whatever the settings say.
    Excluded,
    /// Can run anything: only an explicit per-app setting allows it ("allow all" doesn't).
    Terminal,
    Normal,
}

/// Bundle ids that are never touched: secrets, system security settings, and this app itself
/// (the assistant must not change its own permissions).
const EXCLUDED: &[&str] = &[
    "app.xpucommand.desktop",
    "com.apple.keychainaccess",
    "com.apple.Passwords",
    "com.apple.systempreferences",
    "com.apple.SecurityAgent",
    "com.apple.Settings",
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.agilebits.onepassword-osx",
    "com.bitwarden.desktop",
    "com.lastpass.LastPassMacDesktop",
    "com.dashlane.dashlanephonefinal",
    "org.keepassxc.keepassxc",
    "com.nordpass.macos.NordPass",
    "com.enpass.Enpass",
];

const TERMINALS: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "dev.warp.Warp-Stable",
    "net.kovidgoyal.kitty",
    "com.mitchellh.ghostty",
    "io.alacritty",
    "co.zeit.hyper",
    "com.github.wez.wezterm",
    "com.apple.ScriptEditor2",
    "com.apple.Automator",
];

/// The home folder, gated like an app: "see" finds and reads files, "control" also opens and changes them.
pub const FILES: &str = "xpu.files";

pub fn class(bundle: &str) -> Class {
    if EXCLUDED.iter().any(|b| b.eq_ignore_ascii_case(bundle)) {
        Class::Excluded
    } else if TERMINALS.iter().any(|b| b.eq_ignore_ascii_case(bundle)) {
        Class::Terminal
    } else {
        Class::Normal
    }
}

/// A bundle id as stored in settings: reverse-DNS characters only.
pub fn valid_bundle_id(b: &str) -> bool {
    (3..=155).contains(&b.len()) && b.contains('.') && b.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// A snapshot of the user's settings, taken when a request starts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Policy {
    pub levels: BTreeMap<String, Access>,
    pub allow_all: bool,
    pub step_by_step: bool,
}

impl Policy {
    /// The effective level for an app: an explicit setting wins; "allow all" means Control for
    /// normal apps; excluded apps are always Off; terminals need an explicit setting.
    pub fn level(&self, bundle: &str) -> Access {
        match class(bundle) {
            Class::Excluded => Access::Off,
            Class::Terminal => self.levels.get(bundle).copied().unwrap_or(Access::Off),
            Class::Normal => self.levels.get(bundle).copied().unwrap_or(if self.allow_all { Access::Control } else { Access::Off }),
        }
    }
}

/// What one request has learned and been granted so far.
#[derive(Debug, Default)]
pub struct RunState {
    /// "Just this time" grants from the user, per bundle id.
    pub grants: HashMap<String, Need>,
    /// Apps whose screen content the model has read in this request.
    pub read_apps: HashSet<String>,
    /// Apps the user asked for in this request (named, or "this" with the app in front): asking
    /// is consent, so they are allowed for this request without a question.
    pub named: HashSet<String>,
    /// Apps the user said yes to acting in after content was read: not asked again this request.
    pub trusted: HashSet<String>,
}

impl RunState {
    pub fn tainted(&self) -> bool {
        !self.read_apps.is_empty()
    }
}

/// The app a call touches, as resolved by the system (never as claimed by the model).
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub bundle: String,
    pub name: String,
    pub pid: i32,
    pub need: Need,
    /// The control acted on (e.g. "button Send"), when known: risky names need a yes.
    pub label: Option<String>,
    /// A click at a position whose control could not be identified.
    pub unknown_spot: bool,
    /// The window's frame in screen points (x, y, width, height), to outline it.
    pub frame: Option<(f64, f64, f64, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Run,
    /// Show it; run unless the user presses within the grace period.
    Grace,
    /// Run only if the user presses.
    Confirm,
    /// The app isn't allowed (yet): ask "just this time?".
    Grant(Need),
    Refuse(String),
}

/// Extra facts about a call that change the decision.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    /// Sends something out of the Mac or runs something (a link, a search, a draft, a Shortcut).
    pub outbound: bool,
    /// A key combination or menu command that can't easily be undone (⌘Q, ⌘⌫, "Delete", "Send"…).
    pub risky: bool,
}

/// The gate: the strongest of what the risk, the app's level, the request's history and the
/// user's "step by step" choice require.
pub fn decide(base: Risk, target: Option<&Target>, flags: Flags, policy: &Policy, run: &RunState) -> Decision {
    let mut d = match base {
        Risk::Read | Risk::Safe => Decision::Run,
        Risk::Change => Decision::Grace,
        Risk::Confirm => Decision::Confirm,
    };
    let mut confirm = matches!(d, Decision::Confirm);
    if let Some(t) = target {
        let cls = class(&t.bundle);
        if cls == Class::Excluded {
            return Decision::Refuse(format!("{} is never seen or controlled by the assistant (it holds passwords or security settings, or it is Xpu Command itself).", t.name));
        }
        let level = policy.level(&t.bundle);
        let named = cls == Class::Normal && run.named.contains(&t.bundle);
        let granted = run.grants.get(&t.bundle).copied().or(named.then_some(Need::Control));
        let enough = match t.need {
            Need::See => level >= Access::See || granted.is_some(),
            Need::Control => level >= Access::Control || granted == Some(Need::Control),
        };
        if !enough {
            if cls == Class::Terminal {
                return Decision::Refuse(format!("{} can run anything, so it can only be allowed in Settings › App access.", t.name));
            }
            return Decision::Grant(t.need);
        }
        if level == Access::Ask {
            confirm = true;
        }
        if t.label.as_deref().is_some_and(risky_label) {
            confirm = true;
        }
        if t.unknown_spot && !confirm {
            d = Decision::Grace;
        }
        let expected = run.read_apps.contains(&t.bundle) || named || run.trusted.contains(&t.bundle);
        if run.tainted() && t.need == Need::Control && !expected {
            confirm = true;
        }
    }
    if flags.risky || (flags.outbound && run.tainted()) {
        confirm = true;
    }
    if policy.step_by_step && base != Risk::Read {
        confirm = true;
    }
    if confirm {
        Decision::Confirm
    } else {
        d
    }
}

/// Control names that send, delete, pay or sign out: clicking them always needs a yes.
pub fn risky_label(label: &str) -> bool {
    const WORDS: &[&str] = &[
        "send", "delete", "remove", "pay", "buy", "purchase", "order", "checkout", "check out", "post", "publish", "transfer",
        "submit", "sign out", "log out", "logout", "unsubscribe", "erase", "trash", "empty", "install", "uninstall", "confirm",
        "approve", "withdraw", "deposit", "tweet", "share", "discard", "reset", "format", "archive all", "forward",
    ];
    let l = label.to_lowercase();
    let words: Vec<&str> = l.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let joined = words.join(" ");
    WORDS.iter().any(|w| {
        let w_words: Vec<&str> = w.split(' ').collect();
        words.windows(w_words.len()).any(|win| win == w_words.as_slice()) || joined == *w
    })
}

/// Key combinations that quit, close without saving, delete, send or lock.
pub fn risky_keys(mods: &[&str], key: &str) -> bool {
    let has = |m: &str| mods.contains(&m);
    let cmd = has("cmd");
    match key {
        "q" => cmd,
        "return" | "enter" => cmd || has("ctrl"),
        "delete" | "forwarddelete" => cmd || has("option"),
        "escape" => cmd && has("option"),
        "d" => cmd && has("shift"),
        "w" => cmd && has("option"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(bundle: &str, need: Need) -> Target {
        Target { bundle: bundle.into(), name: bundle.into(), pid: 1, need, label: None, unknown_spot: false, frame: None }
    }
    fn policy(pairs: &[(&str, Access)]) -> Policy {
        Policy { levels: pairs.iter().map(|(b, a)| (b.to_string(), *a)).collect(), ..Default::default() }
    }
    const CHROME: &str = "com.google.Chrome";

    #[test]
    fn off_by_default_and_grants_are_per_request() {
        let p = Policy::default();
        let mut run = RunState::default();
        assert_eq!(decide(Risk::Read, Some(&t(CHROME, Need::See)), Flags::default(), &p, &run), Decision::Grant(Need::See));
        run.grants.insert(CHROME.into(), Need::See);
        assert_eq!(decide(Risk::Read, Some(&t(CHROME, Need::See)), Flags::default(), &p, &run), Decision::Run);
        assert_eq!(decide(Risk::Safe, Some(&t(CHROME, Need::Control)), Flags::default(), &p, &run), Decision::Grant(Need::Control), "a see grant doesn't allow control");
    }

    #[test]
    fn levels() {
        let p = policy(&[(CHROME, Access::See), ("com.apple.Notes", Access::Control), ("com.apple.mail", Access::Ask)]);
        let run = RunState::default();
        let f = Flags::default();
        assert_eq!(decide(Risk::Read, Some(&t(CHROME, Need::See)), f, &p, &run), Decision::Run);
        assert_eq!(decide(Risk::Safe, Some(&t(CHROME, Need::Control)), f, &p, &run), Decision::Grant(Need::Control));
        assert_eq!(decide(Risk::Safe, Some(&t("com.apple.Notes", Need::Control)), f, &p, &run), Decision::Run);
        assert_eq!(decide(Risk::Read, Some(&t("com.apple.mail", Need::See)), f, &p, &run), Decision::Confirm, "ask = every action");
    }

    #[test]
    fn excluded_and_terminals() {
        let all = Policy { allow_all: true, ..Default::default() };
        let run = RunState::default();
        for b in ["com.apple.keychainaccess", "com.1password.1password", "app.xpucommand.desktop", "com.apple.systempreferences"] {
            assert!(matches!(decide(Risk::Read, Some(&t(b, Need::See)), Flags::default(), &all, &run), Decision::Refuse(_)), "{b}");
            let explicit = policy(&[(b, Access::Control)]);
            assert!(matches!(decide(Risk::Read, Some(&t(b, Need::See)), Flags::default(), &explicit, &run), Decision::Refuse(_)), "{b}: even explicitly");
        }
        assert!(matches!(decide(Risk::Safe, Some(&t("com.apple.Terminal", Need::Control)), Flags::default(), &all, &run), Decision::Refuse(_)), "allow-all doesn't cover terminals");
        let explicit = policy(&[("com.apple.Terminal", Access::Control)]);
        assert_eq!(decide(Risk::Safe, Some(&t("com.apple.Terminal", Need::Control)), Flags::default(), &explicit, &run), Decision::Run);
        assert_eq!(decide(Risk::Safe, Some(&t(CHROME, Need::Control)), Flags::default(), &all, &run), Decision::Run);
        let all_but = Policy { allow_all: true, levels: [(CHROME.to_string(), Access::Off)].into(), ..Default::default() };
        assert_eq!(decide(Risk::Safe, Some(&t(CHROME, Need::Control)), Flags::default(), &all_but, &run), Decision::Grant(Need::Control), "explicit Off beats allow-all");
    }

    #[test]
    fn risky_controls_need_a_yes() {
        let p = policy(&[(CHROME, Access::Control)]);
        let run = RunState::default();
        let mut send = t(CHROME, Need::Control);
        send.label = Some("button Send".into());
        assert_eq!(decide(Risk::Safe, Some(&send), Flags::default(), &p, &run), Decision::Confirm);
        send.label = Some("button Sender details".into());
        assert_eq!(decide(Risk::Safe, Some(&send), Flags::default(), &p, &run), Decision::Run);
        assert_eq!(decide(Risk::Safe, Some(&t(CHROME, Need::Control)), Flags { risky: true, ..Default::default() }, &p, &run), Decision::Confirm);
        let mut spot = t(CHROME, Need::Control);
        spot.unknown_spot = true;
        assert_eq!(decide(Risk::Safe, Some(&spot), Flags::default(), &p, &run), Decision::Grace);
    }

    #[test]
    fn after_reading_a_screen_outbound_and_other_apps_need_a_yes() {
        let p = policy(&[(CHROME, Access::Control), ("com.apple.Notes", Access::Control)]);
        let mut run = RunState::default();
        run.read_apps.insert(CHROME.into());
        assert_eq!(decide(Risk::Safe, Some(&t(CHROME, Need::Control)), Flags::default(), &p, &run), Decision::Run, "same app: fine");
        assert_eq!(decide(Risk::Safe, Some(&t("com.apple.Notes", Need::Control)), Flags::default(), &p, &run), Decision::Confirm, "another app");
        assert_eq!(decide(Risk::Safe, None, Flags { outbound: true, ..Default::default() }, &p, &run), Decision::Confirm, "a link after reading");
        assert_eq!(decide(Risk::Safe, None, Flags { outbound: true, ..Default::default() }, &p, &RunState::default()), Decision::Run, "a link before reading");
    }

    #[test]
    fn step_by_step() {
        let p = Policy { step_by_step: true, ..policy(&[(CHROME, Access::Control)]) };
        let run = RunState::default();
        assert_eq!(decide(Risk::Safe, Some(&t(CHROME, Need::Control)), Flags::default(), &p, &run), Decision::Confirm);
        assert_eq!(decide(Risk::Read, Some(&t(CHROME, Need::See)), Flags::default(), &p, &run), Decision::Run, "reading isn't an action");
        assert_eq!(decide(Risk::Safe, None, Flags::default(), &p, &run), Decision::Confirm);
    }

    #[test]
    fn labels_and_keys() {
        for r in ["Send", "button Delete", "Pay now", "Sign out", "Move to Trash", "Buy with 1-click", "Empty Trash…", "Forward"] {
            assert!(risky_label(r), "{r}");
        }
        for ok in ["Inbox", "Compose", "Search mail", "Sender", "Postcard", "Reply", "Orders history page", "Payment methods"] {
            assert!(!risky_label(ok), "{ok}");
        }
        assert!(risky_keys(&["cmd"], "q"));
        assert!(risky_keys(&["cmd"], "return"));
        assert!(risky_keys(&["cmd"], "delete"));
        assert!(!risky_keys(&["cmd"], "l"));
        assert!(!risky_keys(&[], "return"));
        assert!(!risky_keys(&["cmd"], "c"));
    }

    #[test]
    fn bundle_ids() {
        assert!(valid_bundle_id("com.google.Chrome"));
        assert!(!valid_bundle_id("chrome"));
        assert!(!valid_bundle_id("com.x/../y"));
        assert!(!valid_bundle_id(&"a.".repeat(100)));
    }

    #[test]
    fn asking_for_an_app_is_consent_but_not_for_terminals_or_secrets() {
        let p = Policy::default();
        let mut run = RunState::default();
        run.named.extend(["com.apple.Notes".to_string(), "com.apple.Terminal".to_string(), "com.apple.keychainaccess".to_string()]);
        assert_eq!(decide(Risk::Safe, Some(&t("com.apple.Notes", Need::Control)), Flags::default(), &p, &run), Decision::Run);
        assert_eq!(decide(Risk::Read, Some(&t(CHROME, Need::See)), Flags::default(), &p, &run), Decision::Grant(Need::See), "other apps still ask");
        assert!(matches!(decide(Risk::Safe, Some(&t("com.apple.Terminal", Need::Control)), Flags::default(), &p, &run), Decision::Refuse(_)), "naming a terminal isn't enough");
        assert!(matches!(decide(Risk::Read, Some(&t("com.apple.keychainaccess", Need::See)), Flags::default(), &p, &run), Decision::Refuse(_)));
        let risky = Target { label: Some("button Send".into()), ..t("com.apple.Notes", Need::Control) };
        assert_eq!(decide(Risk::Safe, Some(&risky), Flags::default(), &p, &run), Decision::Confirm, "sending still needs a yes");
    }

    #[test]
    fn after_reading_only_unexpected_apps_ask_and_a_yes_holds() {
        let p = policy(&[(CHROME, Access::Control), ("com.apple.Notes", Access::Control), ("com.apple.mail", Access::Control)]);
        let mut run = RunState::default();
        run.read_apps.insert(CHROME.into());
        run.named.insert("com.apple.Notes".into());
        assert_eq!(decide(Risk::Safe, Some(&t("com.apple.Notes", Need::Control)), Flags::default(), &p, &run), Decision::Run, "the app the user named");
        assert_eq!(decide(Risk::Safe, Some(&t("com.apple.mail", Need::Control)), Flags::default(), &p, &run), Decision::Confirm, "an app the user didn't mention");
        run.trusted.insert("com.apple.mail".into());
        assert_eq!(decide(Risk::Safe, Some(&t("com.apple.mail", Need::Control)), Flags::default(), &p, &run), Decision::Run, "once allowed, not asked again");
        assert_eq!(decide(Risk::Safe, None, Flags { outbound: true, risky: false }, &p, &run), Decision::Confirm, "sending out after reading still asks");
    }
}
