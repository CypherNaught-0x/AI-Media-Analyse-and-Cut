use anyhow::{anyhow, Context, Result};
use parakeet_rs::{ParakeetTDT, TimedToken, TimestampMode, Transcriber};
use std::path::{Path, PathBuf};

use crate::local_asr::{
    build_transcript_segments, diarize, download_file_if_missing, emit_progress,
    load_audio_16k_mono, model_root, resolve_sortformer_file, speaker_label_for_word,
    WordWithSpeaker, HF_RESOLVE_BASE, SAMPLE_RATE,
};
use crate::video::TranscriptSegment;

const CHUNK_SECONDS: usize = 240;
const CHUNK_SAMPLES: usize = CHUNK_SECONDS * SAMPLE_RATE;
const CHUNK_OVERLAP_SAMPLES: usize = 2 * SAMPLE_RATE;
const DEFAULT_TDT_DIR_NAME: &str = "parakeet-tdt-int8";
const DEFAULT_TDT_FILES: [(&str, &str); 3] = [
    (
        "encoder-model.int8.onnx",
        "tdt/encoder-model.int8.onnx?download=1",
    ),
    (
        "decoder_joint-model.int8.onnx",
        "tdt/decoder_joint-model.int8.onnx?download=1",
    ),
    ("vocab.txt", "tdt/vocab.txt?download=1"),
];

fn transcribe_words(
    window: &tauri::Window,
    model: &mut ParakeetTDT,
    audio: &[f32],
) -> Result<Vec<TimedToken>> {
    if audio.is_empty() {
        return Ok(Vec::new());
    }

    let chunk_count = audio.len().div_ceil(CHUNK_SAMPLES);
    let mut words = Vec::new();

    for chunk_index in 0..chunk_count {
        let keep_start = chunk_index * CHUNK_SAMPLES;
        let keep_end = (keep_start + CHUNK_SAMPLES).min(audio.len());
        let window_start = keep_start.saturating_sub(CHUNK_OVERLAP_SAMPLES);
        let window_end = (keep_end + CHUNK_OVERLAP_SAMPLES).min(audio.len());

        let keep_start_seconds = keep_start as f32 / SAMPLE_RATE as f32;
        let keep_end_seconds = keep_end as f32 / SAMPLE_RATE as f32;
        let window_offset_seconds = window_start as f32 / SAMPLE_RATE as f32;

        emit_progress(
            window,
            &format!(
                "Parakeet transcription chunk {}/{}...",
                chunk_index + 1,
                chunk_count
            ),
        )?;

        let result = model.transcribe_samples(
            audio[window_start..window_end].to_vec(),
            SAMPLE_RATE as u32,
            1,
            Some(TimestampMode::Words),
        )?;

        for token in result.tokens {
            let text = token.text.trim().to_string();
            if text.is_empty() {
                continue;
            }

            let absolute_start = token.start + window_offset_seconds;
            let absolute_end = token.end + window_offset_seconds;
            let midpoint = (absolute_start + absolute_end) / 2.0;

            let in_primary_range = if chunk_index + 1 == chunk_count {
                midpoint >= keep_start_seconds && midpoint <= keep_end_seconds
            } else {
                midpoint >= keep_start_seconds && midpoint < keep_end_seconds
            };

            if in_primary_range {
                words.push(TimedToken {
                    text,
                    start: absolute_start,
                    end: absolute_end,
                });
            }
        }
    }

    Ok(words)
}

/// Resolve (and download on first use) the Parakeet TDT model directory only.
/// Used both by full transcription and by the lightweight split-point detection
/// path, which deliberately skips the Sortformer diarization model.
async fn resolve_parakeet_dir(
    window: &tauri::Window,
    parakeet_model_path: &str,
) -> Result<PathBuf> {
    let trimmed = parakeet_model_path.trim();
    if !trimmed.is_empty() {
        return Ok(PathBuf::from(trimmed));
    }

    let parakeet_dir = model_root(window, "parakeet-rs")?.join(DEFAULT_TDT_DIR_NAME);
    tokio::fs::create_dir_all(&parakeet_dir).await?;
    let client = reqwest::Client::new();
    for (file_name, relative_url) in DEFAULT_TDT_FILES {
        download_file_if_missing(
            window,
            &client,
            &parakeet_dir.join(file_name),
            &format!("{HF_RESOLVE_BASE}/{relative_url}"),
            file_name,
        )
        .await?;
    }

    Ok(parakeet_dir)
}

