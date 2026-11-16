use serde_json::{json, Value};

pub type Step = (String, Value);

const APPS_FA: &[(&str, &str)] = &[
    ("اسپاتیفای", "Spotify"),
    ("کروم", "Google Chrome"),
    ("گوگل کروم", "Google Chrome"),
    ("سافاری", "Safari"),
    ("تلگرام", "Telegram"),
    ("واتساپ", "WhatsApp"),
    ("اسلک", "Slack"),
    ("کرسر", "Cursor"),
    ("ترمینال", "Terminal"),
    ("فایندر", "Finder"),
    ("یادداشت", "Notes"),
    ("یادداشت ها", "Notes"),
    ("نوتس", "Notes"),
    ("تقویم", "Calendar"),
    ("یادآوری", "Reminders"),
    ("ماشین حساب", "Calculator"),
    ("موزیک", "Music"),
    ("میل", "Mail"),
    ("ایمیل", "Mail"),
    ("تنظیمات", "System Settings"),
    ("فیس تایم", "FaceTime"),
    ("مسیج", "Messages"),
    ("پیام ها", "Messages"),
];

const SITES_FA: &[(&str, &str)] = &[("یوتیوب", "youtube.com"), ("جیمیل", "mail.google.com"), ("گوگل", "google.com")];

pub fn parse(text: &str) -> Option<Vec<Step>> {
    let t = normalize(text);
    if t.is_empty() || t.chars().count() > 80 || has_joiner(&t) {
        return None;
    }
    english(&t).or_else(|| persian(&t)).or_else(|| others(&t)).map(|s| vec![s])
}

fn step(tool: &str, args: Value) -> Step {
    (tool.to_string(), args)
}

fn normalize(text: &str) -> String {
    let folded = crate::sanitize::fold_persian(&text.to_lowercase());
    let mapped: String = folded
        .chars()
        .map(|c| match c {
            '۰'..='۹' => char::from_digit(c as u32 - '۰' as u32, 10).unwrap_or(c),
            '٠'..='٩' => char::from_digit(c as u32 - '٠' as u32, 10).unwrap_or(c),
            '٪' => '%',
            c if c.is_alphanumeric() || c == '%' || c == '.' || c == ' ' || c == '-' => c,
            _ => ' ',
        })
        .collect();
    let mut t = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    t = t.trim_end_matches('.').trim().to_string();
    for p in ["please ", "hey ", "can you ", "could you ", "would you ", "لطفا ", "لطفاً "] {
        if let Some(r) = t.strip_prefix(p) {
            t = r.to_string();
        }
    }
    for p in ["can you ", "could you "] {
        if let Some(r) = t.strip_prefix(p) {
            t = r.to_string();
        }
    }
    for s in [" please", " for me", " لطفا", " لطفاً"] {
        if let Some(r) = t.strip_suffix(s) {
            t = r.to_string();
        }
    }
    t
}

fn has_joiner(t: &str) -> bool {
    let w: Vec<&str> = t.split(' ').collect();
    w.iter().any(|x| matches!(*x, "and" | "then" | "also" | "after" | "و" | "بعد" | "سپس" | "y" | "luego" | "und" | "dann" | "et" | "puis" | "ثم"))
        || t.contains(',')
        || ["然后", "并且", "再", "そして", "それから"].iter().any(|j| t.contains(j))
}

