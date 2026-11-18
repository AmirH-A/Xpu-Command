//! Linux desktop services: the session (X11 or Wayland), accessibility (AT-SPI), installed apps
//! (desktop entries), input, media players (MPRIS), portals, the Trash and notifications.
//!
//! Helper programs (wpctl, wl-copy, xdg-open…) are found only in the system's program folders,
//! never through PATH, and started without a shell; arguments are passed after "--" where the
//! program supports it, so no value from speech or the model can become an option or code.
pub mod atspi;
pub mod clipboard;
pub mod desktop;
pub mod keys;
pub mod mpris;
pub mod notify;
pub mod portal;
pub mod screenshot;
pub mod sound;
pub mod system;
pub mod trash;
pub mod x11;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    X11,
    Wayland,
}

/// Which display server this session runs on. Wayland when the compositor says so, even though
/// X11 apps (XWayland) also have DISPLAY set.
pub fn session() -> Session {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"));
    if wayland {
        Session::Wayland
    } else {
        Session::X11
    }
}

/// The desktop environment, lowercase ("gnome", "kde", "xfce", "hyprland"…), from XDG_CURRENT_DESKTOP.
pub fn desktop_name() -> String {
    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase()
}

pub fn is_desktop(name: &str) -> bool {
    desktop_name().split(':').any(|d| d == name)
}

/// System program folders, searched in this order.
const PROGRAM_DIRS: &[&str] = &["/usr/bin", "/bin", "/usr/local/bin", "/usr/sbin", "/sbin", "/run/current-system/sw/bin", "/snap/bin"];

/// A helper program by name, from the system's program folders only.
pub fn program(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return None;
    }
    PROGRAM_DIRS.iter().map(|d| PathBuf::from(d).join(name)).find(|p| is_executable(p))
}

fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Runs a program with a deadline; returns trimmed stdout, or a short error.
pub fn run_program(prog: &str, args: &[&std::ffi::OsStr], timeout: Duration) -> Result<String, String> {
    run_with_input(prog, args, None, timeout)
}

/// Runs a helper by name (see `program`), with a deadline.
pub fn run(name: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let p = program(name).ok_or_else(|| format!("{name} isn't installed"))?;
    let a: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    run_program(&p.to_string_lossy(), &a, timeout)
}

/// Like `run_program`, writing `input` to the program's standard input first.
pub fn run_with_input(prog: &str, args: &[&std::ffi::OsStr], input: Option<&[u8]>, timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new(prog);
    cmd.args(args).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = {
        let _g = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        cmd.spawn().map_err(|e| format!("could not start {prog}: {e}"))?
    };
    if let (Some(data), Some(mut stdin)) = (input, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    let reader = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = r.by_ref().take(256 * 1024).read_to_string(&mut s);
            let _ = std::io::copy(&mut r, &mut std::io::sink());
            s
        })
    };
    let out = reader(Box::new(child.stdout.take().ok_or("no stdout")?));
    let err = reader(Box::new(child.stderr.take().ok_or("no stderr")?));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("it took too long and was stopped".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() {
        Ok(out.trim().to_string())
    } else {
        Err(if err.trim().is_empty() { format!("failed ({status})") } else { crate::agents::clip(err.trim(), 300) })
    }
}

/// Starts a program that keeps running on its own (an app, a clipboard owner): its own session,
/// no inherited input or output, not waited for beyond a quick check that it started.
pub fn spawn_detached(prog: &std::path::Path, args: &[&std::ffi::OsStr], input: Option<&[u8]>) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(prog);
    cmd.args(args).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(home) = dirs::home_dir() {
        cmd.current_dir(home);
    }
    // Detach from our session so the app outlives us and doesn't get our signals.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = {
        let _g = crate::agents::SPAWN_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        cmd.spawn().map_err(|e| format!("could not start {}: {e}", prog.display()))?
    };
    if let (Some(data), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(data).map_err(|e| e.to_string())?;
    }
    // Reap it in the background so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Opens a link or file with the user's default app (xdg-open, else gio).
pub fn open(target: &std::ffi::OsStr) -> Result<(), String> {
    if let Some(p) = program("xdg-open") {
        return spawn_detached(&p, &[target], None);
    }
    if let Some(p) = program("gio") {
        return spawn_detached(&p, &["open".as_ref(), target], None);
    }
    Err("xdg-open isn't installed (package xdg-utils)".into())
}

pub fn parent_pid(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // "pid (comm) state ppid …": comm may hold spaces or parentheses, so split after the last ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_come_only_from_system_folders() {
        assert!(program("../bin/sh").is_none());
        assert!(program("/bin/sh").is_none());
        assert!(program("sh").is_some(), "every Linux has /bin/sh");
    }

    #[test]
    fn run_has_a_deadline_and_reports_errors() {
        assert_eq!(run("echo", &["hi"], Duration::from_secs(5)).unwrap(), "hi");
        assert!(run("sleep", &["5"], Duration::from_millis(200)).unwrap_err().contains("too long"));
        assert!(run("false", &[], Duration::from_secs(5)).is_err());
        let p = program("cat").unwrap();
        assert_eq!(run_with_input(&p.to_string_lossy(), &[], Some(b"in"), Duration::from_secs(5)).unwrap(), "in");
    }
}
