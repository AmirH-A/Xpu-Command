//! Whisper model files: a fixed catalog with the SHA-256 and size published by the model's
//! repository. A download is streamed to a temp file, hashed, size-checked and only then renamed
//! into place; a model that does not match its pinned hash is never loaded.
use crate::error::{msg, Result};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

pub struct ModelInfo {
    pub id: &'static str,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub label: &'static str,
}

pub const CATALOG: &[ModelInfo] = &[
    ModelInfo { id: "tiny.en", file: "ggml-tiny.en.bin", size: 77_704_715, sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f", label: "Tiny, English (78 MB, fastest)" },
    ModelInfo { id: "base.en", file: "ggml-base.en.bin", size: 147_964_211, sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002", label: "Base, English (148 MB, recommended)" },
    ModelInfo { id: "base", file: "ggml-base.bin", size: 147_951_465, sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe", label: "Base, multilingual (148 MB)" },
    ModelInfo { id: "small.en", file: "ggml-small.en.bin", size: 487_614_201, sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d", label: "Small, English (488 MB)" },
    ModelInfo { id: "small", file: "ggml-small.bin", size: 487_601_967, sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b", label: "Small, multilingual (488 MB)" },
    ModelInfo { id: "large-v3-turbo-q5_0", file: "ggml-large-v3-turbo-q5_0.bin", size: 574_041_195, sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2", label: "Large v3 Turbo, multilingual (574 MB, most accurate; best for Persian)" },
];

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

pub fn find(id: &str) -> Option<&'static ModelInfo> {
    CATALOG.iter().find(|m| m.id == id)
}

pub fn dir() -> Result<PathBuf> {
    let d = dirs::data_local_dir().ok_or_else(|| msg("no data directory"))?.join("xpu-command").join("models");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

pub fn path(m: &ModelInfo) -> Result<PathBuf> {
    Ok(dir()?.join(m.file))
}

pub fn sha256_file(p: &std::path::Path) -> Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

/// The model file, verified against its pinned size and hash. Errors if missing or altered.
pub fn verified_path(m: &ModelInfo) -> Result<PathBuf> {
    let p = path(m)?;
    let meta = std::fs::metadata(&p).map_err(|_| msg(format!("speech model {} is not downloaded yet", m.id)))?;
    if meta.len() != m.size {
        return Err(msg(format!("speech model {} has the wrong size; download it again", m.id)));
    }
    if sha256_file(&p)? != m.sha256 {
        return Err(msg(format!("speech model {} failed its integrity check; download it again", m.id)));
    }
    Ok(p)
}

pub fn is_downloaded(m: &ModelInfo) -> bool {
    path(m).ok().and_then(|p| std::fs::metadata(p).ok()).map(|x| x.len() == m.size).unwrap_or(false)
}

/// Downloads over HTTPS only (redirects too), hashing as it streams. `progress(done, total)`.
pub async fn download(m: &ModelInfo, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .connect_timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("xpu-command/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| msg(format!("http: {e}")))?;
    let res = client.get(format!("{BASE_URL}{}", m.file)).send().await.map_err(|e| msg(format!("download: {e}")))?;
    if !res.status().is_success() {
        return Err(msg(format!("download: HTTP {}", res.status())));
    }
    let final_path = path(m)?;
    let tmp = final_path.with_extension(format!("part-{}", uuid::Uuid::new_v4()));
    let result = async {
        let mut file = std::fs::File::create(&tmp)?;
        let mut hasher = Sha256::new();
        let mut done = 0u64;
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| msg(format!("download: {e}")))?;
            done += chunk.len() as u64;
            if done > m.size {
                return Err(msg("download is larger than the published model; aborted"));
            }
            hasher.update(&chunk);
            file.write_all(&chunk)?;
            progress(done, m.size);
        }
        file.sync_all()?;
        if done != m.size {
            return Err(msg("download incomplete"));
        }
        let got = hex::encode(hasher.finalize());
        if got != m.sha256 {
            return Err(msg(format!("integrity check failed (sha256 {got}); the file was discarded")));
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            std::fs::rename(&tmp, &final_path)?;
            Ok(final_path)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_well_formed() {
        for m in CATALOG {
            assert_eq!(m.sha256.len(), 64);
            assert!(m.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert!(m.file.starts_with("ggml-") && m.file.ends_with(".bin") && !m.file.contains('/'));
        }
        assert!(find("base.en").is_some());
        assert!(find("../x").is_none());
    }
}
