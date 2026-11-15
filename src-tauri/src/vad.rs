//! Listening logic: when the user started and stopped talking (from the microphone level only),
//! trimming silence before transcription, and dropping Whisper's "phantom" phrases.
//! Pure: tested with synthetic levels.

/// Level reports arrive every ~50 ms.
pub const STEP_MS: u32 = 50;
/// Speech must last this long to count (a cough or a click doesn't).
const SPEECH_MIN_MS: u32 = 150;
/// After speech, this much quiet ends the turn (auto-stop).
pub const END_SILENCE_MS: u32 = 1500;
/// With no speech at all for this long, the recording is cancelled.
const NO_SPEECH_MS: u32 = 7000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Continue,
    /// The user spoke and has stopped: send.
    Done,
    /// Nothing was said: cancel.
    NoSpeech,
}

/// Follows the microphone level (RMS per ~50 ms) and decides when a turn ends.
#[derive(Debug, Clone)]
pub struct Endpoint {
    floor: f32,
    elapsed_ms: u32,
    loud_run_ms: u32,
    quiet_ms: u32,
    voiced_ms: u32,
    started: bool,
}

impl Default for Endpoint {
    fn default() -> Self {
        Self { floor: f32::MAX, elapsed_ms: 0, loud_run_ms: 0, quiet_ms: 0, voiced_ms: 0, started: false }
    }
}

impl Endpoint {
    /// Speech threshold: well above the room's noise floor, within sane bounds.
    pub fn threshold(&self) -> f32 {
        let floor = if self.floor == f32::MAX { 0.003 } else { self.floor };
        (floor * 3.0).clamp(0.008, 0.05)
    }

    pub fn spoke(&self) -> bool {
        self.started
    }

    /// Milliseconds of speech so far.
    pub fn voiced_ms(&self) -> u32 {
        self.voiced_ms
    }

    pub fn push(&mut self, rms: f32) -> Verdict {
        let rms = if rms.is_finite() { rms.max(0.0) } else { 0.0 };
        self.elapsed_ms += STEP_MS;
        if rms < self.floor {
            self.floor = rms;
        } else if self.floor != f32::MAX {
            self.floor += (rms.min(0.02) - self.floor) * 0.01;
        }
        if rms > self.threshold() {
            self.loud_run_ms += STEP_MS;
            self.quiet_ms = 0;
            if self.loud_run_ms >= SPEECH_MIN_MS {
                if !self.started {
                    self.voiced_ms += self.loud_run_ms - STEP_MS;
                }
                self.started = true;
                self.voiced_ms += STEP_MS;
            }
        } else {
            self.loud_run_ms = 0;
            self.quiet_ms += STEP_MS;
        }
        if self.started && self.quiet_ms >= END_SILENCE_MS {
            Verdict::Done
        } else if !self.started && self.elapsed_ms >= NO_SPEECH_MS {
            Verdict::NoSpeech
        } else {
            Verdict::Continue
        }
    }
}

/// The part of `samples` worth transcribing: leading and trailing quiet removed (keeping `pad_ms`
/// around speech), and `cut_tail_ms` dropped from the end first (the knock that ended it).
pub fn trim(samples: &[f32], rate: u32, threshold: f32, pad_ms: u32, cut_tail_ms: u32) -> std::ops::Range<usize> {
    let cut = (rate as usize * cut_tail_ms as usize / 1000).min(samples.len());
    let s = &samples[..samples.len() - cut];
    let frame = (rate as usize / 50).max(1);
    let loud = |i: usize| {
        let f = &s[i * frame..((i + 1) * frame).min(s.len())];
        (f.iter().map(|x| x * x).sum::<f32>() / f.len().max(1) as f32).sqrt() > threshold
    };
    let frames = s.len().div_ceil(frame);
    let (Some(first), Some(last)) = ((0..frames).find(|&i| loud(i)), (0..frames).rev().find(|&i| loud(i))) else {
        return 0..s.len();
    };
    let pad = rate as usize * pad_ms as usize / 1000;
    (first * frame).saturating_sub(pad)..((last + 1) * frame + pad).min(s.len())
}