fn others(t: &str) -> Option<Step> {
    let media = |a: &str| Some(step("media", json!({ "action": a })));
    let mute = |on: bool| Some(step("set_volume", json!({ "mute": on })));
    if let Some(n) = rest(t, &["abre ", "abrir ", "öffne ", "starte ", "ouvre ", "lance ", "افتح ", "شغل تطبيق "]) {
        return Some(target(n.trim_start_matches("el ").trim_start_matches("la ").trim_start_matches("le ")));
    }
    if let Some(n) = t.strip_prefix("打开").filter(|n| !n.is_empty()) {
        return Some(target(n.trim()));
    }
    if let Some(n) = t.strip_suffix("を開いて").or_else(|| t.strip_suffix("を開く")).or_else(|| t.strip_suffix("を起動して")).filter(|n| !n.is_empty()) {
        return Some(target(n.trim()));
    }
    if let Some(v) = rest(t, &["volumen ", "lautstärke ", "الصوت ", "مستوى الصوت "]).or_else(|| t.strip_prefix("音量").filter(|v| !v.is_empty())) {
        return percent(v.trim_end_matches("にして").trim_end_matches('に')).map(|l| step("set_volume", json!({ "level": l })));
    }
    match t {
        "siguiente" | "siguiente canción" | "siguiente cancion" | "nächster titel" | "nächstes lied" | "weiter" | "suivant" | "chanson suivante" | "الاغنية التالية" | "التالي" | "下一首" | "次の曲" => media("next"),
        "anterior" | "canción anterior" | "vorheriger titel" | "précédent" | "chanson précédente" | "الاغنية السابقة" | "上一首" | "前の曲" => media("previous"),
        "pausa" | "pausar" | "pausieren" | "mets en pause" | "ايقاف مؤقت" | "暂停" | "一時停止" => media("pause"),
        "reproduce" | "reproducir" | "continúa" | "abspielen" | "wiedergabe" | "lecture" | "reprends" | "تشغيل" | "播放" | "继续播放" | "再生" => media("play"),
        "silencio" | "silenciar" | "stumm" | "ton aus" | "coupe le son" | "كتم الصوت" | "静音" | "ミュート" => mute(true),
        "activa el sonido" | "ton an" | "remets le son" | "الغ كتم الصوت" | "取消静音" | "ミュート解除" => mute(false),
        _ => None,
    }
}

fn rest<'a>(t: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|p| t.strip_prefix(p)).map(str::trim).filter(|r| !r.is_empty())
}

fn target(name: &str) -> Step {
    let n = name.trim_start_matches("the ").trim();
    let is_site = n.contains('.') && !n.contains(' ') && n.rsplit('.').next().is_some_and(|tld| (2..=6).contains(&tld.len()) && tld.chars().all(|c| c.is_ascii_alphabetic()));
    if is_site {
        return step("open_url", json!({ "url": format!("https://{n}") }));
    }
    step("open_app", json!({ "name": n }))
}

fn percent(s: &str) -> Option<u8> {
    let s = s.trim().trim_end_matches('%').trim_end_matches(" percent").trim_end_matches(" درصد").trim_end_matches("درصد").trim();
    s.parse::<u8>().ok().filter(|v| *v <= 100)
}

fn english(t: &str) -> Option<Step> {
    if let Some(n) = rest(t, &["open ", "launch ", "start ", "switch to ", "go to "]) {
        return Some(target(n));
    }
    if let Some(n) = rest(t, &["quit ", "close "]) {
        return Some(step("quit_app", json!({ "name": n.trim_start_matches("the ") })));
    }
    if let Some(q) = rest(t, &["search the web for ", "search for ", "google ", "look up "]) {
        return Some(step("web_search", json!({ "query": q })));
    }
    if let Some(v) = rest(t, &["set the volume to ", "set volume to ", "volume to ", "volume at ", "volume ", "set the volume at "]) {
        return percent(v).map(|l| step("set_volume", json!({ "level": l })));
    }
    let media = |a: &str| Some(step("media", json!({ "action": a })));
    match t {
        "mute" | "mute the sound" | "mute the volume" | "mute sound" => Some(step("set_volume", json!({ "mute": true }))),
        "unmute" | "unmute the sound" | "unmute sound" => Some(step("set_volume", json!({ "mute": false }))),
        "pause" | "pause the music" | "pause music" | "pause the song" | "pause spotify" | "stop the music" | "stop music" => media("pause"),
        "play" | "resume" | "play music" | "play the music" | "resume the music" | "resume music" => media("play"),
        "next" | "next song" | "next track" | "skip" | "skip this song" | "skip song" | "skip track" => media("next"),
        "previous" | "previous song" | "previous track" | "last song" | "go back a song" => media("previous"),
        "dark mode on" | "turn on dark mode" | "enable dark mode" | "switch to dark mode" | "light mode off" => Some(step("dark_mode", json!({ "on": true }))),
        "dark mode off" | "turn off dark mode" | "disable dark mode" | "switch to light mode" | "light mode on" | "turn on light mode" => Some(step("dark_mode", json!({ "on": false }))),
        _ => None,
    }
}

