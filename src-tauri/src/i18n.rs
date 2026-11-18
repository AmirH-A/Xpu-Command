//! The app's own words in the user's language: the pill (hints, steps, results, errors) and what it
//! says aloud. The language is the one chosen in Settings (or the Mac's, for "system").
//!
//! Every message is written in English in the code, wrapped in `tr` (or `trf` with `{name}`
//! placeholders); the tables below map it to each language. A test checks that every wrapped
//! message has a translation in every language with the same placeholders.
use std::collections::HashMap;
use std::sync::OnceLock;

pub const LANGS: [&str; 8] = ["en", "fa", "es", "de", "fr", "ar", "zh", "ja"];

static CURRENT: parking_lot::RwLock<&'static str> = parking_lot::RwLock::new("en");

/// Sets the language from the Settings value ("system" or a code).
pub fn set_language(ui_language: &str) {
    let want = if ui_language == "system" { system_language() } else { ui_language.to_string() };
    *CURRENT.write() = LANGS.iter().find(|l| **l == want).copied().unwrap_or("en");
}

pub fn language() -> &'static str {
    *CURRENT.read()
}

/// The Mac's first preferred language that the app speaks, else English.
#[cfg(target_os = "macos")]
fn system_language() -> String {
    use core_foundation::array::CFArray;
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> core_foundation::array::CFArrayRef;
    }
    let raw = unsafe { CFLocaleCopyPreferredLanguages() };
    if raw.is_null() {
        return "en".into();
    }
    let langs: CFArray<CFString> = unsafe { CFArray::wrap_under_create_rule(raw) };
    langs.iter().map(|l| l.to_string()[..2.min(l.to_string().len())].to_lowercase()).find(|c| LANGS.contains(&c.as_str())).unwrap_or_else(|| "en".into())
}

/// The session's language (LANGUAGE, LC_ALL, LC_MESSAGES, LANG, in that order), else English.
#[cfg(target_os = "linux")]
fn system_language() -> String {
    for var in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(v) = std::env::var(var) else { continue };
        for part in v.split(':') {
            let code: String = part.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_lowercase();
            if LANGS.contains(&code.as_str()) {
                return code;
            }
        }
    }
    "en".into()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn system_language() -> String {
    "en".into()
}

/// `en` in the current language.
pub fn tr(en: &'static str) -> &'static str {
    lookup(language(), en)
}

/// `en` in the current language, with `{name}` placeholders filled in.
pub fn trf(en: &'static str, args: &[(&str, &str)]) -> String {
    fill(lookup(language(), en), args)
}

fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

fn lookup(lang: &'static str, en: &'static str) -> &'static str {
    static TABLES: OnceLock<HashMap<&'static str, HashMap<&'static str, &'static str>>> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        [("fa", FA), ("es", ES), ("de", DE), ("fr", FR), ("ar", AR), ("zh", ZH), ("ja", JA)].into_iter().map(|(l, t)| (l, t.iter().copied().collect())).collect()
    });
    let s = tables.get(lang).and_then(|t| t.get(en)).copied().unwrap_or(en);
    #[cfg(target_os = "linux")]
    return for_linux(lang, s);
    #[cfg(not(target_os = "linux"))]
    s
}

/// `s` with the computer named as on Linux, made once per message and kept.
#[cfg(target_os = "linux")]
fn for_linux(lang: &'static str, s: &'static str) -> &'static str {
    static CACHE: OnceLock<parking_lot::Mutex<HashMap<(&'static str, &'static str), &'static str>>> = OnceLock::new();
    let mut c = CACHE.get_or_init(Default::default).lock();
    c.entry((lang, s)).or_insert_with(|| {
        let changed = computer_words(lang, s);
        if changed == s {
            s
        } else {
            Box::leak(changed.into_boxed_str())
        }
    })
}

#[cfg(any(target_os = "linux", test))]
/// Replaces `word` where it stands as a word of its own (`letter` tells what continues a word);
/// with `eat_spaces`, one space on each side goes too (Chinese and Japanese put none there).
fn replace_word(s: &str, word: &str, with: &str, letter: impl Fn(char) -> bool, eat_spaces: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (i, _) in s.match_indices(word) {
        if i < last {
            continue;
        }
        let before = s[..i].chars().next_back();
        let after = s[i + word.len()..].chars().next();
        if before.is_some_and(&letter) || after.is_some_and(&letter) {
            continue;
        }
        let mut head = &s[last..i];
        if eat_spaces && head.ends_with(' ') {
            head = &head[..head.len() - 1];
        }
        out.push_str(head);
        out.push_str(with);
        last = i + word.len();
        if eat_spaces && s[last..].starts_with(' ') {
            last += 1;
        }
    }
    out.push_str(&s[last..]);
    out
}

#[cfg(any(target_os = "linux", test))]
/// The Mac's names in a message, as Linux would say them: "this Mac" → "this computer", in every
/// language the app speaks (also "System Settings", the menu-bar icon and the keychain in English).
pub fn computer_words(lang: &str, s: &str) -> String {
    let latin = |c: char| c.is_alphanumeric();
    let arabic = |c: char| ('\u{0600}'..='\u{06FF}').contains(&c) || c == '\u{200C}';
    match lang {
        "fa" => replace_word(s, "مک", "کامپیوتر", arabic, false),
        "ar" => s.replace("الماك", "الحاسوب"),
        "es" => replace_word(s, "Mac", "ordenador", latin, false),
        "de" => replace_word(s, "Mac", "Computer", latin, false),
        "fr" => replace_word(&replace_word(&replace_word(s, "ce Mac", "cet ordinateur", latin, false), "le Mac", "l’ordinateur", latin, false), "Mac", "ordinateur", latin, false),
        "zh" => replace_word(s, "Mac", "电脑", latin, true),
        "ja" => replace_word(s, "Mac", "パソコン", latin, true),
        _ => replace_word(&s.replace("System Settings", "Settings").replace("menu-bar icon", "tray icon").replace("Keychain", "keyring").replace("keychain", "keyring"), "Mac", "computer", latin, false),
    }
}

type Table = &'static [(&'static str, &'static str)];

const FA: Table = &[
    ("Listening", "در حال شنیدن"), ("Transcribing", "تبدیل به متن"), ("Typing", "در حال تایپ"), ("Working", "در حال انجام"), ("Done", "انجام شد"), ("Problem", "مشکل"), ("About to", "در آستانهٔ"), ("Allow?", "اجازه؟"),
    ("Listening… release to send", "گوش می‌دهم… رها کنید تا بفرستم"),
    ("Listening… pause or click to send", "گوش می‌دهم… مکث کنید یا کلیک کنید تا بفرستم"),
    ("Listening… click to send", "گوش می‌دهم… کلیک کنید تا بفرستم"),
    ("Listening… pause or knock to send", "گوش می‌دهم… مکث کنید یا دو بار بزنید تا بفرستم"),
    ("Listening… knock twice to send", "گوش می‌دهم… دو بار بزنید تا بفرستم"),
    ("Listening… pause or press your AirPods to send", "گوش می‌دهم… مکث کنید یا ایرپاد را فشار دهید تا بفرستم"),
    ("Listening… press your AirPods to send", "گوش می‌دهم… ایرپاد را فشار دهید تا بفرستم"),
    ("Listening… pause to send", "گوش می‌دهم… مکث کنید تا بفرستم"),
    ("Listening for your reply… (or say “that's all”)", "منتظر جواب شما هستم… (یا بگویید «کافیه»)"),
    ("Transcribing…", "در حال تبدیل به متن…"),
    ("Working…", "در حال انجام…"),
    ("Rewriting…", "در حال بازنویسی…"),
    ("Didn't hear anything", "چیزی نشنیدم"),
    ("Didn't catch that", "متوجه نشدم"),
    ("No sound from the microphone. Turn on Xpu Command in System Settings › Privacy & Security › Microphone.", "صدایی از میکروفون نمی‌آید. در System Settings › Privacy & Security › Microphone، Xpu Command را روشن کنید."),
    ("No sound from the microphone. Check microphone access and the input device in Settings.", "صدایی از میکروفون نمی‌آید. دسترسی میکروفون و ورودی صدا را در تنظیمات بررسی کنید."),
    ("This speech model only understands English. In Settings › Speech to text, pick a multilingual model (Small is best for Persian).", "این مدل گفتار فقط انگلیسی می‌فهمد. در تنظیمات › گفتار، یک مدل چندزبانه انتخاب کنید (برای فارسی Small بهترین است)."),
    ("Press or knock to cancel", "برای لغو، فشار دهید یا دو بار بزنید"),
    ("Press or knock to allow", "برای اجازه، فشار دهید یا دو بار بزنید"),
    ("{what}? Press or knock to allow.", "{what}؟ برای اجازه، فشار دهید یا دو بار بزنید."),
    ("This {danger}: say “yes” to allow", "این کار {danger}: برای اجازه بگویید «بله»"),
    ("Say yes or no, or press to allow", "بگویید بله یا نه، یا برای اجازه فشار دهید"),
    ("{agent} wants to {what}", "{agent} می‌خواهد {what}"),
    ("{agent} wants to {what}. This {danger}. Say yes to allow, or no.", "{agent} می‌خواهد {what}. این کار {danger}. برای اجازه بگویید بله، وگرنه نه."),
    ("{agent} wants to {what}. Say yes or no.", "{agent} می‌خواهد {what}. بگویید بله یا نه."),
    ("A press can't allow this one: say “yes”", "این یکی با فشار دادن مجاز نمی‌شود: بگویید «بله»"),
    ("Sorry: yes or no?", "ببخشید: بله یا نه؟"),
    ("Sorry, yes or no?", "ببخشید، بله یا نه؟"),
    ("Stopped the agent", "عامل متوقف شد"),
    ("Nothing to stop", "چیزی برای توقف نیست"),
    ("New conversation: the next request starts fresh", "گفت‌وگوی تازه: درخواست بعدی از نو شروع می‌شود"),
    ("Removed it", "پاکش کردم"),
    ("Routine: {name}", "روتین: {name}"),
    ("Ran {name}: 1 step", "{name} اجرا شد: ۱ مرحله"),
    ("Ran {name}: {count} steps", "{name} اجرا شد: {count} مرحله"),
    ("Stopped {name} after {done} of {total} steps: {why}", "{name} بعد از {done} مرحله از {total} متوقف شد: {why}"),
    ("Cancelled", "لغو شد"),
    ("Undo failed", "برگرداندن انجام نشد"),
    ("Still working on the last request; say “stop” first", "هنوز روی درخواست قبلی کار می‌کنم؛ اول بگویید «بس کن»"),
    ("Still working on the last request; say “stop” to stop it", "هنوز روی درخواست قبلی کار می‌کنم؛ برای توقف بگویید «بس کن»"),
    ("The agent is still working; say “stop” to stop it", "عامل هنوز مشغول است؛ برای توقف بگویید «بس کن»"),
    ("Choose a project folder in Settings first", "اول در تنظیمات یک پوشهٔ پروژه انتخاب کنید"),
    ("{name} · continuing", "{name} · ادامه"),
    ("Undo", "برگرداندن"), ("Dictation", "دیکته"), ("Routine", "روتین"),
    ("Nothing to undo", "چیزی برای برگرداندن نیست"),
    ("Undid: {items}", "برگردانده شد: {items}"),
    ("Couldn't undo: {items}", "برگردانده نشد: {items}"),
    ("Undid: {items}. Couldn't undo: {failed}", "برگردانده شد: {items}. برگردانده نشد: {failed}"),
    ("Claude Code isn't installed. Install it and sign in, then try again. Settings › Health shows how.", "Claude Code نصب نیست. نصبش کنید و وارد شوید، بعد دوباره امتحان کنید. تنظیمات › سلامت راهش را نشان می‌دهد."),
    ("No microphone found. Connect one, or pick another in Settings › Speech.", "میکروفونی پیدا نشد. یکی وصل کنید یا در تنظیمات › گفتار میکروفون دیگری انتخاب کنید."),
    ("The speech model isn't downloaded yet. Download it in Settings › Get started.", "مدل گفتار هنوز دانلود نشده. در تنظیمات › شروع دانلودش کنید."),
    ("That took too long and was stopped. Try a shorter request.", "خیلی طول کشید و متوقف شد. درخواست کوتاه‌تری امتحان کنید."),
    ("Can't reach the internet. Check your connection, or switch Speech to On this Mac.", "به اینترنت وصل نمی‌شوم. اتصال را بررسی کنید یا گفتار را روی «روی همین مک» بگذارید."),
    ("Cloud speech needs an OpenAI key. Add one in Settings › Speech, or switch to On this Mac.", "گفتار ابری کلید OpenAI لازم دارد. در تنظیمات › گفتار اضافه‌اش کنید یا به «روی همین مک» بروید."),
    ("Claude Code isn't signed in for this profile: run it in a terminal (e.g. claude-work) and use /login, or pick another profile in Settings", "Claude Code در این پروفایل وارد نشده: آن را در ترمینال اجرا کنید (مثلاً claude-work) و ‎/login را بزنید، یا در تنظیمات پروفایل دیگری انتخاب کنید"),
    ("Codex isn't signed in: run `codex login` in a terminal", "Codex وارد نشده: در ترمینال ‎`codex login` را اجرا کنید"),
    ("Codex is rate limited; try again shortly", "Codex به سقف درخواست رسیده؛ کمی بعد دوباره امتحان کنید"),
    ("Let the assistant see {app} for this request", "اجازه می‌دهید دستیار برای این درخواست {app} را ببیند"),
    ("Let the assistant control {app} for this request", "اجازه می‌دهید دستیار برای این درخواست {app} را کنترل کند"),
    ("The app I typed into isn't in front any more", "برنامه‌ای که در آن تایپ کردم دیگر جلو نیست"),
    ("I can't find what I typed in this field any more", "دیگر متنی را که تایپ کردم در این کادر پیدا نمی‌کنم"),
    ("I couldn't select what I typed, so I left it as it is", "نتوانستم متن تایپ‌شده را انتخاب کنم، پس دست نزدم"),
    ("This app doesn't show me its text, so I can only edit right after typing", "این برنامه متنش را به من نشان نمی‌دهد، پس فقط بلافاصله بعد از تایپ می‌توانم ویرایش کنم"),
    ("Editing by voice uses Claude Code, which isn't installed", "ویرایش با صدا به Claude Code نیاز دارد که نصب نیست"),
    ("it needs a speech model on this Mac: download one in Speech", "به یک مدل گفتار روی همین مک نیاز دارد: در بخش گفتار دانلودش کنید"),
    ("the microphone stopped", "میکروفون قطع شد"),
    ("Reading {file}", "در حال خواندن {file}"), ("Editing {file}", "در حال ویرایش {file}"), ("Searching the project", "در حال جست‌وجو در پروژه"), ("Planning", "در حال برنامه‌ریزی"),
    ("Delegating a subtask", "سپردن یک کار فرعی"), ("Using {tool}", "در حال استفاده از {tool}"), ("Running: {command}", "در حال اجرا: {command}"), ("Editing files", "در حال ویرایش فایل‌ها"), ("Searching the web", "در حال جست‌وجو در وب"),
    ("run {command}", "{command} را اجرا کند"), ("open {host}", "{host} را باز کند"), ("search the web for “{query}”", "در وب «{query}» را جست‌وجو کند"), ("edit {file}", "{file} را ویرایش کند"), ("read {file}", "{file} را بخواند"), ("use {tool}", "از {tool} استفاده کند"),
    ("deletes files", "فایل‌ها را پاک می‌کند"), ("runs as administrator", "با دسترسی مدیر اجرا می‌شود"), ("downloads and runs code", "کدی را دانلود و اجرا می‌کند"), ("pushes to a remote repository", "به مخزن راه دور push می‌کند"),
    ("discards work", "کارِ انجام‌شده را دور می‌ریزد"), ("publishes a package", "یک بسته منتشر می‌کند"), ("can erase a disk", "می‌تواند یک دیسک را پاک کند"), ("stops the system or other programs", "سیستم یا برنامه‌های دیگر را متوقف می‌کند"),
    ("changes permissions widely", "دسترسی‌ها را گسترده تغییر می‌دهد"), ("deletes database data", "داده‌های پایگاه داده را پاک می‌کند"), ("deletes infrastructure", "زیرساخت را حذف می‌کند"), ("can freeze the computer", "می‌تواند کامپیوتر را قفل کند"),
    ("Looking at your apps", "نگاه به برنامه‌های شما"), ("Opening {name}", "باز کردن {name}"), ("Quit {name}", "بستن {name}"), ("the page", "صفحه"), ("Searching the web for “{query}”", "جست‌وجوی «{query}» در وب"),
    ("Playing", "پخش"), ("Pausing", "توقف موقت"), ("Next track", "آهنگ بعدی"), ("Previous track", "آهنگ قبلی"), ("Playing “{query}”", "پخش «{query}»"), ("Muting", "بی‌صدا کردن"), ("Unmuting", "باصدا کردن"),
    ("Volume {level}%", "صدا {level}٪"), ("Dark mode on", "حالت تیره روشن"), ("Dark mode off", "حالت تیره خاموش"), ("Looking at your Shortcuts", "نگاه به میانبرهای شما"), ("Run the shortcut “{name}”", "اجرای میانبر «{name}»"),
    ("Reminder: {title} · {when}", "یادآور: {title} · {when}"), ("Reminder: {title}", "یادآور: {title}"), ("Event: {title} · {when}", "رویداد: {title} · {when}"), ("Note: {title}", "یادداشت: {title}"),
    ("Draft email to {to} +{more}: {subject}", "پیش‌نویس ایمیل به {to} و {more} نفر دیگر: {subject}"), ("Draft email to {to}: {subject}", "پیش‌نویس ایمیل به {to}: {subject}"), ("Draft email: {subject}", "پیش‌نویس ایمیل: {subject}"),
    ("Looking at open windows", "نگاه به پنجره‌های باز"), ("Reading {app}", "خواندن {app}"), ("Looking at {app}", "نگاه به {app}"),
    ("Clicking", "کلیک روی"), ("Double-clicking", "دوبار کلیک روی"), ("Right-clicking", "کلیک راست روی"), ("Pointing at", "اشاره به"),
    ("{verb} control {n}", "{verb} کنترل {n}"), ("{verb} ({x}, {y}) in {app}", "{verb} ‎({x}, {y})‎ در {app}"), ("{verb} page item {n}", "{verb} مورد {n} صفحه"), ("{verb} {label} in {app}", "{verb} {label} در {app}"),
    ("Typing “{text}” into control {n}", "تایپ «{text}» در کنترل {n}"), ("Typing “{text}” into control {n} and pressing Return", "تایپ «{text}» در کنترل {n} و زدن Return"),
    ("Pressing {keys} in {app}", "زدن {keys} در {app}"), ("Menu {path} in {app}", "منوی {path} در {app}"), ("Scrolling down in {app}", "پیمایش به پایین در {app}"), ("Scrolling up in {app}", "پیمایش به بالا در {app}"),
    ("Dragging control {from} onto control {to}", "کشیدن کنترل {from} روی کنترل {to}"), ("Waiting for “{text}” in {app}", "منتظر «{text}» در {app}"), ("Waiting {seconds} s for {app}", "{seconds} ثانیه صبر برای {app}"),
    ("Reading the page in {app}", "خواندن صفحه در {app}"), ("Filling page item {n} with “{text}”", "پر کردن مورد {n} صفحه با «{text}»"), ("Filling page item {n} with “{text}” and submitting", "پر کردن مورد {n} صفحه با «{text}» و ارسال"),
    ("Copying “{text}”", "کپی «{text}»"), ("Reading the clipboard", "خواندن کلیپ‌بورد"), ("Finding files named “{query}”", "پیدا کردن فایل‌هایی به نام «{query}»"), ("Opening {file} in {app}", "باز کردن {file} در {app}"),
    ("Move {items} to {dest}", "انتقال {items} به {dest}"), ("Rename {file} to {name}", "تغییر نام {file} به {name}"), ("Move {items} to the Trash", "انتقال {items} به سطل زباله"), ("New folder “{name}” in {parent}", "پوشهٔ تازهٔ «{name}» در {parent}"),
    ("{count} items", "{count} مورد"), ("Remembering: {text}", "به خاطر سپردن: {text}"), ("Forgetting “{text}”", "فراموش کردن «{text}»"), ("Save the routine “{name}”: {steps}", "ذخیرهٔ روتین «{name}»: {steps}"),
    ("Typing “{text}” into {label} in {app}", "تایپ «{text}» در {label} در {app}"), ("Typing “{text}” into {label} in {app}, then Return", "تایپ «{text}» در {label} در {app} و بعد Return"),
    ("Filling {label} in {app} with “{text}”", "پر کردن {label} در {app} با «{text}»"), ("Filling {label} in {app} with “{text}”, then submitting", "پر کردن {label} در {app} با «{text}» و بعد ارسال"), ("Dragging {label} in {app}", "کشیدن {label} در {app}"),
    ("Say “always” to stop asking, “yes” for now, or press", "بگویید «همیشه» تا دیگر نپرسم، «بله» برای همین بار، یا فشار دهید"),
    ("{question}? Say always, yes, or no.", "{question}؟ بگویید همیشه، بله یا نه."),
    ("{app} is allowed from now on. Change it in Settings › Apps.", "از این به بعد {app} مجاز است. در تنظیمات › برنامه‌ها عوضش کنید."),
    ("What you said is kept in Settings › History.", "آنچه گفتید در تنظیمات › تاریخچه نگه داشته شده است."),
    ("That selection is too long to rewrite by voice; select a shorter part", "این انتخاب برای بازنویسی با صدا خیلی طولانی است؛ بخش کوتاه‌تری را انتخاب کنید"),
    ("The selection changed, so I left it as it is", "متن انتخاب‌شده تغییر کرد، پس دست نزدم"),
    ("This app doesn't let me replace text with several lines; select one paragraph", "این برنامه اجازه نمی‌دهد متن چندخطی را جایگزین کنم؛ یک پاراگراف را انتخاب کنید"),
    ("“{word}…” is too long for a word (at most {max} characters)", "«{word}…» برای یک واژه خیلی طولانی است (حداکثر {max} نویسه)"),
    ("At most {max} words", "حداکثر {max} واژه"),
    ("A replacement needs words to replace (“{to}”)", "جایگزینی باید واژه‌ای برای جایگزین‌کردن داشته باشد («{to}»)"),
    ("The replacement for “{from}” is too long", "جایگزینیِ «{from}» خیلی طولانی است"),
    ("At most {max} replacements", "حداکثر {max} جایگزینی"),
    ("A snippet’s phrase needs 1 to 6 words (“{phrase}”)", "عبارت میان‌بُر باید ۱ تا ۶ واژه باشد («{phrase}»)"),
    ("The snippet “{phrase}” needs text, at most {max} characters", "میان‌بُر «{phrase}» به متن نیاز دارد، حداکثر {max} نویسه"),
    ("Two snippets have the phrase “{phrase}”", "دو میان‌بُر عبارت یکسان «{phrase}» دارند"),
    ("At most {max} snippets", "حداکثر {max} میان‌بُر"),
    ("That suggestion is gone", "آن پیشنهاد دیگر وجود ندارد"),
];

