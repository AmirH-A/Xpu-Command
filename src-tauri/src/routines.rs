use crate::error::{msg, Result};
use crate::mac::tools::{Call, MediaAction, Player};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const MAX_ROUTINES: usize = 50;
pub const MAX_STEPS: usize = 10;
const HISTORY_KEEP: usize = 100;
const SUGGEST_AFTER: usize = 3;
const SUGGEST_DAYS: usize = 2;
const ALLOWED: &[&str] = &["open_app", "quit_app", "open_url", "web_search", "media", "play_music", "set_volume", "dark_mode", "run_shortcut"];
static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub tool: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub steps: Vec<Step>,
    pub added: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Seen {
    day: String,
    steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Suggestion {
    pub steps: Vec<Step>,
    pub labels: Vec<String>,
    pub times: usize,
}

pub fn step_of(call: &Call) -> Option<Step> {
    let player = |p: &Option<Player>| p.map(|p| json!(match p { Player::Music => "music", Player::Spotify => "spotify" }));
    let (tool, args) = match call {
        Call::OpenApp { name } => ("open_app", json!({ "name": name })),
        Call::QuitApp { name } => ("quit_app", json!({ "name": name })),
        Call::OpenUrl { url } => ("open_url", json!({ "url": url.as_str() })),
        Call::WebSearch { query } => ("web_search", json!({ "query": query })),
        Call::Media { action, player: p } => {
            let a = match action {
                MediaAction::Play => "play",
                MediaAction::Pause => "pause",
                MediaAction::Next => "next",
                MediaAction::Previous => "previous",
            };
            let mut v = json!({ "action": a });
            if let Some(p) = player(p) {
                v["player"] = p;
            }
            ("media", v)
        }
        Call::PlayMusic { query, player: p } => {
            let mut v = json!({ "query": query });
            if let Some(p) = player(p) {
                v["player"] = p;
            }
            ("play_music", v)
        }
        Call::SetVolume { level, mute } => {
            let mut v = json!({});
            if let Some(l) = level {
                v["level"] = json!(l);
            }
            if let Some(m) = mute {
                v["mute"] = json!(m);
            }
            ("set_volume", v)
        }
        Call::DarkMode { on } => ("dark_mode", json!({ "on": on })),
        Call::RunShortcut { name } => ("run_shortcut", json!({ "name": name })),
        _ => return None,
    };
    Some(Step { tool: tool.into(), args })
}

pub fn parse_step(step: &Step) -> std::result::Result<Call, String> {
    if !ALLOWED.contains(&step.tool.as_str()) {
        return Err(format!("{} can't be part of a routine", step.tool));
    }
    crate::mac::tools::parse(&step.tool, &step.args, chrono::Local::now().naive_local())
}

pub fn label(step: &Step) -> String {
    parse_step(step).map(|c| c.describe()).unwrap_or_else(|_| step.tool.clone())
}

pub fn normalize_name(name: &str) -> String {
    let t: String = crate::sanitize::fold_persian(&name.to_lowercase()).chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn validate(name: &str, steps: &[Step]) -> Result<(String, Vec<Step>)> {
    let n = normalize_name(name);
    if n.chars().count() < 2 || n.chars().count() > 40 {
        return Err(msg("give the routine a name of 2 to 40 letters"));
    }
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(msg(format!("a routine has 1 to {MAX_STEPS} steps")));
    }
    let mut clean = Vec::new();
    for s in steps {
        let call = parse_step(s).map_err(msg)?;
        clean.push(step_of(&call).ok_or_else(|| msg(format!("{} can't be part of a routine", s.tool)))?);
    }
    Ok((n, clean))
}

pub fn match_spoken<'a>(text: &str, routines: &'a [Routine]) -> Option<&'a Routine> {
    let t = normalize_name(text);
    let name = ["run the routine ", "run routine ", "start routine ", "routine ", "run ", "اجرای ", "روتین "].iter().find_map(|p| t.strip_prefix(p)).unwrap_or(&t);
    routines.iter().find(|r| r.name == name || r.name == t)
}

fn suggestions(history: &[Seen], routines: &[Routine]) -> Vec<Suggestion> {
    let mut groups: BTreeMap<String, (Vec<Step>, usize, BTreeSet<String>)> = BTreeMap::new();
    for h in history.iter().filter(|h| h.steps.len() >= 2) {
        let key = serde_json::to_string(&h.steps).unwrap_or_default();
        let g = groups.entry(key).or_insert_with(|| (h.steps.clone(), 0, BTreeSet::new()));
        g.1 += 1;
        g.2.insert(h.day.clone());
    }
    let mut out: Vec<Suggestion> = groups
        .into_values()
        .filter(|(steps, times, days)| *times >= SUGGEST_AFTER && days.len() >= SUGGEST_DAYS && !routines.iter().any(|r| &r.steps == steps))
        .map(|(steps, times, _)| Suggestion { labels: steps.iter().map(label).collect(), steps, times })
        .collect();
    out.sort_by(|a, b| b.times.cmp(&a.times));
    out.truncate(3);
    out
}

fn dir() -> Result<PathBuf> {
    crate::settings::config_dir()
}

pub fn list() -> Vec<Routine> {
    let _g = LOCK.lock();
    dir().map(|d| read_json(&d.join("routines.json"))).unwrap_or_default()
}

pub fn save(name: &str, steps: &[Step]) -> Result<Routine> {
    let (name, steps) = validate(name, steps)?;
    let _g = LOCK.lock();
    let p = dir()?.join("routines.json");
    let mut all: Vec<Routine> = read_json(&p);
    all.retain(|r| r.name != name);
    if all.len() >= MAX_ROUTINES {
        return Err(msg(format!("at most {MAX_ROUTINES} routines")));
    }
    let r = Routine { id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(), name, steps, added: chrono::Local::now().format("%Y-%m-%d").to_string() };
    all.push(r.clone());
    write_json(&p, &all)?;
    Ok(r)
}

