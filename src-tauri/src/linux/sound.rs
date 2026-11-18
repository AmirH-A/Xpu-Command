//! Short sound cues, made in memory and played through the default output (no sound files or
//! players needed, so they sound the same on every distribution).
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Notes of a cue: (frequency Hz, milliseconds).
pub fn notes(cue: crate::cues::Cue) -> &'static [(f32, u32)] {
    use crate::cues::Cue;
    match cue {
        Cue::Start => &[(880.0, 55), (1320.0, 70)],
        Cue::Send => &[(660.0, 90)],
        Cue::Cancel => &[(660.0, 60), (440.0, 90)],
    }
}

/// The cue as samples at `rate`: soft sine notes with short fades, quiet.
pub fn render(cue: crate::cues::Cue, rate: u32) -> Vec<f32> {
    let mut out = Vec::new();
    for &(f, ms) in notes(cue) {
        let n = (rate as usize * ms as usize) / 1000;
        let fade = (rate as usize * 6 / 1000).max(1);
        for i in 0..n {
            let env = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
            out.push((i as f32 * f * std::f32::consts::TAU / rate as f32).sin() * 0.18 * env);
        }
    }
    out
}

pub fn play(cue: crate::cues::Cue) -> Result<(), String> {
    let host = cpal::default_host();
    let dev = host.default_output_device().ok_or("no sound output")?;
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    let rate = cfg.sample_rate().0;
    let channels = usize::from(cfg.channels());
    let samples = render(cue, rate);
    let secs = samples.len() as f32 / rate as f32;
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => stream::<f32>(&dev, &config, samples, channels),
        cpal::SampleFormat::I16 => stream::<i16>(&dev, &config, samples, channels),
        cpal::SampleFormat::U16 => stream::<u16>(&dev, &config, samples, channels),
        cpal::SampleFormat::I32 => stream::<i32>(&dev, &config, samples, channels),
        other => return Err(format!("unsupported sound format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_secs_f32(secs + 0.08));
    Ok(())
}

fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(dev: &cpal::Device, config: &cpal::StreamConfig, samples: Vec<f32>, channels: usize) -> Result<cpal::Stream, String> {
    let mut i = 0usize;
    dev.build_output_stream(
        config,
        move |data: &mut [T], _| {
            for frame in data.chunks_mut(channels) {
                let s = samples.get(i).copied().unwrap_or(0.0);
                i += 1;
                for x in frame {
                    *x = T::from_sample(s);
                }
            }
        },
        |e| log::debug!("cue: {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_are_short_quiet_and_fade() {
        for cue in [crate::cues::Cue::Start, crate::cues::Cue::Send, crate::cues::Cue::Cancel] {
            let s = render(cue, 48_000);
            assert!(!s.is_empty() && s.len() < 48_000 / 4, "under a quarter second");
            assert!(s.iter().all(|x| x.abs() <= 0.18));
            assert!(s[0].abs() < 0.01 && s[s.len() - 1].abs() < 0.02, "no click at the ends");
        }
    }
}
