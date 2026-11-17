//! What the user's own words already allow: the apps they named ("reply in Slack"), the app in
//! front when they point at it ("summarize this"), and their files when they mention one. Asking
//! for it is consent for that request, so the assistant doesn't ask again. Pure, so it is tested.

/// App names that are also everyday words: they count only when written with their capital
/// ("open Notes", not "take notes"), so "add these numbers" doesn't allow Numbers.
const COMMON_WORDS: &[&str] = &[
    "notes", "numbers", "music", "photos", "messages", "mail", "calendar", "maps", "books", "news", "home", "clock", "preview", "pages",
    "reminders", "contacts", "podcasts", "stocks", "weather", "shortcuts", "freeform", "finder", "word", "teams", "things", "bear", "arc",
    "spark", "notion", "mercury", "craft", "reeder", "tv", "keynote", "calculator", "dictionary", "chess", "stickies", "terminal", "console",
    "automator", "passwords", "journal", "games", "phone", "safari", "excel", "outlook", "edge", "opera", "brave", "zoom", "discord",
];

/// The names among `names` that the request mentions, in any case, except names that are also
/// everyday words, which need their capital. A vendor prefix may be left out ("Chrome", "Word").
pub fn mentioned<'a>(text: &str, names: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let phrase_at = |needle: &[&str], exact: bool| {
        needle.is_empty()
            || (0..words.len()).any(|i| {
                i + needle.len() <= words.len()
                    && needle.iter().enumerate().all(|(k, n)| if exact { words[i + k] == *n } else { lower[i + k] == n.to_lowercase() })
            })
    };
    let mut out = Vec::new();
    for name in names {
        let parts: Vec<&str> = name.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).collect();
        if parts.is_empty() || parts.iter().map(|p| p.len()).sum::<usize>() < 3 {
            continue;
        }
        let short: Vec<&str> = match parts.first() {
            Some(&("Google" | "Microsoft" | "Adobe" | "Apple")) if parts.len() > 1 => parts[1..].to_vec(),
            _ => parts.clone(),
        };
        let found = [parts.as_slice(), short.as_slice()].iter().any(|p| {
            let common = p.len() == 1 && COMMON_WORDS.contains(&p[0].to_lowercase().as_str());
            phrase_at(p, common)
        });
        if found {
            out.push(name);
        }
    }
    out
}

/// "this", "it", "here": the request is about what is in front.
pub fn points_at_front(text: &str) -> bool {
    const WORDS: &[&str] = &[
        "this", "these", "it", "here", "that", "selection", "selected", "این", "اینو", "اینجا", "اینا", "esto", "esta", "este", "aquí", "aqui",
        "dies", "diese", "dieses", "hier", "ceci", "cela", "ça", "ici", "هذا", "هذه", "هنا",
    ];
    let lower = crate::sanitize::fold_persian(&text.to_lowercase());
    lower.split(|c: char| !c.is_alphanumeric()).any(|w| WORDS.contains(&w)) || ["这个", "这里", "これ", "ここ"].iter().any(|w| lower.contains(w))
}

/// The request is about files or folders in the home folder.
pub fn mentions_files(text: &str) -> bool {
    const WORDS: &[&str] = &[
        "file", "files", "folder", "folders", "document", "documents", "pdf", "pdfs", "downloads", "desktop", "screenshot", "screenshots", "invoice",
        "فایل", "فایلها", "فایل‌ها", "پوشه", "دانلودها", "دسکتاپ", "سند", "archivo", "archivos", "carpeta", "documento", "datei", "dateien", "ordner",
        "dokument", "fichier", "fichiers", "dossier", "ملف", "ملفات", "مجلد",
    ];
    let lower = crate::sanitize::fold_persian(&text.to_lowercase());
    lower.split(|c: char| !c.is_alphanumeric() && c != '\u{200c}').any(|w| WORDS.contains(&w)) || ["文件", "ファイル", "フォルダ"].iter().any(|w| lower.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPS: &[&str] = &["Notes", "Numbers", "Music", "Google Chrome", "Microsoft Word", "WhatsApp", "Slack", "Visual Studio Code", "Mail", "Safari"];

    #[test]
    fn named_apps_count_ordinary_words_dont() {
        assert_eq!(mentioned("Open Notes and write my shopping list", APPS.iter().copied()), ["Notes"]);
        assert_eq!(mentioned("reply to Sara in slack", APPS.iter().copied()), ["Slack"]);
        assert_eq!(mentioned("Find the pricing page in Chrome", APPS.iter().copied()), ["Google Chrome"]);
        assert_eq!(mentioned("open my report in word", APPS.iter().copied()), Vec::<&str>::new(), "a plain word must be capitalized");
        assert_eq!(mentioned("Open my report in Word", APPS.iter().copied()), ["Microsoft Word"]);
        assert_eq!(mentioned("send it on whatsapp", APPS.iter().copied()), ["WhatsApp"]);
        assert_eq!(mentioned("add these numbers and play some music", APPS.iter().copied()), Vec::<&str>::new());
        assert_eq!(mentioned("check my email", APPS.iter().copied()), Vec::<&str>::new(), "Mail is not email");
        assert_eq!(mentioned("Copy this from Safari into Notes", APPS.iter().copied()), ["Notes", "Safari"]);
        assert_eq!(mentioned("open visual studio code", APPS.iter().copied()), ["Visual Studio Code"]);
    }

    #[test]
    fn pointing_and_files() {
        assert!(points_at_front("Summarize this"));
        assert!(points_at_front("reply to it politely"));
        assert!(points_at_front("اینو خلاصه کن"));
        assert!(!points_at_front("Open Spotify"));
        assert!(mentions_files("find my tax PDF"));
        assert!(mentions_files("move the screenshots to a new folder"));
        assert!(mentions_files("فایل قرارداد رو باز کن"));
        assert!(!mentions_files("play some jazz"));
    }
}
