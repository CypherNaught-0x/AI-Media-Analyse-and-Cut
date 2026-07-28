use crate::{format_ffmpeg_spawn_error, format_path_io_error};
use crate::run_control::{RunControl, RUN_CANCELLED_MESSAGE};
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use ffmpeg_sidecar::paths::ffmpeg_path;
use log::{debug, info};
use regex::Regex;
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

#[derive(Serialize, Debug, Clone)]
pub struct SilenceInterval {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
}

#[derive(Serialize, Debug, Clone)]
pub struct SegmentOffset {
    pub min_time: f64,
    pub offset: f64,
}

#[derive(Serialize, Debug, Clone)]
pub struct ProcessedAudio {
    pub path: String,
    pub silence_intervals: Vec<SilenceInterval>,
    pub offsets: Vec<SegmentOffset>,
}

/// Shortest stretch of audio worth keeping. `silencedetect` regularly splits one
/// long silence into two intervals separated by a fraction of a millisecond (a
/// single sample poking above the threshold), and the probed duration can end a
/// few milliseconds past the last silence. Keeping those slivers would add
/// offset-table entries that map the timestamps falling inside them onto the
/// wrong original time, so they are folded into the surrounding silence instead.
const MIN_KEEP_SECONDS: f64 = 0.05;

/// Sort silences and merge the ones that overlap or are separated by less than
/// [`MIN_KEEP_SECONDS`], so every gap between them is an audible stretch.
fn merge_silence_intervals(intervals: &[SilenceInterval]) -> Vec<SilenceInterval> {
    let mut sorted = intervals.to_vec();
    sorted.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut merged: Vec<SilenceInterval> = Vec::with_capacity(sorted.len());
    for interval in sorted {
        match merged.last_mut() {
            Some(last) if interval.start - last.end < MIN_KEEP_SECONDS => {
                if interval.end > last.end {
                    last.end = interval.end;
                    last.duration = last.end - last.start;
                }
            }
            _ => merged.push(interval),
        }
    }

    merged
}

/// The stretches of `duration` seconds of audio that survive silence removal, in
/// original-timeline seconds. `silence_intervals` must already be merged.
fn plan_keep_segments(silence_intervals: &[SilenceInterval], duration: f64) -> Vec<(f64, f64)> {
    let mut keep_segments = Vec::new();
    let mut last_end = 0.0;

    for interval in silence_intervals {
        if interval.start - last_end >= MIN_KEEP_SECONDS {
            keep_segments.push((last_end, interval.start));
        }
        last_end = last_end.max(interval.end);
    }

    if duration - last_end >= MIN_KEEP_SECONDS {
        keep_segments.push((last_end, duration));
    }

    keep_segments
}

/// Table that maps a position in the trimmed timeline back onto the original
/// one: for every keep segment, the amount of removed silence to add back.
fn build_offsets(keep_segments: &[(f64, f64)]) -> Vec<SegmentOffset> {
    let mut offsets = Vec::with_capacity(keep_segments.len());
    let mut current_new_time = 0.0;

    for (start, end) in keep_segments {
        offsets.push(SegmentOffset {
            min_time: current_new_time,
            offset: *start - current_new_time,
        });
        current_new_time += end - start;
    }

    offsets
}

#[tauri::command]
pub async fn detect_silence(
    run_id: Option<u64>,
    path: String,
    min_duration: Option<f64>,
    run_control: State<'_, RunControl>,
) -> Result<Vec<SilenceInterval>, String> {
    let requested_run_id = run_id.unwrap_or(0);
    detect_silence_internal(
        &path,
        min_duration.unwrap_or(0.5),
        if requested_run_id == 0 {
            None
        } else {
            Some((requested_run_id, run_control.inner()))
        },
    )
    .await
}

