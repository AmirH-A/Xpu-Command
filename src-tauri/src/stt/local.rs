//! Local transcription with whisper.cpp. The model is verified and loaded once, and its decoding
//! state (which sets up the GPU) is created once and reused: every talk after the first is fast.
use super::{model, Request, Transcript};
use crate::error::{msg, Result};
use parking_lot::Mutex;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

struct Loaded {
    id: String,
    _ctx: WhisperContext,
    state: WhisperState,
}

#[derive(Default)]
pub struct LocalEngine {
    loaded: Mutex<Option<Loaded>>,
}

impl LocalEngine {
    fn ensure<'a>(slot: &'a mut Option<Loaded>, model_id: &str) -> Result<&'a mut WhisperState> {
        if slot.as_ref().map(|l| l.id.as_str()) != Some(model_id) {
            *slot = None;
            let info = model::find(model_id).ok_or_else(|| msg("unknown speech model"))?;
            let path = model::verified_path(info)?;
            let ctx = WhisperContext::new_with_params(path.to_str().ok_or_else(|| msg("model path is not UTF-8"))?, WhisperContextParameters::default())
                .map_err(|e| msg(format!("loading speech model: {e}")))?;
            let state = ctx.create_state().map_err(|e| msg(format!("speech engine: {e}")))?;
            *slot = Some(Loaded { id: model_id.to_string(), _ctx: ctx, state });
        }
        Ok(&mut slot.as_mut().unwrap().state)
    }

    /// Verify and load the model now, so the first talk doesn't wait for it.
    pub fn preload(&self, model_id: &str) -> Result<()> {
        Self::ensure(&mut self.loaded.lock(), model_id).map(|_| ())
    }

    pub fn transcribe(&self, model_id: &str, req: &Request<'_>) -> Result<Transcript> {
        let mut slot = self.loaded.lock();
        let state = Self::ensure(&mut slot, model_id)?;
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let threads = std::thread::available_parallelism().map(|n| n.get().min(8) as i32).unwrap_or(4);
        p.set_n_threads(threads);
        p.set_language(if req.language == "auto" { None } else { Some(req.language) });
        p.set_translate(false);
        p.set_no_context(true);
        p.set_single_segment(false);
        p.set_print_special(false);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_timestamps(false);
        p.set_suppress_blank(true);
        if !req.vocabulary.is_empty() {
            p.set_initial_prompt(req.vocabulary);
        }
        state.full(p, &req.audio.0).map_err(|e| msg(format!("transcription failed: {e}")))?;
        let mut text = String::new();
        for i in 0..state.full_n_segments() {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(s) = seg.to_str_lossy() {
                    text.push_str(&s);
                }
            }
        }
        Ok(text)
    }
}
