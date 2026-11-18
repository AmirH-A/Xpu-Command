//! Your words, kept on this Mac (`vocab.json`, mode 600): a personal dictionary, replacements and
//! voice snippets, plus fixes it noticed you making.
//!
//! - **Words** (names, jargon, products) steer the speech model toward them, and are typed with
//!   your spelling and capitals ("kubernetes" → "Kubernetes", "x ai" stays a replacement's job).
//! - **Replacements** turn what the model writes into what you want ("jason" → "JSON").
//! - **Snippets**: a short phrase said on its own types longer text ("my address" → the address).
//!   Only the whole utterance counts, so "send it to my address" is still typed as said.
//! - **Suggestions**: when you correct a word right after it was typed, the correction is offered
//!   here for one click. Nothing is ever added without you.
use crate::error::{msg, Result};
use crate::i18n::{tr, trf};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_WORDS: usize = 500;
pub const MAX_REPLACEMENTS: usize = 200;
pub const MAX_SNIPPETS: usize = 100;
const MAX_SUGGESTIONS: usize = 30;
const MAX_TERM: usize = 60;
const MAX_SNIPPET_TEXT: usize = 2000;
/// Characters of words passed to the speech model: Whisper reads at most ~224 tokens of prompt.
const PROMPT_CHARS: usize = 600;
static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vocab {
    pub words: Vec<String>,
    pub replacements: Vec<Replacement>,
    pub snippets: Vec<Snippet>,
    pub suggestions: Vec<Suggestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Replacement {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    pub trigger: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    /// What was typed.
    pub heard: String,
    /// What you changed it to.
    pub fixed: String,
    pub times: u32,
}

impl Vocab {
    /// Trims, checks limits and removes duplicates. Errors name the entry that is wrong.
    pub fn validated(mut self) -> Result<Vocab> {
        let clean = |s: &str| crate::sanitize::clean_transcript(s);
        let mut words: Vec<String> = Vec::new();
        for w in self.words.iter().map(|w| clean(w)).filter(|w| !w.is_empty()) {
            if w.chars().count() > MAX_TERM {
                return Err(msg(trf("“{word}…” is too long for a word (at most {max} characters)", &[("word", &w.chars().take(20).collect::<String>()), ("max", &MAX_TERM.to_string())])));
            }
            if !words.iter().any(|x| x.to_lowercase() == w.to_lowercase()) {
                words.push(w);
            }
        }
        if words.len() > MAX_WORDS {
            return Err(msg(trf("At most {max} words", &[("max", &MAX_WORDS.to_string())])));
        }
        let mut replacements: Vec<Replacement> = Vec::new();
        for r in &self.replacements {
            let (from, to) = (clean(&r.from), clean(&r.to));
            if from.is_empty() && to.is_empty() {
                continue;
            }
            if norm(&from).is_empty() {
                return Err(msg(trf("A replacement needs words to replace (“{to}”)", &[("to", &to)])));
            }
            if from.chars().count() > MAX_TERM || to.chars().count() > MAX_TERM * 3 {
                return Err(msg(trf("The replacement for “{from}” is too long", &[("from", &from)])));
            }
            if !replacements.iter().any(|x| norm(&x.from) == norm(&from)) {
                replacements.push(Replacement { from, to });
            }
        }
        if replacements.len() > MAX_REPLACEMENTS {
            return Err(msg(trf("At most {max} replacements", &[("max", &MAX_REPLACEMENTS.to_string())])));
        }
        let mut snippets: Vec<Snippet> = Vec::new();
        for s in &self.snippets {
            let (trigger, text) = (clean(&s.trigger), clean(&s.text));
            if trigger.is_empty() && text.is_empty() {
                continue;
            }
            let words = norm(&trigger).split(' ').filter(|w| !w.is_empty()).count();
            if !(1..=6).contains(&words) || norm(&trigger).chars().count() < 3 {
                return Err(msg(trf("A snippet’s phrase needs 1 to 6 words (“{phrase}”)", &[("phrase", &trigger)])));
            }
            if text.is_empty() || text.chars().count() > MAX_SNIPPET_TEXT {
                return Err(msg(trf("The snippet “{phrase}” needs text, at most {max} characters", &[("phrase", &trigger), ("max", &MAX_SNIPPET_TEXT.to_string())])));
            }
            if snippets.iter().any(|x| norm(&x.trigger) == norm(&trigger)) {
                return Err(msg(trf("Two snippets have the phrase “{phrase}”", &[("phrase", &trigger)])));
            }
            snippets.push(Snippet { trigger, text });
        }
        if snippets.len() > MAX_SNIPPETS {
            return Err(msg(trf("At most {max} snippets", &[("max", &MAX_SNIPPETS.to_string())])));
        }
        self.suggestions.truncate(MAX_SUGGESTIONS);
        Ok(Vocab { words, replacements, snippets, suggestions: self.suggestions })
    }
}

/// Lowercase, Persian letters folded, punctuation as spaces, single spaces: how phrases are compared.
pub fn norm(s: &str) -> String {
    let t: String = crate::sanitize::fold_persian(&s.to_lowercase()).chars().map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' }).collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The words for the speech model's prompt: yours first, then `extra` (names from the project),
/// within what the model reads.
pub fn prompt(v: &Vocab, extra: &str) -> String {
    let mut terms: Vec<&str> = v.words.iter().map(String::as_str).collect();
    terms.extend(v.replacements.iter().map(|r| r.to.as_str()).filter(|t| !t.is_empty() && t.chars().count() <= 30));
    terms.extend(extra.split(", ").filter(|t| !t.is_empty()));
    let mut out = String::new();
    let mut seen: Vec<String> = Vec::new();
    for t in terms {
        let key = t.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        if out.chars().count() + t.chars().count() + 2 > PROMPT_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(t);
        seen.push(key);
    }
    out
}

/// The snippet the whole utterance asks for ("my address", "insert my address").
pub fn snippet<'a>(text: &str, v: &'a Vocab) -> Option<&'a Snippet> {
    let t = norm(text);
    let t = ["insert ", "type ", "paste "].iter().find_map(|p| t.strip_prefix(p)).unwrap_or(&t);
    v.snippets.iter().find(|s| norm(&s.trigger) == t)
}

