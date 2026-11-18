//! Web pages on Linux. Browsers here have no scripting bridge like the Mac's "JavaScript from
//! Apple Events", so the page tools (read_page, click_page, fill_page) aren't offered: pages are
//! read and operated through accessibility instead (read_window, click, type_text), which Firefox
//! and Chromium browsers support fully. The address and the loading state come from the page's
//! accessible document.
use super::policy::{Need, Target};
use super::ui::RunningApp;
use crate::linux::atspi::{self, role, state, El};

/// Desktop ids of browsers (native packages, Flatpak and Snap).
const BROWSERS: &[&str] = &[
    "firefox", "firefox-esr", "org.mozilla.firefox", "firefox_firefox", "librewolf", "io.gitlab.librewolf-community", "google-chrome", "google-chrome-beta",
    "google-chrome-unstable", "com.google.Chrome", "chromium", "chromium-browser", "chromium_chromium", "org.chromium.Chromium", "brave-browser",
    "com.brave.Browser", "microsoft-edge", "com.microsoft.Edge", "vivaldi-stable", "com.vivaldi.Vivaldi", "org.gnome.Epiphany", "epiphany",
    "zen", "app.zen_browser.zen", "opera", "com.opera.Opera", "org.kde.falkon", "falkon",
];

pub fn is_browser(bundle: &str) -> bool {
    BROWSERS.iter().any(|b| b.eq_ignore_ascii_case(bundle))
}

/// The page on screen in the browser's front window (its accessible document).
fn document(app: &RunningApp) -> Option<El> {
    let win = atspi::applications().into_iter().filter(|a| a.pid() == Some(app.pid)).flat_map(|a| a.children()).find(|w| w.states().has(state::ACTIVE) || w.states().has(state::SHOWING))?;
    atspi::find(&win, 800, |e| matches!(e.role(), Some(role::DOCUMENT_WEB)) && e.states().has(state::SHOWING))
}

/// The front tab's address.
pub fn url(app: &RunningApp) -> Option<String> {
    let doc = document(app)?;
    let u = ["DocURL", "URI", "URL"].iter().find_map(|k| doc.document_attribute(k))?;
    let u = u.trim();
    (u.starts_with("http://") || u.starts_with("https://")).then(|| crate::agents::clip(u, 300))
}

const NOT_HERE: &str = "doesn't let Xpu Command work on pages on Linux: use read_window, click and type_text (browsers show their pages to accessibility)";

/// Page tools aren't available on Linux; this keeps the shared code's calls answering plainly.
#[derive(Default)]
pub struct Pages;

impl Pages {
    pub fn read(&self, app: &RunningApp) -> Result<String, String> {
        Err(format!("{} {NOT_HERE}", app.name))
    }

    pub fn target(&self, _id: u32, _need: Need, _submit: bool) -> Result<Target, String> {
        Err(format!("this browser {NOT_HERE}"))
    }

    pub fn click(&self, _id: u32, _kind: super::tools::ClickKind) -> Result<String, String> {
        Err(format!("this browser {NOT_HERE}"))
    }

    pub fn fill(&self, _id: u32, _text: &str, _submit: bool) -> Result<String, String> {
        Err(format!("this browser {NOT_HERE}"))
    }

    /// Without text: whether the page has finished loading. With text: not answered here, so the
    /// caller looks in the window instead.
    pub fn probe(&self, app: &RunningApp, text: Option<&str>) -> Result<bool, String> {
        if text.is_some() {
            return Err("look in the window".into());
        }
        Ok(document(app).is_some_and(|d| !d.states().has(state::BUSY)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn browsers_are_known_by_desktop_id() {
        assert!(super::is_browser("firefox") && super::is_browser("org.mozilla.firefox") && super::is_browser("google-chrome"));
        assert!(!super::is_browser("org.gnome.Nautilus"));
    }
}
