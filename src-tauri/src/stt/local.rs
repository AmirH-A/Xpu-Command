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
            // Flash attention: the same output, much faster on the GPU (word timestamps by DTW,
            // which it rules out, aren't used).
            let mut params = WhisperContextParameters::default();
            params.flash_attn(true);
            let ctx = WhisperContext::new_with_params(path.to_str().ok_or_else(|| msg("model path is not UTF-8"))?, params)
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
        p.set_audio_ctx(audio_ctx(req.audio.seconds()));
        if !req.vocabulary.is_empty() {
            p.set_initial_prompt(req.vocabulary);
        }
        state.full(p, &req.audio.0).map_err(|e| msg(format!("transcription failed: {e}")))?;
        let mut text = String::new();
        let audio_cs = (req.audio.seconds() * 100.0) as i64;
        for i in 0..state.full_n_segments() {
            if let Some(seg) = state.get_segment(i) {
                if !heard_in_audio(i, seg.end_timestamp(), audio_cs) {
                    log::debug!("dropped a segment past the end of the audio ({} of {audio_cs} cs)", seg.end_timestamp());
                    continue;
                }
                if let Ok(s) = seg.to_str_lossy() {
                    text.push_str(&s);
                }
            }
        }
        Ok(collapse_loops(text.trim()))
    }
}

/// How much of Whisper's 30-second window the encoder reads: the audio (50 frames a second) plus a
/// margin, instead of always the whole window. A short command is 3-4x faster this way (measured:
/// "Open Slack" 1.1 s → 0.3 s with Large v3 Turbo on an M4) with the same words. Below 512 frames
/// (about 10 s) short clips made the model loop ("Open Slack. Open Slack. …"), so that is the floor.
fn audio_ctx(seconds: f32) -> i32 {
    let frames = (seconds.max(0.0) * 50.0) as i32 + 128;
    ((frames + 63) / 64 * 64).clamp(512, 1500)
}

/// A segment after the first that ends well past the audio (times in centiseconds) was made up:
/// the model repeating itself into the silence after the speech (seen: a German sentence said once,
/// written twice, the copy "ending" 16 s after a 4 s clip). Real segments end inside the audio.
fn heard_in_audio(index: i32, end_cs: i64, audio_cs: i64) -> bool {
    index == 0 || end_cs <= audio_cs + 150
}

/// Whisper sometimes loops on a phrase: the same sentence three or more times in a row is said
/// once. Two in a row ("Thank you. Thank you.") can be real speech and stay.
pub fn collapse_loops(text: &str) -> String {
    let sentences: Vec<&str> = text.split_inclusive(['.', '!', '?', '。', '؟']).map(str::trim).filter(|s| !s.is_empty()).collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < sentences.len() {
        let same = |a: &str, b: &str| a.trim_end_matches(['.', '!', '?', '。', '؟']).eq_ignore_ascii_case(b.trim_end_matches(['.', '!', '?', '。', '؟']));
        let run = sentences[i..].iter().take_while(|s| same(s, sentences[i])).count();
        let keep = if run >= 3 { 1 } else { run };
        out.extend(std::iter::repeat(sentences[i]).take(keep));
        i += run;
    }
    if out.len() == sentences.len() { text.to_string() } else { out.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_fits_the_audio_with_a_floor() {
        assert_eq!(audio_ctx(0.0), 512);
        assert_eq!(audio_ctx(1.0), 512);
        assert_eq!(audio_ctx(20.8), 1216);
        assert_eq!(audio_ctx(30.0), 1500);
        assert_eq!(audio_ctx(120.0), 1500);
        assert_eq!(audio_ctx(f32::NAN), 512);
    }

    #[test]
    fn segments_past_the_audio_are_dropped() {
        assert!(heard_in_audio(0, 2002, 417), "the first segment always counts");
        assert!(!heard_in_audio(1, 2002, 417), "a copy running into the silence");
        assert!(heard_in_audio(3, 2054, 2078));
        assert!(heard_in_audio(1, 560, 417), "a little past the end is rounding");
    }

    #[test]
    fn loops_collapse_but_real_repeats_stay() {
        assert_eq!(collapse_loops("Open Slack. Open Slack. Open Slack. Open Slack."), "Open Slack.");
        assert_eq!(collapse_loops("Hi. Open Slack. open slack. Open Slack. Thanks."), "Hi. Open Slack. Thanks.");
        assert_eq!(collapse_loops("Thank you. Thank you."), "Thank you. Thank you.");
        assert_eq!(collapse_loops("It was very very good."), "It was very very good.");
        assert_eq!(collapse_loops("no punctuation at all"), "no punctuation at all");
        assert_eq!(collapse_loops(""), "");
    }
}
