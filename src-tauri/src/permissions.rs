//! OS permissions the app depends on, checked rather than assumed.
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PermissionStatus {
    /// macOS: needed to capture (swallow) the talk button and to type into other apps.
    pub accessibility: bool,
    /// Whether this OS needs the Accessibility grant at all.
    pub accessibility_required: bool,
    /// macOS: screenshots of other apps' windows ("Control my Mac" fallback only).
    pub screen_recording: bool,
    pub microphone: Mic,
    pub needs_move: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mic {
    Allowed,
    NotAsked,
    Denied,
}

pub fn status() -> PermissionStatus {
    PermissionStatus {
        accessibility: accessibility_trusted(false),
        accessibility_required: cfg!(target_os = "macos"),
        screen_recording: screen_recording(),
        microphone: microphone(),
        needs_move: running_bundle().is_some_and(|b| needs_move(&b)),
    }
}

pub const BUNDLE_ID: &str = "app.xpucommand.desktop";

pub fn running_bundle() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(std::path::Path::to_path_buf)
}

pub fn needs_move(bundle: &std::path::Path) -> bool {
    let s = bundle.to_string_lossy();
    s.contains("/AppTranslocation/") || s.starts_with("/Volumes/") || s.contains("/Downloads/")
}

#[cfg(target_os = "macos")]
pub fn reset(service: &str) {
    let r = crate::mac::exec::run_program("/usr/bin/tccutil", &["reset".as_ref(), service.as_ref(), BUNDLE_ID.as_ref()], std::time::Duration::from_secs(10));
    log::info!("reset {service} permission: {r:?}");
}

#[cfg(target_os = "macos")]
pub fn move_to_applications() -> crate::error::Result<std::path::PathBuf> {
    use crate::error::msg;
    let src = running_bundle().ok_or_else(|| msg("Xpu Command isn't running from an app bundle"))?;
    let system = std::path::PathBuf::from("/Applications");
    let dir = if writable(&system) { system } else { dirs::home_dir().ok_or_else(|| msg("no home folder"))?.join("Applications") };
    move_bundle(&src, &dir)
}

#[cfg(target_os = "macos")]
pub fn move_bundle(src: &std::path::Path, dir: &std::path::Path) -> crate::error::Result<std::path::PathBuf> {
    use crate::error::msg;
    let name = src.file_name().ok_or_else(|| msg("odd app path"))?.to_os_string();
    std::fs::create_dir_all(dir)?;
    let dst = dir.join(&name);
    if dst == src {
        return Ok(dst);
    }
    let staged = dir.join(format!(".{}.new", name.to_string_lossy()));
    let _ = std::fs::remove_dir_all(&staged);
    let run = |prog: &str, args: &[&std::ffi::OsStr]| crate::mac::exec::run_program(prog, args, std::time::Duration::from_secs(120)).map_err(msg);
    run("/usr/bin/ditto", &[src.as_os_str(), staged.as_os_str()])?;
    if !staged.join("Contents").is_dir() {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(msg("the copy is incomplete; nothing was replaced"));
    }
    let _ = run("/usr/bin/xattr", &["-dr".as_ref(), "com.apple.quarantine".as_ref(), staged.as_os_str()]);
    if dst.exists() {
        std::fs::remove_dir_all(&dst)?;
    }
    std::fs::rename(&staged, &dst)?;
    Ok(dst)
}

#[cfg(target_os = "macos")]
fn writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!(".xpu-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[cfg(target_os = "macos")]
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeAudio: &'static objc2_foundation::NSString;
}

#[cfg(target_os = "macos")]
pub fn microphone() -> Mic {
    let status: isize = unsafe { objc2::msg_send![objc2::class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio] };
    match status {
        3 => Mic::Allowed,
        0 => Mic::NotAsked,
        _ => Mic::Denied,
    }
}
#[cfg(not(target_os = "macos"))]
pub fn microphone() -> Mic {
    Mic::Allowed
}

#[cfg(target_os = "macos")]
pub async fn request_microphone() -> bool {
    match microphone() {
        Mic::Allowed => return true,
        Mic::Denied => reset("Microphone"),
        Mic::NotAsked => {}
    }
    if microphone() != Mic::NotAsked {
        let _ = open_settings("microphone");
        return false;
    }
    ask_microphone().await.unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn ask_microphone() -> tokio::sync::oneshot::Receiver<bool> {
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    let tx = std::sync::Mutex::new(Some(tx));
    let done = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(granted.as_bool());
        }
    });
    unsafe {
        let _: () = objc2::msg_send![objc2::class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeAudio, completionHandler: &*done];
    }
    rx
}
#[cfg(not(target_os = "macos"))]
pub async fn request_microphone() -> bool {
    true
}