const ES: Table = &[
    ("Listening", "Escuchando"), ("Transcribing", "Transcribiendo"), ("Typing", "Escribiendo"), ("Working", "Trabajando"), ("Done", "Hecho"), ("Problem", "Problema"), ("About to", "A punto de"), ("Allow?", "¿Permitir?"),
    ("Listening… release to send", "Escuchando… suelta para enviar"),
    ("Listening… pause or click to send", "Escuchando… haz una pausa o clic para enviar"),
    ("Listening… click to send", "Escuchando… haz clic para enviar"),
    ("Listening… pause or knock to send", "Escuchando… haz una pausa o da dos golpes para enviar"),
    ("Listening… knock twice to send", "Escuchando… da dos golpes para enviar"),
    ("Listening… pause or press your AirPods to send", "Escuchando… haz una pausa o pulsa tus AirPods para enviar"),
    ("Listening… press your AirPods to send", "Escuchando… pulsa tus AirPods para enviar"),
    ("Listening… pause to send", "Escuchando… haz una pausa para enviar"),
    ("Listening for your reply… (or say “that's all”)", "Esperando tu respuesta… (o di «eso es todo»)"),
    ("Transcribing…", "Transcribiendo…"), ("Working…", "Trabajando…"), ("Rewriting…", "Reescribiendo…"),
    ("Didn't hear anything", "No oí nada"), ("Didn't catch that", "No lo entendí"),
    ("No sound from the microphone. Turn on Xpu Command in System Settings › Privacy & Security › Microphone.", "El micrófono no capta sonido. Activa Xpu Command en Ajustes del Sistema › Privacidad y seguridad › Micrófono."),
    ("No sound from the microphone. Check microphone access and the input device in Settings.", "El micrófono no capta sonido. Revisa el acceso al micrófono y el dispositivo de entrada en Ajustes."),
    ("This speech model only understands English. In Settings › Speech to text, pick a multilingual model (Small is best for Persian).", "Este modelo de voz solo entiende inglés. En Ajustes › Voz a texto, elige un modelo multilingüe (Small es el mejor para persa)."),
    ("Press or knock to cancel", "Pulsa o da dos golpes para cancelar"), ("Press or knock to allow", "Pulsa o da dos golpes para permitir"),
    ("{what}? Press or knock to allow.", "¿{what}? Pulsa o da dos golpes para permitir."),
    ("This {danger}: say “yes” to allow", "Esto {danger}: di «sí» para permitir"), ("Say yes or no, or press to allow", "Di sí o no, o pulsa para permitir"),
    ("{agent} wants to {what}", "{agent} quiere {what}"),
    ("{agent} wants to {what}. This {danger}. Say yes to allow, or no.", "{agent} quiere {what}. Esto {danger}. Di sí para permitir, o no."),
    ("{agent} wants to {what}. Say yes or no.", "{agent} quiere {what}. Di sí o no."),
    ("A press can't allow this one: say “yes”", "Esto no se permite pulsando: di «sí»"), ("Sorry: yes or no?", "Perdón: ¿sí o no?"), ("Sorry, yes or no?", "Perdón, ¿sí o no?"),
    ("Stopped the agent", "Agente detenido"), ("Nothing to stop", "Nada que detener"), ("New conversation: the next request starts fresh", "Conversación nueva: la próxima petición empieza de cero"), ("Removed it", "Borrado"),
    ("Routine: {name}", "Rutina: {name}"), ("Ran {name}: 1 step", "{name} ejecutada: 1 paso"), ("Ran {name}: {count} steps", "{name} ejecutada: {count} pasos"),
    ("Stopped {name} after {done} of {total} steps: {why}", "{name} se detuvo tras {done} de {total} pasos: {why}"), ("Cancelled", "Cancelado"), ("Undo failed", "No se pudo deshacer"),
    ("Still working on the last request; say “stop” first", "Sigo con la petición anterior; di «para» primero"), ("Still working on the last request; say “stop” to stop it", "Sigo con la petición anterior; di «para» para detenerla"),
    ("The agent is still working; say “stop” to stop it", "El agente sigue trabajando; di «para» para detenerlo"), ("Choose a project folder in Settings first", "Primero elige una carpeta de proyecto en Ajustes"),
    ("{name} · continuing", "{name} · continuando"), ("Undo", "Deshacer"), ("Dictation", "Dictado"), ("Routine", "Rutina"),
    ("Nothing to undo", "Nada que deshacer"), ("Undid: {items}", "Deshecho: {items}"), ("Couldn't undo: {items}", "No se pudo deshacer: {items}"), ("Undid: {items}. Couldn't undo: {failed}", "Deshecho: {items}. No se pudo deshacer: {failed}"),
    ("Claude Code isn't installed. Install it and sign in, then try again. Settings › Health shows how.", "Claude Code no está instalado. Instálalo e inicia sesión, y vuelve a intentarlo. Ajustes › Estado muestra cómo."),
    ("No microphone found. Connect one, or pick another in Settings › Speech.", "No hay micrófono. Conecta uno o elige otro en Ajustes › Voz."),
    ("The speech model isn't downloaded yet. Download it in Settings › Get started.", "El modelo de voz aún no está descargado. Descárgalo en Ajustes › Primeros pasos."),
    ("That took too long and was stopped. Try a shorter request.", "Tardó demasiado y se detuvo. Prueba con una petición más corta."),
    ("Can't reach the internet. Check your connection, or switch Speech to On this Mac.", "No hay conexión a internet. Revisa tu conexión o cambia Voz a En este Mac."),
    ("Cloud speech needs an OpenAI key. Add one in Settings › Speech, or switch to On this Mac.", "La voz en la nube necesita una clave de OpenAI. Añádela en Ajustes › Voz o cambia a En este Mac."),
    ("Claude Code isn't signed in for this profile: run it in a terminal (e.g. claude-work) and use /login, or pick another profile in Settings", "Claude Code no tiene sesión en este perfil: ábrelo en un terminal (p. ej. claude-work) y usa /login, o elige otro perfil en Ajustes"),
    ("Codex isn't signed in: run `codex login` in a terminal", "Codex no tiene sesión: ejecuta `codex login` en un terminal"), ("Codex is rate limited; try again shortly", "Codex alcanzó su límite; inténtalo en un momento"),
    ("Let the assistant see {app} for this request", "¿Dejar que el asistente vea {app} en esta petición"), ("Let the assistant control {app} for this request", "¿Dejar que el asistente controle {app} en esta petición"),
    ("The app I typed into isn't in front any more", "La app donde escribí ya no está delante"), ("I can't find what I typed in this field any more", "Ya no encuentro lo que escribí en este campo"),
    ("I couldn't select what I typed, so I left it as it is", "No pude seleccionar lo que escribí, así que lo dejé como estaba"),
    ("This app doesn't show me its text, so I can only edit right after typing", "Esta app no me muestra su texto, así que solo puedo editar justo después de escribir"),
    ("Editing by voice uses Claude Code, which isn't installed", "Editar por voz usa Claude Code, que no está instalado"),
    ("it needs a speech model on this Mac: download one in Speech", "necesita un modelo de voz en este Mac: descarga uno en Voz"), ("the microphone stopped", "el micrófono se detuvo"),
    ("Reading {file}", "Leyendo {file}"), ("Editing {file}", "Editando {file}"), ("Searching the project", "Buscando en el proyecto"), ("Planning", "Planificando"),
    ("Delegating a subtask", "Delegando una subtarea"), ("Using {tool}", "Usando {tool}"), ("Running: {command}", "Ejecutando: {command}"), ("Editing files", "Editando archivos"), ("Searching the web", "Buscando en la web"),
    ("run {command}", "ejecutar {command}"), ("open {host}", "abrir {host}"), ("search the web for “{query}”", "buscar «{query}» en la web"), ("edit {file}", "editar {file}"), ("read {file}", "leer {file}"), ("use {tool}", "usar {tool}"),
    ("deletes files", "borra archivos"), ("runs as administrator", "se ejecuta como administrador"), ("downloads and runs code", "descarga y ejecuta código"), ("pushes to a remote repository", "sube cambios a un repositorio remoto"),
    ("discards work", "descarta trabajo"), ("publishes a package", "publica un paquete"), ("can erase a disk", "puede borrar un disco"), ("stops the system or other programs", "detiene el sistema u otros programas"),
    ("changes permissions widely", "cambia permisos de forma amplia"), ("deletes database data", "borra datos de la base de datos"), ("deletes infrastructure", "elimina infraestructura"), ("can freeze the computer", "puede bloquear el ordenador"),
    ("Looking at your apps", "Mirando tus apps"), ("Opening {name}", "Abriendo {name}"), ("Quit {name}", "Cerrar {name}"), ("the page", "la página"), ("Searching the web for “{query}”", "Buscando «{query}» en la web"),
    ("Playing", "Reproduciendo"), ("Pausing", "Pausando"), ("Next track", "Siguiente canción"), ("Previous track", "Canción anterior"), ("Playing “{query}”", "Reproduciendo «{query}»"), ("Muting", "Silenciando"), ("Unmuting", "Activando el sonido"),
    ("Volume {level}%", "Volumen {level} %"), ("Dark mode on", "Modo oscuro activado"), ("Dark mode off", "Modo oscuro desactivado"), ("Looking at your Shortcuts", "Mirando tus Atajos"), ("Run the shortcut “{name}”", "Ejecutar el atajo «{name}»"),
    ("Reminder: {title} · {when}", "Recordatorio: {title} · {when}"), ("Reminder: {title}", "Recordatorio: {title}"), ("Event: {title} · {when}", "Evento: {title} · {when}"), ("Note: {title}", "Nota: {title}"),
    ("Draft email to {to} +{more}: {subject}", "Borrador para {to} y {more} más: {subject}"), ("Draft email to {to}: {subject}", "Borrador para {to}: {subject}"), ("Draft email: {subject}", "Borrador: {subject}"),
    ("Looking at open windows", "Mirando las ventanas abiertas"), ("Reading {app}", "Leyendo {app}"), ("Looking at {app}", "Mirando {app}"),
    ("Clicking", "Clic en"), ("Double-clicking", "Doble clic en"), ("Right-clicking", "Clic derecho en"), ("Pointing at", "Señalando"),
    ("{verb} control {n}", "{verb} el control {n}"), ("{verb} ({x}, {y}) in {app}", "{verb} ({x}, {y}) en {app}"), ("{verb} page item {n}", "{verb} el elemento {n} de la página"), ("{verb} {label} in {app}", "{verb} {label} en {app}"),
    ("Typing “{text}” into control {n}", "Escribiendo «{text}» en el control {n}"), ("Typing “{text}” into control {n} and pressing Return", "Escribiendo «{text}» en el control {n} y pulsando Intro"),
    ("Pressing {keys} in {app}", "Pulsando {keys} en {app}"), ("Menu {path} in {app}", "Menú {path} en {app}"), ("Scrolling down in {app}", "Desplazando hacia abajo en {app}"), ("Scrolling up in {app}", "Desplazando hacia arriba en {app}"),
    ("Dragging control {from} onto control {to}", "Arrastrando el control {from} al control {to}"), ("Waiting for “{text}” in {app}", "Esperando «{text}» en {app}"), ("Waiting {seconds} s for {app}", "Esperando {seconds} s a {app}"),
    ("Reading the page in {app}", "Leyendo la página en {app}"), ("Filling page item {n} with “{text}”", "Rellenando el elemento {n} con «{text}»"), ("Filling page item {n} with “{text}” and submitting", "Rellenando el elemento {n} con «{text}» y enviando"),
    ("Copying “{text}”", "Copiando «{text}»"), ("Reading the clipboard", "Leyendo el portapapeles"), ("Finding files named “{query}”", "Buscando archivos llamados «{query}»"), ("Opening {file} in {app}", "Abriendo {file} en {app}"),
    ("Move {items} to {dest}", "Mover {items} a {dest}"), ("Rename {file} to {name}", "Renombrar {file} como {name}"), ("Move {items} to the Trash", "Mover {items} a la Papelera"), ("New folder “{name}” in {parent}", "Carpeta nueva «{name}» en {parent}"),
    ("{count} items", "{count} elementos"), ("Remembering: {text}", "Recordando: {text}"), ("Forgetting “{text}”", "Olvidando «{text}»"), ("Save the routine “{name}”: {steps}", "Guardar la rutina «{name}»: {steps}"),
    ("Typing “{text}” into {label} in {app}", "Escribiendo «{text}» en {label} de {app}"), ("Typing “{text}” into {label} in {app}, then Return", "Escribiendo «{text}» en {label} de {app} y luego Intro"),
    ("Filling {label} in {app} with “{text}”", "Rellenando {label} de {app} con «{text}»"), ("Filling {label} in {app} with “{text}”, then submitting", "Rellenando {label} de {app} con «{text}» y enviando"), ("Dragging {label} in {app}", "Arrastrando {label} en {app}"),
    ("Say “always” to stop asking, “yes” for now, or press", "Di «siempre» para no volver a preguntar, «sí» para esta vez, o pulsa"),
    ("{question}? Say always, yes, or no.", "¿{question}? Di siempre, sí o no."),
    ("{app} is allowed from now on. Change it in Settings › Apps.", "{app} queda permitido a partir de ahora. Cámbialo en Ajustes › Apps."),
    ("What you said is kept in Settings › History.", "Lo que dijiste está guardado en Ajustes › Historial."),
    ("That selection is too long to rewrite by voice; select a shorter part", "La selección es demasiado larga para reescribirla por voz; selecciona una parte más corta"),
    ("The selection changed, so I left it as it is", "La selección cambió, así que la dejé como estaba"),
    ("This app doesn't let me replace text with several lines; select one paragraph", "Esta app no me deja reemplazar texto de varias líneas; selecciona un solo párrafo"),
    ("“{word}…” is too long for a word (at most {max} characters)", "«{word}…» es demasiado larga para una palabra (máximo {max} caracteres)"),
    ("At most {max} words", "Máximo {max} palabras"),
    ("A replacement needs words to replace (“{to}”)", "Un reemplazo necesita palabras que reemplazar («{to}»)"),
    ("The replacement for “{from}” is too long", "El reemplazo de «{from}» es demasiado largo"),
    ("At most {max} replacements", "Máximo {max} reemplazos"),
    ("A snippet’s phrase needs 1 to 6 words (“{phrase}”)", "La frase de un atajo necesita de 1 a 6 palabras («{phrase}»)"),
    ("The snippet “{phrase}” needs text, at most {max} characters", "El atajo «{phrase}» necesita texto, como máximo {max} caracteres"),
    ("Two snippets have the phrase “{phrase}”", "Dos atajos tienen la frase «{phrase}»"),
    ("At most {max} snippets", "Máximo {max} atajos"),
    ("That suggestion is gone", "Esa sugerencia ya no está"),
];

