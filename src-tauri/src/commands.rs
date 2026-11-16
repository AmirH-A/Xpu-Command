//! Commands the settings window may call (and only it: see capabilities/). Every input is
//! validated here; nothing returns a secret.
use crate::controller::Controller;
use crate::error::{msg, Result};
use crate::settings::{self, Settings};
use crate::{agents, permissions, secrets, stt::model};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use zeroize::Zeroizing;

type Ctl<'a> = State<'a, Arc<Controller>>;

#[tauri::command]
pub fn get_settings(ctl: Ctl) -> Settings {
    ctl.settings.read().clone()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, ctl: Ctl, new: Settings) -> Result<Settings> {
    let mut new = new;
    if let Some(d) = &new.project_dir {
        new.project_dir = Some(settings::validate_project_dir(d)?);
    }
    settings::save(&new)?;
    if new.hotkey != ctl.settings.read().hotkey {
        crate::hotkey::apply(&app, new.hotkey)?;
    }
    if new.airpods != ctl.settings.read().airpods {
        crate::airpods::apply(&app, new.airpods);
    }
    ctl.hook.set_button(new.button);
    ctl.knock.set_sensitivity(new.knock_sensitivity);
    ctl.knock.enabled.store(new.knock, std::sync::atomic::Ordering::Relaxed);
    *ctl.settings.write() = new.clone();
    if new.knock {
        crate::knock_start(&app, ctl.inner());
    }
    ctl.preload();
    Ok(new)
}

#[tauri::command]
pub fn set_cloud_key(key: String) -> Result<()> {
    let key = Zeroizing::new(key.trim().to_string());
    if !secrets::looks_like_openai_key(&key) {
        return Err(msg("that doesn't look like an OpenAI API key (sk-…)"));
    }
    secrets::set(secrets::OPENAI_KEY, key)
}

#[tauri::command]
pub fn clear_cloud_key() -> Result<()> {
    secrets::delete(secrets::OPENAI_KEY)
}

#[tauri::command]
pub fn has_cloud_key() -> bool {
    matches!(secrets::get(secrets::OPENAI_KEY), Ok(Some(_)))
}

#[tauri::command]
pub fn permission_status() -> permissions::PermissionStatus {
    permissions::status()
}

#[tauri::command]
pub async fn move_to_applications(app: AppHandle) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || move_and_relaunch(&app)).await.map_err(|_| msg("the move was interrupted"))?
}

#[cfg(target_os = "macos")]
pub fn move_and_relaunch(app: &AppHandle) -> Result<()> {
    let dst = permissions::move_to_applications()?;
    log::info!("moved to {}", dst.display());
    let pid = std::process::id().to_string();
    let _guard = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::process::Command::new("/bin/sh")
        .args(["-c", "while /bin/kill -0 \"$1\" 2>/dev/null; do /bin/sleep 0.2; done; /usr/bin/open \"$2\"", "relaunch"])
        .arg(&pid)
        .arg(&dst)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| msg(format!("moved, but couldn't reopen it: {e}")))?;
    drop(_guard);
    app.exit(0);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn move_and_relaunch(_app: &AppHandle) -> Result<()> {
    Err(msg("only on macOS"))
}

const LINKS: &[(&str, &str)] = &[
    ("privacy", "https://github.com/XPU-LAB/xpu-command/blob/main/PRIVACY.md"),
    ("terms", "https://github.com/XPU-LAB/xpu-command/blob/main/TERMS.md"),
    ("license", "https://github.com/XPU-LAB/xpu-command/blob/main/LICENSE"),
    ("issues", "https://github.com/XPU-LAB/xpu-command/issues/new"),
    ("website", "https://xpu-lab.github.io/xpu-command/"),
    ("claude-install", "https://docs.anthropic.com/en/docs/claude-code/setup"),
];

#[tauri::command]
pub fn health_check(ctl: Ctl) -> Vec<crate::health::Check> {
    let s = ctl.settings.read().clone();
    crate::health::run(&s)
}

#[tauri::command]
pub fn crash_pending() -> Option<crate::health::crash::Crash> {
    crate::health::crash::pending()
}

#[tauri::command]
pub fn crash_report() -> Result<()> {
    let url = crate::health::crash::report_url().ok_or_else(|| msg("no crash report"))?;
    let _guard = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    #[cfg(target_os = "macos")]
    std::process::Command::new("/usr/bin/open").arg(&url).spawn().map_err(|e| msg(e.to_string()))?;
    drop(_guard);
    crate::health::crash::dismiss();
    Ok(())
}

