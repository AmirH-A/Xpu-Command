//! Speech-to-text: local whisper.cpp (default, private) or the OpenAI API (opt-in).
pub mod cloud;
pub mod local;
pub mod model;

use crate::audio::Captured;
use crate::error::Result;

pub struct Request<'a> {
    pub audio: &'a Captured,
    pub language: &'a str,
    /// Words to bias recognition toward (file names, identifiers from the project).
    pub vocabulary: &'a str,
}
pub type Transcript = String;

pub fn min_audio_ok(a: &Captured) -> bool {
    a.seconds() >= 0.3
}

pub use local::LocalEngine;

pub async fn transcribe_cloud(req: &Request<'_>) -> Result<Transcript> {
    cloud::transcribe(req).await
}
