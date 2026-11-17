//! Push-to-talk recording. Audio lives only in memory, is capped in length, is never written to
//! disk, and the buffers are zeroed once transcribed.
use crate::error::{msg, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use zeroize::Zeroize;

pub const TARGET_RATE: u32 = 16_000;

/// Mono 16 kHz samples in [-1, 1]. Zeroed on drop.
pub struct Captured(pub Vec<f32>);
impl Drop for Captured {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
impl Captured {
    pub fn seconds(&self) -> f32 {
        self.0.len() as f32 / TARGET_RATE as f32
    }
    pub fn peak(&self) -> f32 {
        self.0.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }
}

/// Names of the input devices, for the settings window.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices().map(|ds| ds.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
}

pub struct Recording {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<Captured>>>,
    buf: Arc<Mutex<Vec<f32>>>,
    rate: Arc<std::sync::atomic::AtomicU32>,
}

impl Recording {
    /// Starts capturing from the default input device. `level` gets ~20 RMS updates per second.
    /// `device`: an input device name from `input_devices()`, or None for the system default.
    pub fn start(device: Option<&str>, max_seconds: u32, level: impl Fn(f32) + Send + Sync + 'static) -> Result<Recording> {
        Self::open(device, max_seconds, false, level)
    }

    /// Listens without end, keeping only the last `keep_seconds` (older audio is zeroed and dropped).
    /// For the wake phrase: read with `snapshot`.
    pub fn listen(device: Option<&str>, keep_seconds: u32, level: impl Fn(f32) + Send + Sync + 'static) -> Result<Recording> {
        Self::open(device, keep_seconds, true, level)
    }

    fn open(device: Option<&str>, max_seconds: u32, ring: bool, level: impl Fn(f32) + Send + Sync + 'static) -> Result<Recording> {
        let wanted = device.map(str::to_string);
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let shared = Arc::new(Mutex::new(Vec::<f32>::new()));
        let rate_shared = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (buf, rate_out) = (shared.clone(), rate_shared.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        let thread = std::thread::Builder::new().name("xpu-audio".into()).spawn(move || -> Result<Captured> {
            let host = cpal::default_host();
            let device = match &wanted {
                Some(name) => host
                    .input_devices()
                    .ok()
                    .and_then(|mut ds| ds.find(|d| d.name().ok().as_deref() == Some(name.as_str())))
                    .or_else(|| {
                        log::warn!("input device {name:?} not found; using the default");
                        host.default_input_device()
                    }),
                None => host.default_input_device(),
            }
            .ok_or_else(|| msg("no microphone found"))?;
            let config = device.default_input_config().map_err(|e| msg(format!("microphone: {e}")))?;
            let rate = config.sample_rate().0;
            let channels = config.channels() as usize;
            let max_samples = (rate as usize) * (max_seconds as usize);
            buf.lock().reserve(rate as usize * 10);
            rate_out.store(rate, Ordering::Relaxed);
            let level = Arc::new(level);
            let (b2, l2) = (buf.clone(), level.clone());
            let mut since_level = 0usize;
            let mut push = move |mono: &mut dyn Iterator<Item = f32>| {
                let mut b = b2.lock();
                let mut added = 0usize;
                for s in mono {
                    if b.len() >= max_samples {
                        if !ring {
                            break;
                        }
                        drop_oldest(&mut b, rate as usize / 2);
                    }
                    b.push(s);
                    added += 1;
                }
                since_level += added;
                if since_level >= rate as usize / 20 {
                    let tail = &b[b.len().saturating_sub(since_level)..];
                    let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len().max(1) as f32).sqrt();
                    since_level = 0;
                    l2(rms);
                }
            };
            let err = |e| log::warn!("audio stream error: {e}");
            let cfg: cpal::StreamConfig = config.clone().into();
            let stream = match config.sample_format() {
                cpal::SampleFormat::F32 => device.build_input_stream(&cfg, move |d: &[f32], _: &_| push(&mut d.chunks(channels).map(avg)), err, None),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    &cfg,
                    move |d: &[i16], _: &_| push(&mut d.chunks(channels).map(|c| c.iter().map(|&x| x as f32 / 32768.0).sum::<f32>() / c.len() as f32)),
                    err,
                    None,
                ),
                cpal::SampleFormat::U16 => device.build_input_stream(
                    &cfg,
                    move |d: &[u16], _: &_| push(&mut d.chunks(channels).map(|c| c.iter().map(|&x| (x as f32 - 32768.0) / 32768.0).sum::<f32>() / c.len() as f32)),
                    err,
                    None,
                ),
                f => return Err(msg(format!("unsupported microphone format {f:?}"))),
            }
            .map_err(|e| msg(format!("microphone: {e} (is microphone access allowed?)")))?;
            stream.play().map_err(|e| msg(format!("microphone: {e}")))?;
            let _ = ready_tx.send(Ok(()));
            while !stop2.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            drop(stream);
            let mut raw = std::mem::take(&mut *buf.lock());
            let out = resample(&raw, rate, TARGET_RATE);
            raw.zeroize();
            Ok(Captured(out))
        })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Recording { stop, thread: Some(thread), buf: shared, rate: rate_shared }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(thread.join().ok().and_then(|r| r.err()).unwrap_or_else(|| msg("microphone failed to start"))),
        }
    }

    pub fn snapshot(&self, max_seconds: f32) -> Option<Captured> {
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 {
            return None;
        }
        let tail: Vec<f32> = {
            let b = self.buf.lock();
            let keep = (rate as f32 * max_seconds) as usize;
            b[b.len().saturating_sub(keep)..].to_vec()
        };
        let mut tail = tail;
        let out = resample(&tail, rate, TARGET_RATE);
        tail.zeroize();
        Some(Captured(out))
    }

    pub fn finish(mut self) -> Result<Captured> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().map_err(|_| msg("audio thread panicked"))?
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Removes the oldest `n` samples and zeroes the memory they leave behind.
fn drop_oldest(b: &mut Vec<f32>, n: usize) {
    let n = n.min(b.len());
    let keep = b.len() - n;
    b.copy_within(n.., 0);
    b[keep..].zeroize();
    b.truncate(keep);
}

