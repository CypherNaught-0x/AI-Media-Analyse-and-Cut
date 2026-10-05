//! Split a prepared audio file into analysis-sized chunks so that each LLM
//! transcription request stays well under the router's request timeout
//! (long videos otherwise return `504 Gateway Timeout`).
//!
//! Chunk boundaries are chosen so that no speech is cut mid-word:
//!   1. Primary: a ~1s silence gap nearest each target boundary (±10% window).
//!   2. Fallback: a Parakeet word boundary nearest the target (on-demand, no
//!      diarization) when no suitable silence exists in the window.
//!   3. Last resort: a hard cut at the exact target time.

use crate::ffmpeg::{run_ffmpeg, FfmpegTask};
use crate::format_path_io_error;
use crate::parakeet::parakeet_word_boundaries;
use crate::run_control::RunControl;
use crate::silence::{detect_silence_internal, probe_duration};
use ffmpeg_sidecar::command::FfmpegCommand;
#[allow(unused_imports)]
use log::{info, warn};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{Emitter, State};

/// One analysis chunk: a file on disk plus its start time (seconds) relative to
/// the input audio, used to shift transcript timestamps back onto the original
/// timeline.
#[derive(Serialize, Debug, Clone, specta::Type)]
pub struct AudioChunk {
    pub path: String,
    #[specta(type = specta_typescript::Number)] // never NaN; plain `number` in TS
    pub start_offset: f64,
}

/// Silence shorter than this (seconds) is not considered a usable split point.
const SILENCE_MIN_DURATION: f64 = 1.0;
/// A cut must be at least this far from 0 / the end to avoid tiny chunks.
const MIN_CHUNK_SECONDS: f64 = 5.0;

fn emit_message(window: &tauri::Window, message: &str) {
    let _ = window.emit("progress", serde_json::json!({ "message": message }));
}