pub(crate) async fn detect_silence_internal(
    path: &str,
    min_duration: f64,
    run_control: Option<(u64, &RunControl)>,
) -> Result<Vec<SilenceInterval>, String> {
    let input_path = PathBuf::from(path);
    if !input_path.exists() {
        return Err("File not found".to_string());
    }

    info!(
        "Starting silence detection for {:?} with min_duration {}",
        input_path, min_duration
    );

    // ffmpeg -i input.mp4 -af silencedetect=noise=-30dB:d=min_duration -f null -
    if let Some((run_id, run_control)) = run_control {
        run_control.ensure_active(run_id)?;
    }

    let mut child = FfmpegCommand::new()
        .input(input_path.to_str().unwrap())
        .args([
            "-af",
            &format!("silencedetect=noise=-30dB:d={}", min_duration),
            "-f",
            "null",
            "-",
        ])
        .spawn()
        .map_err(|e| format_ffmpeg_spawn_error("detect silence", &input_path, None, &e))?;

    let pid = child.as_inner().id();
    if let Some((run_id, run_control)) = run_control {
        run_control.register_pid(run_id, pid)?;
    }

    let events = child
        .iter()
        .map_err(|e| {
            format!(
                "Failed while reading FFmpeg output during silence detection for '{}': {}",
                input_path.display(),
                e
            )
        })?;

    let mut intervals = Vec::new();
    let mut current_start = None;

    // Regex for start: silence_start: 12.345
    let re_start = Regex::new(r"silence_start: (\d+(\.\d+)?)").unwrap();
    // Regex for end: silence_end: 15.678
    let re_end = Regex::new(r"silence_end: (\d+(\.\d+)?)").unwrap();

    for event in events {
        if let Some((run_id, run_control)) = run_control {
            if run_control.is_cancelled(run_id) {
                break;
            }
        }

        if let FfmpegEvent::Log(_, line) = event {
            // debug!("[FFmpeg] {}", line); // Too verbose
            if let Some(caps) = re_start.captures(&line) {
                if let Some(m) = caps.get(1) {
                    if let Ok(val) = m.as_str().parse::<f64>() {
                        current_start = Some(val);
                        debug!("Silence start detected at {}", val);
                    }
                }
            } else if let Some(caps) = re_end.captures(&line) {
                if let Some(m) = caps.get(1) {
                    if let Ok(end_val) = m.as_str().parse::<f64>() {
                        if let Some(start_val) = current_start {
                            intervals.push(SilenceInterval {
                                start: start_val,
                                end: end_val,
                                duration: end_val - start_val,
                            });
                            debug!(
                                "Silence interval: {} - {} (duration: {})",
                                start_val,
                                end_val,
                                end_val - start_val
                            );
                            current_start = None;
                        }
                    }
                }
            }
        }
    }

    if let Some((run_id, run_control)) = run_control {
        run_control.clear_pid(run_id, pid);
        if run_control.is_cancelled(run_id) {
            return Err(RUN_CANCELLED_MESSAGE.to_string());
        }
    }

    info!(
        "Silence detection complete. Found {} intervals.",
        intervals.len()
    );
    Ok(intervals)
}

#[tauri::command]
pub async fn remove_silence(
    run_id: Option<u64>,
    path: String,
    min_duration: Option<f64>,
    run_control: State<'_, RunControl>,
) -> Result<ProcessedAudio, String> {
    let requested_run_id = run_id.unwrap_or(0);
    remove_silence_internal(
        path,
        min_duration,
        if requested_run_id == 0 {
            None
        } else {
            Some((requested_run_id, run_control.inner()))
        },
    )
    .await
}

