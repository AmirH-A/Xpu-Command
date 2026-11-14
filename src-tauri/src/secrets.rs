//! Secrets live in the OS keychain (macOS Keychain, Windows Credential Manager), never in files,
//! settings, logs or the web layer. Values are zeroed from memory when dropped.
use crate::error::{msg, Result};
use zeroize::Zeroizing;

const SERVICE: &str = "app.xpucommand.desktop";
pub const OPENAI_KEY: &str = "openai-api-key";

fn entry(name: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, name).map_err(|e| msg(format!("keychain: {e}")))
}

pub fn get(name: &str) -> Result<Option<Zeroizing<String>>> {
    match entry(name)?.get_password() {
        Ok(v) => Ok(Some(Zeroizing::new(v))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(msg(format!("keychain: {e}"))),
    }
}

pub fn set(name: &str, value: Zeroizing<String>) -> Result<()> {
    entry(name)?.set_password(value.as_str()).map_err(|e| msg(format!("keychain: {e}")))
}

pub fn delete(name: &str) -> Result<()> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(msg(format!("keychain: {e}"))),
    }
}

/// Shape check only (we never call the API to "test" a key without the user asking).
pub fn looks_like_openai_key(k: &str) -> bool {
    k.starts_with("sk-") && (20..=300).contains(&k.len()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    #[test]
    fn key_shape() {
        assert!(super::looks_like_openai_key("sk-proj-abcdefghijklmnopqrstuvwxyz0123456789"));
        assert!(!super::looks_like_openai_key("sk-short"));
        assert!(!super::looks_like_openai_key("sk-abcdefghijklmnopqrstuv\nwxyz"));
        assert!(!super::looks_like_openai_key("not-a-key-at-all-but-long-enough"));
    }
}
