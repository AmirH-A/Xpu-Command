//! Activity log: one JSON line per Mac action (time, tool, short summary, outcome). Mode 600 in the
//! config folder, kept to the last `KEEP` entries. Only short summaries are stored, never note or
//! email bodies.
use crate::mac::server::Outcome;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

const KEEP: usize = 300;
const TRIM_AT_BYTES: u64 = 200 * 1024;
static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub at: String,
    pub tool: String,
    pub summary: String,
    pub outcome: Outcome,
    pub detail: String,
}

fn path() -> Option<PathBuf> {
    crate::settings::config_dir().ok().map(|d| d.join("activity.jsonl"))
}

pub fn record(tool: &str, summary: &str, outcome: Outcome, detail: &str) {
    let e = Entry {
        at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        tool: crate::agents::clip(tool, 40),
        summary: crate::agents::clip(summary, 140),
        outcome,
        detail: crate::agents::clip(detail, 160),
    };
    if let Some(p) = path() {
        if let Err(err) = append(&p, &e) {
            log::warn!("activity log: {err}");
        }
    }
}

pub fn recent(n: usize) -> Vec<Entry> {
    path().map(|p| read(&p, n)).unwrap_or_default()
}

pub fn clear() -> std::io::Result<()> {
    let _g = LOCK.lock();
    match path() {
        Some(p) if p.exists() => std::fs::remove_file(p),
        _ => Ok(()),
    }
}

fn open_append(p: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p)
}

fn append(p: &Path, e: &Entry) -> std::io::Result<()> {
    let _g = LOCK.lock();
    let mut f = open_append(p)?;
    writeln!(f, "{}", serde_json::to_string(e)?)?;
    if f.metadata()?.len() > TRIM_AT_BYTES {
        let text = std::fs::read_to_string(p)?;
        let lines: Vec<&str> = text.lines().collect();
        let keep = lines[lines.len().saturating_sub(KEEP)..].join("\n") + "\n";
        let tmp = p.with_extension("jsonl.tmp");
        {
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            o.open(&tmp)?.write_all(keep.as_bytes())?;
        }
        std::fs::rename(tmp, p)?;
    }
    Ok(())
}

/// The newest `n` entries, newest first. Unreadable lines are skipped.
fn read(p: &Path, n: usize) -> Vec<Entry> {
    let _g = LOCK.lock();
    let Ok(text) = std::fs::read_to_string(p) else { return Vec::new() };
    text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()).take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_read_trim() {
        let dir = std::env::temp_dir().join(format!("xpu-act-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("activity.jsonl");
        let e = |i: usize| Entry { at: "t".into(), tool: "open_app".into(), summary: format!("Opening {i} {}", "x".repeat(600)), outcome: Outcome::Done, detail: String::new() };
        for i in 0..500 {
            append(&p, &e(i)).unwrap();
        }
        let all = read(&p, 10_000);
        assert!(all.len() <= KEEP + 400 && all.len() >= KEEP, "trimmed: {}", all.len());
        assert!(all[0].summary.starts_with("Opening 499"), "newest first");
        assert_eq!(read(&p, 5).len(), 5);
        std::fs::write(&p, "not json\n").unwrap();
        assert!(read(&p, 5).is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::remove_file(&p).unwrap();
            append(&p, &e(1)).unwrap();
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