/// Pick the candidate (silence midpoint or word boundary) whose time is closest
/// to `target`, considering only candidates within `window_half` of it.
fn nearest_within<I>(candidates: I, target: f64, window_half: f64) -> Option<f64>
where
    I: IntoIterator<Item = f64>,
{
    candidates
        .into_iter()
        .filter(|&candidate| (candidate - target).abs() <= window_half)
        .min_by(|a, b| {
            (a - target)
                .abs()
                .partial_cmp(&(b - target).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

#[tauri::command]
#[specta::specta]
pub async fn split_audio_for_analysis(
    run_id: u64,
    window: tauri::Window,
    path: String,
    max_chunk_seconds: f64,
    parakeet_model_path: String,
    run_control: State<'_, RunControl>,
) -> Result<Vec<AudioChunk>, String> {
    run_control.ensure_active(run_id)?;

    let single_chunk = vec![AudioChunk {
        path: path.clone(),
        start_offset: 0.0,
    }];

    // A non-positive limit disables chunking entirely.
    if max_chunk_seconds <= 0.0 {
        return Ok(single_chunk);
    }

    let duration = probe_duration(&path).await?;
    if duration <= max_chunk_seconds {
        return Ok(single_chunk);
    }

    info!(
        "Audio '{}' is {:.1}s long (max chunk {:.1}s); planning split.",
        path, duration, max_chunk_seconds
    );
    emit_message(&window, "Planning audio chunks...");

    let window_half = (max_chunk_seconds * 0.10).max(1.0);

    // Fixed grid of target boundaries at multiples of the max chunk length. A
    // grid (rather than cumulative-from-last-cut) keeps every chunk within
    // ±10% of the configured length.
    let mut targets: Vec<f64> = Vec::new();
    let mut target = max_chunk_seconds;
    while target < duration - MIN_CHUNK_SECONDS {
        targets.push(target);
        target += max_chunk_seconds;
    }
    if targets.is_empty() {
        return Ok(single_chunk);
    }

    // Primary: silence gaps of at least ~1s.
    let silences = detect_silence_internal(
        &path,
        SILENCE_MIN_DURATION,
        Some((run_id, run_control.inner())),
    )
    .await?;
    run_control.ensure_active(run_id)?;
    let silence_midpoints: Vec<f64> = silences
        .iter()
        .map(|interval| (interval.start + interval.end) / 2.0)
        .collect();

    let mut cuts: Vec<f64> = Vec::with_capacity(targets.len());
    let mut unresolved: Vec<usize> = Vec::new();
    for (index, &target) in targets.iter().enumerate() {
        match nearest_within(silence_midpoints.iter().copied(), target, window_half) {
            Some(cut) => cuts.push(cut),
            None => {
                cuts.push(target); // placeholder, possibly refined below
                unresolved.push(index);
            }
        }
    }

    // Fallback: run Parakeet (TDT only) once to get word boundaries for any
    // target that had no nearby silence.
    if !unresolved.is_empty() {
        info!(
            "{} of {} boundaries have no nearby silence; trying Parakeet word boundaries.",
            unresolved.len(),
            targets.len()
        );
        emit_message(
            &window,
            "No silence near some boundaries; locating word boundaries with Parakeet...",
        );
        match parakeet_word_boundaries(&window, &path, &parakeet_model_path).await {
            Ok(boundaries) => {
                for &index in &unresolved {
                    if let Some(cut) =
                        nearest_within(boundaries.iter().copied(), targets[index], window_half)
                    {
                        cuts[index] = cut;
                    }
                    // else: keep the hard-cut placeholder at the target time.
                }
            }
            Err(error) => {
                warn!(
                    "Parakeet split-point fallback failed ({}); using hard cuts at target times.",
                    error
                );
            }
        }
        run_control.ensure_active(run_id)?;
    }

    // Clean up the cut list: in-bounds, strictly increasing, de-duplicated.
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut boundaries: Vec<f64> = Vec::with_capacity(cuts.len());
    for cut in cuts {
        if cut <= MIN_CHUNK_SECONDS || cut >= duration - MIN_CHUNK_SECONDS {
            continue;
        }
        if boundaries
            .last()
            .map(|&last| cut - last < MIN_CHUNK_SECONDS)
            .unwrap_or(false)
        {
            continue;
        }
        boundaries.push(cut);
    }

    if boundaries.is_empty() {
        return Ok(single_chunk);
    }

    // Build [start, end) ranges from the boundaries.
    let mut ranges: Vec<(f64, f64)> = Vec::with_capacity(boundaries.len() + 1);
    let mut start = 0.0;
    for &boundary in &boundaries {
        ranges.push((start, boundary));
        start = boundary;
    }
    ranges.push((start, duration));

    let input_path = PathBuf::from(&path);
    let stem = input_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "audio".to_string());
    let chunk_dir = std::env::temp_dir().join("ai-media-cutter-chunks");
    std::fs::create_dir_all(&chunk_dir)
        .map_err(|error| format_path_io_error("create chunk directory", &chunk_dir, &error))?;
    let nonce = fastrand::u64(..);

    info!("Splitting '{}' into {} chunks.", path, ranges.len());

    let mut chunks: Vec<AudioChunk> = Vec::with_capacity(ranges.len());
    for (index, (chunk_start, chunk_end)) in ranges.iter().enumerate() {
        run_control.ensure_active(run_id)?;
        emit_message(
            &window,
            &format!("Splitting audio (part {}/{})...", index + 1, ranges.len()),
        );

        let output_path = chunk_dir.join(format!("{}_{:x}_chunk{:03}.ogg", stem, nonce, index));
        let chunk_duration = (chunk_end - chunk_start).max(0.0);

        let mut command = FfmpegCommand::new();
        command
            .args(["-y", "-ss", &chunk_start.to_string()])
            .input(input_path.to_str().unwrap())
            .args([
                "-t",
                &chunk_duration.to_string(),
                "-vn",
                "-c:a",
                "libopus",
                "-b:a",
                "96k",
            ])
            .output(output_path.to_str().unwrap());
        run_ffmpeg(
            command,
            FfmpegTask {
                operation: &format!("split audio for analysis (part {})", index + 1),
                input: &input_path,
                output: Some(&output_path),
                run: Some((run_id, run_control.inner())),
            },
            |_| {},
        )?;

        chunks.push(AudioChunk {
            path: output_path.to_string_lossy().to_string(),
            start_offset: *chunk_start,
        });
    }

    Ok(chunks)
}