async fn resolve_model_paths(
    window: &tauri::Window,
    parakeet_model_path: &str,
    sortformer_model_path: &str,
) -> Result<(PathBuf, PathBuf)> {
    let parakeet_dir = resolve_parakeet_dir(window, parakeet_model_path).await?;
    let sortformer_file = resolve_sortformer_file(window, sortformer_model_path).await?;

    Ok((parakeet_dir, sortformer_file))
}

/// Transcribe with Parakeet TDT only (no diarization) and return the sorted
/// word *end* times in seconds. Used as a fallback to pick chunk split points
/// that land cleanly between words when no suitable silence gap exists.
pub(crate) async fn parakeet_word_boundaries(
    window: &tauri::Window,
    audio_path: &str,
    parakeet_model_path: &str,
) -> Result<Vec<f64>> {
    let parakeet_dir = resolve_parakeet_dir(window, parakeet_model_path).await?;

    let audio_file = Path::new(audio_path);
    if !audio_file.exists() {
        return Err(anyhow!("Audio file not found: {}", audio_file.display()));
    }
    if !parakeet_dir.is_dir() {
        return Err(anyhow!(
            "Parakeet model path must be a directory: {}",
            parakeet_dir.display()
        ));
    }

    emit_progress(window, "Loading audio for split-point detection...")?;
    let audio = load_audio_16k_mono(audio_file)
        .with_context(|| format!("Failed to load audio '{}'", audio_file.display()))?;

    emit_progress(window, "Loading Parakeet TDT for split-point detection...")?;
    let mut parakeet = ParakeetTDT::from_pretrained(&parakeet_dir, None).with_context(|| {
        format!(
            "Failed to load Parakeet TDT model directory '{}'",
            parakeet_dir.display()
        )
    })?;

    let words =
        transcribe_words(window, &mut parakeet, &audio).context("Parakeet transcription failed")?;

    let mut boundaries: Vec<f64> = words.iter().map(|word| word.end as f64).collect();
    boundaries.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Ok(boundaries)
}

#[tauri::command]
pub async fn transcribe_with_parakeet(
    window: tauri::Window,
    audio_path: String,
    parakeet_model_path: String,
    sortformer_model_path: String,
) -> Result<Vec<TranscriptSegment>, String> {
    let (resolved_parakeet_dir, resolved_sortformer_file) =
        resolve_model_paths(&window, &parakeet_model_path, &sortformer_model_path)
            .await
            .map_err(|error| error.to_string())?;

    let run = || -> Result<Vec<TranscriptSegment>> {
        let audio_file = Path::new(&audio_path);
        if !audio_file.exists() {
            return Err(anyhow!("Audio file not found: {}", audio_file.display()));
        }

        let parakeet_dir = resolved_parakeet_dir.as_path();
        if !parakeet_dir.is_dir() {
            return Err(anyhow!(
                "Parakeet model path must be a directory: {}",
                parakeet_dir.display()
            ));
        }

        emit_progress(&window, "Loading local audio...")?;
        let audio = load_audio_16k_mono(audio_file)
            .with_context(|| format!("Failed to load audio '{}'", audio_file.display()))?;

        emit_progress(&window, "Running Sortformer diarization...")?;
        let diarization = diarize(&resolved_sortformer_file, audio.clone())?;

        emit_progress(&window, "Loading Parakeet TDT...")?;
        let mut parakeet = ParakeetTDT::from_pretrained(parakeet_dir, None).with_context(|| {
            format!(
                "Failed to load Parakeet TDT model directory '{}'",
                parakeet_dir.display()
            )
        })?;

        let words = transcribe_words(&window, &mut parakeet, &audio)
            .context("Parakeet transcription failed")?;

        let speaker_words = words
            .into_iter()
            .map(|word| WordWithSpeaker {
                speaker: speaker_label_for_word(word.start, word.end, &diarization),
                start: word.start,
                end: word.end,
                text: word.text,
            })
            .collect::<Vec<_>>();

        Ok(build_transcript_segments(&speaker_words))
    };

    run().map_err(|error| error.to_string())
}