const DE: Table = &[
    ("Listening", "Hört zu"), ("Transcribing", "Transkribiert"), ("Typing", "Tippt"), ("Working", "Arbeitet"), ("Done", "Fertig"), ("Problem", "Problem"), ("About to", "Gleich"), ("Allow?", "Erlauben?"),
    ("Listening… release to send", "Hört zu… loslassen zum Senden"),
    ("Listening… pause or click to send", "Hört zu… Pause oder Klick zum Senden"),
    ("Listening… click to send", "Hört zu… klicken zum Senden"),
    ("Listening… pause or knock to send", "Hört zu… Pause oder zweimal klopfen zum Senden"),
    ("Listening… knock twice to send", "Hört zu… zweimal klopfen zum Senden"),
    ("Listening… pause or press your AirPods to send", "Hört zu… Pause oder AirPods drücken zum Senden"),
    ("Listening… press your AirPods to send", "Hört zu… AirPods drücken zum Senden"),
    ("Listening… pause to send", "Hört zu… Pause zum Senden"),
    ("Listening for your reply… (or say “that's all”)", "Wartet auf deine Antwort… (oder sag „das war’s“)"),
    ("Transcribing…", "Transkribiert…"), ("Working…", "Arbeitet…"), ("Rewriting…", "Formuliert um…"),
    ("Didn't hear anything", "Nichts gehört"), ("Didn't catch that", "Nicht verstanden"),
    ("No sound from the microphone. Turn on Xpu Command in System Settings › Privacy & Security › Microphone.", "Kein Ton vom Mikrofon. Aktiviere Xpu Command unter Systemeinstellungen › Datenschutz & Sicherheit › Mikrofon."),
    ("No sound from the microphone. Check microphone access and the input device in Settings.", "Kein Ton vom Mikrofon. Prüfe den Mikrofonzugriff und das Eingabegerät in den Einstellungen."),
    ("This speech model only understands English. In Settings › Speech to text, pick a multilingual model (Small is best for Persian).", "Dieses Sprachmodell versteht nur Englisch. Wähle unter Einstellungen › Sprache zu Text ein mehrsprachiges Modell (Small ist am besten für Persisch)."),
    ("Press or knock to cancel", "Drücken oder klopfen zum Abbrechen"), ("Press or knock to allow", "Drücken oder klopfen zum Erlauben"),
    ("{what}? Press or knock to allow.", "{what}? Drücken oder klopfen zum Erlauben."),
    ("This {danger}: say “yes” to allow", "Das {danger}: sag „ja“ zum Erlauben"), ("Say yes or no, or press to allow", "Sag ja oder nein, oder drücke zum Erlauben"),
    ("{agent} wants to {what}", "{agent} möchte {what}"),
    ("{agent} wants to {what}. This {danger}. Say yes to allow, or no.", "{agent} möchte {what}. Das {danger}. Sag ja zum Erlauben, oder nein."),
    ("{agent} wants to {what}. Say yes or no.", "{agent} möchte {what}. Sag ja oder nein."),
    ("A press can't allow this one: say “yes”", "Das geht nicht per Druck: sag „ja“"), ("Sorry: yes or no?", "Entschuldigung: ja oder nein?"), ("Sorry, yes or no?", "Entschuldigung, ja oder nein?"),
    ("Stopped the agent", "Agent gestoppt"), ("Nothing to stop", "Nichts zu stoppen"), ("New conversation: the next request starts fresh", "Neues Gespräch: die nächste Anfrage beginnt von vorn"), ("Removed it", "Entfernt"),
    ("Routine: {name}", "Routine: {name}"), ("Ran {name}: 1 step", "{name} ausgeführt: 1 Schritt"), ("Ran {name}: {count} steps", "{name} ausgeführt: {count} Schritte"),
    ("Stopped {name} after {done} of {total} steps: {why}", "{name} nach {done} von {total} Schritten gestoppt: {why}"), ("Cancelled", "Abgebrochen"), ("Undo failed", "Rückgängig fehlgeschlagen"),
    ("Still working on the last request; say “stop” first", "Arbeite noch an der letzten Anfrage; sag zuerst „stopp“"), ("Still working on the last request; say “stop” to stop it", "Arbeite noch an der letzten Anfrage; sag „stopp“ zum Anhalten"),
    ("The agent is still working; say “stop” to stop it", "Der Agent arbeitet noch; sag „stopp“ zum Anhalten"), ("Choose a project folder in Settings first", "Wähle zuerst einen Projektordner in den Einstellungen"),
    ("{name} · continuing", "{name} · fortgesetzt"), ("Undo", "Rückgängig"), ("Dictation", "Diktat"), ("Routine", "Routine"),
    ("Nothing to undo", "Nichts rückgängig zu machen"), ("Undid: {items}", "Rückgängig gemacht: {items}"), ("Couldn't undo: {items}", "Nicht rückgängig: {items}"), ("Undid: {items}. Couldn't undo: {failed}", "Rückgängig gemacht: {items}. Nicht rückgängig: {failed}"),
    ("Claude Code isn't installed. Install it and sign in, then try again. Settings › Health shows how.", "Claude Code ist nicht installiert. Installiere es, melde dich an und versuche es erneut. Einstellungen › Zustand zeigt, wie."),
    ("No microphone found. Connect one, or pick another in Settings › Speech.", "Kein Mikrofon gefunden. Schließe eins an oder wähle ein anderes unter Einstellungen › Sprache."),
    ("The speech model isn't downloaded yet. Download it in Settings › Get started.", "Das Sprachmodell ist noch nicht geladen. Lade es unter Einstellungen › Erste Schritte."),
    ("That took too long and was stopped. Try a shorter request.", "Das hat zu lange gedauert und wurde gestoppt. Versuch eine kürzere Anfrage."),
    ("Can't reach the internet. Check your connection, or switch Speech to On this Mac.", "Keine Internetverbindung. Prüfe die Verbindung oder stelle Sprache auf Auf diesem Mac."),
    ("Cloud speech needs an OpenAI key. Add one in Settings › Speech, or switch to On this Mac.", "Cloud-Sprache braucht einen OpenAI-Schlüssel. Füge ihn unter Einstellungen › Sprache hinzu oder wechsle zu Auf diesem Mac."),
    ("Claude Code isn't signed in for this profile: run it in a terminal (e.g. claude-work) and use /login, or pick another profile in Settings", "Claude Code ist in diesem Profil nicht angemeldet: starte es im Terminal (z. B. claude-work) und nutze /login, oder wähle ein anderes Profil in den Einstellungen"),
    ("Codex isn't signed in: run `codex login` in a terminal", "Codex ist nicht angemeldet: führe `codex login` im Terminal aus"), ("Codex is rate limited; try again shortly", "Codex ist ausgelastet; versuch es gleich noch mal"),
    ("Let the assistant see {app} for this request", "Darf der Assistent {app} für diese Anfrage sehen"), ("Let the assistant control {app} for this request", "Darf der Assistent {app} für diese Anfrage steuern"),
    ("The app I typed into isn't in front any more", "Die App, in die ich getippt habe, ist nicht mehr vorne"), ("I can't find what I typed in this field any more", "Ich finde den getippten Text in diesem Feld nicht mehr"),
    ("I couldn't select what I typed, so I left it as it is", "Ich konnte den getippten Text nicht auswählen und habe ihn so gelassen"),
    ("This app doesn't show me its text, so I can only edit right after typing", "Diese App zeigt mir ihren Text nicht, darum kann ich nur direkt nach dem Tippen ändern"),
    ("Editing by voice uses Claude Code, which isn't installed", "Bearbeiten per Stimme nutzt Claude Code, das nicht installiert ist"),
    ("it needs a speech model on this Mac: download one in Speech", "es braucht ein Sprachmodell auf diesem Mac: lade eins unter Sprache"), ("the microphone stopped", "das Mikrofon hat aufgehört"),
    ("Reading {file}", "Liest {file}"), ("Editing {file}", "Bearbeitet {file}"), ("Searching the project", "Durchsucht das Projekt"), ("Planning", "Plant"),
    ("Delegating a subtask", "Gibt eine Teilaufgabe ab"), ("Using {tool}", "Nutzt {tool}"), ("Running: {command}", "Führt aus: {command}"), ("Editing files", "Bearbeitet Dateien"), ("Searching the web", "Sucht im Web"),
    ("run {command}", "{command} ausführen"), ("open {host}", "{host} öffnen"), ("search the web for “{query}”", "im Web nach „{query}“ suchen"), ("edit {file}", "{file} bearbeiten"), ("read {file}", "{file} lesen"), ("use {tool}", "{tool} verwenden"),
    ("deletes files", "löscht Dateien"), ("runs as administrator", "läuft als Administrator"), ("downloads and runs code", "lädt Code herunter und führt ihn aus"), ("pushes to a remote repository", "pusht in ein entferntes Repository"),
    ("discards work", "verwirft Arbeit"), ("publishes a package", "veröffentlicht ein Paket"), ("can erase a disk", "kann ein Laufwerk löschen"), ("stops the system or other programs", "stoppt das System oder andere Programme"),
    ("changes permissions widely", "ändert Rechte weitreichend"), ("deletes database data", "löscht Datenbankdaten"), ("deletes infrastructure", "löscht Infrastruktur"), ("can freeze the computer", "kann den Computer einfrieren"),
    ("Looking at your apps", "Sieht deine Apps an"), ("Opening {name}", "Öffnet {name}"), ("Quit {name}", "{name} beenden"), ("the page", "die Seite"), ("Searching the web for “{query}”", "Sucht im Web nach „{query}“"),
    ("Playing", "Spielt"), ("Pausing", "Pausiert"), ("Next track", "Nächster Titel"), ("Previous track", "Vorheriger Titel"), ("Playing “{query}”", "Spielt „{query}“"), ("Muting", "Stummschalten"), ("Unmuting", "Ton an"),
    ("Volume {level}%", "Lautstärke {level} %"), ("Dark mode on", "Dunkelmodus an"), ("Dark mode off", "Dunkelmodus aus"), ("Looking at your Shortcuts", "Sieht deine Kurzbefehle an"), ("Run the shortcut “{name}”", "Kurzbefehl „{name}“ ausführen"),
    ("Reminder: {title} · {when}", "Erinnerung: {title} · {when}"), ("Reminder: {title}", "Erinnerung: {title}"), ("Event: {title} · {when}", "Termin: {title} · {when}"), ("Note: {title}", "Notiz: {title}"),
    ("Draft email to {to} +{more}: {subject}", "E-Mail-Entwurf an {to} und {more} weitere: {subject}"), ("Draft email to {to}: {subject}", "E-Mail-Entwurf an {to}: {subject}"), ("Draft email: {subject}", "E-Mail-Entwurf: {subject}"),
    ("Looking at open windows", "Sieht offene Fenster an"), ("Reading {app}", "Liest {app}"), ("Looking at {app}", "Sieht {app} an"),
    ("Clicking", "Klickt"), ("Double-clicking", "Doppelklickt"), ("Right-clicking", "Rechtsklickt"), ("Pointing at", "Zeigt auf"),
    ("{verb} control {n}", "{verb} Element {n}"), ("{verb} ({x}, {y}) in {app}", "{verb} ({x}, {y}) in {app}"), ("{verb} page item {n}", "{verb} Seitenelement {n}"), ("{verb} {label} in {app}", "{verb} {label} in {app}"),
    ("Typing “{text}” into control {n}", "Tippt „{text}“ in Element {n}"), ("Typing “{text}” into control {n} and pressing Return", "Tippt „{text}“ in Element {n} und drückt Return"),
    ("Pressing {keys} in {app}", "Drückt {keys} in {app}"), ("Menu {path} in {app}", "Menü {path} in {app}"), ("Scrolling down in {app}", "Scrollt nach unten in {app}"), ("Scrolling up in {app}", "Scrollt nach oben in {app}"),
    ("Dragging control {from} onto control {to}", "Zieht Element {from} auf Element {to}"), ("Waiting for “{text}” in {app}", "Wartet auf „{text}“ in {app}"), ("Waiting {seconds} s for {app}", "Wartet {seconds} s auf {app}"),
    ("Reading the page in {app}", "Liest die Seite in {app}"), ("Filling page item {n} with “{text}”", "Füllt Seitenelement {n} mit „{text}“"), ("Filling page item {n} with “{text}” and submitting", "Füllt Seitenelement {n} mit „{text}“ und sendet"),
    ("Copying “{text}”", "Kopiert „{text}“"), ("Reading the clipboard", "Liest die Zwischenablage"), ("Finding files named “{query}”", "Sucht Dateien namens „{query}“"), ("Opening {file} in {app}", "Öffnet {file} in {app}"),
    ("Move {items} to {dest}", "{items} nach {dest} bewegen"), ("Rename {file} to {name}", "{file} in {name} umbenennen"), ("Move {items} to the Trash", "{items} in den Papierkorb legen"), ("New folder “{name}” in {parent}", "Neuer Ordner „{name}“ in {parent}"),
    ("{count} items", "{count} Objekte"), ("Remembering: {text}", "Merkt sich: {text}"), ("Forgetting “{text}”", "Vergisst „{text}“"), ("Save the routine “{name}”: {steps}", "Routine „{name}“ speichern: {steps}"),
    ("Typing “{text}” into {label} in {app}", "Tippt „{text}“ in {label} in {app}"), ("Typing “{text}” into {label} in {app}, then Return", "Tippt „{text}“ in {label} in {app}, dann Return"),
    ("Filling {label} in {app} with “{text}”", "Füllt {label} in {app} mit „{text}“"), ("Filling {label} in {app} with “{text}”, then submitting", "Füllt {label} in {app} mit „{text}“, dann senden"), ("Dragging {label} in {app}", "Zieht {label} in {app}"),
    ("Say “always” to stop asking, “yes” for now, or press", "Sag „immer“, damit ich nicht mehr frage, „ja“ für dieses Mal, oder drück"),
    ("{question}? Say always, yes, or no.", "{question}? Sag immer, ja oder nein."),
    ("{app} is allowed from now on. Change it in Settings › Apps.", "{app} ist ab jetzt erlaubt. Ändern unter Einstellungen › Apps."),
    ("What you said is kept in Settings › History.", "Was du gesagt hast, ist unter Einstellungen › Verlauf gespeichert."),
    ("That selection is too long to rewrite by voice; select a shorter part", "Die Auswahl ist zu lang, um sie per Sprache umzuschreiben; wähle einen kürzeren Teil"),
    ("The selection changed, so I left it as it is", "Die Auswahl hat sich geändert, also habe ich sie gelassen"),
    ("This app doesn't let me replace text with several lines; select one paragraph", "Diese App lässt mich keinen mehrzeiligen Text ersetzen; wähle einen Absatz"),
    ("“{word}…” is too long for a word (at most {max} characters)", "„{word}…“ ist zu lang für ein Wort (höchstens {max} Zeichen)"),
    ("At most {max} words", "Höchstens {max} Wörter"),
    ("A replacement needs words to replace (“{to}”)", "Eine Ersetzung braucht Wörter, die ersetzt werden („{to}“)"),
    ("The replacement for “{from}” is too long", "Die Ersetzung für „{from}“ ist zu lang"),
    ("At most {max} replacements", "Höchstens {max} Ersetzungen"),
    ("A snippet’s phrase needs 1 to 6 words (“{phrase}”)", "Die Phrase eines Textbausteins braucht 1 bis 6 Wörter („{phrase}“)"),
    ("The snippet “{phrase}” needs text, at most {max} characters", "Der Textbaustein „{phrase}“ braucht Text, höchstens {max} Zeichen"),
    ("Two snippets have the phrase “{phrase}”", "Zwei Textbausteine haben die Phrase „{phrase}“"),
    ("At most {max} snippets", "Höchstens {max} Textbausteine"),
    ("That suggestion is gone", "Dieser Vorschlag ist nicht mehr da"),
];