#[cfg(target_os = "macos")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[cfg(target_os = "macos")]
pub fn screen_recording() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}
#[cfg(not(target_os = "macos"))]
pub fn screen_recording() -> bool {
    false
}

/// Shows the macOS prompt (once; after that macOS only lists the app in System Settings).
#[cfg(target_os = "macos")]
pub fn request_screen_recording() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}
#[cfg(not(target_os = "macos"))]
pub fn request_screen_recording() -> bool {
    false
}

#[cfg(target_os = "macos")]
pub fn accessibility_trusted(prompt: bool) -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
    }
    let key = CFString::from_static_string("AXTrustedCheckOptionPrompt");
    let value = if prompt { CFBoolean::true_value() } else { CFBoolean::false_value() };
    let opts = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef()) }
}

#[cfg(not(target_os = "macos"))]
pub fn accessibility_trusted(_prompt: bool) -> bool {
    true
}

/// macOS secure input (a password field is focused, or a terminal's "Secure Keyboard Entry"):
/// we must not type then, and macOS would drop synthetic keys anyway.
#[cfg(target_os = "macos")]
pub fn secure_input_active() -> bool {
    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn IsSecureEventInputEnabled() -> bool;
    }
    unsafe { IsSecureEventInputEnabled() }
}
#[cfg(not(target_os = "macos"))]
pub fn secure_input_active() -> bool {
    false
}

/// Opens the system settings page for a permission. Fixed URLs only, launched without a shell.
pub fn open_settings(which: &str) -> crate::error::Result<()> {
    #[cfg(target_os = "macos")]
    let target = match which {
        "accessibility" => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        "microphone" => "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        "screen" => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        "automation" => "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
        _ => return Err(crate::error::msg("unknown permission")),
    };
    #[cfg(target_os = "windows")]
    let target = match which {
        "microphone" => "ms-settings:privacy-microphone",
        _ => return Err(crate::error::msg("unknown permission")),
    };
    let _guard = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    #[cfg(target_os = "macos")]
    std::process::Command::new("/usr/bin/open").arg(target).spawn()?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer.exe").arg(target).spawn()?;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = which;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn temporary_copies_need_moving() {
        assert!(needs_move(Path::new("/private/var/folders/cz/x/T/AppTranslocation/FEB/d/Xpu Command.app")));
        assert!(needs_move(Path::new("/Volumes/Xpu Command/Xpu Command.app")));
        assert!(needs_move(Path::new("/Users/a/Downloads/Xpu Command.app")));
        assert!(!needs_move(Path::new("/Applications/Xpu Command.app")));
        assert!(!needs_move(Path::new("/Users/a/Applications/Xpu Command.app")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn moving_replaces_the_old_copy_and_clears_quarantine() {
        let root = std::env::temp_dir().join(format!("xpu-move-{}", uuid::Uuid::new_v4().simple()));
        let src = root.join("Downloads").join("Xpu Command.app");
        std::fs::create_dir_all(src.join("Contents/MacOS")).unwrap();
        std::fs::write(src.join("Contents/MacOS/xpu-command"), b"new").unwrap();
        let q = std::process::Command::new("/usr/bin/xattr").args(["-w", "com.apple.quarantine", "0081;00000000;Safari;"]).arg(&src).status().unwrap();
        assert!(q.success());
        let apps = root.join("Applications");
        std::fs::create_dir_all(apps.join("Xpu Command.app/Contents/MacOS")).unwrap();
        std::fs::write(apps.join("Xpu Command.app/Contents/MacOS/xpu-command"), b"old").unwrap();
        std::fs::write(apps.join("Xpu Command.app/Contents/stale.txt"), b"x").unwrap();

        let dst = move_bundle(&src, &apps).unwrap();
        assert_eq!(dst, apps.join("Xpu Command.app"));
        assert_eq!(std::fs::read(dst.join("Contents/MacOS/xpu-command")).unwrap(), b"new");
        assert!(!dst.join("Contents/stale.txt").exists(), "the old copy is replaced, not merged");
        let attrs = std::process::Command::new("/usr/bin/xattr").arg(&dst).output().unwrap();
        assert!(!String::from_utf8_lossy(&attrs.stdout).contains("quarantine"));
        assert!(!apps.join(".Xpu Command.app.new").exists());
        assert_eq!(move_bundle(&dst, &apps).unwrap(), dst, "already there: nothing to do");
        std::fs::remove_dir_all(root).unwrap();
    }
}