#[tauri::command]
pub fn crash_dismiss() {
    crate::health::crash::dismiss();
}

#[tauri::command]
pub async fn diagnostics_zip(ctl: Ctl<'_>) -> Result<String> {
    let s = ctl.settings.read().clone();
    let zip = tauri::async_runtime::spawn_blocking(move || crate::health::diagnostics_zip(&s)).await.map_err(|_| msg("couldn't make the file"))??;
    #[cfg(target_os = "macos")]
    {
        let _guard = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(&zip).spawn();
    }
    Ok(zip.display().to_string())
}

#[tauri::command]
pub fn open_link(which: String) -> Result<()> {
    let url = LINKS.iter().find(|(k, _)| *k == which).map(|(_, u)| *u).ok_or_else(|| msg("unknown link"))?;
    let _guard = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    #[cfg(target_os = "macos")]
    std::process::Command::new("/usr/bin/open").arg(url).spawn().map_err(|e| msg(e.to_string()))?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer").arg(url).spawn().map_err(|e| msg(e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub async fn request_microphone() -> bool {
    permissions::request_microphone().await
}

#[tauri::command]
pub fn open_permission_settings(which: String) -> Result<()> {
    if which == "accessibility" && !permissions::accessibility_trusted(false) {
        #[cfg(target_os = "macos")]
        permissions::reset("Accessibility");
        permissions::accessibility_trusted(true);
    }
    permissions::open_settings(&which)
}

#[derive(Serialize)]
pub struct ModelRow {
    id: &'static str,
    label: &'static str,
    downloaded: bool,
}

#[tauri::command]
pub fn model_status() -> Vec<ModelRow> {
    model::CATALOG.iter().map(|m| ModelRow { id: m.id, label: m.label, downloaded: model::is_downloaded(m) }).collect()
}

#[tauri::command]
pub async fn download_model(app: AppHandle, id: String) -> Result<()> {
    let m = model::find(&id).ok_or_else(|| msg("unknown model"))?;
    let mut last = 0u64;
    model::download(m, move |done, total| {
        let pct = done * 100 / total.max(1);
        if pct != last {
            last = pct;
            let _ = app.emit_to("main", "model-progress", (m.id, pct));
        }
    })
    .await?;
    Ok(())
}

#[tauri::command]
pub async fn pick_project(app: AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("Choose the project folder the agent may work in").pick_folder(move |p| {
        let _ = tx.send(p);
    });
    let Some(p) = rx.await.map_err(|_| msg("dialog closed"))? else { return Ok(None) };
    let p = p.into_path().map_err(|_| msg("not a local folder"))?;
    Ok(Some(settings::validate_project_dir(&p)?.display().to_string()))
}

#[tauri::command]
pub async fn pick_custom_agent(app: AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("Choose the agent program (it reads the request on stdin)").pick_file(move |p| {
        let _ = tx.send(p);
    });
    let Some(p) = rx.await.map_err(|_| msg("dialog closed"))? else { return Ok(None) };
    let p = p.into_path().map_err(|_| msg("not a local file"))?;
    Ok(Some(crate::agents::custom::validate_command(&p)?.display().to_string()))
}

#[tauri::command]
pub fn claude_profiles() -> Vec<String> {
    settings::claude_profiles()
}

/// None when double knock works (or is off and untried); otherwise why it can't work here.
#[tauri::command]
pub fn knock_status() -> Option<String> {
    crate::knock_error()
}

#[derive(serde::Serialize)]
pub struct AppRow {
    pub bundle: String,
    pub name: String,
    /// "normal", "terminal" (needs its own setting) or "excluded" (never accessible).
    pub class: &'static str,
}