pub fn remove(id: &str) -> Result<bool> {
    let _g = LOCK.lock();
    let p = dir()?.join("routines.json");
    let mut all: Vec<Routine> = read_json(&p);
    let before = all.len();
    all.retain(|r| r.id != id);
    let removed = all.len() != before;
    if removed {
        write_json(&p, &all)?;
    }
    Ok(removed)
}

pub fn record(steps: Vec<Step>) {
    if steps.len() < 2 {
        return;
    }
    let _g = LOCK.lock();
    let Ok(d) = dir() else { return };
    let p = d.join("routine-history.json");
    let mut h: Vec<Seen> = read_json(&p);
    h.push(Seen { day: chrono::Local::now().format("%Y-%m-%d").to_string(), steps });
    let skip = h.len().saturating_sub(HISTORY_KEEP);
    h.drain(..skip);
    if let Err(e) = write_json(&p, &h) {
        log::warn!("routine history: {e}");
    }
}

pub fn current_suggestions() -> Vec<Suggestion> {
    let routines = list();
    let _g = LOCK.lock();
    let h: Vec<Seen> = dir().map(|d| read_json(&d.join("routine-history.json"))).unwrap_or_default();
    suggestions(&h, &routines)
}

pub fn forget_suggestion(steps: &[Step]) -> Result<()> {
    let _g = LOCK.lock();
    let p = dir()?.join("routine-history.json");
    let mut h: Vec<Seen> = read_json(&p);
    h.retain(|s| s.steps != steps);
    write_json(&p, &h)
}

fn read_json<T: serde::de::DeserializeOwned>(p: &Path) -> Vec<T> {
    let Ok(text) = std::fs::read_to_string(p) else { return Vec::new() };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        log::warn!("{} unreadable, starting empty: {e}", p.display());
        Vec::new()
    })
}

fn write_json<T: Serialize>(p: &Path, v: &T) -> Result<()> {
    let tmp = p.with_extension("json.tmp");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    use std::io::Write;
    o.open(&tmp)?.write_all(&serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(tool: &str, args: Value) -> Step {
        Step { tool: tool.into(), args }
    }

    fn morning() -> Vec<Step> {
        vec![st("open_app", json!({"name":"Slack"})), st("set_volume", json!({"level":30}))]
    }

    #[test]
    fn only_fixed_actions_can_be_routine_steps() {
        assert!(validate("start my day", &morning()).is_ok());
        assert!(validate("x", &morning()).is_err(), "name too short");
        assert!(validate("ok", &[]).is_err());
        assert!(validate("ok", &[st("click", json!({"element":3}))]).is_err());
        assert!(validate("ok", &[st("create_reminder", json!({"title":"x"}))]).is_err(), "dates would go stale");
        assert!(validate("ok", &[st("open_url", json!({"url":"file:///etc/passwd"}))]).is_err(), "the normal checks still apply");
        assert!(validate("ok", &vec![st("dark_mode", json!({"on":true})); MAX_STEPS + 1]).is_err());
        let (name, steps) = validate("  Start   my DAY! ", &[st("open_app", json!({"name":"  Slack "}))]).unwrap();
        assert_eq!(name, "start my day");
        assert_eq!(steps, [st("open_app", json!({"name":"Slack"}))]);
    }

    #[test]
    fn steps_round_trip_through_calls() {
        for s in [
            st("open_app", json!({"name":"Slack"})),
            st("media", json!({"action":"play","player":"spotify"})),
            st("play_music", json!({"query":"lofi"})),
            st("set_volume", json!({"mute":true})),
            st("dark_mode", json!({"on":false})),
            st("run_shortcut", json!({"name":"Focus"})),
            st("open_url", json!({"url":"https://example.com/a"})),
        ] {
            assert_eq!(step_of(&parse_step(&s).unwrap()).unwrap(), s);
        }
    }

    #[test]
    fn spoken_names_match_exactly() {
        let r = vec![Routine { id: "1".into(), name: "start my day".into(), steps: morning(), added: "d".into() }, Routine { id: "2".into(), name: "شروع کار".into(), steps: morning(), added: "d".into() }];
        assert_eq!(match_spoken("Start my day.", &r).unwrap().id, "1");
        assert_eq!(match_spoken("run start my day", &r).unwrap().id, "1");
        assert_eq!(match_spoken("شروع کار", &r).unwrap().id, "2");
        assert!(match_spoken("start my day and open notes", &r).is_none());
        assert!(match_spoken("start", &r).is_none());
    }

    #[test]
    fn repeated_requests_become_suggestions() {
        let seen = |day: &str, steps: Vec<Step>| Seen { day: day.into(), steps };
        let mut h = vec![seen("2026-11-13", morning()), seen("2026-11-14", morning())];
        assert!(suggestions(&h, &[]).is_empty(), "twice is not a habit");
        h.push(seen("2026-11-14", morning()));
        h.push(seen("2026-11-14", vec![st("open_app", json!({"name":"Notes"}))]));
        let s = suggestions(&h, &[]);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].times, 3);
        assert_eq!(s[0].labels, ["Opening Slack", "Volume 30%"]);
        let same_day: Vec<Seen> = (0..5).map(|_| seen("2026-11-14", morning())).collect();
        assert!(suggestions(&same_day, &[]).is_empty(), "needs more than one day");
        let saved = vec![Routine { id: "1".into(), name: "start my day".into(), steps: morning(), added: "d".into() }];
        assert!(suggestions(&h, &saved).is_empty(), "already a routine");
    }
}
