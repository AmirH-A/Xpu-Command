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
pub fn open_permission_settings(which: String) -> Result<()> {
    if which == "accessibility" {
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