fn avg(c: &[f32]) -> f32 {
    c.iter().sum::<f32>() / c.len() as f32
}

/// Band-limited resampling (rubato sinc) to Whisper's 16 kHz.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    };
    let chunk = 1024;
    let mut r = match SincFixedIn::<f32>::new(to as f64 / from as f64, 1.0, params, chunk, 1) {
        Ok(r) => r,
        Err(_) => return input.to_vec(),
    };
    let mut out = Vec::with_capacity(input.len() * to as usize / from as usize + chunk);
    let mut i = 0;
    while i < input.len() {
        let end = (i + chunk).min(input.len());
        let mut block = input[i..end].to_vec();
        block.resize(chunk, 0.0);
        if let Ok(o) = r.process(&[block], None) {
            out.extend_from_slice(&o[0]);
        }
        i = end;
    }
    out.truncate(input.len() * to as usize / from as usize);
    out
}

/// 16-bit PCM WAV in memory, for the cloud engine only.
pub fn wav_bytes(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        v.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_duration_and_tone() {
        let from = 48_000;
        let tone: Vec<f32> = (0..from).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / from as f32).sin() * 0.5).collect();
        let out = resample(&tone, from, TARGET_RATE);
        assert_eq!(out.len(), TARGET_RATE as usize, "one second stays one second");
        let rms = (out[2000..14000].iter().map(|x| x * x).sum::<f32>() / 12000.0).sqrt();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.03, "a 440 Hz tone keeps its level, rms {rms}");
    }

    #[test]
    fn the_ring_keeps_the_newest_and_zeroes_the_rest() {
        let mut b: Vec<f32> = (1..=10).map(|i| i as f32).collect();
        let base = b.as_ptr();
        drop_oldest(&mut b, 4);
        assert_eq!(b, [5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
        let spare = unsafe { std::slice::from_raw_parts(base.add(6), 4) };
        assert!(spare.iter().all(|x| *x == 0.0), "dropped audio doesn't linger in memory");
        drop_oldest(&mut b, 100);
        assert!(b.is_empty());
    }

    #[test]
    fn wav_header_is_valid() {
        let w = wav_bytes(&[0.0, 1.0, -1.0], 16_000);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(w.len(), 50);
    }
}