async fn remove_silence_internal(
    path: String,
    min_duration: Option<f64>,
    run_context: Option<(u64, &RunControl)>,
) -> Result<ProcessedAudio, String> {
    let min_duration_val = min_duration.unwrap_or(10.0);
    let detected_intervals = detect_silence_internal(&path, min_duration_val, run_context).await?;
    let silence_intervals = merge_silence_intervals(&detected_intervals);
    let input_path = PathBuf::from(&path);

    if silence_intervals.is_empty() {
        return Ok(ProcessedAudio {
            path,
            silence_intervals,
            offsets: vec![SegmentOffset {
                min_time: 0.0,
                offset: 0.0,
            }],
        });
    }

    let output_path = input_path.with_file_name(format!(
        "{}_nosilence.ogg",
        input_path.file_stem().unwrap().to_string_lossy()
    ));

    // `silencedetect` does not report a silence that runs to EOF as ending, so
    // the tail is derived from the probed duration rather than from the intervals.
    let duration = probe_duration(&path)
        .await
        .map_err(|e| format!("Failed to probe media duration for silence removal: {}", e))?;

    // The keep segments drive both the filtergraph and the offset table, so the
    // trimmed audio and the trimmed -> original mapping cannot disagree.
    let keep_segments = plan_keep_segments(&silence_intervals, duration);

    if keep_segments.is_empty() {
        info!("Silence removal found nothing worth keeping; using the original audio.");
        return Ok(ProcessedAudio {
            path,
            silence_intervals,
            offsets: vec![SegmentOffset {
                min_time: 0.0,
                offset: 0.0,
            }],
        });
    }

    info!("Removing silence. Keep segments: {:?}", keep_segments);

    let offsets = build_offsets(&keep_segments);

    // Build filter complex
    let mut filter_complex = String::new();
    let mut inputs = String::new();

    for (i, (start, end)) in keep_segments.iter().enumerate() {
        filter_complex.push_str(&format!(
            "[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{}];",
            start, end, i
        ));
        inputs.push_str(&format!("[a{}]", i));
    }

    filter_complex.push_str(&format!(
        "{}concat=n={}:v=0:a=1[outa]",
        inputs,
        keep_segments.len()
    ));

    info!("Running FFmpeg to remove silence...");

    let mut last_error = None;

    if let Some((run_id, run_control)) = run_context {
        run_control.ensure_active(run_id)?;
    }

    let mut child = FfmpegCommand::new()
        .input(input_path.to_str().unwrap())
        .args([
            "-y",
            "-filter_complex",
            &filter_complex,
            "-map",
            "[outa]",
            "-c:a",
            "libopus",
            "-b:a",
            "96k",
        ])
        .output(output_path.to_str().unwrap())
        .spawn()
        .map_err(|e| {
            format_ffmpeg_spawn_error("remove silence", &input_path, Some(&output_path), &e)
        })?;

    let pid = child.as_inner().id();
    if let Some((run_id, run_control)) = run_context {
        run_control.register_pid(run_id, pid)?;
    }

    child
        .iter()
        .map_err(|e| {
            format!(
                "Failed while reading FFmpeg output during silence removal for '{}': {}",
                input_path.display(),
                e
            )
        })?
        .for_each(|event| match event {
            FfmpegEvent::Log(_, msg) => {
                debug!("[FFmpeg Remove Silence] {}", msg);
            }
            FfmpegEvent::Error(err) => {
                last_error = Some(err);
            }
            _ => {}
        });

    if let Some((run_id, run_control)) = run_context {
        run_control.clear_pid(run_id, pid);
        if run_control.is_cancelled(run_id) {
            return Err(RUN_CANCELLED_MESSAGE.to_string());
        }
    }

    if !output_path.exists() {
        let msg = last_error.unwrap_or_else(|| "Unknown error".to_string());
        return Err(format!(
            "FFmpeg failed to create output file: {:?}. Error: {}",
            output_path, msg
        ));
    }

    info!("Silence removed. New file: {:?}", output_path);

    Ok(ProcessedAudio {
        path: output_path.to_string_lossy().to_string(),
        silence_intervals,
        offsets,
    })
}

