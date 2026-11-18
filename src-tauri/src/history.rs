//! Dictation history, kept on this Mac only (`dictations.jsonl`, mode 600, the newest `KEEP`): what
//! was typed, where, and when, so nothing said is lost: when typing fails (a password field, a
//! missing permission, the app went away), the text is still here to copy. Turned off in Settings,
//! nothing is written and the file is removed.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const KEEP: usize = 500;
static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub at: String,
    /// The app it was typed into (its name), if known.
    pub app: String,
    pub text: String,
    /// False when it could not be typed: then this is the only copy.
    pub typed: bool,
}

fn path() -> Option<PathBuf> {
    crate::settings::config_dir().ok().map(|d| d.join("dictations.jsonl"))
}

pub fn record(app: &str, text: &str, typed: bool) {
    if text.trim().is_empty() {
        return;
    }
    let e = Entry { at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(), app: crate::agents::clip(app, 60), text: text.to_string(), typed };
    if let Some(p) = path() {
        if let Err(err) = append(&p, &e, KEEP) {
            log::warn!("dictation history: {err}");
        }
    }
}

/// Newest first; `query` matches words in the text or the app name, any case.
pub fn list(query: &str, limit: usize) -> Vec<Entry> {
    path().map(|p| read(&p)).unwrap_or_default().into_iter().rev().filter(|e| matches(e, query)).take(limit).collect()
}

pub fn clear() -> std::io::Result<()> {
    let _g = LOCK.lock();
    match path() {
        Some(p) if p.exists() => std::fs::remove_file(p),
        _ => Ok(()),
    }
}

fn matches(e: &Entry, query: &str) -> bool {
    let (text, app) = (crate::vocab::norm(&e.text), crate::vocab::norm(&e.app));
    crate::vocab::norm(query).split(' ').filter(|w| !w.is_empty()).all(|w| text.contains(w) || app.contains(w))
}

fn read(p: &Path) -> Vec<Entry> {
    std::fs::read_to_string(p).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Appends, rewriting the file with the newest `keep` once it holds more.
fn append(p: &Path, e: &Entry, keep: usize) -> std::io::Result<()> {
    use std::io::Write;
    let _g = LOCK.lock();
    let mut entries = read(p);
    entries.push(e.clone());
    let skip = entries.len().saturating_sub(keep);
    let tmp = p.with_extension("jsonl.tmp");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp)?;
    for e in &entries[skip..] {
        writeln!(f, "{}", serde_json::to_string(e)?)?;
    }
    f.sync_all()?;
    std::fs::rename(tmp, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str, app: &str) -> Entry {
        Entry { at: "2026-11-18 10:00:00".into(), app: app.into(), text: text.into(), typed: true }
    }

    #[test]
    fn keeps_the_newest_privately() {
        let dir = std::env::temp_dir().join(format!("xpu-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("dictations.jsonl");
        for i in 0..7 {
            append(&p, &entry(&format!("note {i}"), "Notes"), 5).unwrap();
        }
        let all = read(&p);
        assert_eq!(all.len(), 5);
        assert_eq!(all.first().unwrap().text, "note 2");
        assert_eq!(all.last().unwrap().text, "note 6");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&p, "not json\n").unwrap();
        append(&p, &entry("after", "Mail"), 5).unwrap();
        assert_eq!(read(&p).len(), 1, "a broken line is dropped, not fatal");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn search_matches_all_words_in_text_or_app() {
        let e = entry("Ship the Kubernetes update on Friday", "Slack");
        assert!(matches(&e, ""));
        assert!(matches(&e, "kubernetes friday"));
        assert!(matches(&e, "slack ship"));
        assert!(!matches(&e, "kubernetes monday"));
        assert!(matches(&entry("جلسه فردا", "Telegram"), "جلسه"));
    }
}
