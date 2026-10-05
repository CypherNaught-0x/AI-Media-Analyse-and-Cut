use anyhow::{anyhow, Context, Result};
use parakeet_rs::{ParakeetTDT, TimedToken, TimestampMode, Transcriber};
use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::local_asr::{
    build_transcript_segments, diarize, emit_progress, load_audio_16k_mono, model_root,
    onnx_execution_config, resolve_sortformer_file, speaker_label_for_word, WordWithSpeaker,
    SAMPLE_RATE,
};
use crate::model_download::{ensure_pinned_file, HUGGING_FACE, PARAKEET_TDT_INT8};
use crate::run_control::{run_blocking, RunControl};
use crate::video::TranscriptSegment;
use tauri::State;

const CHUNK_SECONDS: usize = 240;
const CHUNK_SAMPLES: usize = CHUNK_SECONDS * SAMPLE_RATE;
const CHUNK_OVERLAP_SAMPLES: usize = 2 * SAMPLE_RATE;
const DEFAULT_TDT_DIR_NAME: &str = "parakeet-tdt-int8";

/// Transcribe `audio` in overlapping chunks. `check` runs before each chunk
/// and aborts with its error, so a cancelled run stops within one chunk.
fn transcribe_words(
    window: &tauri::Window,
    model: &mut ParakeetTDT,
    audio: &[f32],
    check: &dyn Fn() -> Result<()>,
) -> Result<Vec<TimedToken>> {
    if audio.is_empty() {
        return Ok(Vec::new());
    }

    let chunk_count = audio.len().div_ceil(CHUNK_SAMPLES);
    let mut words = Vec::new();

    for chunk_index in 0..chunk_count {
        check()?;
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
    for file in &PARAKEET_TDT_INT8 {
        ensure_pinned_file(
            &client,
            HUGGING_FACE,
            file,
            &parakeet_dir.join(file.file_name()),
            &|message| {
                let _ = emit_progress(window, message);
            },
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
    run: (u64, &RunControl),
) -> Result<Vec<f64>> {
    let parakeet_dir = resolve_parakeet_dir(window, parakeet_model_path).await?;
    let window = window.clone();
    let audio_path = audio_path.to_string();
    let (run_id, run_control) = (run.0, run.1.clone());
    run_blocking(move || {
        let check = || {
            run_control
                .ensure_active(run_id)
                .map_err(|error| anyhow!(error))
        };
        word_boundaries_blocking(&window, &audio_path, &parakeet_dir, &check)
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| anyhow!(error))
}

fn word_boundaries_blocking(
    window: &tauri::Window,
    audio_path: &str,
    parakeet_dir: &Path,
    check: &dyn Fn() -> Result<()>,
) -> Result<Vec<f64>> {
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
    let mut parakeet = ParakeetTDT::from_pretrained(parakeet_dir, Some(onnx_execution_config()))
        .with_context(|| {
            format!(
                "Failed to load Parakeet TDT model directory '{}'",
                parakeet_dir.display()
            )
        })?;

    let words = transcribe_words(window, &mut parakeet, &audio, check)
        .context("Parakeet transcription failed")?;

    let mut boundaries: Vec<f64> = words.iter().map(|word| word.end as f64).collect();
    boundaries.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Ok(boundaries)
}

#[tauri::command]
#[specta::specta]
pub async fn transcribe_with_parakeet(
    run_id: u64,
    window: tauri::Window,
    audio_path: String,
    parakeet_model_path: String,
    sortformer_model_path: String,
    run_control: State<'_, RunControl>,
) -> Result<Vec<TranscriptSegment>, AppError> {
    run_control.ensure_active(run_id)?;
    let run_control = run_control.inner().clone();
    let (resolved_parakeet_dir, resolved_sortformer_file) =
        resolve_model_paths(&window, &parakeet_model_path, &sortformer_model_path)
            .await
            .map_err(|error| error.to_string())?;

    // Audio decoding and both ONNX models are CPU-bound for minutes on long
    // recordings; keep them off the async runtime.
    let run = move || -> Result<Vec<TranscriptSegment>> {
        // Diarization and model loading can't be interrupted; check between
        // the steps and before every transcription chunk.
        let check = || {
            run_control
                .ensure_active(run_id)
                .map_err(|error| anyhow!(error))
        };
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

        check()?;
        emit_progress(&window, "Running Sortformer diarization...")?;
        let diarization = diarize(&resolved_sortformer_file, audio.clone())?;

        check()?;
        emit_progress(&window, "Loading Parakeet TDT...")?;
        let mut parakeet =
            ParakeetTDT::from_pretrained(parakeet_dir, Some(onnx_execution_config()))
                .with_context(|| {
                    format!(
                        "Failed to load Parakeet TDT model directory '{}'",
                        parakeet_dir.display()
                    )
                })?;

        let words = transcribe_words(&window, &mut parakeet, &audio, &check)
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

    run_blocking(move || run().map_err(|error| format!("{error:#}")))
        .await
        .map_err(AppError::from)
}

#[cfg(test)]
mod profiling {
    use super::*;
    use parakeet_rs::sortformer::{DiarizationConfig, Sortformer};
    use parakeet_rs::ExecutionConfig as ModelConfig;
    #[cfg(feature = "profile-coreml")]
    use parakeet_rs::ExecutionProvider;
    use std::time::Instant;

    fn models_dir() -> PathBuf {
        std::env::var_os("PARAKEET_PROFILE_MODELS")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs_home()
                    .join("Library/Application Support/itemis.ai-media-cutter/models/parakeet-rs")
            })
    }

    fn dirs_home() -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap())
    }

    /// Times Sortformer and Parakeet TDT on the 74 s test recording under
    /// different execution configs (see `onnx_execution_config`). Needs the
    /// downloaded models; add `--features profile-coreml` to include CoreML:
    /// `cargo test --release --lib profiling -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn profile_execution_configs() {
        let audio_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../dev-resources/test-data/test_podcast.m4a");
        let audio = load_audio_16k_mono(&audio_path).unwrap();
        let seconds = audio.len() as f32 / SAMPLE_RATE as f32;
        let dir = models_dir();

        #[cfg_attr(not(feature = "profile-coreml"), allow(unused_mut))]
        let mut configs: Vec<(String, ModelConfig)> = [4usize, 6, 8, 12]
            .into_iter()
            .map(|threads| {
                (
                    format!("cpu x{threads}"),
                    ModelConfig::default().with_intra_threads(threads),
                )
            })
            .collect();
        #[cfg(feature = "profile-coreml")]
        configs.push((
            "coreml".into(),
            ModelConfig::default().with_execution_provider(ExecutionProvider::CoreML),
        ));

        for (name, config) in configs {
            let started = Instant::now();
            let mut sortformer = Sortformer::with_config(
                dir.join("diar_streaming_sortformer_4spk-v2.onnx"),
                Some(config.clone()),
                DiarizationConfig::callhome(),
            )
            .unwrap();
            let loaded = started.elapsed();
            sortformer
                .diarize(audio.clone(), SAMPLE_RATE as u32, 1)
                .unwrap();
            let diarized = started.elapsed() - loaded;

            let started = Instant::now();
            let mut tdt =
                ParakeetTDT::from_pretrained(dir.join(DEFAULT_TDT_DIR_NAME), Some(config)).unwrap();
            let tdt_loaded = started.elapsed();
            let result = tdt
                .transcribe_samples(
                    audio.clone(),
                    SAMPLE_RATE as u32,
                    1,
                    Some(TimestampMode::Words),
                )
                .unwrap();
            let transcribed = started.elapsed() - tdt_loaded;

            println!(
                "{name:>10}: sortformer load {:>5.2}s run {:>5.2}s | tdt load {:>5.2}s run {:>5.2}s ({:.1}x realtime, {} tokens)",
                loaded.as_secs_f32(),
                diarized.as_secs_f32(),
                tdt_loaded.as_secs_f32(),
                transcribed.as_secs_f32(),
                seconds / transcribed.as_secs_f32(),
                result.tokens.len()
            );
        }
    }
}
