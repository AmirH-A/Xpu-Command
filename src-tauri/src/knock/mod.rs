//! Double-knock trigger (experimental): two quick taps on the laptop start talking, two more send.
//! It reads the laptop's built-in accelerometer; nothing else. Only acceleration magnitudes are
//! looked at, in memory, and only to find two sharp taps. Apple Silicon MacBooks (Apple doesn't
//! document this sensor, so a macOS update may break it; the app then says so), and on Linux,
//! laptops and tablets whose accelerometer the kernel exposes (IIO).
use crate::settings::KnockSensitivity;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

/// Ignore further peaks for this long after a tap: one knock rings for a few tens of ms.
const RING: f64 = 0.08;
/// The two taps of a double knock are this far apart (seconds).
const MIN_GAP: f64 = 0.10;
const MAX_GAP: f64 = 0.60;
/// No tap this long before the first one (so typing or a bump doesn't count) …
const QUIET_BEFORE: f64 = 0.80;
/// … and none this long after the second (three or more taps is not a double knock).
const QUIET_AFTER: f64 = 0.30;
/// A tap must also stand this many times above the current background shaking.
const NOISE_FACTOR: f32 = 8.0;

pub fn threshold_g(s: KnockSensitivity) -> f32 {
    match s {
        KnockSensitivity::High => 0.08,
        KnockSensitivity::Medium => 0.15,
        KnockSensitivity::Low => 0.30,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// One sharp tap (strength in g above gravity), reported for calibration.
    Tap(f32),
    /// `span_ms`: from the first tap until now (what a recording must drop from its end).
    DoubleKnock { span_ms: u32 },
}

/// Finds double knocks in a stream of accelerometer samples. Pure, so it is tested with synthetic signals.
pub struct Detector {
    gravity: Option<[f32; 3]>,
    noise: f32,
    ring_until: f64,
    prev: f64,
    taps: Vec<f64>,
}

impl Default for Detector {
    fn default() -> Self {
        Self { gravity: None, noise: 0.01, ring_until: f64::NEG_INFINITY, prev: f64::NEG_INFINITY, taps: Vec::new() }
    }
}

impl Detector {
    /// `t` in seconds (monotonic), `a` in g, `threshold` in g. Returns what this sample completed.
    pub fn feed(&mut self, t: f64, a: [f32; 3], threshold: f32) -> Option<Event> {
        let g = self.gravity.get_or_insert(a);
        let d = [a[0] - g[0], a[1] - g[1], a[2] - g[2]];
        for i in 0..3 {
            g[i] += d[i] * 0.02;
        }
        let jerk = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();

        let mut out = None;
        if self.taps.len() == 1 && t - self.taps[0] > MAX_GAP {
            self.prev = self.taps[0];
            self.taps.clear();
        }
        if self.taps.len() == 2 && t - self.taps[1] >= QUIET_AFTER {
            let ok = self.taps[1] - self.taps[0] >= MIN_GAP && self.taps[0] - self.prev >= QUIET_BEFORE;
            let span_ms = ((t - self.taps[0]) * 1000.0) as u32;
            self.prev = self.taps[1];
            self.taps.clear();
            if ok {
                out = Some(Event::DoubleKnock { span_ms });
            }
        }

        let limit = threshold.max(self.noise * NOISE_FACTOR);
        if jerk > limit {
            if t >= self.ring_until {
                self.ring_until = t + RING;
                self.tap(t);
                return out.or(Some(Event::Tap(jerk)));
            }
        } else if t >= self.ring_until {
            self.noise += (jerk - self.noise) * 0.005;
        }
        out
    }

    fn tap(&mut self, t: f64) {
        if self.taps.len() == 2 {
            self.prev = t;
            self.taps.clear();
            return;
        }
        self.taps.push(t);
    }
}

/// Samples read from the sensor so far (shows it is alive).
pub static SAMPLES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Keyboard or trackpad use within this long before a double knock means it was probably typing.
pub const INPUT_QUIET_SECS: f64 = 1.2;

/// True when no key, click or scroll happened within `INPUT_QUIET_SECS`.
pub fn input_quiet(seconds_since: &[f64]) -> bool {
    seconds_since.iter().all(|s| !s.is_finite() || *s >= INPUT_QUIET_SECS)
}

/// Shared with the sensor thread: on/off and the current threshold.
pub struct KnockConfig {
    pub enabled: AtomicBool,
    threshold_mg: AtomicU32,
}

impl KnockConfig {
    pub fn new(on: bool, s: KnockSensitivity) -> Self {
        let c = Self { enabled: AtomicBool::new(on), threshold_mg: AtomicU32::new(0) };
        c.set_sensitivity(s);
        c
    }
    pub fn set_sensitivity(&self, s: KnockSensitivity) {
        self.threshold_mg.store((threshold_g(s) * 1000.0) as u32, Ordering::Relaxed);
    }
    /// Debug calibration: an exact threshold in g.
    #[cfg(debug_assertions)]
    pub fn set_threshold_g(&self, g: f32) {
        self.threshold_mg.store((g * 1000.0) as u32, Ordering::Relaxed);
    }
    pub fn threshold(&self) -> f32 {
        self.threshold_mg.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

/// Starts reading the sensor on its own thread; events go to `tx` while `cfg.enabled` is set.
/// Errors when this computer has no supported motion sensor.
pub fn start(cfg: &'static KnockConfig, tx: Sender<Event>) -> crate::error::Result<()> {
    #[cfg(target_os = "macos")]
    return macos::start(cfg, tx);
    #[cfg(target_os = "linux")]
    return linux::start(cfg, tx);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (cfg, tx);
        Err(crate::error::msg("double knock needs a MacBook with Apple silicon"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 400.0;
    const REST: [f32; 3] = [0.04, 0.0, -1.0];

    /// Runs `secs` of signal: rest plus small noise, with a sharp spike at each time in `taps`.
    fn run(secs: f64, taps: &[(f64, f32)], threshold: f32) -> Vec<Event> {
        let mut d = Detector::default();
        let mut out = Vec::new();
        let n = (secs * RATE) as usize;
        for i in 0..n {
            let t = i as f64 / RATE;
            let wobble = if i % 2 == 0 { 0.008 } else { -0.008 };
            let mut a = [REST[0] + wobble, REST[1], REST[2] - wobble];
            for &(at, g) in taps {
                let k = ((t - at) * RATE).round();
                if (0.0..3.0).contains(&k) {
                    a[2] += g * (1.0 - k as f32 / 3.0);
                }
            }
            if let Some(e) = d.feed(t, a, threshold) {
                out.push(e);
            }
        }
        out
    }

    fn knocks(ev: &[Event]) -> usize {
        ev.iter().filter(|e| matches!(e, Event::DoubleKnock { .. })).count()
    }

    #[test]
    fn a_double_knock_is_detected_once() {
        let ev = run(3.0, &[(1.0, 0.5), (1.25, 0.5)], 0.15);
        assert_eq!(knocks(&ev), 1, "{ev:?}");
        let span = ev.iter().find_map(|e| match e { Event::DoubleKnock { span_ms } => Some(*span_ms), _ => None }).unwrap();
        assert!((540..=600).contains(&span), "first tap to decision: 0.25 s gap + 0.3 s quiet ({span} ms)");
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::Tap(_))).count(), 2);
    }

    #[test]
    fn rest_and_weak_taps_do_nothing() {
        assert!(run(3.0, &[], 0.15).is_empty());
        assert!(run(3.0, &[(1.0, 0.1), (1.25, 0.1)], 0.15).is_empty(), "below the threshold");
        assert_eq!(knocks(&run(3.0, &[(1.0, 0.1), (1.25, 0.1)], 0.08)), 1, "high sensitivity catches them");
    }

    #[test]
    fn single_triple_and_slow_taps_are_not_double_knocks() {
        assert_eq!(knocks(&run(3.0, &[(1.0, 0.5)], 0.15)), 0, "single");
        assert_eq!(knocks(&run(3.0, &[(1.0, 0.5), (1.2, 0.5), (1.4, 0.5)], 0.15)), 0, "triple");
        assert_eq!(knocks(&run(3.0, &[(1.0, 0.5), (1.9, 0.5)], 0.15)), 0, "too slow");
        assert_eq!(knocks(&run(3.0, &[(1.0, 0.5), (1.05, 0.5)], 0.15)), 0, "one knock ringing twice");
    }

    #[test]
    fn taps_right_after_activity_do_not_count() {
        let mut taps: Vec<(f64, f32)> = (0..8).map(|i| (0.5 + i as f64 * 0.15, 0.5)).collect();
        taps.extend([(1.9, 0.5), (2.15, 0.5)]);
        assert_eq!(knocks(&run(4.0, &taps, 0.15)), 0);
    }

    #[test]
    fn typing_or_clicking_just_before_vetoes_a_knock() {
        assert!(input_quiet(&[30.0, 30.0, 5.0, 100.0, 2.0]));
        assert!(!input_quiet(&[0.4, 30.0, 5.0, 100.0, 2.0]), "a key 0.4 s ago");
        assert!(!input_quiet(&[30.0, 30.0, 0.9, 100.0, 2.0]), "a click 0.9 s ago");
        assert!(input_quiet(&[f64::INFINITY; 5]), "never used");
    }

    #[test]
    fn two_double_knocks_in_a_row() {
        let ev = run(5.0, &[(1.0, 0.5), (1.25, 0.5), (3.0, 0.5), (3.25, 0.5)], 0.15);
        assert_eq!(knocks(&ev), 2, "{ev:?}");
    }
}