/// Applies replacements, then writes your words with your spelling and capitals.
pub fn apply(text: &str, v: &Vocab) -> String {
    let mut out = text.to_string();
    let mut reps: Vec<&Replacement> = v.replacements.iter().collect();
    // Longest first, so "new york times" wins over "new york".
    reps.sort_by_key(|r| std::cmp::Reverse(r.from.chars().count()));
    for r in reps {
        out = replace_phrase(&out, &r.from, &r.to);
    }
    for w in &v.words {
        if fixes_case(w) {
            out = replace_phrase(&out, w, w);
        }
    }
    out
}

/// A word is written with your capitals wherever it appears when that can't hurt ordinary text:
/// it has a capital inside or a digit ("xAI", "GPT4"), several words, or 3+ letters ("Sara").
/// Two-letter capitalized words ("Go") would change every "go".
fn fixes_case(w: &str) -> bool {
    let chars: Vec<char> = w.chars().collect();
    chars.iter().skip(1).any(|c| c.is_uppercase() || c.is_ascii_digit()) || w.contains(' ') || chars.len() >= 3
}

/// Replaces `from` (whole words, any case) with `to` everywhere in `text`.
fn replace_phrase(text: &str, from: &str, to: &str) -> String {
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let hay: Vec<char> = text.chars().collect();
    let needle: Vec<char> = from.trim().chars().map(fold).collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return text.to_string();
    }
    let lower: Vec<char> = hay.iter().map(|&c| fold(c)).collect();
    let word = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '\'');
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < hay.len() {
        let at_start = i == 0 || !word(hay.get(i - 1));
        if at_start && lower[i..].starts_with(&needle) && !word(hay.get(i + needle.len())) {
            out.push_str(to);
            i += needle.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

/// A correction you made to what was just typed: the field's text now has `fixed` where `typed`
/// had `heard`. `before` is the text that was right before the dictation when it was typed
/// (empty: it started the field). Only spelling-like fixes count (similar letters, or a name or
/// term with capitals, digits or another script), and only one short span changed.
pub fn find_fix(typed: &str, before: &str, value: &str) -> Option<(String, String)> {
    if typed.trim().is_empty() || value.contains(typed) {
        return None;
    }
    let start = if before.is_empty() { 0 } else { value.rfind(before)? + before.len() };
    let core = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_string();
    let t: Vec<String> = typed.split_whitespace().map(core).collect();
    let c: Vec<String> = value[start..].split_whitespace().take(t.len() + 6).map(core).collect();
    let p = t.iter().zip(&c).take_while(|(a, b)| a == b).count();
    if p >= t.len() {
        return None;
    }
    let mut found = None;
    'search: for dt in 1..=3.min(t.len() - p) {
        let rest = &t[p + dt..];
        if rest.is_empty() {
            // The fix is at the end: where it stops in the field is unknown, so take as many
            // words as were changed (or what is left).
            let dc = dt.min(c.len().saturating_sub(p));
            if dc > 0 {
                found = Some((t[p..].join(" "), c[p..p + dc].join(" ")));
            }
            break;
        }
        for dc in 1..=3 {
            if c.len() >= p + dc + rest.len() && c[p + dc..p + dc + rest.len()] == *rest {
                found = Some((t[p..p + dt].join(" "), c[p..p + dc].join(" ")));
                break 'search;
            }
        }
    }
    let (heard, fixed) = found?;
    if heard == fixed || heard.is_empty() || fixed.is_empty() || fixed.chars().count() > MAX_TERM || !fixed.chars().any(char::is_alphabetic) {
        return None;
    }
    let squash = |s: &str| s.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let (h, f) = (squash(&heard), squash(&fixed));
    let similar = h == f || (levenshtein(&h, &f) as f64) < 0.5 * h.chars().count().max(f.chars().count()) as f64;
    let term = fixed.chars().any(|c| c.is_uppercase() || c.is_ascii_digit() || (c.is_alphabetic() && !c.is_ascii()));
    (similar && term).then_some((heard, fixed))
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// Counts a noticed fix, unless your words already have it.
fn note(v: &mut Vocab, heard: &str, fixed: &str) {
    let known = v.words.iter().any(|w| w == fixed) || v.replacements.iter().any(|r| norm(&r.from) == norm(heard));
    if known {
        return;
    }
    match v.suggestions.iter_mut().find(|s| s.heard == heard && s.fixed == fixed) {
        Some(s) => s.times += 1,
        None => {
            v.suggestions.insert(0, Suggestion { heard: heard.into(), fixed: fixed.into(), times: 1 });
            v.suggestions.truncate(MAX_SUGGESTIONS);
        }
    }
}

/// Takes a suggestion: a fix of capitals only adds the word; any other fix also adds a replacement.
fn accept(v: &mut Vocab, heard: &str) -> Option<Suggestion> {
    let i = v.suggestions.iter().position(|s| s.heard == heard)?;
    let s = v.suggestions.remove(i);
    if !v.words.iter().any(|w| w.to_lowercase() == s.fixed.to_lowercase()) {
        v.words.push(s.fixed.clone());
    }
    if norm(&s.heard) != norm(&s.fixed) && !v.replacements.iter().any(|r| norm(&r.from) == norm(&s.heard)) {
        v.replacements.push(Replacement { from: s.heard.clone(), to: s.fixed.clone() });
    }
    Some(s)
}

fn file() -> Result<PathBuf> {
    Ok(crate::settings::config_dir()?.join("vocab.json"))
}

fn read(p: &Path) -> Vocab {
    let Ok(text) = std::fs::read_to_string(p) else { return Vocab::default() };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        log::warn!("{} unreadable, starting empty: {e}", p.display());
        Vocab::default()
    })
}

