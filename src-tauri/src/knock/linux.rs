//! Linux: the accelerometer through the kernel's Industrial I/O subsystem (convertible laptops
//! and tablets have one; most clamshell laptops don't). Read from sysfs, about 200 times a second,
//! in m/s², converted to g for the same detector the Mac uses.
use super::{Detector, Event, KnockConfig};
use crate::error::{msg, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

const G: f32 = 9.806_65;

struct Accel {
    files: [PathBuf; 3],
    scale: [f32; 3],
    offset: [f32; 3],
}

fn read_f32(p: &Path) -> Option<f32> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// The first IIO accelerometer, with its scale (per axis or shared) and offset.
fn find() -> Option<Accel> {
    let rd = std::fs::read_dir("/sys/bus/iio/devices").ok()?;
    for e in rd.flatten() {
        let d = e.path();
        let raw = |a: &str| d.join(format!("in_accel_{a}_raw"));
        if !["x", "y", "z"].iter().all(|a| raw(a).exists()) {
            continue;
        }
        let shared_scale = read_f32(&d.join("in_accel_scale"));
        let shared_offset = read_f32(&d.join("in_accel_offset")).unwrap_or(0.0);
        let scale = ["x", "y", "z"].map(|a| read_f32(&d.join(format!("in_accel_{a}_scale"))).or(shared_scale).unwrap_or(0.0));
        if scale.iter().any(|s| *s <= 0.0) {
            continue;
        }
        let offset = ["x", "y", "z"].map(|a| read_f32(&d.join(format!("in_accel_{a}_offset"))).unwrap_or(shared_offset));
        return Some(Accel { files: ["x", "y", "z"].map(raw), scale, offset });
    }
    None
}

impl Accel {
    /// One sample in g.
    fn read(&self) -> Option<[f32; 3]> {
        let mut out = [0.0; 3];
        for i in 0..3 {
            out[i] = (read_f32(&self.files[i])? + self.offset[i]) * self.scale[i] / G;
        }
        Some(out)
    }
}

pub fn start(cfg: &'static KnockConfig, tx: Sender<Event>) -> Result<()> {
    let acc = find().ok_or_else(|| msg("this computer has no motion sensor Linux can read (double knock needs a laptop or tablet with an accelerometer)"))?;
    acc.read().ok_or_else(|| msg("the motion sensor can't be read"))?;
    std::thread::Builder::new()
        .name("xpu-knock".into())
        .spawn(move || {
            let mut det = Detector::default();
            let t0 = Instant::now();
            let tick = Duration::from_millis(5);
            loop {
                let started = Instant::now();
                if let Some(a) = acc.read().filter(|a| a.iter().all(|v| v.is_finite() && v.abs() < 16.0)) {
                    super::SAMPLES.fetch_add(1, Ordering::Relaxed);
                    if let Some(ev) = det.feed(t0.elapsed().as_secs_f64(), a, cfg.threshold()) {
                        if matches!(ev, Event::DoubleKnock { .. }) && !super::input_quiet(&[crate::linux::x11::idle_seconds().unwrap_or(f64::INFINITY)]) {
                            log::debug!("knock ignored: the keyboard or mouse was just used");
                        } else if cfg.enabled.load(Ordering::Relaxed) {
                            let _ = tx.send(ev);
                        }
                    }
                }
                std::thread::sleep(tick.saturating_sub(started.elapsed()));
            }
        })
        .map_err(|e| msg(format!("motion sensor thread: {e}")))?;
    Ok(())
}