const FR: Table = &[
    ("Listening", "À l’écoute"), ("Transcribing", "Transcription"), ("Typing", "Saisie"), ("Working", "En cours"), ("Done", "Terminé"), ("Problem", "Problème"), ("About to", "Sur le point de"), ("Allow?", "Autoriser ?"),
    ("Listening… release to send", "À l’écoute… relâchez pour envoyer"),
    ("Listening… pause or click to send", "À l’écoute… faites une pause ou cliquez pour envoyer"),
    ("Listening… click to send", "À l’écoute… cliquez pour envoyer"),
    ("Listening… pause or knock to send", "À l’écoute… faites une pause ou tapez deux fois pour envoyer"),
    ("Listening… knock twice to send", "À l’écoute… tapez deux fois pour envoyer"),
    ("Listening… pause or press your AirPods to send", "À l’écoute… faites une pause ou appuyez sur vos AirPods pour envoyer"),
    ("Listening… press your AirPods to send", "À l’écoute… appuyez sur vos AirPods pour envoyer"),
    ("Listening… pause to send", "À l’écoute… faites une pause pour envoyer"),
    ("Listening for your reply… (or say “that's all”)", "J’attends votre réponse… (ou dites « c’est tout »)"),
    ("Transcribing…", "Transcription…"), ("Working…", "En cours…"), ("Rewriting…", "Réécriture…"),
    ("Didn't hear anything", "Je n’ai rien entendu"), ("Didn't catch that", "Je n’ai pas compris"),
    ("No sound from the microphone. Turn on Xpu Command in System Settings › Privacy & Security › Microphone.", "Aucun son du micro. Activez Xpu Command dans Réglages Système › Confidentialité et sécurité › Micro."),
    ("No sound from the microphone. Check microphone access and the input device in Settings.", "Aucun son du micro. Vérifiez l’accès au micro et l’entrée audio dans les Réglages."),
    ("This speech model only understands English. In Settings › Speech to text, pick a multilingual model (Small is best for Persian).", "Ce modèle vocal ne comprend que l’anglais. Dans Réglages › Parole en texte, choisissez un modèle multilingue (Small est le meilleur pour le persan)."),
    ("Press or knock to cancel", "Appuyez ou tapez deux fois pour annuler"), ("Press or knock to allow", "Appuyez ou tapez deux fois pour autoriser"),
    ("{what}? Press or knock to allow.", "{what} ? Appuyez ou tapez deux fois pour autoriser."),
    ("This {danger}: say “yes” to allow", "Ceci {danger} : dites « oui » pour autoriser"), ("Say yes or no, or press to allow", "Dites oui ou non, ou appuyez pour autoriser"),
    ("{agent} wants to {what}", "{agent} veut {what}"),
    ("{agent} wants to {what}. This {danger}. Say yes to allow, or no.", "{agent} veut {what}. Ceci {danger}. Dites oui pour autoriser, ou non."),
    ("{agent} wants to {what}. Say yes or no.", "{agent} veut {what}. Dites oui ou non."),
    ("A press can't allow this one: say “yes”", "Un appui ne suffit pas ici : dites « oui »"), ("Sorry: yes or no?", "Pardon : oui ou non ?"), ("Sorry, yes or no?", "Pardon, oui ou non ?"),
    ("Stopped the agent", "Agent arrêté"), ("Nothing to stop", "Rien à arrêter"), ("New conversation: the next request starts fresh", "Nouvelle conversation : la prochaine demande repart de zéro"), ("Removed it", "Supprimé"),
    ("Routine: {name}", "Routine : {name}"), ("Ran {name}: 1 step", "{name} exécutée : 1 étape"), ("Ran {name}: {count} steps", "{name} exécutée : {count} étapes"),
    ("Stopped {name} after {done} of {total} steps: {why}", "{name} arrêtée après {done} étapes sur {total} : {why}"), ("Cancelled", "Annulé"), ("Undo failed", "Annulation impossible"),
    ("Still working on the last request; say “stop” first", "Je travaille encore sur la demande précédente ; dites d’abord « arrête »"), ("Still working on the last request; say “stop” to stop it", "Je travaille encore sur la demande précédente ; dites « arrête » pour l’arrêter"),
    ("The agent is still working; say “stop” to stop it", "L’agent travaille encore ; dites « arrête » pour l’arrêter"), ("Choose a project folder in Settings first", "Choisissez d’abord un dossier de projet dans les Réglages"),
    ("{name} · continuing", "{name} · suite"), ("Undo", "Annuler"), ("Dictation", "Dictée"), ("Routine", "Routine"),
    ("Nothing to undo", "Rien à annuler"), ("Undid: {items}", "Annulé : {items}"), ("Couldn't undo: {items}", "Impossible d’annuler : {items}"), ("Undid: {items}. Couldn't undo: {failed}", "Annulé : {items}. Impossible d’annuler : {failed}"),
    ("Claude Code isn't installed. Install it and sign in, then try again. Settings › Health shows how.", "Claude Code n’est pas installé. Installez-le, connectez-vous et réessayez. Réglages › État explique comment."),
    ("No microphone found. Connect one, or pick another in Settings › Speech.", "Aucun micro trouvé. Branchez-en un ou choisissez-en un autre dans Réglages › Parole."),
    ("The speech model isn't downloaded yet. Download it in Settings › Get started.", "Le modèle vocal n’est pas encore téléchargé. Téléchargez-le dans Réglages › Bien démarrer."),
    ("That took too long and was stopped. Try a shorter request.", "C’était trop long et a été arrêté. Essayez une demande plus courte."),
    ("Can't reach the internet. Check your connection, or switch Speech to On this Mac.", "Pas d’accès à Internet. Vérifiez la connexion ou passez Parole sur Sur ce Mac."),
    ("Cloud speech needs an OpenAI key. Add one in Settings › Speech, or switch to On this Mac.", "La parole dans le cloud demande une clé OpenAI. Ajoutez-la dans Réglages › Parole ou passez sur Sur ce Mac."),
    ("Claude Code isn't signed in for this profile: run it in a terminal (e.g. claude-work) and use /login, or pick another profile in Settings", "Claude Code n’est pas connecté pour ce profil : lancez-le dans un terminal (p. ex. claude-work) et utilisez /login, ou choisissez un autre profil dans les Réglages"),
    ("Codex isn't signed in: run `codex login` in a terminal", "Codex n’est pas connecté : lancez `codex login` dans un terminal"), ("Codex is rate limited; try again shortly", "Codex est limité pour l’instant ; réessayez bientôt"),
    ("Let the assistant see {app} for this request", "Laisser l’assistant voir {app} pour cette demande"), ("Let the assistant control {app} for this request", "Laisser l’assistant contrôler {app} pour cette demande"),
    ("The app I typed into isn't in front any more", "L’app où j’ai tapé n’est plus au premier plan"), ("I can't find what I typed in this field any more", "Je ne retrouve plus ce que j’ai tapé dans ce champ"),
    ("I couldn't select what I typed, so I left it as it is", "Je n’ai pas pu sélectionner ce que j’ai tapé, donc je n’y ai pas touché"),
    ("This app doesn't show me its text, so I can only edit right after typing", "Cette app ne me montre pas son texte : je ne peux modifier que juste après la saisie"),
    ("Editing by voice uses Claude Code, which isn't installed", "La modification à la voix utilise Claude Code, qui n’est pas installé"),
    ("it needs a speech model on this Mac: download one in Speech", "il faut un modèle vocal sur ce Mac : téléchargez-en un dans Parole"), ("the microphone stopped", "le micro s’est arrêté"),
    ("Reading {file}", "Lecture de {file}"), ("Editing {file}", "Modification de {file}"), ("Searching the project", "Recherche dans le projet"), ("Planning", "Planification"),
    ("Delegating a subtask", "Délégation d’une sous-tâche"), ("Using {tool}", "Utilise {tool}"), ("Running: {command}", "Exécute : {command}"), ("Editing files", "Modifie des fichiers"), ("Searching the web", "Recherche sur le Web"),
    ("run {command}", "exécuter {command}"), ("open {host}", "ouvrir {host}"), ("search the web for “{query}”", "chercher « {query} » sur le Web"), ("edit {file}", "modifier {file}"), ("read {file}", "lire {file}"), ("use {tool}", "utiliser {tool}"),
    ("deletes files", "supprime des fichiers"), ("runs as administrator", "s’exécute en administrateur"), ("downloads and runs code", "télécharge et exécute du code"), ("pushes to a remote repository", "pousse vers un dépôt distant"),
    ("discards work", "jette du travail"), ("publishes a package", "publie un paquet"), ("can erase a disk", "peut effacer un disque"), ("stops the system or other programs", "arrête le système ou d’autres programmes"),
    ("changes permissions widely", "modifie largement les permissions"), ("deletes database data", "supprime des données de base"), ("deletes infrastructure", "supprime de l’infrastructure"), ("can freeze the computer", "peut figer l’ordinateur"),
    ("Looking at your apps", "Regarde vos apps"), ("Opening {name}", "Ouverture de {name}"), ("Quit {name}", "Quitter {name}"), ("the page", "la page"), ("Searching the web for “{query}”", "Recherche de « {query} » sur le Web"),
    ("Playing", "Lecture"), ("Pausing", "Pause"), ("Next track", "Morceau suivant"), ("Previous track", "Morceau précédent"), ("Playing “{query}”", "Lecture de « {query} »"), ("Muting", "Son coupé"), ("Unmuting", "Son rétabli"),
    ("Volume {level}%", "Volume {level} %"), ("Dark mode on", "Mode sombre activé"), ("Dark mode off", "Mode sombre désactivé"), ("Looking at your Shortcuts", "Regarde vos Raccourcis"), ("Run the shortcut “{name}”", "Lancer le raccourci « {name} »"),
    ("Reminder: {title} · {when}", "Rappel : {title} · {when}"), ("Reminder: {title}", "Rappel : {title}"), ("Event: {title} · {when}", "Événement : {title} · {when}"), ("Note: {title}", "Note : {title}"),
    ("Draft email to {to} +{more}: {subject}", "Brouillon pour {to} et {more} autres : {subject}"), ("Draft email to {to}: {subject}", "Brouillon pour {to} : {subject}"), ("Draft email: {subject}", "Brouillon : {subject}"),
    ("Looking at open windows", "Regarde les fenêtres ouvertes"), ("Reading {app}", "Lecture de {app}"), ("Looking at {app}", "Regarde {app}"),
    ("Clicking", "Clic sur"), ("Double-clicking", "Double-clic sur"), ("Right-clicking", "Clic droit sur"), ("Pointing at", "Survol de"),
    ("{verb} control {n}", "{verb} le contrôle {n}"), ("{verb} ({x}, {y}) in {app}", "{verb} ({x}, {y}) dans {app}"), ("{verb} page item {n}", "{verb} l’élément {n} de la page"), ("{verb} {label} in {app}", "{verb} {label} dans {app}"),
    ("Typing “{text}” into control {n}", "Saisie de « {text} » dans le contrôle {n}"), ("Typing “{text}” into control {n} and pressing Return", "Saisie de « {text} » dans le contrôle {n} puis Entrée"),
    ("Pressing {keys} in {app}", "Appui sur {keys} dans {app}"), ("Menu {path} in {app}", "Menu {path} dans {app}"), ("Scrolling down in {app}", "Défilement vers le bas dans {app}"), ("Scrolling up in {app}", "Défilement vers le haut dans {app}"),
    ("Dragging control {from} onto control {to}", "Glisser le contrôle {from} sur le contrôle {to}"), ("Waiting for “{text}” in {app}", "Attente de « {text} » dans {app}"), ("Waiting {seconds} s for {app}", "Attente de {seconds} s pour {app}"),
    ("Reading the page in {app}", "Lecture de la page dans {app}"), ("Filling page item {n} with “{text}”", "Remplissage de l’élément {n} avec « {text} »"), ("Filling page item {n} with “{text}” and submitting", "Remplissage de l’élément {n} avec « {text} » puis envoi"),
    ("Copying “{text}”", "Copie de « {text} »"), ("Reading the clipboard", "Lecture du presse-papiers"), ("Finding files named “{query}”", "Recherche des fichiers nommés « {query} »"), ("Opening {file} in {app}", "Ouverture de {file} dans {app}"),
    ("Move {items} to {dest}", "Déplacer {items} vers {dest}"), ("Rename {file} to {name}", "Renommer {file} en {name}"), ("Move {items} to the Trash", "Mettre {items} à la Corbeille"), ("New folder “{name}” in {parent}", "Nouveau dossier « {name} » dans {parent}"),
    ("{count} items", "{count} éléments"), ("Remembering: {text}", "Je retiens : {text}"), ("Forgetting “{text}”", "J’oublie « {text} »"), ("Save the routine “{name}”: {steps}", "Enregistrer la routine « {name} » : {steps}"),
    ("Typing “{text}” into {label} in {app}", "Saisie de « {text} » dans {label} de {app}"), ("Typing “{text}” into {label} in {app}, then Return", "Saisie de « {text} » dans {label} de {app}, puis Entrée"),
    ("Filling {label} in {app} with “{text}”", "Remplissage de {label} dans {app} avec « {text} »"), ("Filling {label} in {app} with “{text}”, then submitting", "Remplissage de {label} dans {app} avec « {text} », puis envoi"), ("Dragging {label} in {app}", "Glisser {label} dans {app}"),
    ("Say “always” to stop asking, “yes” for now, or press", "Dites « toujours » pour ne plus demander, « oui » pour cette fois, ou appuyez"),
    ("{question}? Say always, yes, or no.", "{question} ? Dites toujours, oui ou non."),
    ("{app} is allowed from now on. Change it in Settings › Apps.", "{app} est autorisé désormais. Modifiez-le dans Réglages › Apps."),
    ("What you said is kept in Settings › History.", "Ce que vous avez dit est conservé dans Réglages › Historique."),
    ("That selection is too long to rewrite by voice; select a shorter part", "La sélection est trop longue pour être réécrite à la voix ; sélectionnez une partie plus courte"),
    ("The selection changed, so I left it as it is", "La sélection a changé, je l’ai donc laissée telle quelle"),
    ("This app doesn't let me replace text with several lines; select one paragraph", "Cette app ne me laisse pas remplacer un texte de plusieurs lignes ; sélectionnez un seul paragraphe"),
    ("“{word}…” is too long for a word (at most {max} characters)", "« {word}… » est trop long pour un mot ({max} caractères au plus)"),
    ("At most {max} words", "{max} mots au plus"),
    ("A replacement needs words to replace (“{to}”)", "Un remplacement a besoin de mots à remplacer (« {to} »)"),
    ("The replacement for “{from}” is too long", "Le remplacement de « {from} » est trop long"),
    ("At most {max} replacements", "{max} remplacements au plus"),
    ("A snippet’s phrase needs 1 to 6 words (“{phrase}”)", "La phrase d’un raccourci doit compter de 1 à 6 mots (« {phrase} »)"),
    ("The snippet “{phrase}” needs text, at most {max} characters", "Le raccourci « {phrase} » a besoin d’un texte, {max} caractères au plus"),
    ("Two snippets have the phrase “{phrase}”", "Deux raccourcis ont la phrase « {phrase} »"),
    ("At most {max} snippets", "{max} raccourcis au plus"),
    ("That suggestion is gone", "Cette suggestion n’existe plus"),
];