pub(crate) async fn probe_duration(path: &str) -> Result<f64, String> {
    use std::process::Command;

    // Try using ffmpeg -i path
    // We assume ffmpeg is in PATH (which it should be if init_ffmpeg was called or if installed globally)
    // In tests, we saw it works.
    let output = Command::new(ffmpeg_path())
        .arg("-i")
        .arg(path)
        .output()
        .map_err(|e| {
            format_path_io_error("run ffmpeg for duration probing", &PathBuf::from(path), &e)
        })?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    let re_duration = Regex::new(r"Duration: (\d{2}):(\d{2}):(\d{2}\.\d{2})").unwrap();

    if let Some(caps) = re_duration.captures(&stderr) {
        let hours: f64 = caps[1].parse().unwrap_or(0.0);
        let minutes: f64 = caps[2].parse().unwrap_or(0.0);
        let seconds: f64 = caps[3].parse().unwrap_or(0.0);
        return Ok(hours * 3600.0 + minutes * 60.0 + seconds);
    }

    Err(format!(
        "Failed to parse duration from ffmpeg output. Stderr: {}",
        stderr
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn interval(start: f64, end: f64) -> SilenceInterval {
        SilenceInterval {
            start,
            end,
            duration: end - start,
        }
    }

    /// Maps a trimmed-timeline position back onto the original one, mirroring
    /// `adjustTimestamp` in `src/composables/useTimeFormat.ts`.
    fn to_original(trimmed: f64, offsets: &[SegmentOffset]) -> f64 {
        let mut offset = 0.0;
        for entry in offsets {
            if trimmed >= entry.min_time {
                offset = entry.offset;
            } else {
                break;
            }
        }
        trimmed + offset
    }

    #[test]
    fn test_merges_near_adjacent_silences() {
        // silencedetect splits one silence in two when a single sample pokes
        // above the threshold; the 104us gap is not a keepable stretch.
        let merged =
            merge_silence_intervals(&[interval(0.0, 183.634167), interval(183.634271, 197.049583)]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start, 0.0);
        assert_eq!(merged[0].end, 197.049583);
        assert!((merged[0].duration - 197.049583).abs() < 1e-9);
    }

    #[test]
    fn test_merges_overlapping_and_unsorted_silences() {
        let merged = merge_silence_intervals(&[
            interval(30.0, 45.0),
            interval(10.0, 20.0),
            interval(15.0, 25.0),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!((merged[0].start, merged[0].end), (10.0, 25.0));
        assert_eq!((merged[1].start, merged[1].end), (30.0, 45.0));
    }

    #[test]
    fn test_offsets_map_trimmed_start_onto_first_kept_audio() {
        // Real numbers from a 4517.47s recording with a silent 3:17 intro: the
        // 104us sliver between the two intro silences and the 6.5ms tail past
        // the last silence must not become keep segments, or the timestamps
        // landing in them are mapped onto the wrong original time.
        let duration = 4517.467833;
        let silences = merge_silence_intervals(&[
            interval(0.0, 183.634167),
            interval(183.634271, 197.049583),
            interval(4462.551937, 4517.461333),
        ]);
        let keep_segments = plan_keep_segments(&silences, duration);
        let offsets = build_offsets(&keep_segments);

        assert_eq!(keep_segments, vec![(197.049583, 4462.551937)]);
        assert_eq!(offsets.len(), 1);
        assert_eq!(offsets[0].min_time, 0.0);

        // The first word of the trimmed audio is the first word of the show.
        assert!((to_original(0.0, &offsets) - 197.049583).abs() < 1e-6);
        assert!((to_original(0.4, &offsets) - 197.449583).abs() < 1e-6);
        // The last trimmed instant still maps inside the original media.
        let trimmed_duration: f64 = keep_segments.iter().map(|(a, b)| b - a).sum();
        assert!(to_original(trimmed_duration, &offsets) <= duration);
    }

    #[test]
    fn test_keep_segments_and_offsets_cover_speech_between_silences() {
        let silences = merge_silence_intervals(&[interval(0.0, 60.0), interval(120.0, 180.0)]);
        let keep_segments = plan_keep_segments(&silences, 240.0);
        let offsets = build_offsets(&keep_segments);

        assert_eq!(keep_segments, vec![(60.0, 120.0), (180.0, 240.0)]);
        assert_eq!(offsets.len(), 2);
        // Trimmed 0..60 is original 60..120, trimmed 60..120 is original 180..240.
        assert_eq!(to_original(0.0, &offsets), 60.0);
        assert_eq!(to_original(59.999, &offsets), 119.999);
        assert_eq!(to_original(60.0, &offsets), 180.0);
        assert_eq!(to_original(119.999, &offsets), 239.999);
    }

    #[test]
    fn test_keep_segments_empty_when_everything_is_silent() {
        let silences = merge_silence_intervals(&[interval(0.0, 120.0)]);
        assert!(plan_keep_segments(&silences, 120.001).is_empty());
    }

    fn get_test_file_path() -> PathBuf {
        let mut path = std::env::current_dir().unwrap();
        // If we are in src-tauri, go up one level
        if path.ends_with("src-tauri") {
            path.pop();
        }
        path.join("dev-resources")
            .join("test-data")
            .join("test_podcast.m4a")
    }

    #[tokio::test]
    async fn test_silence_detection_and_removal() {
        let original_path = get_test_file_path();
        assert!(
            original_path.exists(),
            "Test file not found at {:?}",
            original_path
        );

        let temp_dir = std::env::temp_dir().join("ai-media-cutter-tests");
        if !temp_dir.exists() {
            std::fs::create_dir_all(&temp_dir).unwrap();
        }

        let test_file_path = temp_dir.join("test_with_silence.m4a");

        // Create a file with silence prepended and appended
        // Prepend 2s silence, Append 2s silence
        // Using ffmpeg command directly as FfmpegCommand might be harder to construct for complex filter with lavfi
        // We assume ffmpeg is in PATH for tests

        let status = Command::new("ffmpeg")
            .args(&[
                "-y",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=stereo:d=2",
                "-i",
                original_path.to_str().unwrap(),
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=stereo:d=2",
                "-filter_complex",
                "[0:a][1:a][2:a]concat=n=3:v=0:a=1[out]",
                "-map",
                "[out]",
                test_file_path.to_str().unwrap(),
            ])
            .status()
            .expect("Failed to execute ffmpeg");

        assert!(status.success(), "Failed to create test file with silence");
        assert!(test_file_path.exists());

        // 1. Test Detect Silence
        let intervals = detect_silence_internal(test_file_path.to_str().unwrap(), 0.5, None)
            .await
            .unwrap();

        println!("Detected intervals: {:?}", intervals);

        // We expect at least 2 intervals: one at start (approx 0-2s) and one at end.
        // Note: silencedetect might not be perfect at exact boundaries.

        let start_silence = intervals.iter().find(|i| i.start < 0.5 && i.end > 1.5);
        assert!(
            start_silence.is_some(),
            "Should detect silence at the beginning"
        );

        // 2. Test Remove Silence
        let processed = remove_silence_internal(
            test_file_path.to_str().unwrap().to_string(),
            Some(0.5),
            None,
        )
            .await
            .unwrap();

        assert!(
            Path::new(&processed.path).exists(),
            "Processed file should exist"
        );

        // Check duration of processed file
        let processed_duration = probe_duration(&processed.path).await.unwrap();

        // Calculate expected duration
        // We need the duration of the input file (test_file_path) which has the added silence
        // But we can't probe it easily here because we might have deleted it? No, we haven't.
        // But wait, remove_silence takes a path string.

        let test_file_duration = probe_duration(test_file_path.to_str().unwrap())
            .await
            .unwrap();
        let total_silence_duration: f64 =
            processed.silence_intervals.iter().map(|i| i.duration).sum();
        let expected_duration = test_file_duration - total_silence_duration;

        println!("Test file duration: {}", test_file_duration);
        println!("Total silence detected: {}", total_silence_duration);
        println!("Processed duration (probe): {}", processed_duration);
        println!("Expected duration (calc): {}", expected_duration);

        assert!(
            (processed_duration - expected_duration).abs() < 1.0,
            "Processed duration should match expected duration (original - silence)"
        );

        // Clean up
        // std::fs::remove_file(test_file_path).unwrap();
        // std::fs::remove_file(processed.path).unwrap();
    }
}
