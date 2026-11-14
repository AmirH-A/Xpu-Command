//! Opt-in cloud transcription (OpenAI). The key comes from the OS keychain at call time and is
//! never logged; audio is sent over TLS and not kept.
use super::{Request, Transcript};
use crate::error::{msg, Result};

const URL: &str = "https://api.openai.com/v1/audio/transcriptions";
const MODEL: &str = "gpt-4o-mini-transcribe";

pub async fn transcribe(req: &Request<'_>) -> Result<Transcript> {
    let key = crate::secrets::get(crate::secrets::OPENAI_KEY)?.ok_or_else(|| msg("cloud speech is on but no API key is set"))?;
    let wav = crate::audio::wav_bytes(&req.audio.0, crate::audio::TARGET_RATE);
    let mut form = reqwest::multipart::Form::new()
        .text("model", MODEL)
        .text("response_format", "json")
        .part("file", reqwest::multipart::Part::bytes(wav).file_name("speech.wav").mime_str("audio/wav").map_err(|e| msg(e.to_string()))?);
    if req.language != "auto" {
        form = form.text("language", req.language.to_string());
    }
    if !req.vocabulary.is_empty() {
        form = form.text("prompt", req.vocabulary.to_string());
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| msg(format!("http: {e}")))?;
    let res = client.post(URL).bearer_auth(key.as_str()).multipart(form).send().await.map_err(|e| msg(format!("cloud speech: {}", e.without_url())))?;
    let status = res.status();
    if !status.is_success() {
        return Err(msg(format!("cloud speech: HTTP {status}{}", if status == 401 { " (check the API key)" } else { "" })));
    }
    #[derive(serde::Deserialize)]
    struct Body {
        text: String,
    }
    let body: Body = res.json().await.map_err(|e| msg(format!("cloud speech: {}", e.without_url())))?;
    Ok(body.text)
}