fn write(p: &Path, v: &Vocab) -> Result<()> {
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

pub fn load() -> Vocab {
    file().map(|p| read(&p)).unwrap_or_default()
}

/// Saves your words, replacements and snippets (suggestions are kept as they are on disk).
pub fn save(v: Vocab) -> Result<Vocab> {
    let _g = LOCK.lock();
    let p = file()?;
    let v = Vocab { suggestions: read(&p).suggestions, ..v }.validated()?;
    write(&p, &v)?;
    Ok(v)
}

pub fn notice_fix(heard: &str, fixed: &str) {
    let _g = LOCK.lock();
    let Ok(p) = file() else { return };
    let mut v = read(&p);
    note(&mut v, heard, fixed);
    if let Err(e) = write(&p, &v) {
        log::warn!("saving a suggestion: {e}");
    }
}

/// `accept`: add it to your words; otherwise dismiss it.
pub fn decide(heard: &str, take: bool) -> Result<Vocab> {
    let _g = LOCK.lock();
    let p = file()?;
    let mut v = read(&p);
    if take {
        accept(&mut v, heard).ok_or_else(|| msg(tr("That suggestion is gone")))?;
    } else {
        v.suggestions.retain(|s| s.heard != heard);
    }
    let v = v.validated()?;
    write(&p, &v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocab {
        Vocab {
            words: vec!["Kubernetes".into(), "xAI".into(), "Go".into(), "Hooman Khatibzadeh".into(), "Sara".into()],
            replacements: vec![
                Replacement { from: "jason".into(), to: "JSON".into() },
                Replacement { from: "new york".into(), to: "NY".into() },
                Replacement { from: "new york times".into(), to: "NYT".into() },
            ],
            snippets: vec![Snippet { trigger: "my email".into(), text: "hooman@example.com".into() }],
            suggestions: vec![],
        }
    }

    #[test]
    fn replacements_and_capitals_apply_to_whole_words_only() {
        let v = vocab();
        assert_eq!(apply("deploy it on kubernetes and send the jason file", &v), "deploy it on Kubernetes and send the JSON file");
        assert_eq!(apply("Jasonville and jasons stay", &v), "Jasonville and jasons stay", "only whole words");
        assert_eq!(apply("XAI released it, xai said", &v), "xAI released it, xAI said");
        assert_eq!(apply("let's go now", &v), "let's go now", "a two-letter word doesn't change ordinary text");
        assert_eq!(apply("call hooman khatibzadeh and sara.", &v), "call Hooman Khatibzadeh and Sara.");
        assert_eq!(apply("I read the new york times in new york", &v), "I read the NYT in NY", "longest phrase first");
        assert_eq!(apply("", &v), "");
    }

    #[test]
    fn persian_and_other_scripts_work() {
        let v = Vocab { replacements: vec![Replacement { from: "زیروکس".into(), to: "Zerox".into() }], words: vec!["تهران".into()], ..Default::default() };
        assert_eq!(apply("گزارش زیروکس آماده است", &v), "گزارش Zerox آماده است");
        assert_eq!(apply("زیروکسی", &v), "زیروکسی", "part of a longer word stays");
    }

    #[test]
    fn snippets_need_the_whole_utterance() {
        let v = vocab();
        assert_eq!(snippet("My email.", &v).map(|s| s.text.as_str()), Some("hooman@example.com"));
        assert_eq!(snippet("insert my email", &v).map(|s| s.text.as_str()), Some("hooman@example.com"));
        assert!(snippet("send it to my email", &v).is_none());
        assert!(snippet("", &v).is_none());
    }

    #[test]
    fn the_prompt_has_your_words_first_and_is_bounded() {
        let v = vocab();
        let p = prompt(&v, "main.rs, Kubernetes, Cargo.toml");
        assert!(p.starts_with("Kubernetes, xAI, Go"), "{p}");
        assert_eq!(p.matches("Kubernetes").count(), 1, "no duplicates");
        assert!(p.contains("JSON") && p.ends_with("Cargo.toml"));
        let many = Vocab { words: (0..400).map(|i| format!("Term{i}")).collect(), ..Default::default() };
        assert!(prompt(&many, "").chars().count() <= PROMPT_CHARS);
        assert_eq!(prompt(&Vocab::default(), ""), "");
    }

    #[test]
    fn validation_cleans_dedupes_and_limits() {
        let v = Vocab {
            words: vec![" Kubernetes ".into(), "kubernetes".into(), "".into(), "line\nbreak".into()],
            replacements: vec![Replacement { from: "".into(), to: "".into() }, Replacement { from: "jason".into(), to: "JSON".into() }],
            snippets: vec![Snippet { trigger: "sig".into(), text: "Best,\nHooman".into() }],
            suggestions: vec![],
        }
        .validated()
        .unwrap();
        assert_eq!(v.words, ["Kubernetes", "line break"]);
        assert_eq!(v.replacements.len(), 1);
        assert_eq!(v.snippets[0].text, "Best, Hooman", "no newline can press Enter");
        assert!(Vocab { words: vec!["x".repeat(61)], ..Default::default() }.validated().is_err());
        assert!(Vocab { snippets: vec![Snippet { trigger: "a b c d e f g".into(), text: "x".into() }], ..Default::default() }.validated().is_err());
        assert!(Vocab { snippets: vec![Snippet { trigger: "ok".into(), text: "x".into() }], ..Default::default() }.validated().is_err(), "too short to say on purpose");
        assert!(Vocab { snippets: vec![Snippet { trigger: "my sig".into(), text: "".into() }], ..Default::default() }.validated().is_err());
        let dup = vec![Snippet { trigger: "My sig".into(), text: "a".into() }, Snippet { trigger: "my sig!".into(), text: "b".into() }];
        assert!(Vocab { snippets: dup, ..Default::default() }.validated().is_err());
        assert!(Vocab { replacements: vec![Replacement { from: "!!".into(), to: "x".into() }], ..Default::default() }.validated().is_err());
    }

    #[test]
    fn fixes_are_found_only_for_spelling_like_changes() {
        // A name the model got wrong, fixed in place.
        assert_eq!(find_fix("Deploy to cooper netties today.", "Note: ", "Note: Deploy to Kubernetes today."), Some(("cooper netties".into(), "Kubernetes".into())));
        assert_eq!(find_fix("Call sarah tomorrow", "", "Call Sara tomorrow"), Some(("sarah".into(), "Sara".into())));
        assert_eq!(find_fix("ask about x ai", "", "ask about xAI"), Some(("x ai".into(), "xAI".into())), "the changed word is the last one");
        // Unchanged, rewritten, or ordinary words: nothing to learn.
        assert_eq!(find_fix("Call Sara tomorrow", "", "Call Sara tomorrow and more"), None);
        assert_eq!(find_fix("Meet on Friday", "", "Meet on Monday"), None, "a different word, not a spelling");
        assert_eq!(find_fix("their going home", "", "they're going home"), None, "ordinary words are grammar, not your words");
        assert_eq!(find_fix("Call sarah tomorrow", "Hi ", "Something else entirely"), None, "the text before it is gone");
        assert_eq!(find_fix("a b c d e", "", "x y z"), None);
        assert_eq!(find_fix("", "", "anything"), None);
        // Persian: another script counts as a term.
        assert_eq!(find_fix("جلسه با زیراکس", "", "جلسه با زیروکس"), Some(("زیراکس".into(), "زیروکس".into())));
    }

    #[test]
    fn suggestions_are_counted_and_accepted_on_request() {
        let mut v = Vocab::default();
        note(&mut v, "cooper netties", "Kubernetes");
        note(&mut v, "cooper netties", "Kubernetes");
        note(&mut v, "sarah", "Sara");
        assert_eq!(v.suggestions.len(), 2);
        assert_eq!(v.suggestions.iter().find(|s| s.fixed == "Kubernetes").unwrap().times, 2);
        accept(&mut v, "cooper netties").unwrap();
        assert_eq!(v.words, ["Kubernetes"]);
        assert_eq!(v.replacements, [Replacement { from: "cooper netties".into(), to: "Kubernetes".into() }]);
        note(&mut v, "cooper netties", "Kubernetes");
        assert_eq!(v.suggestions.len(), 1, "already known: not suggested again");
        note(&mut v, "xai", "xAI");
        accept(&mut v, "xai").unwrap();
        assert!(v.words.contains(&"xAI".to_string()));
        assert!(!v.replacements.iter().any(|r| r.from == "xai"), "a fix of capitals needs only the word");
        assert!(accept(&mut v, "missing").is_none());
        for i in 0..40 {
            note(&mut v, &format!("w{i}"), &format!("W{i}x"));
        }
        assert_eq!(v.suggestions.len(), MAX_SUGGESTIONS);
    }

    #[test]
    fn file_is_private_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("xpu-vocab-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("vocab.json");
        assert_eq!(read(&p), Vocab::default(), "missing file: empty");
        write(&p, &vocab()).unwrap();
        assert_eq!(read(&p), vocab());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&p, "{broken").unwrap();
        assert_eq!(read(&p), Vocab::default(), "a broken file starts empty instead of failing");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
