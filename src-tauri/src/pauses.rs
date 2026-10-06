//! Pauses in speech (shorts phase S4), for tightening clips.
//!
//! Parakeet's word timings are contiguous (a word ends where the next
//! starts), so pauses hide inside word spans; the audio is the source of
//! truth. The level threshold adapts to each range: between its noise floor
//! and its speech level, so studio room tone and quiet speakers both work.

use crate::ffmpeg::{run_ffmpeg, FfmpegTask};
use crate::run_control::RunControl;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use std::path::Path;

const SAMPLE_RATE: u32 = 16_000;
/// Loudness is measured in windows of this length (seconds).
const WINDOW: f64 = 0.01;

/// Loudness (dBFS) per `WINDOW` of a mono signal.
pub(crate) fn envelope(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let window = ((f64::from(sample_rate) * WINDOW) as usize).max(1);
    samples
        .chunks(window)
        .map(|chunk| {
            let power = chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32;
            10.0 * power.max(1e-10).log10()
        })
        .collect()
}

/// Spans (seconds from the start) quieter than an adaptive threshold for at
/// least `min_seconds`.
pub(crate) fn quiet_spans(levels: &[f32], min_seconds: f64) -> Vec<(f64, f64)> {
    if levels.is_empty() {
        return Vec::new();
    }
    let mut sorted = levels.to_vec();
    sorted.sort_by(f32::total_cmp);
    let percentile = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
    let floor = percentile(0.1);
    let speech = percentile(0.9);
    // Closer to the floor than to speech, but clearly above it.
    let threshold = (floor + 8.0).max(speech - 30.0).min(speech - 6.0);

    let min_windows = (min_seconds / WINDOW).ceil() as usize;
    let mut spans = Vec::new();
    let mut start = None;
    for (index, &level) in levels
        .iter()
        .chain(std::iter::once(&f32::INFINITY))
        .enumerate()
    {
        match (level < threshold, start) {
            (true, None) => start = Some(index),
            (false, Some(first)) => {
                if index - first >= min_windows {
                    spans.push((first as f64 * WINDOW, index as f64 * WINDOW));
                }
                start = None;
            }
            _ => {}
        }
    }
    spans
}

/// Pauses of at least `min_seconds` in `start..end` of `path`, in source
/// seconds. Blocks.
pub(crate) fn detect_pauses(
    path: &Path,
    start: f64,
    end: f64,
    min_seconds: f64,
    run: Option<(u64, &RunControl)>,
) -> Result<Vec<(f64, f64)>, String> {
    if end <= start {
        return Ok(Vec::new());
    }
    let mut command = FfmpegCommand::new();
    command
        .args([
            "-ss",
            &format!("{start:.6}"),
            "-t",
            &format!("{:.6}", end - start),
        ])
        .input(path.to_string_lossy())
        .args(["-vn", "-sn", "-ac", "1", "-ar", &SAMPLE_RATE.to_string()])
        .args(["-f", "f32le", "-acodec", "pcm_f32le"])
        .pipe_stdout();
    let mut bytes = Vec::new();
    run_ffmpeg(
        command,
        FfmpegTask {
            operation: "measure pauses",
            input: path,
            output: None,
            run,
        },
        |event| {
            if let FfmpegEvent::OutputChunk(chunk) = event {
                bytes.extend_from_slice(chunk);
            }
        },
    )?;
    let samples: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let levels = envelope(&samples, SAMPLE_RATE);
    Ok(quiet_spans(&levels, min_seconds)
        .into_iter()
        .map(|(a, b)| (start + a, start + b))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_stretches_between_speech_are_found() {
        // 10 ms windows: speech at -20 dB, room tone at -60 dB.
        let mut levels = vec![-20.0; 50];
        levels.extend(vec![-60.0; 40]); // 0.4 s pause
        levels.extend(vec![-22.0; 50]);
        levels.extend(vec![-58.0; 5]); // 50 ms: too short
        levels.extend(vec![-21.0; 50]);
        let spans = quiet_spans(&levels, 0.15);
        assert_eq!(spans.len(), 1, "{spans:?}");
        assert!((spans[0].0 - 0.5).abs() < 1e-9 && (spans[0].1 - 0.9).abs() < 1e-9);
    }

    #[test]
    fn the_threshold_adapts_to_loud_room_tone() {
        // Noisy room at -35 dB, speech at -15 dB: a fixed -40 dB threshold
        // would find nothing.
        let mut levels = vec![-15.0; 60];
        levels.extend(vec![-35.0; 30]);
        levels.extend(vec![-16.0; 60]);
        let spans = quiet_spans(&levels, 0.2);
        assert_eq!(spans.len(), 1, "{spans:?}");
    }

    #[test]
    fn pauses_are_measured_from_real_audio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("speechy.wav");
        // Tone 0-1 s, silence 1-1.6 s, tone 1.6-2.5 s.
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=300:duration=2.5,volume='if(between(t,1,1.6),0,1)':eval=frame",
            ])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let pauses = detect_pauses(&path, 0.5, 2.5, 0.2, None).unwrap();
        assert_eq!(pauses.len(), 1, "{pauses:?}");
        // `volume` with eval=frame switches on audio frame boundaries
        // (~23 ms), so the edges are that precise.
        assert!((pauses[0].0 - 1.0).abs() < 0.05, "{pauses:?}");
        assert!((pauses[0].1 - 1.6).abs() < 0.05, "{pauses:?}");
    }
}