const AR: Table = &[
    ("Listening", "أستمع"), ("Transcribing", "تحويل إلى نص"), ("Typing", "أكتب"), ("Working", "أعمل"), ("Done", "تم"), ("Problem", "مشكلة"), ("About to", "على وشك"), ("Allow?", "سماح؟"),
    ("Listening… release to send", "أستمع… اترك الزر للإرسال"),
    ("Listening… pause or click to send", "أستمع… توقف أو انقر للإرسال"),
    ("Listening… click to send", "أستمع… انقر للإرسال"),
    ("Listening… pause or knock to send", "أستمع… توقف أو انقر مرتين على الجهاز للإرسال"),
    ("Listening… knock twice to send", "أستمع… انقر مرتين على الجهاز للإرسال"),
    ("Listening… pause or press your AirPods to send", "أستمع… توقف أو اضغط على AirPods للإرسال"),
    ("Listening… press your AirPods to send", "أستمع… اضغط على AirPods للإرسال"),
    ("Listening… pause to send", "أستمع… توقف للإرسال"),
    ("Listening for your reply… (or say “that's all”)", "أنتظر ردك… (أو قل «هذا كل شيء»)"),
    ("Transcribing…", "تحويل إلى نص…"), ("Working…", "أعمل…"), ("Rewriting…", "إعادة صياغة…"),
    ("Didn't hear anything", "لم أسمع شيئًا"), ("Didn't catch that", "لم أفهم ذلك"),
    ("No sound from the microphone. Turn on Xpu Command in System Settings › Privacy & Security › Microphone.", "لا صوت من الميكروفون. فعّل Xpu Command في إعدادات النظام › الخصوصية والأمن › الميكروفون."),
    ("No sound from the microphone. Check microphone access and the input device in Settings.", "لا صوت من الميكروفون. تحقق من إذن الميكروفون وجهاز الإدخال في الإعدادات."),
    ("This speech model only understands English. In Settings › Speech to text, pick a multilingual model (Small is best for Persian).", "نموذج الكلام هذا يفهم الإنجليزية فقط. اختر نموذجًا متعدد اللغات في الإعدادات › الكلام إلى نص (Small الأفضل للفارسية)."),
    ("Press or knock to cancel", "اضغط أو انقر مرتين للإلغاء"), ("Press or knock to allow", "اضغط أو انقر مرتين للسماح"),
    ("{what}? Press or knock to allow.", "{what}؟ اضغط أو انقر مرتين للسماح."),
    ("This {danger}: say “yes” to allow", "هذا {danger}: قل «نعم» للسماح"), ("Say yes or no, or press to allow", "قل نعم أو لا، أو اضغط للسماح"),
    ("{agent} wants to {what}", "يريد {agent} أن {what}"),
    ("{agent} wants to {what}. This {danger}. Say yes to allow, or no.", "يريد {agent} أن {what}. هذا {danger}. قل نعم للسماح، أو لا."),
    ("{agent} wants to {what}. Say yes or no.", "يريد {agent} أن {what}. قل نعم أو لا."),
    ("A press can't allow this one: say “yes”", "لا يكفي الضغط هنا: قل «نعم»"), ("Sorry: yes or no?", "عذرًا: نعم أم لا؟"), ("Sorry, yes or no?", "عذرًا، نعم أم لا؟"),
    ("Stopped the agent", "أوقفت الوكيل"), ("Nothing to stop", "لا شيء لإيقافه"), ("New conversation: the next request starts fresh", "محادثة جديدة: الطلب التالي يبدأ من جديد"), ("Removed it", "حذفته"),
    ("Routine: {name}", "روتين: {name}"), ("Ran {name}: 1 step", "تم تشغيل {name}: خطوة واحدة"), ("Ran {name}: {count} steps", "تم تشغيل {name}: {count} خطوات"),
    ("Stopped {name} after {done} of {total} steps: {why}", "توقف {name} بعد {done} من {total} خطوات: {why}"), ("Cancelled", "أُلغي"), ("Undo failed", "تعذّر التراجع"),
    ("Still working on the last request; say “stop” first", "ما زلت أعمل على الطلب السابق؛ قل «توقف» أولًا"), ("Still working on the last request; say “stop” to stop it", "ما زلت أعمل على الطلب السابق؛ قل «توقف» لإيقافه"),
    ("The agent is still working; say “stop” to stop it", "الوكيل ما زال يعمل؛ قل «توقف» لإيقافه"), ("Choose a project folder in Settings first", "اختر مجلد مشروع في الإعدادات أولًا"),
    ("{name} · continuing", "{name} · متابعة"), ("Undo", "تراجع"), ("Dictation", "إملاء"), ("Routine", "روتين"),
    ("Nothing to undo", "لا شيء للتراجع عنه"), ("Undid: {items}", "تم التراجع: {items}"), ("Couldn't undo: {items}", "تعذّر التراجع: {items}"), ("Undid: {items}. Couldn't undo: {failed}", "تم التراجع: {items}. تعذّر التراجع: {failed}"),
    ("Claude Code isn't installed. Install it and sign in, then try again. Settings › Health shows how.", "Claude Code غير مثبت. ثبّته وسجّل الدخول ثم حاول مجددًا. الإعدادات › الحالة تشرح الطريقة."),
    ("No microphone found. Connect one, or pick another in Settings › Speech.", "لم يُعثر على ميكروفون. وصّل واحدًا أو اختر غيره في الإعدادات › الكلام."),
    ("The speech model isn't downloaded yet. Download it in Settings › Get started.", "نموذج الكلام لم يُنزّل بعد. نزّله من الإعدادات › البدء."),
    ("That took too long and was stopped. Try a shorter request.", "استغرق ذلك وقتًا طويلًا فأُوقف. جرّب طلبًا أقصر."),
    ("Can't reach the internet. Check your connection, or switch Speech to On this Mac.", "لا يمكن الوصول إلى الإنترنت. تحقق من الاتصال أو حوّل الكلام إلى على هذا الماك."),
    ("Cloud speech needs an OpenAI key. Add one in Settings › Speech, or switch to On this Mac.", "الكلام السحابي يحتاج مفتاح OpenAI. أضفه في الإعدادات › الكلام أو حوّل إلى على هذا الماك."),
    ("Claude Code isn't signed in for this profile: run it in a terminal (e.g. claude-work) and use /login, or pick another profile in Settings", "Claude Code غير مسجّل الدخول في هذا الملف الشخصي: شغّله في الطرفية (مثل claude-work) واستخدم ‎/login، أو اختر ملفًا آخر في الإعدادات"),
    ("Codex isn't signed in: run `codex login` in a terminal", "Codex غير مسجّل الدخول: شغّل ‎`codex login` في الطرفية"), ("Codex is rate limited; try again shortly", "Codex بلغ حد الطلبات؛ حاول بعد قليل"),
    ("Let the assistant see {app} for this request", "هل تسمح للمساعد برؤية {app} لهذا الطلب"), ("Let the assistant control {app} for this request", "هل تسمح للمساعد بالتحكم في {app} لهذا الطلب"),
    ("The app I typed into isn't in front any more", "التطبيق الذي كتبت فيه لم يعد في المقدمة"), ("I can't find what I typed in this field any more", "لم أعد أجد ما كتبته في هذا الحقل"),
    ("I couldn't select what I typed, so I left it as it is", "لم أستطع تحديد ما كتبته، فتركته كما هو"),
    ("This app doesn't show me its text, so I can only edit right after typing", "هذا التطبيق لا يعرض لي نصه، لذا لا أعدّل إلا مباشرة بعد الكتابة"),
    ("Editing by voice uses Claude Code, which isn't installed", "التعديل بالصوت يستخدم Claude Code، وهو غير مثبت"),
    ("it needs a speech model on this Mac: download one in Speech", "يحتاج نموذج كلام على هذا الماك: نزّل واحدًا من قسم الكلام"), ("the microphone stopped", "توقف الميكروفون"),
    ("Reading {file}", "قراءة {file}"), ("Editing {file}", "تعديل {file}"), ("Searching the project", "البحث في المشروع"), ("Planning", "التخطيط"),
    ("Delegating a subtask", "إسناد مهمة فرعية"), ("Using {tool}", "استخدام {tool}"), ("Running: {command}", "تشغيل: {command}"), ("Editing files", "تعديل الملفات"), ("Searching the web", "البحث في الويب"),
    ("run {command}", "يشغّل {command}"), ("open {host}", "يفتح {host}"), ("search the web for “{query}”", "يبحث في الويب عن «{query}»"), ("edit {file}", "يعدّل {file}"), ("read {file}", "يقرأ {file}"), ("use {tool}", "يستخدم {tool}"),
    ("deletes files", "يحذف ملفات"), ("runs as administrator", "يعمل بصلاحيات المسؤول"), ("downloads and runs code", "ينزّل شيفرة ويشغّلها"), ("pushes to a remote repository", "يدفع إلى مستودع بعيد"),
    ("discards work", "يتخلّص من عمل"), ("publishes a package", "ينشر حزمة"), ("can erase a disk", "قد يمسح قرصًا"), ("stops the system or other programs", "يوقف النظام أو برامج أخرى"),
    ("changes permissions widely", "يغيّر الأذونات على نطاق واسع"), ("deletes database data", "يحذف بيانات قاعدة البيانات"), ("deletes infrastructure", "يحذف البنية التحتية"), ("can freeze the computer", "قد يجمّد الحاسوب"),
    ("Looking at your apps", "أطّلع على تطبيقاتك"), ("Opening {name}", "فتح {name}"), ("Quit {name}", "إنهاء {name}"), ("the page", "الصفحة"), ("Searching the web for “{query}”", "البحث في الويب عن «{query}»"),
    ("Playing", "تشغيل"), ("Pausing", "إيقاف مؤقت"), ("Next track", "المقطع التالي"), ("Previous track", "المقطع السابق"), ("Playing “{query}”", "تشغيل «{query}»"), ("Muting", "كتم الصوت"), ("Unmuting", "إلغاء الكتم"),
    ("Volume {level}%", "مستوى الصوت {level}٪"), ("Dark mode on", "الوضع الداكن مفعّل"), ("Dark mode off", "الوضع الداكن متوقف"), ("Looking at your Shortcuts", "أطّلع على اختصاراتك"), ("Run the shortcut “{name}”", "تشغيل الاختصار «{name}»"),
    ("Reminder: {title} · {when}", "تذكير: {title} · {when}"), ("Reminder: {title}", "تذكير: {title}"), ("Event: {title} · {when}", "حدث: {title} · {when}"), ("Note: {title}", "ملاحظة: {title}"),
    ("Draft email to {to} +{more}: {subject}", "مسودة بريد إلى {to} و{more} آخرين: {subject}"), ("Draft email to {to}: {subject}", "مسودة بريد إلى {to}: {subject}"), ("Draft email: {subject}", "مسودة بريد: {subject}"),
    ("Looking at open windows", "أطّلع على النوافذ المفتوحة"), ("Reading {app}", "قراءة {app}"), ("Looking at {app}", "النظر إلى {app}"),
    ("Clicking", "النقر على"), ("Double-clicking", "النقر المزدوج على"), ("Right-clicking", "النقر الأيمن على"), ("Pointing at", "الإشارة إلى"),
    ("{verb} control {n}", "{verb} العنصر {n}"), ("{verb} ({x}, {y}) in {app}", "{verb} ‎({x}, {y})‎ في {app}"), ("{verb} page item {n}", "{verb} عنصر الصفحة {n}"), ("{verb} {label} in {app}", "{verb} {label} في {app}"),
    ("Typing “{text}” into control {n}", "كتابة «{text}» في العنصر {n}"), ("Typing “{text}” into control {n} and pressing Return", "كتابة «{text}» في العنصر {n} والضغط على Return"),
    ("Pressing {keys} in {app}", "الضغط على {keys} في {app}"), ("Menu {path} in {app}", "القائمة {path} في {app}"), ("Scrolling down in {app}", "التمرير لأسفل في {app}"), ("Scrolling up in {app}", "التمرير لأعلى في {app}"),
    ("Dragging control {from} onto control {to}", "سحب العنصر {from} إلى العنصر {to}"), ("Waiting for “{text}” in {app}", "انتظار «{text}» في {app}"), ("Waiting {seconds} s for {app}", "انتظار {seconds} ث لـ {app}"),
    ("Reading the page in {app}", "قراءة الصفحة في {app}"), ("Filling page item {n} with “{text}”", "ملء عنصر الصفحة {n} بـ «{text}»"), ("Filling page item {n} with “{text}” and submitting", "ملء عنصر الصفحة {n} بـ «{text}» والإرسال"),
    ("Copying “{text}”", "نسخ «{text}»"), ("Reading the clipboard", "قراءة الحافظة"), ("Finding files named “{query}”", "البحث عن ملفات باسم «{query}»"), ("Opening {file} in {app}", "فتح {file} في {app}"),
    ("Move {items} to {dest}", "نقل {items} إلى {dest}"), ("Rename {file} to {name}", "إعادة تسمية {file} إلى {name}"), ("Move {items} to the Trash", "نقل {items} إلى سلة المهملات"), ("New folder “{name}” in {parent}", "مجلد جديد «{name}» في {parent}"),
    ("{count} items", "{count} عناصر"), ("Remembering: {text}", "أتذكّر: {text}"), ("Forgetting “{text}”", "أنسى «{text}»"), ("Save the routine “{name}”: {steps}", "حفظ الروتين «{name}»: {steps}"),
    ("Typing “{text}” into {label} in {app}", "كتابة «{text}» في {label} في {app}"), ("Typing “{text}” into {label} in {app}, then Return", "كتابة «{text}» في {label} في {app} ثم Return"),
    ("Filling {label} in {app} with “{text}”", "ملء {label} في {app} بـ «{text}»"), ("Filling {label} in {app} with “{text}”, then submitting", "ملء {label} في {app} بـ «{text}» ثم الإرسال"), ("Dragging {label} in {app}", "سحب {label} في {app}"),
    ("Say “always” to stop asking, “yes” for now, or press", "قل «دائمًا» حتى لا أسأل مجددًا، «نعم» لهذه المرة، أو اضغط"),
    ("{question}? Say always, yes, or no.", "{question}؟ قل دائمًا أو نعم أو لا."),
    ("{app} is allowed from now on. Change it in Settings › Apps.", "{app} مسموح من الآن. غيّر ذلك في الإعدادات › التطبيقات."),
    ("What you said is kept in Settings › History.", "ما قلته محفوظ في الإعدادات › السجل."),
    ("That selection is too long to rewrite by voice; select a shorter part", "التحديد أطول من أن يُعاد كتابته بالصوت؛ حدّد جزءًا أقصر"),
    ("The selection changed, so I left it as it is", "تغيّر التحديد، لذا تركته كما هو"),
    ("This app doesn't let me replace text with several lines; select one paragraph", "هذا التطبيق لا يسمح لي باستبدال نص من عدة أسطر؛ حدّد فقرة واحدة"),
    ("“{word}…” is too long for a word (at most {max} characters)", "«{word}…» طويلة جدًا لكلمة (بحد أقصى {max} حرفًا)"),
    ("At most {max} words", "بحد أقصى {max} كلمة"),
    ("A replacement needs words to replace (“{to}”)", "يحتاج الاستبدال إلى كلمات لاستبدالها («{to}»)"),
    ("The replacement for “{from}” is too long", "استبدال «{from}» طويل جدًا"),
    ("At most {max} replacements", "بحد أقصى {max} استبدالًا"),
    ("A snippet’s phrase needs 1 to 6 words (“{phrase}”)", "تحتاج عبارة الاختصار إلى 1 إلى 6 كلمات («{phrase}»)"),
    ("The snippet “{phrase}” needs text, at most {max} characters", "يحتاج الاختصار «{phrase}» إلى نص، بحد أقصى {max} حرفًا"),
    ("Two snippets have the phrase “{phrase}”", "اختصاران لهما العبارة «{phrase}»"),
    ("At most {max} snippets", "بحد أقصى {max} اختصارًا"),
    ("That suggestion is gone", "ذلك الاقتراح لم يعد موجودًا"),
];

