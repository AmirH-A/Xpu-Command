use super::tools::Player;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const MAX_AGE: Duration = Duration::from_secs(30 * 60);
const KEEP: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    QuitApp { name: String },
    OpenApp { name: String, path: PathBuf },
    Volume { level: u8, muted: bool },
    DarkMode { on: bool },
    Media { play: bool, player: Player },
    DeleteReminder { id: String },
    DeleteEvent { uid: String, calendar: String },
    DeleteNote { id: String },
    ForgetFact { id: String },
    RememberFact { text: String },
    DeleteRoutine { id: String },
    /// What was on the clipboard before (None: no text, so clear it).
    Clipboard { text: Option<String> },
    /// A moved, renamed or trashed item: back from `from` to `to`.
    MoveBack { from: PathBuf, to: PathBuf },
    RemoveFolder { path: PathBuf },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub request: u64,
    pub at: Instant,
    pub what: String,
    pub step: Step,
}

#[derive(Default)]
pub struct Stack {
    entries: Vec<Entry>,
}

impl Stack {
    pub fn push(&mut self, request: u64, what: String, step: Step, at: Instant) {
        self.entries.push(Entry { request, at, what, step });
        if self.entries.len() > KEEP {
            self.entries.remove(0);
        }
    }

    pub fn take_last_request(&mut self, now: Instant) -> Vec<Entry> {
        self.entries.retain(|e| now.duration_since(e.at) <= MAX_AGE);
        let Some(last) = self.entries.last().map(|e| e.request) else { return Vec::new() };
        let split = self.entries.iter().position(|e| e.request == last).unwrap_or(self.entries.len());
        let mut taken = self.entries.split_off(split);
        taken.reverse();
        taken
    }

    pub fn peek_last_request(&self, now: Instant) -> Vec<String> {
        let live: Vec<&Entry> = self.entries.iter().filter(|e| now.duration_since(e.at) <= MAX_AGE).collect();
        let Some(last) = live.last().map(|e| e.request) else { return Vec::new() };
        live.iter().rev().filter(|e| e.request == last).map(|e| e.what.clone()).collect()
    }
}

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
static STACK: Mutex<Stack> = Mutex::new(Stack { entries: Vec::new() });

pub fn next_request() -> u64 {
    NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
}

pub fn push(request: u64, what: impl Into<String>, step: Step) {
    STACK.lock().push(request, what.into(), step, Instant::now());
}

pub fn take_last_request() -> Vec<Entry> {
    STACK.lock().take_last_request(Instant::now())
}

pub fn peek_last_request() -> Vec<String> {
    STACK.lock().peek_last_request(Instant::now())
}

pub fn summary(undone: &[String], failed: &[String]) -> String {
    match (undone.is_empty(), failed.is_empty()) {
        (true, true) => crate::i18n::tr("Nothing to undo").into(),
        (false, true) => crate::i18n::trf("Undid: {items}", &[("items", &undone.join(", "))]),
        (true, false) => crate::i18n::trf("Couldn't undo: {items}", &[("items", &failed.join(", "))]),
        (false, false) => crate::i18n::trf("Undid: {items}. Couldn't undo: {failed}", &[("items", &undone.join(", ")), ("failed", &failed.join(", "))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(n: &str) -> Step {
        Step::QuitApp { name: n.into() }
    }

    #[test]
    fn undo_takes_the_whole_last_request_newest_first() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push(1, "Opened Notes".into(), step("Notes"), t0);
        s.push(2, "Opened Spotify".into(), step("Spotify"), t0);
        s.push(2, "Set volume to 30%".into(), Step::Volume { level: 50, muted: false }, t0);
        assert_eq!(s.peek_last_request(t0), ["Set volume to 30%", "Opened Spotify"]);
        let taken = s.take_last_request(t0);
        assert_eq!(taken.iter().map(|e| e.what.as_str()).collect::<Vec<_>>(), ["Set volume to 30%", "Opened Spotify"]);
        let next = s.take_last_request(t0);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].what, "Opened Notes");
        assert!(s.take_last_request(t0).is_empty());
    }

    #[test]
    fn old_actions_are_not_undone() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        s.push(1, "Opened Notes".into(), step("Notes"), t0);
        let later = t0 + MAX_AGE + Duration::from_secs(1);
        assert!(s.peek_last_request(later).is_empty());
        assert!(s.take_last_request(later).is_empty());
    }

    #[test]
    fn the_stack_is_bounded() {
        let t0 = Instant::now();
        let mut s = Stack::default();
        for i in 0..(KEEP as u64 + 10) {
            s.push(i, format!("{i}"), step("x"), t0);
        }
        assert_eq!(s.entries.len(), KEEP);
        assert_eq!(s.entries[0].what, "10");
    }

    #[test]
    fn summaries() {
        assert_eq!(summary(&[], &[]), "Nothing to undo");
        assert_eq!(summary(&["closed Spotify".into()], &[]), "Undid: closed Spotify");
        assert_eq!(summary(&["closed Spotify".into()], &["the note".into()]), "Undid: closed Spotify. Couldn't undo: the note");
    }
}