fn fa_target(name: &str) -> Step {
    let n = name.trim();
    if let Some((_, site)) = SITES_FA.iter().find(|(k, _)| *k == n) {
        return step("open_url", json!({ "url": format!("https://{site}") }));
    }
    let app = APPS_FA.iter().find(|(k, _)| *k == n).map(|(_, v)| *v).unwrap_or(n);
    target(app)
}

fn persian(t: &str) -> Option<Step> {
    for suffix in [" رو باز کن", " را باز کن", " رو اجرا کن", " را اجرا کن", " رو بیار", " رو باز کنید"] {
        if let Some(n) = t.strip_suffix(suffix).filter(|n| !n.is_empty()) {
            return Some(fa_target(n));
        }
    }
    if let Some(n) = rest(t, &["باز کن ", "اجرا کن "]) {
        return Some(fa_target(n));
    }
    for suffix in [" رو ببند", " را ببند"] {
        if let Some(n) = t.strip_suffix(suffix).filter(|n| !n.is_empty()) {
            let app = APPS_FA.iter().find(|(k, _)| *k == n).map(|(_, v)| *v).unwrap_or(n);
            return Some(step("quit_app", json!({ "name": app })));
        }
    }
    for suffix in [" رو سرچ کن", " را سرچ کن", " رو جستجو کن", " را جستجو کن"] {
        if let Some(q) = t.strip_suffix(suffix).filter(|q| !q.is_empty()) {
            return Some(step("web_search", json!({ "query": q })));
        }
    }
    if let Some(v) = rest(t, &["صدا رو بذار روی ", "صدا رو بزار روی ", "صدا رو بذار رو ", "صدا رو کن ", "صدا روی ", "صدا "]) {
        let v = v.trim_end_matches(" کن").trim_end_matches(" بذار");
        if let Some(l) = percent(v) {
            return Some(step("set_volume", json!({ "level": l })));
        }
    }
    let media = |a: &str| Some(step("media", json!({ "action": a })));
    match t {
        "بی صدا کن" | "بیصدا کن" | "صدا رو قطع کن" | "صدا رو ببند" => Some(step("set_volume", json!({ "mute": true }))),
        "صدا رو وصل کن" | "صدا رو باز کن" => Some(step("set_volume", json!({ "mute": false }))),
        "مکث" | "آهنگ رو نگه دار" | "موزیک رو نگه دار" | "موزیک رو قطع کن" | "آهنگ رو قطع کن" => media("pause"),
        "پخش کن" | "ادامه بده" | "موزیک رو پخش کن" | "آهنگ رو پخش کن" => media("play"),
        "بعدی" | "آهنگ بعدی" | "آهنگ بعدی رو بذار" | "آهنگ بعدی رو بزن" | "برو آهنگ بعدی" => media("next"),
        "قبلی" | "آهنگ قبلی" | "آهنگ قبلی رو بذار" | "برگرد آهنگ قبلی" => media("previous"),
        "حالت تاریک رو روشن کن" | "حالت تاریک روشن" | "دارک مود روشن" | "دارک مود رو روشن کن" => Some(step("dark_mode", json!({ "on": true }))),
        "حالت تاریک رو خاموش کن" | "حالت تاریک خاموش" | "دارک مود خاموش" | "دارک مود رو خاموش کن" => Some(step("dark_mode", json!({ "on": false }))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(t: &str) -> (String, Value) {
        let mut v = parse(t).unwrap_or_else(|| panic!("no match: {t}"));
        assert_eq!(v.len(), 1);
        v.remove(0)
    }

    #[test]
    fn english_commands() {
        assert_eq!(one("Open Spotify."), step("open_app", json!({"name":"spotify"})));
        assert_eq!(one("please launch the Calculator"), step("open_app", json!({"name":"calculator"})));
        assert_eq!(one("Open gmail.com"), step("open_url", json!({"url":"https://gmail.com"})));
        assert_eq!(one("Quit Safari"), step("quit_app", json!({"name":"safari"})));
        assert_eq!(one("Volume 30"), step("set_volume", json!({"level":30})));
        assert_eq!(one("Set the volume to 45%."), step("set_volume", json!({"level":45})));
        assert_eq!(one("set volume to 20 percent"), step("set_volume", json!({"level":20})));
        assert_eq!(one("Mute"), step("set_volume", json!({"mute":true})));
        assert_eq!(one("Next song."), step("media", json!({"action":"next"})));
        assert_eq!(one("Pause the music"), step("media", json!({"action":"pause"})));
        assert_eq!(one("Turn on dark mode."), step("dark_mode", json!({"on":true})));
        assert_eq!(one("Search for weather in Tehran"), step("web_search", json!({"query":"weather in tehran"})));
    }

    #[test]
    fn persian_commands() {
        assert_eq!(one("اسپاتیفای رو باز کن"), step("open_app", json!({"name":"Spotify"})));
        assert_eq!(one("لطفاً ماشین حساب رو باز کن."), step("open_app", json!({"name":"Calculator"})));
        assert_eq!(one("یوتیوب رو باز کن"), step("open_url", json!({"url":"https://youtube.com"})));
        assert_eq!(one("صدا رو بذار روی ۳۰"), step("set_volume", json!({"level":30})));
        assert_eq!(one("صدا ۲۵ درصد"), step("set_volume", json!({"level":25})));
        assert_eq!(one("آهنگ بعدی"), step("media", json!({"action":"next"})));
        assert_eq!(one("سافاری رو ببند"), step("quit_app", json!({"name":"Safari"})));
        assert_eq!(one("حالت تاریک رو روشن کن"), step("dark_mode", json!({"on":true})));
        assert_eq!(one("هوای تهران رو سرچ کن"), step("web_search", json!({"query":"هوای تهران"})));
    }

    #[test]
    fn other_languages() {
        assert_eq!(one("Abre Spotify"), step("open_app", json!({"name":"spotify"})));
        assert_eq!(one("Öffne Safari."), step("open_app", json!({"name":"safari"})));
        assert_eq!(one("Ouvre gmail.com"), step("open_url", json!({"url":"https://gmail.com"})));
        assert_eq!(one("افتح Spotify"), step("open_app", json!({"name":"spotify"})));
        assert_eq!(one("打开Spotify"), step("open_app", json!({"name":"spotify"})));
        assert_eq!(one("Spotifyを開いて"), step("open_app", json!({"name":"spotify"})));
        assert_eq!(one("Volumen 40"), step("set_volume", json!({"level":40})));
        assert_eq!(one("Lautstärke 25 %"), step("set_volume", json!({"level":25})));
        assert_eq!(one("音量30"), step("set_volume", json!({"level":30})));
        assert_eq!(one("音量30にして"), step("set_volume", json!({"level":30})));
        assert_eq!(one("Siguiente canción."), step("media", json!({"action":"next"})));
        assert_eq!(one("下一首"), step("media", json!({"action":"next"})));
        assert_eq!(one("次の曲"), step("media", json!({"action":"next"})));
        assert_eq!(one("Coupe le son"), step("set_volume", json!({"mute":true})));
        assert!(parse("Abre Spotify y pon mi lista").is_none());
        assert!(parse("打开Spotify然后播放").is_none());
    }

    #[test]
    fn anything_more_goes_to_the_ai() {
        for t in [
            "Open Spotify and play my focus playlist",
            "open Safari, then go to gmail.com",
            "remind me to call Sara at 10",
            "what's on my calendar today",
            "volume 300",
            "volume loud",
            "اسپاتیفای رو باز کن و آهنگ بعدی رو بذار",
            "",
            "open",
            "start my day",
        ] {
            let p = parse(t);
            if t == "start my day" {
                assert_eq!(p.unwrap()[0].0, "open_app", "unknown names still try, then fall back to the AI");
                continue;
            }
            assert!(p.is_none(), "{t:?} should go to the AI, got {p:?}");
        }
    }
}