/// Whisper invents these from silence or noise. Dropped only when there was little real speech.
pub fn is_phantom(text: &str, voiced_ms: u32) -> bool {
    const PHANTOMS: &[&str] = &[
        "thank you", "thanks", "thank you very much", "thanks for watching", "thank you for watching", "you", "bye", "bye bye",
        "okay", "oh", "uh", "um", "hmm", "so", "please subscribe", "subscribe", "subtitles by the amaraorg community", "music",
        "ممنون", "مرسی", "خداحافظ", "زیرنویس", "زیرنویس فارسی", "ممنون که تماشا کردید", "موسیقی",
    ];
    if voiced_ms >= 1200 {
        return false;
    }
    let t: String = text.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == ' ').collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    t.is_empty() || PHANTOMS.contains(&t.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(levels: &[(f32, u32)]) -> (Verdict, Endpoint) {
        let mut e = Endpoint::default();
        let mut v = Verdict::Continue;
        for &(rms, ms) in levels {
            for _ in 0..ms / STEP_MS {
                v = e.push(rms);
                if v != Verdict::Continue {
                    return (v, e);
                }
            }
        }
        (v, e)
    }

    #[test]
    fn speech_then_quiet_ends_the_turn() {
        let (v, e) = run(&[(0.002, 400), (0.08, 2000), (0.002, 3000)]);
        assert_eq!(v, Verdict::Done);
        assert!(e.voiced_ms() >= 1900, "{}", e.voiced_ms());
    }

    #[test]
    fn short_pauses_dont_end_it() {
        let (v, _) = run(&[(0.002, 300), (0.08, 1000), (0.002, 800), (0.08, 1000), (0.002, 1000)]);
        assert_eq!(v, Verdict::Continue, "an 800 ms pause between sentences keeps listening");
    }

    #[test]
    fn nothing_said_cancels_and_clicks_dont_count() {
        let (v, e) = run(&[(0.002, 3000), (0.3, 50), (0.002, 5000)]);
        assert_eq!(v, Verdict::NoSpeech);
        assert!(!e.spoke(), "a 50 ms click is not speech");
    }

    #[test]
    fn noisy_room_raises_the_threshold() {
        let mut quiet = Endpoint::default();
        let mut noisy = Endpoint::default();
        for _ in 0..10 {
            quiet.push(0.002);
            noisy.push(0.012);
        }
        assert!(noisy.threshold() > quiet.threshold());
        let (v, _) = run(&[(0.012, 400), (0.013, 3000), (0.012, 5000)]);
        assert_eq!(v, Verdict::NoSpeech, "steady fan noise is not speech");
    }

    #[test]
    fn trimming() {
        let rate = 16_000;
        let mut s = vec![0.0f32; rate as usize];
        s.extend(std::iter::repeat_n(0.2f32, rate as usize / 2));
        s.extend(vec![0.0f32; rate as usize]);
        s.extend(std::iter::repeat_n(0.5f32, rate as usize / 10));
        let r = trim(&s, rate, 0.01, 200, 300);
        assert!(r.start >= 12_000 && r.start <= 13_000, "{r:?}");
        assert!(r.end <= 16_000 + 8_000 + 3_200 + 320, "trailing quiet and the knock are gone: {r:?}");
        assert_eq!(trim(&[0.0; 1000], rate, 0.01, 200, 0), 0..1000, "all quiet: untouched");
        assert_eq!(trim(&[0.3; 100], rate, 0.01, 200, 5000), 0..0, "cut longer than the audio");
    }

    #[test]
    fn phantoms() {
        assert!(is_phantom("Thank you.", 300));
        assert!(is_phantom(" you ", 0));
        assert!(!is_phantom("Thank you.", 2000), "said for real");
        assert!(!is_phantom("open spotify", 300));
        assert!(is_phantom("ممنون.", 400));
        assert!(!is_phantom("اسپاتیفای رو باز کن", 400));
    }
}
