//! Installed apps: only `.app` bundles in the standard Applications folders can be opened by name,
//! so a spoken name can never become an arbitrary path. Matching is forgiving (speech is fuzzy)
//! but never guesses between several candidates.
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub name: String,
    pub path: PathBuf,
}

const SYSTEM_APPS: &[&str] = &[
    "/System/Library/CoreServices/Finder.app",
    "/System/Library/CoreServices/Applications/Screen Sharing.app",
    "/System/Library/CoreServices/Applications/Archive Utility.app",
    "/System/Library/CoreServices/Applications/About This Mac.app",
];

fn roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities"].iter().map(PathBuf::from).collect();
    if let Some(h) = dirs::home_dir() {
        v.push(h.join("Applications"));
    }
    v
}

/// All apps in the Applications folders, plus one level of sub-folders (e.g. "/Applications/Adobe X/").
pub fn installed() -> Vec<App> {
    let mut out: Vec<App> = Vec::new();
    for root in roots() {
        scan(&root, 1, &mut out);
    }
    // System apps that live outside the Applications folders.
    for extra in SYSTEM_APPS {
        let p = Path::new(extra);
        if p.is_dir() {
            if let Some(name) = p.file_stem().map(|n| n.to_string_lossy().to_string()) {
                out.push(App { name, path: p.to_path_buf() });
            }
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

fn scan(dir: &Path, depth: u8, out: &mut Vec<App>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten().take(2000) {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if let Some(stem) = name.strip_suffix(".app") {
            if ft.is_dir() && !stem.is_empty() {
                out.push(App { name: stem.to_string(), path: p });
            }
        } else if ft.is_dir() && depth > 0 {
            scan(&p, depth - 1, out);
        }
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Finds the app the user meant: exact name, then a unique prefix, then a unique "contains".
/// Several candidates is an error that lists them, so the model can ask or pick explicitly.
pub fn find<'a>(apps: &'a [App], spoken: &str) -> Result<&'a App, String> {
    pick(apps, |a| &a.name, spoken).map_err(|e| e.unwrap_or_else(|| format!("no installed app is called \"{spoken}\". Call list_apps to see the names.")))
}

/// The matcher behind `find`, for any list with names. Err(None) = no match; Err(Some) = ambiguous.
pub fn pick<'a, T>(items: &'a [T], name: impl Fn(&T) -> &str, spoken: &str) -> Result<&'a T, Option<String>> {
    let q = norm(spoken.trim_end_matches(".app"));
    if q.is_empty() {
        return Err(Some("no app name".into()));
    }
    let by = |f: &dyn Fn(&str) -> bool| items.iter().filter(|a| f(&norm(name(a)))).collect::<Vec<_>>();
    for hits in [by(&|n| n == q), by(&|n| n.starts_with(&q)), by(&|n| n.contains(&q))] {
        match hits.as_slice() {
            [] => continue,
            [one] => return Ok(one),
            many => {
                let mut names: Vec<&str> = many.iter().take(6).map(|a| name(a)).collect();
                names.dedup();
                return Err(Some(format!("several apps match \"{spoken}\": {}. Use the exact name.", names.join(", "))));
            }
        }
    }
    Err(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps() -> Vec<App> {
        ["Spotify", "Safari", "Safari Technology Preview", "Visual Studio Code", "Notes", "Notion", "Google Chrome"]
            .iter()
            .map(|n| App { name: n.to_string(), path: PathBuf::from(format!("/Applications/{n}.app")) })
            .collect()
    }

    #[test]
    fn matching() {
        let a = apps();
        assert_eq!(find(&a, "spotify").unwrap().name, "Spotify");
        assert_eq!(find(&a, "Safari").unwrap().name, "Safari", "exact beats prefix");
        assert_eq!(find(&a, "visual studio").unwrap().name, "Visual Studio Code");
        assert_eq!(find(&a, "chrome").unwrap().name, "Google Chrome");
        assert_eq!(find(&a, "Spotify.app").unwrap().name, "Spotify");
        let e = find(&a, "no").unwrap_err();
        assert!(e.contains("Notes") && e.contains("Notion"), "{e}");
        assert!(find(&a, "Photoshop").unwrap_err().contains("list_apps"));
        assert!(find(&a, "  ").is_err());
    }

    #[test]
    fn finds_real_apps_on_macos() {
        if cfg!(target_os = "macos") {
            let all = installed();
            assert!(all.iter().any(|a| a.name == "Calculator"), "Calculator ships with macOS");
            assert!(all.iter().all(|a| a.path.extension().is_some_and(|e| e == "app")));
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn finder_is_an_installed_app() {
        let all = installed();
        assert!(find(&all, "Finder").is_ok(), "Finder lives in CoreServices, not Applications");
    }
}