const ZH: Table = &[
    ("Listening", "正在听"), ("Transcribing", "正在转写"), ("Typing", "正在输入"), ("Working", "处理中"), ("Done", "完成"), ("Problem", "出错"), ("About to", "即将"), ("Allow?", "允许？"),
    ("Listening… release to send", "正在听… 松开即发送"),
    ("Listening… pause or click to send", "正在听… 停顿或点击即发送"),
    ("Listening… click to send", "正在听… 点击即发送"),
    ("Listening… pause or knock to send", "正在听… 停顿或敲两下即发送"),
    ("Listening… knock twice to send", "正在听… 敲两下即发送"),
    ("Listening… pause or press your AirPods to send", "正在听… 停顿或按 AirPods 即发送"),
    ("Listening… press your AirPods to send", "正在听… 按 AirPods 即发送"),
    ("Listening… pause to send", "正在听… 停顿即发送"),
    ("Listening for your reply… (or say “that's all”)", "等待你的回复…（或说“就这样”）"),
    ("Transcribing…", "正在转写…"), ("Working…", "处理中…"), ("Rewriting…", "正在改写…"),
    ("Didn't hear anything", "没有听到声音"), ("Didn't catch that", "没听清"),
    ("No sound from the microphone. Turn on Xpu Command in System Settings › Privacy & Security › Microphone.", "麦克风没有声音。请在“系统设置 › 隐私与安全性 › 麦克风”中打开 Xpu Command。"),
    ("No sound from the microphone. Check microphone access and the input device in Settings.", "麦克风没有声音。请在设置中检查麦克风权限和输入设备。"),
    ("This speech model only understands English. In Settings › Speech to text, pick a multilingual model (Small is best for Persian).", "此语音模型只懂英语。请在“设置 › 语音转文字”中选择多语言模型（波斯语最适合 Small）。"),
    ("Press or knock to cancel", "按下或敲两下即可取消"), ("Press or knock to allow", "按下或敲两下即可允许"),
    ("{what}? Press or knock to allow.", "{what}？按下或敲两下即可允许。"),
    ("This {danger}: say “yes” to allow", "此操作{danger}：说“是”以允许"), ("Say yes or no, or press to allow", "请说是或否，或按下以允许"),
    ("{agent} wants to {what}", "{agent} 想要{what}"),
    ("{agent} wants to {what}. This {danger}. Say yes to allow, or no.", "{agent} 想要{what}。此操作{danger}。说“是”允许，或说“否”。"),
    ("{agent} wants to {what}. Say yes or no.", "{agent} 想要{what}。请说是或否。"),
    ("A press can't allow this one: say “yes”", "这一项不能按键允许：请说“是”"), ("Sorry: yes or no?", "抱歉：是还是否？"), ("Sorry, yes or no?", "抱歉，是还是否？"),
    ("Stopped the agent", "已停止智能体"), ("Nothing to stop", "没有可停止的任务"), ("New conversation: the next request starts fresh", "新对话：下一个请求重新开始"), ("Removed it", "已删除"),
    ("Routine: {name}", "例程：{name}"), ("Ran {name}: 1 step", "已运行 {name}：1 步"), ("Ran {name}: {count} steps", "已运行 {name}：{count} 步"),
    ("Stopped {name} after {done} of {total} steps: {why}", "{name} 在第 {done}/{total} 步后停止：{why}"), ("Cancelled", "已取消"), ("Undo failed", "撤销失败"),
    ("Still working on the last request; say “stop” first", "还在处理上一个请求；请先说“停止”"), ("Still working on the last request; say “stop” to stop it", "还在处理上一个请求；说“停止”可以停下"),
    ("The agent is still working; say “stop” to stop it", "智能体仍在工作；说“停止”可以停下"), ("Choose a project folder in Settings first", "请先在设置中选择项目文件夹"),
    ("{name} · continuing", "{name} · 继续"), ("Undo", "撤销"), ("Dictation", "听写"), ("Routine", "例程"),
    ("Nothing to undo", "没有可撤销的操作"), ("Undid: {items}", "已撤销：{items}"), ("Couldn't undo: {items}", "无法撤销：{items}"), ("Undid: {items}. Couldn't undo: {failed}", "已撤销：{items}。无法撤销：{failed}"),
    ("Claude Code isn't installed. Install it and sign in, then try again. Settings › Health shows how.", "未安装 Claude Code。请安装并登录后重试。“设置 › 状态”中有说明。"),
    ("No microphone found. Connect one, or pick another in Settings › Speech.", "未找到麦克风。请连接一个，或在“设置 › 语音”中选择其他麦克风。"),
    ("The speech model isn't downloaded yet. Download it in Settings › Get started.", "语音模型尚未下载。请在“设置 › 开始使用”中下载。"),
    ("That took too long and was stopped. Try a shorter request.", "耗时太长，已停止。请尝试更短的请求。"),
    ("Can't reach the internet. Check your connection, or switch Speech to On this Mac.", "无法连接互联网。请检查网络，或将语音切换为“在这台 Mac 上”。"),
    ("Cloud speech needs an OpenAI key. Add one in Settings › Speech, or switch to On this Mac.", "云端语音需要 OpenAI 密钥。请在“设置 › 语音”中添加，或切换为“在这台 Mac 上”。"),
    ("Claude Code isn't signed in for this profile: run it in a terminal (e.g. claude-work) and use /login, or pick another profile in Settings", "此配置下 Claude Code 未登录：请在终端运行（如 claude-work）并使用 /login，或在设置中选择其他配置"),
    ("Codex isn't signed in: run `codex login` in a terminal", "Codex 未登录：请在终端运行 `codex login`"), ("Codex is rate limited; try again shortly", "Codex 已达速率限制；请稍后再试"),
    ("Let the assistant see {app} for this request", "允许助手在本次请求中查看 {app}"), ("Let the assistant control {app} for this request", "允许助手在本次请求中控制 {app}"),
    ("The app I typed into isn't in front any more", "我输入内容的应用已不在前台"), ("I can't find what I typed in this field any more", "在此输入框中找不到我输入的内容了"),
    ("I couldn't select what I typed, so I left it as it is", "无法选中我输入的内容，已保持原样"),
    ("This app doesn't show me its text, so I can only edit right after typing", "此应用不提供其文本，只能在刚输入后修改"),
    ("Editing by voice uses Claude Code, which isn't installed", "语音编辑需要 Claude Code，但它未安装"),
    ("it needs a speech model on this Mac: download one in Speech", "需要本机上的语音模型：请在“语音”中下载"), ("the microphone stopped", "麦克风已停止"),
    ("Reading {file}", "正在读取 {file}"), ("Editing {file}", "正在编辑 {file}"), ("Searching the project", "正在搜索项目"), ("Planning", "正在规划"),
    ("Delegating a subtask", "正在委派子任务"), ("Using {tool}", "正在使用 {tool}"), ("Running: {command}", "正在运行：{command}"), ("Editing files", "正在编辑文件"), ("Searching the web", "正在搜索网页"),
    ("run {command}", "运行 {command}"), ("open {host}", "打开 {host}"), ("search the web for “{query}”", "在网上搜索“{query}”"), ("edit {file}", "编辑 {file}"), ("read {file}", "读取 {file}"), ("use {tool}", "使用 {tool}"),
    ("deletes files", "会删除文件"), ("runs as administrator", "以管理员身份运行"), ("downloads and runs code", "会下载并运行代码"), ("pushes to a remote repository", "会推送到远程仓库"),
    ("discards work", "会丢弃工作"), ("publishes a package", "会发布软件包"), ("can erase a disk", "可能抹掉磁盘"), ("stops the system or other programs", "会停止系统或其他程序"),
    ("changes permissions widely", "会大范围修改权限"), ("deletes database data", "会删除数据库数据"), ("deletes infrastructure", "会删除基础设施"), ("can freeze the computer", "可能让电脑卡死"),
    ("Looking at your apps", "正在查看你的应用"), ("Opening {name}", "正在打开 {name}"), ("Quit {name}", "退出 {name}"), ("the page", "该页面"), ("Searching the web for “{query}”", "正在网上搜索“{query}”"),
    ("Playing", "播放"), ("Pausing", "暂停"), ("Next track", "下一首"), ("Previous track", "上一首"), ("Playing “{query}”", "正在播放“{query}”"), ("Muting", "静音"), ("Unmuting", "取消静音"),
    ("Volume {level}%", "音量 {level}%"), ("Dark mode on", "深色模式已打开"), ("Dark mode off", "深色模式已关闭"), ("Looking at your Shortcuts", "正在查看你的快捷指令"), ("Run the shortcut “{name}”", "运行快捷指令“{name}”"),
    ("Reminder: {title} · {when}", "提醒事项：{title} · {when}"), ("Reminder: {title}", "提醒事项：{title}"), ("Event: {title} · {when}", "日程：{title} · {when}"), ("Note: {title}", "备忘录：{title}"),
    ("Draft email to {to} +{more}: {subject}", "给 {to} 等另外 {more} 人的邮件草稿：{subject}"), ("Draft email to {to}: {subject}", "给 {to} 的邮件草稿：{subject}"), ("Draft email: {subject}", "邮件草稿：{subject}"),
    ("Looking at open windows", "正在查看打开的窗口"), ("Reading {app}", "正在读取 {app}"), ("Looking at {app}", "正在查看 {app}"),
    ("Clicking", "点击"), ("Double-clicking", "双击"), ("Right-clicking", "右键点击"), ("Pointing at", "指向"),
    ("{verb} control {n}", "{verb}控件 {n}"), ("{verb} ({x}, {y}) in {app}", "在 {app} 中{verb} ({x}, {y})"), ("{verb} page item {n}", "{verb}页面项 {n}"), ("{verb} {label} in {app}", "在 {app} 中{verb}{label}"),
    ("Typing “{text}” into control {n}", "在控件 {n} 中输入“{text}”"), ("Typing “{text}” into control {n} and pressing Return", "在控件 {n} 中输入“{text}”并按回车"),
    ("Pressing {keys} in {app}", "在 {app} 中按 {keys}"), ("Menu {path} in {app}", "{app} 中的菜单 {path}"), ("Scrolling down in {app}", "在 {app} 中向下滚动"), ("Scrolling up in {app}", "在 {app} 中向上滚动"),
    ("Dragging control {from} onto control {to}", "把控件 {from} 拖到控件 {to}"), ("Waiting for “{text}” in {app}", "等待 {app} 中出现“{text}”"), ("Waiting {seconds} s for {app}", "等待 {app} {seconds} 秒"),
    ("Reading the page in {app}", "正在读取 {app} 中的页面"), ("Filling page item {n} with “{text}”", "在页面项 {n} 中填入“{text}”"), ("Filling page item {n} with “{text}” and submitting", "在页面项 {n} 中填入“{text}”并提交"),
    ("Copying “{text}”", "正在复制“{text}”"), ("Reading the clipboard", "正在读取剪贴板"), ("Finding files named “{query}”", "正在查找名为“{query}”的文件"), ("Opening {file} in {app}", "用 {app} 打开 {file}"),
    ("Move {items} to {dest}", "把 {items} 移到 {dest}"), ("Rename {file} to {name}", "把 {file} 重命名为 {name}"), ("Move {items} to the Trash", "把 {items} 移到废纸篓"), ("New folder “{name}” in {parent}", "在 {parent} 中新建文件夹“{name}”"),
    ("{count} items", "{count} 个项目"), ("Remembering: {text}", "记住：{text}"), ("Forgetting “{text}”", "忘记“{text}”"), ("Save the routine “{name}”: {steps}", "保存例程“{name}”：{steps}"),
    ("Typing “{text}” into {label} in {app}", "在 {app} 的{label}中输入“{text}”"), ("Typing “{text}” into {label} in {app}, then Return", "在 {app} 的{label}中输入“{text}”，然后回车"),
    ("Filling {label} in {app} with “{text}”", "在 {app} 的{label}中填入“{text}”"), ("Filling {label} in {app} with “{text}”, then submitting", "在 {app} 的{label}中填入“{text}”，然后提交"), ("Dragging {label} in {app}", "在 {app} 中拖动{label}"),
    ("Say “always” to stop asking, “yes” for now, or press", "说“总是”以后不再询问，说“是”仅限这次，或按下"),
    ("{question}? Say always, yes, or no.", "{question}？请说总是、是或否。"),
    ("{app} is allowed from now on. Change it in Settings › Apps.", "{app} 从现在起已允许。可在“设置 › 应用”中更改。"),
    ("What you said is kept in Settings › History.", "你说的内容已保存在 设置 › 历史 中。"),
    ("That selection is too long to rewrite by voice; select a shorter part", "选中的内容太长，无法用语音改写；请选择较短的部分"),
    ("The selection changed, so I left it as it is", "选中的内容已变化，所以我没有改动"),
    ("This app doesn't let me replace text with several lines; select one paragraph", "此应用不允许我替换多行文本；请选择一个段落"),
    ("“{word}…” is too long for a word (at most {max} characters)", "“{word}…”作为词语太长（最多 {max} 个字符）"),
    ("At most {max} words", "最多 {max} 个词语"),
    ("A replacement needs words to replace (“{to}”)", "替换需要有被替换的词语（“{to}”）"),
    ("The replacement for “{from}” is too long", "“{from}”的替换太长"),
    ("At most {max} replacements", "最多 {max} 条替换"),
    ("A snippet’s phrase needs 1 to 6 words (“{phrase}”)", "片段的短语需要 1 到 6 个词（“{phrase}”）"),
    ("The snippet “{phrase}” needs text, at most {max} characters", "片段“{phrase}”需要文本，最多 {max} 个字符"),
    ("Two snippets have the phrase “{phrase}”", "有两个片段使用了短语“{phrase}”"),
    ("At most {max} snippets", "最多 {max} 个片段"),
    ("That suggestion is gone", "该建议已不存在"),
];

