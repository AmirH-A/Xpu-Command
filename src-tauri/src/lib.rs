//! Xpu Command: hold a mouse button, talk to your AI coding agent.
mod activity;
mod agents;
mod audio;
mod commands;
mod controller;
mod cues;
mod error;
mod frame;
mod inject;
mod knock;
mod mac;
mod hotkey;
mod input;
mod overlay_window;
mod permissions;
mod sanitize;
mod secrets;
mod settings;
mod speech;
mod stt;
mod vad;
#[cfg(debug_assertions)]
mod selftest;

use controller::Controller;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,whisper_rs=error")).init();
    whisper_rs::install_logging_hooks();

    #[cfg(debug_assertions)]
    if selftest::maybe_run() {
        return;
    }

    let s = settings::load();
    let hook: &'static input::HookConfig = Box::leak(Box::new(input::HookConfig::new(s.button)));
    let knock_cfg: &'static knock::KnockConfig = Box::leak(Box::new(knock::KnockConfig::new(s.knock, s.knock_sensitivity)));
    let ctl = Arc::new(Controller::new(s, hook, knock_cfg));

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(ctl.clone())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::set_cloud_key,
            commands::clear_cloud_key,
            commands::has_cloud_key,
            commands::permission_status,
            commands::open_permission_settings,
            commands::model_status,
            commands::download_model,
            commands::pick_project,
            commands::agent_status,
            commands::input_devices,
            commands::claude_profiles,
            commands::knock_status,
            commands::activity_recent,
            commands::app_access_list,
            commands::request_screen_recording,
            commands::activity_clear,
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let open = MenuItem::with_id(app, "open", "Settings…", true, None::<&str>)?;
            let pause = MenuItem::with_id(app, "pause", "Pause talk button", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Xpu Command", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &pause, &quit])?;
            let hook_cfg = ctl.hook;
            let pause_item = pause.clone();
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().cloned().ok_or("no icon")?)
                .tooltip("Xpu Command")
                .menu(&menu)
                .on_menu_event(move |app, e| match e.id().as_ref() {
                    "open" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "pause" => {
                        let on = !hook_cfg.enabled.load(std::sync::atomic::Ordering::Relaxed);
                        hook_cfg.enabled.store(on, std::sync::atomic::Ordering::Relaxed);
                        let _ = pause_item.set_text(if on { "Pause talk button" } else { "Resume talk button" });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            ctl.preload();
            mac::clean_stale_configs();
            let hk = ctl.settings.read().hotkey;
            match hotkey::apply(app.handle(), hk) {
                Ok(()) => log::info!("keyboard shortcut ready: {hk:?}"),
                Err(e) => log::warn!("{e}"),
            }
            if settings::first_run() {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            #[cfg(debug_assertions)]
            if std::env::var_os("XPU_OVERLAY_DEMO").is_some() {
                let (c, h) = (ctl.clone(), app.handle().clone());
                std::thread::spawn(move || {
                    use controller::Phase::*;
                    let step = |p, t: &str, a: Option<&str>| {
                        c.overlay(&h, p, t, a);
                        std::thread::sleep(std::time::Duration::from_millis(1500));
                    };
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    step(Listening, "Listening…", None);
                    step(Transcribing, "Transcribing…", None);
                    step(Working, "Editing login.py", Some("Claude Code"));
                    step(Done, "Added input validation. Tests pass.", Some("Claude Code"));
                    step(Hidden, "", None);
                    log::info!("overlay demo finished");
                });
            }

            if ctl.settings.read().knock {
                knock_start(app.handle(), &ctl);
            }

            let (tx, rx) = std::sync::mpsc::channel();
            let handle = app.handle().clone();
            let c = ctl.clone();
            std::thread::Builder::new().name("xpu-buttons".into()).spawn(move || {
                for ev in rx {
                    c.on_button(&handle, ev);
                }
            })?;
            let handle = app.handle().clone();
            std::thread::Builder::new().name("xpu-hook-starter".into()).spawn(move || {
                let mut asked = false;
                loop {
                    match input::start(hook, tx.clone()) {
                        Ok(()) => {
                            log::info!("talk button ready");
                            return;
                        }
                        Err(e) => {
                            if !asked {
                                log::warn!("{e}");
                                permissions::accessibility_trusted(true);
                                if let Some(w) = handle.get_webview_window("main") {
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                }
                                asked = true;
                            }
                            std::thread::sleep(std::time::Duration::from_secs(2));
                        }
                    }
                }
            })?;
            Ok(())
        })
        .on_window_event(|w, e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                if w.label() == "main" {
                    api.prevent_close();
                    let _ = w.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Xpu Command");
}

/// Starts the motion-sensor thread once; later on/off is the `enabled` flag. Logs when unsupported.
pub(crate) fn knock_start(app: &tauri::AppHandle, ctl: &Arc<Controller>) {
    let r = KNOCK_STARTED.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (handle, c) = (app.clone(), ctl.clone());
        std::thread::Builder::new()
            .name("xpu-knocks".into())
            .spawn(move || {
                for ev in rx {
                    match ev {
                        knock::Event::DoubleKnock { span_ms } => {
                            log::info!("double knock");
                            c.on_knock(&handle, span_ms);
                        }
                        knock::Event::Tap(g) => log::debug!("tap {g:.3} g"),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        knock::start(ctl.knock, tx).map_err(|e| e.to_string())
    });
    match r {
        Ok(()) => log::info!("double knock ready"),
        Err(e) => log::warn!("double knock unavailable: {e}"),
    }
}

static KNOCK_STARTED: std::sync::OnceLock<std::result::Result<(), String>> = std::sync::OnceLock::new();

/// Why double knock doesn't work here, if it was tried and failed.
pub(crate) fn knock_error() -> Option<String> {
    KNOCK_STARTED.get().and_then(|r| r.clone().err())
}
