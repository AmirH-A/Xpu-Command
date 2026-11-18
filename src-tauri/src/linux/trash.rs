//! The Trash, by the freedesktop.org Trash specification (what Files, Dolphin and Nemo use): the
//! item moves into ~/.local/share/Trash/files with a `.trashinfo` record of where it came from,
//! so the file manager can show and restore it, and undo can put it back.
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn trash_dir() -> Result<PathBuf, String> {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => dirs::home_dir().ok_or("no home folder")?.join(".local/share"),
    };
    Ok(base.join("Trash"))
}

/// Percent-encodes a path for the .trashinfo record (RFC 2396 path characters stay).
pub fn encode_path(p: &Path) -> String {
    const KEEP: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'/').remove(b'-').remove(b'_').remove(b'.').remove(b'~');
    use std::os::unix::ffi::OsStrExt;
    percent_encoding::percent_encode(p.as_os_str().as_bytes(), KEEP).to_string()
}

pub fn info_text(original: &Path, when: chrono::NaiveDateTime) -> String {
    format!("[Trash Info]\nPath={}\nDeletionDate={}\n", encode_path(original), when.format("%Y-%m-%dT%H:%M:%S"))
}

/// Moves `p` to the Trash; returns where it now is. Only on the same disk as the home folder.
pub fn trash(p: &Path) -> Result<PathBuf, String> {
    trash_into(p, &trash_dir()?)
}

pub fn trash_into(p: &Path, trash: &Path) -> Result<PathBuf, String> {
    let (files, info) = (trash.join("files"), trash.join("info"));
    for d in [&files, &info] {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
    }
    let name = p.file_name().ok_or("that has no name")?.to_string_lossy().to_string();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    // The record is created first and exclusively: it reserves the name.
    for n in 1..1000 {
        let candidate = if n == 1 { name.clone() } else { format!("{stem}.{n}{ext}") };
        let record = info.join(format!("{candidate}.trashinfo"));
        let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).open(&record) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        };
        let dst = files.join(&candidate);
        if dst.exists() {
            let _ = std::fs::remove_file(&record);
            continue;
        }
        f.write_all(info_text(p, chrono::Local::now().naive_local()).as_bytes()).map_err(|e| e.to_string())?;
        drop(f);
        return match std::fs::rename(p, &dst) {
            Ok(()) => Ok(dst),
            Err(e) => {
                let _ = std::fs::remove_file(&record);
                Err(if e.raw_os_error() == Some(libc::EXDEV) { "that is on another disk; moving it to the Trash isn't supported".to_string() } else { e.to_string() })
            }
        };
    }
    Err("the Trash has too many items with that name".into())
}

/// After an item came back out of the Trash: drop its record.
pub fn forget(in_trash: &Path) {
    let (Some(dir), Some(name)) = (in_trash.parent(), in_trash.file_name()) else { return };
    if dir.file_name().is_some_and(|n| n == "files") {
        if let Some(trash) = dir.parent() {
            let _ = std::fs::remove_file(trash.join("info").join(format!("{}.trashinfo", name.to_string_lossy())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_and_put_back() {
        let root = std::env::temp_dir().join(format!("xpu-trash-{}", uuid::Uuid::new_v4().simple()));
        let trash = root.join("Trash");
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("report v2.pdf");
        std::fs::write(&a, b"1").unwrap();
        let t1 = trash_into(&a, &trash).unwrap();
        assert_eq!(t1, trash.join("files/report v2.pdf"));
        let info = std::fs::read_to_string(trash.join("info/report v2.pdf.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=") && info.contains("report%20v2.pdf\nDeletionDate="), "{info}");
        std::fs::write(&a, b"2").unwrap();
        let t2 = trash_into(&a, &trash).unwrap();
        assert_eq!(t2, trash.join("files/report v2.2.pdf"), "same name: numbered");
        std::fs::rename(&t2, &a).unwrap();
        forget(&t2);
        assert!(!trash.join("info/report v2.2.pdf.trashinfo").exists());
        assert!(trash.join("info/report v2.pdf.trashinfo").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paths_are_percent_encoded() {
        assert_eq!(encode_path(Path::new("/home/a/my file#1.txt")), "/home/a/my%20file%231.txt");
        assert_eq!(encode_path(Path::new("/home/a/سند.md")), "/home/a/%D8%B3%D9%86%D8%AF.md");
    }
}
