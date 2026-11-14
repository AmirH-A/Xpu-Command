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
}

pub fn status() -> PermissionStatus {
    PermissionStatus { accessibility: accessibility_trusted(false), accessibility_required: cfg!(target_os = "macos"), screen_recording: screen_recording() }
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