const JA: Table = &[
    ("Listening", "聞き取り中"), ("Transcribing", "文字起こし中"), ("Typing", "入力中"), ("Working", "処理中"), ("Done", "完了"), ("Problem", "問題"), ("About to", "まもなく"), ("Allow?", "許可？"),
    ("Listening… release to send", "聞き取り中… 離すと送信"),
    ("Listening… pause or click to send", "聞き取り中… 話を止めるかクリックで送信"),
    ("Listening… click to send", "聞き取り中… クリックで送信"),
    ("Listening… pause or knock to send", "聞き取り中… 話を止めるか2回ノックで送信"),
    ("Listening… knock twice to send", "聞き取り中… 2回ノックで送信"),
    ("Listening… pause or press your AirPods to send", "聞き取り中… 話を止めるか AirPods を押して送信"),
    ("Listening… press your AirPods to send", "聞き取り中… AirPods を押して送信"),
    ("Listening… pause to send", "聞き取り中… 話を止めると送信"),
    ("Listening for your reply… (or say “that's all”)", "返事を待っています…（または「以上です」）"),
    ("Transcribing…", "文字起こし中…"), ("Working…", "処理中…"), ("Rewriting…", "書き直し中…"),
    ("Didn't hear anything", "何も聞こえませんでした"), ("Didn't catch that", "聞き取れませんでした"),
    ("No sound from the microphone. Turn on Xpu Command in System Settings › Privacy & Security › Microphone.", "マイクの音がありません。システム設定 › プライバシーとセキュリティ › マイク で Xpu Command をオンにしてください。"),
    ("No sound from the microphone. Check microphone access and the input device in Settings.", "マイクの音がありません。設定でマイクのアクセスと入力装置を確認してください。"),
    ("This speech model only understands English. In Settings › Speech to text, pick a multilingual model (Small is best for Persian).", "この音声モデルは英語のみです。設定 › 音声からテキスト で多言語モデルを選んでください（ペルシャ語には Small が最適）。"),
    ("Press or knock to cancel", "押すかノックで取り消し"), ("Press or knock to allow", "押すかノックで許可"),
    ("{what}? Press or knock to allow.", "{what}？ 押すかノックで許可します。"),
    ("This {danger}: say “yes” to allow", "この操作は{danger}：許可するには「はい」と言ってください"), ("Say yes or no, or press to allow", "はいかいいえで答えるか、押して許可"),
    ("{agent} wants to {what}", "{agent} が{what}しようとしています"),
    ("{agent} wants to {what}. This {danger}. Say yes to allow, or no.", "{agent} が{what}しようとしています。この操作は{danger}。許可するなら「はい」、しないなら「いいえ」。"),
    ("{agent} wants to {what}. Say yes or no.", "{agent} が{what}しようとしています。はいかいいえで答えてください。"),
    ("A press can't allow this one: say “yes”", "これは押すだけでは許可できません：「はい」と言ってください"), ("Sorry: yes or no?", "すみません：はい、いいえ？"), ("Sorry, yes or no?", "すみません、はいですか、いいえですか？"),
    ("Stopped the agent", "エージェントを止めました"), ("Nothing to stop", "止めるものはありません"), ("New conversation: the next request starts fresh", "新しい会話：次の依頼は最初からです"), ("Removed it", "削除しました"),
    ("Routine: {name}", "ルーティン：{name}"), ("Ran {name}: 1 step", "{name} を実行：1 ステップ"), ("Ran {name}: {count} steps", "{name} を実行：{count} ステップ"),
    ("Stopped {name} after {done} of {total} steps: {why}", "{name} は {total} ステップ中 {done} で停止：{why}"), ("Cancelled", "取り消しました"), ("Undo failed", "元に戻せませんでした"),
    ("Still working on the last request; say “stop” first", "前の依頼を処理中です。まず「止めて」と言ってください"), ("Still working on the last request; say “stop” to stop it", "前の依頼を処理中です。止めるには「止めて」と言ってください"),
    ("The agent is still working; say “stop” to stop it", "エージェントはまだ作業中です。止めるには「止めて」と言ってください"), ("Choose a project folder in Settings first", "先に設定でプロジェクトフォルダを選んでください"),
    ("{name} · continuing", "{name} · 続き"), ("Undo", "取り消し"), ("Dictation", "音声入力"), ("Routine", "ルーティン"),
    ("Nothing to undo", "取り消すものはありません"), ("Undid: {items}", "元に戻しました：{items}"), ("Couldn't undo: {items}", "元に戻せません：{items}"), ("Undid: {items}. Couldn't undo: {failed}", "元に戻しました：{items}。元に戻せません：{failed}"),
    ("Claude Code isn't installed. Install it and sign in, then try again. Settings › Health shows how.", "Claude Code がインストールされていません。インストールしてサインインし、もう一度お試しください。方法は 設定 › 状態 にあります。"),
    ("No microphone found. Connect one, or pick another in Settings › Speech.", "マイクが見つかりません。接続するか、設定 › 音声 で別のマイクを選んでください。"),
    ("The speech model isn't downloaded yet. Download it in Settings › Get started.", "音声モデルがまだダウンロードされていません。設定 › はじめに でダウンロードしてください。"),
    ("That took too long and was stopped. Try a shorter request.", "時間がかかりすぎたため停止しました。短い依頼でお試しください。"),
    ("Can't reach the internet. Check your connection, or switch Speech to On this Mac.", "インターネットに接続できません。接続を確認するか、音声を「この Mac で」に切り替えてください。"),
    ("Cloud speech needs an OpenAI key. Add one in Settings › Speech, or switch to On this Mac.", "クラウド音声には OpenAI キーが必要です。設定 › 音声 で追加するか、「この Mac で」に切り替えてください。"),
    ("Claude Code isn't signed in for this profile: run it in a terminal (e.g. claude-work) and use /login, or pick another profile in Settings", "このプロファイルの Claude Code はサインインしていません：ターミナルで起動して（例：claude-work）/login を使うか、設定で別のプロファイルを選んでください"),
    ("Codex isn't signed in: run `codex login` in a terminal", "Codex はサインインしていません：ターミナルで `codex login` を実行してください"), ("Codex is rate limited; try again shortly", "Codex は利用制限中です。少し待ってからお試しください"),
    ("Let the assistant see {app} for this request", "この依頼でアシスタントに {app} の表示を許可"), ("Let the assistant control {app} for this request", "この依頼でアシスタントに {app} の操作を許可"),
    ("The app I typed into isn't in front any more", "入力したアプリが前面にありません"), ("I can't find what I typed in this field any more", "この欄に入力した文が見つかりません"),
    ("I couldn't select what I typed, so I left it as it is", "入力した文を選択できなかったので、そのままにしました"),
    ("This app doesn't show me its text, so I can only edit right after typing", "このアプリは文字を読み取れないため、入力直後しか直せません"),
    ("Editing by voice uses Claude Code, which isn't installed", "音声での修正には Claude Code が必要ですが、インストールされていません"),
    ("it needs a speech model on this Mac: download one in Speech", "この Mac に音声モデルが必要です：音声 でダウンロードしてください"), ("the microphone stopped", "マイクが停止しました"),
    ("Reading {file}", "{file} を読み込み中"), ("Editing {file}", "{file} を編集中"), ("Searching the project", "プロジェクトを検索中"), ("Planning", "計画中"),
    ("Delegating a subtask", "サブタスクを委任中"), ("Using {tool}", "{tool} を使用中"), ("Running: {command}", "実行中：{command}"), ("Editing files", "ファイルを編集中"), ("Searching the web", "ウェブを検索中"),
    ("run {command}", "{command} を実行"), ("open {host}", "{host} にアクセス"), ("search the web for “{query}”", "ウェブで「{query}」を検索"), ("edit {file}", "{file} を編集"), ("read {file}", "{file} を参照"), ("use {tool}", "{tool} を使用"),
    ("deletes files", "ファイルを削除します"), ("runs as administrator", "管理者として実行します"), ("downloads and runs code", "コードをダウンロードして実行します"), ("pushes to a remote repository", "リモートリポジトリにプッシュします"),
    ("discards work", "作業内容を破棄します"), ("publishes a package", "パッケージを公開します"), ("can erase a disk", "ディスクを消去する可能性があります"), ("stops the system or other programs", "システムや他のプログラムを停止します"),
    ("changes permissions widely", "権限を広範囲に変更します"), ("deletes database data", "データベースのデータを削除します"), ("deletes infrastructure", "インフラを削除します"), ("can freeze the computer", "コンピュータを固まらせる可能性があります"),
    ("Looking at your apps", "アプリを確認中"), ("Opening {name}", "{name} を開いています"), ("Quit {name}", "{name} を終了"), ("the page", "ページ"), ("Searching the web for “{query}”", "ウェブで「{query}」を検索中"),
    ("Playing", "再生"), ("Pausing", "一時停止"), ("Next track", "次の曲"), ("Previous track", "前の曲"), ("Playing “{query}”", "「{query}」を再生中"), ("Muting", "消音"), ("Unmuting", "消音を解除"),
    ("Volume {level}%", "音量 {level}%"), ("Dark mode on", "ダークモード オン"), ("Dark mode off", "ダークモード オフ"), ("Looking at your Shortcuts", "ショートカットを確認中"), ("Run the shortcut “{name}”", "ショートカット「{name}」を実行"),
    ("Reminder: {title} · {when}", "リマインダー：{title} · {when}"), ("Reminder: {title}", "リマインダー：{title}"), ("Event: {title} · {when}", "予定：{title} · {when}"), ("Note: {title}", "メモ：{title}"),
    ("Draft email to {to} +{more}: {subject}", "{to} ほか {more} 名宛ての下書き：{subject}"), ("Draft email to {to}: {subject}", "{to} 宛ての下書き：{subject}"), ("Draft email: {subject}", "メールの下書き：{subject}"),
    ("Looking at open windows", "開いているウインドウを確認中"), ("Reading {app}", "{app} を読み込み中"), ("Looking at {app}", "{app} を確認中"),
    ("Clicking", "クリック："), ("Double-clicking", "ダブルクリック："), ("Right-clicking", "右クリック："), ("Pointing at", "ポイント："),
    ("{verb} control {n}", "{verb}項目 {n}"), ("{verb} ({x}, {y}) in {app}", "{app} で{verb}({x}, {y})"), ("{verb} page item {n}", "{verb}ページ項目 {n}"), ("{verb} {label} in {app}", "{app} で{verb}{label}"),
    ("Typing “{text}” into control {n}", "項目 {n} に「{text}」を入力中"), ("Typing “{text}” into control {n} and pressing Return", "項目 {n} に「{text}」を入力して Return"),
    ("Pressing {keys} in {app}", "{app} で {keys} を押しています"), ("Menu {path} in {app}", "{app} のメニュー {path}"), ("Scrolling down in {app}", "{app} を下にスクロール"), ("Scrolling up in {app}", "{app} を上にスクロール"),
    ("Dragging control {from} onto control {to}", "項目 {from} を項目 {to} にドラッグ"), ("Waiting for “{text}” in {app}", "{app} に「{text}」が出るのを待っています"), ("Waiting {seconds} s for {app}", "{app} を {seconds} 秒待っています"),
    ("Reading the page in {app}", "{app} のページを読み込み中"), ("Filling page item {n} with “{text}”", "ページ項目 {n} に「{text}」を入力"), ("Filling page item {n} with “{text}” and submitting", "ページ項目 {n} に「{text}」を入力して送信"),
    ("Copying “{text}”", "「{text}」をコピー"), ("Reading the clipboard", "クリップボードを読み込み中"), ("Finding files named “{query}”", "「{query}」という名前のファイルを検索中"), ("Opening {file} in {app}", "{file} を {app} で開いています"),
    ("Move {items} to {dest}", "{items} を {dest} に移動"), ("Rename {file} to {name}", "{file} の名前を {name} に変更"), ("Move {items} to the Trash", "{items} をゴミ箱に入れる"), ("New folder “{name}” in {parent}", "{parent} に新しいフォルダ「{name}」"),
    ("{count} items", "{count} 項目"), ("Remembering: {text}", "記憶：{text}"), ("Forgetting “{text}”", "「{text}」を忘れます"), ("Save the routine “{name}”: {steps}", "ルーティン「{name}」を保存：{steps}"),
    ("Typing “{text}” into {label} in {app}", "{app} の{label}に「{text}」を入力中"), ("Typing “{text}” into {label} in {app}, then Return", "{app} の{label}に「{text}」を入力して Return"),
    ("Filling {label} in {app} with “{text}”", "{app} の{label}に「{text}」を入力"), ("Filling {label} in {app} with “{text}”, then submitting", "{app} の{label}に「{text}」を入力して送信"), ("Dragging {label} in {app}", "{app} で{label}をドラッグ"),
    ("Say “always” to stop asking, “yes” for now, or press", "「いつも」で今後は確認しません。「はい」で今回だけ、または押してください"),
    ("{question}? Say always, yes, or no.", "{question}？ いつも、はい、いいえで答えてください。"),
    ("{app} is allowed from now on. Change it in Settings › Apps.", "{app} を今後は許可します。設定 › アプリ で変更できます。"),
    ("What you said is kept in Settings › History.", "話した内容は 設定 › 履歴 に保存されています。"),
    ("That selection is too long to rewrite by voice; select a shorter part", "選択範囲が長すぎて音声で書き直せません。短い部分を選択してください"),
    ("The selection changed, so I left it as it is", "選択範囲が変わったので、そのままにしました"),
    ("This app doesn't let me replace text with several lines; select one paragraph", "このアプリでは複数行のテキストを置き換えられません。1つの段落を選択してください"),
    ("“{word}…” is too long for a word (at most {max} characters)", "「{word}…」は単語として長すぎます（最大 {max} 文字）"),
    ("At most {max} words", "単語は最大 {max} 個まで"),
    ("A replacement needs words to replace (“{to}”)", "置き換えには置き換える語句が必要です（「{to}」）"),
    ("The replacement for “{from}” is too long", "「{from}」の置き換えが長すぎます"),
    ("At most {max} replacements", "置き換えは最大 {max} 個まで"),
    ("A snippet’s phrase needs 1 to 6 words (“{phrase}”)", "スニペットのフレーズは 1〜6 語にしてください（「{phrase}」）"),
    ("The snippet “{phrase}” needs text, at most {max} characters", "スニペット「{phrase}」にはテキストが必要です（最大 {max} 文字）"),
    ("Two snippets have the phrase “{phrase}”", "2つのスニペットが同じフレーズ「{phrase}」を使っています"),
    ("At most {max} snippets", "スニペットは最大 {max} 個まで"),
    ("That suggestion is gone", "その提案はもうありません"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_names_the_computer_in_every_language() {
        assert_eq!(computer_words("en", "Stays on this Mac. Pause it from the menu-bar icon."), "Stays on this computer. Pause it from the tray icon.");
        assert_eq!(computer_words("en", "Turned off in System Settings; your Keychain"), "Turned off in Settings; your keyring");
        assert_eq!(computer_words("en", "macOS and MacBook stay"), "macOS and MacBook stay", "only the word Mac");
        assert_eq!(computer_words("fa", "روی همین مک می‌ماند. کمک"), "روی همین کامپیوتر می‌ماند. کمک", "not inside کمک");
        assert_eq!(computer_words("ar", "على هذا الماك"), "على هذا الحاسوب");
        assert_eq!(computer_words("fr", "Reste sur ce Mac. Contrôler le Mac. votre Mac"), "Reste sur cet ordinateur. Contrôler l’ordinateur. votre ordinateur");
        assert_eq!(computer_words("de", "Auf diesem Mac"), "Auf diesem Computer");
        assert_eq!(computer_words("zh", "在这台 Mac 上"), "在这台电脑上");
        assert_eq!(computer_words("ja", "この Mac で"), "このパソコンで");
        assert_eq!(computer_words("es", "En este Mac"), "En este ordenador");
    }

    const SOURCES: &[&str] = &[
        include_str!("controller.rs"), include_str!("health.rs"), include_str!("inject.rs"), include_str!("dictation.rs"), include_str!("wake.rs"), include_str!("vocab.rs"),
        include_str!("mac/tools.rs"), include_str!("mac/server.rs"), include_str!("mac/undo.rs"),
        include_str!("agents/claude.rs"), include_str!("agents/codex.rs"), include_str!("agents/approvals.rs"),
    ];

    /// Every message key: the string literals in the first argument of each `tr(…)` / `trf(…)`
    /// call, including the arms of a `tr(match …)` or `tr(if …)`.
    fn used() -> Vec<String> {
        let mut out = Vec::new();
        for src in SOURCES {
            let b = src.as_bytes();
            for marker in ["tr(", "trf("] {
                for (i, _) in src.match_indices(marker) {
                    if src[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == ':') && !src[..i].ends_with("i18n::") {
                        continue;
                    }
                    out.extend(first_arg_literals(&b[i + marker.len()..]));
                }
            }
            // The danger phrases are returned as `Some("…")` and translated where shown.
            if src.contains("pub fn dangerous_command") {
                for (i, _) in src.match_indices("return Some(\"") {
                    let rest = &src[i + 13..];
                    out.push(rest[..rest.find('"').unwrap()].to_string());
                }
            }
        }
        out.retain(|s| !s.is_empty());
        out.sort();
        out.dedup();
        out
    }

    /// String literals up to the end of the first argument (a top-level comma or the closing paren).
    fn first_arg_literals(b: &[u8]) -> Vec<String> {
        let (mut depth, mut i, mut out) = (0i32, 0usize, Vec::new());
        while i < b.len() {
            match b[i] {
                b'"' => {
                    let start = i + 1;
                    i += 1;
                    while b[i] != b'"' {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                    out.push(String::from_utf8(b[start..i].to_vec()).unwrap());
                }
                b'\'' if b.get(i + 2) == Some(&b'\'') => i += 2,
                b'(' | b'{' | b'[' => depth += 1,
                b')' | b'}' | b']' if depth == 0 => break,
                b')' | b'}' | b']' => depth -= 1,
                b',' if depth == 0 => break,
                _ => {}
            }
            i += 1;
        }
        out
    }

    fn placeholders(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s.match_indices('{').filter_map(|(i, _)| s[i + 1..].find('}').map(|j| s[i + 1..i + 1 + j].to_string())).collect();
        v.sort();
        v
    }

    #[test]
    fn every_message_is_translated_with_the_same_placeholders() {
        let used = used();
        assert!(used.len() > 100, "found only {} messages", used.len());
        for lang in &LANGS[1..] {
            for en in &used {
                let en: &'static str = Box::leak(en.clone().into_boxed_str());
                let got = lookup(lang, en);
                let table = [("fa", FA), ("es", ES), ("de", DE), ("fr", FR), ("ar", AR), ("zh", ZH), ("ja", JA)].into_iter().find(|(l, _)| l == lang).unwrap().1;
                assert!(table.iter().any(|(k, _)| *k == en), "{lang}: no translation for {en:?}");
                assert_eq!(placeholders(got), placeholders(en), "{lang}: placeholders differ for {en:?}");
            }
        }
    }

    #[test]
    fn tables_have_no_unused_or_duplicate_keys() {
        let used = used();
        for (lang, table) in [("fa", FA), ("es", ES), ("de", DE), ("fr", FR), ("ar", AR), ("zh", ZH), ("ja", JA)] {
            let mut keys: Vec<&str> = table.iter().map(|(k, _)| *k).collect();
            for k in &keys {
                assert!(used.iter().any(|u| u == k), "{lang}: {k:?} is not used anywhere");
            }
            let n = keys.len();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), n, "{lang}: a key appears twice");
        }
    }

    #[test]
    fn placeholders_are_filled_and_english_is_the_fallback() {
        assert_eq!(fill(lookup("fa", "Opening {name}"), &[("name", "Notes")]), "باز کردن Notes");
        assert_eq!(fill(lookup("de", "Ran {name}: {count} steps"), &[("name", "Start"), ("count", "3")]), "Start ausgeführt: 3 Schritte");
        assert_eq!(lookup("xx", "Opening {name}"), "Opening {name}");
        assert_eq!(lookup("en", "Done"), "Done");
    }
}