/// Installed apps for the App access list, with their bundle ids.
#[tauri::command]
pub fn app_access_list() -> Vec<AppRow> {
    #[cfg(target_os = "macos")]
    {
        use crate::mac::policy::{class, Class};
        let mut rows: Vec<AppRow> = crate::mac::apps::installed()
            .into_iter()
            .filter_map(|a| crate::mac::ui::bundle_info(&a.path))
            .filter(|(b, _)| crate::mac::policy::valid_bundle_id(b))
            .map(|(bundle, name)| {
                let class = match class(&bundle) {
                    Class::Excluded => "excluded",
                    Class::Terminal => "terminal",
                    Class::Normal => "normal",
                };
                AppRow { bundle, name, class }
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        rows.dedup_by(|a, b| a.bundle == b.bundle);
        rows
    }
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

/// Asks macOS for Screen Recording (only needed for the screenshot fallback).
#[tauri::command]
pub fn request_screen_recording() -> bool {
    crate::permissions::request_screen_recording()
}

/// The newest activity-log entries (Mac actions), newest first.
#[tauri::command]
pub fn activity_recent() -> Vec<crate::activity::Entry> {
    crate::activity::recent(20)
}

#[tauri::command]
pub fn memory_list() -> Vec<crate::memory::Fact> {
    let mut all = crate::memory::list();
    all.reverse();
    all
}

#[tauri::command]
pub fn memory_add(text: String) -> Result<crate::memory::Fact> {
    crate::memory::add(&text)
}

#[tauri::command]
pub fn memory_remove(id: String) -> Result<bool> {
    crate::memory::remove(&id)
}

#[tauri::command]
pub fn memory_clear() -> Result<()> {
    crate::memory::clear()
}

#[derive(serde::Serialize)]
pub struct RoutineRow {
    #[serde(flatten)]
    routine: crate::routines::Routine,
    labels: Vec<String>,
}

#[tauri::command]
pub fn routines_list() -> Vec<RoutineRow> {
    crate::routines::list().into_iter().map(|r| RoutineRow { labels: r.steps.iter().map(crate::routines::label).collect(), routine: r }).collect()
}

#[tauri::command]
pub fn routine_suggestions() -> Vec<crate::routines::Suggestion> {
    crate::routines::current_suggestions()
}

#[tauri::command]
pub fn routine_save(name: String, steps: Vec<crate::routines::Step>) -> Result<crate::routines::Routine> {
    crate::routines::save(&name, &steps)
}

#[tauri::command]
pub fn routine_dismiss(steps: Vec<crate::routines::Step>) -> Result<()> {
    crate::routines::forget_suggestion(&steps)
}

#[tauri::command]
pub fn routine_remove(id: String) -> Result<bool> {
    crate::routines::remove(&id)
}

#[tauri::command]
pub async fn routine_run(app: AppHandle, ctl: Ctl<'_>, id: String) -> Result<()> {
    let r = crate::routines::list().into_iter().find(|r| r.id == id).ok_or_else(|| crate::error::msg("that routine no longer exists"))?;
    let s = ctl.settings.read().clone();
    let c: Arc<Controller> = ctl.inner().clone();
    c.run_routine(&app, &s, r).await
}

#[tauri::command]
pub fn undo_preview() -> Vec<String> {
    crate::mac::undo::peek_last_request()
}

#[tauri::command]
pub async fn undo_last(ctl: Ctl<'_>) -> Result<String> {
    Ok(ctl.undo().await)
}

#[tauri::command]
pub fn activity_clear() -> Result<()> {
    crate::activity::clear().map_err(Into::into)
}

#[tauri::command]
pub fn input_devices() -> Vec<String> {
    crate::audio::input_devices()
}

#[tauri::command]
pub fn agent_status() -> Vec<agents::AgentInfo> {
    agents::status()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    fn invoked(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for part in text.split("invoke").skip(1) {
            let rest = match part.strip_prefix('<') {
                Some(generic) => match generic.find(">(") {
                    Some(i) => &generic[i + 1..],
                    None => continue,
                },
                None => part,
            };
            if let Some(name) = rest.strip_prefix("('").and_then(|r| r.split('\'').next()) {
                out.insert(name.to_string());
            }
        }
        out
    }

    #[test]
    fn every_command_the_settings_window_calls_is_registered_and_allowed() {
        let ui = include_str!("../../src/main.ts");
        let used = invoked(ui);
        let allowed = include_str!("../permissions/app.toml");
        let registered = include_str!("lib.rs");
        assert!(used.len() > 20, "found {used:?}");
        for c in &used {
            assert!(allowed.contains(&format!("\"{c}\"")), "{c} is called by Settings but missing from permissions/app.toml");
            assert!(registered.contains(&format!("commands::{c},")), "{c} is called by Settings but not registered in lib.rs");
        }
    }
}
