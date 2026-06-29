use anyhow::{anyhow, Context, Result};
use ffmpeg_sidecar::paths::ffmpeg_path;
use hound::WavReader;
use parakeet_rs::sortformer::{DiarizationConfig, Sortformer, SpeakerSegment};
use parakeet_rs::{ParakeetTDT, TimedToken, TimestampMode, Transcriber};
use std::path::{Path, PathBuf};
use tauri::{Emitter, Manager};
use tokio::io::AsyncWriteExt;

use crate::time_utils::format_time;
use crate::video::{TranscriptSegment, TranscriptWord};

const SAMPLE_RATE: usize = 16_000;
const CHUNK_SECONDS: usize = 240;
const CHUNK_SAMPLES: usize = CHUNK_SECONDS * SAMPLE_RATE;
const CHUNK_OVERLAP_SAMPLES: usize = 2 * SAMPLE_RATE;
const MAX_SEGMENT_CHARS: usize = 120;
const MIN_SEGMENT_CHARS_FOR_PUNCT_BREAK: usize = 48;
const MIN_SEGMENT_WORDS_FOR_PUNCT_BREAK: usize = 6;
const MIN_SEGMENT_CHARS_FOR_PAUSE_BREAK: usize = 32;
const PAUSE_BREAK_SECONDS: f32 = 0.9;
const DEFAULT_TDT_DIR_NAME: &str = "parakeet-tdt-int8";
const DEFAULT_SORTFORMER_FILE_NAME: &str = "diar_streaming_sortformer_4spk-v2.onnx";
const HF_RESOLVE_BASE: &str = "https://huggingface.co/altunenes/parakeet-rs/resolve/main";
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

#[derive(Clone)]
struct WordWithSpeaker {
    start: f32,
    end: f32,
    text: String,
    speaker: String,
}

fn emit_progress(window: &tauri::Window, message: &str) -> Result<()> {
    window
        .emit(
            "progress",
            serde_json::json!({
                "message": message,
            }),
        )
        .map_err(|e| anyhow!(e.to_string()))
}

fn format_timestamp(seconds: f32) -> String {
    format_time(seconds.max(0.0) as f64)
}

fn is_punctuation_only(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|char| matches!(char, '.' | ',' | '!' | '?' | ';' | ':' | ')' | ']' | '"'))
}

fn split_leading_punctuation(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim_start();
    let prefix_len = trimmed
        .char_indices()
        .take_while(|(_, char)| matches!(char, '.' | ',' | '!' | '?' | ';' | ':' | ')' | ']' | '"'))
        .map(|(index, char)| index + char.len_utf8())
        .last()
        .unwrap_or(0);

    if prefix_len == 0 {
        return None;
    }

    let prefix = trimmed[..prefix_len].to_string();
    let remainder = trimmed[prefix_len..].trim_start().to_string();
    (!remainder.is_empty()).then_some((prefix, remainder))
}

fn append_token_text(buffer: &mut String, token: &str) {
    if token.is_empty() {
        return;
    }

    if !buffer.is_empty() && !is_punctuation_only(token) {
        buffer.push(' ');
    }
    buffer.push_str(token);
}

/// Attach punctuation to the word it belongs to *before* segmentation, so a
/// trailing mark can never be stranded at the start of the next segment.
///
/// Parakeet emits punctuation either as a standalone token (e.g. `"."`) or as a
/// prefix on the following word (e.g. `". Kümmern"`). Because diarization can
/// misattribute that lone mark to the next speaker — and pause/length breaks are
/// evaluated per word — the mark would otherwise open a new segment as `". ..."`.
/// Normalising at the word level removes the problem at its source instead of
/// trying to repair already-built segments afterwards.
fn glue_punctuation_to_previous(words: &[WordWithSpeaker]) -> Vec<WordWithSpeaker> {
    let mut result: Vec<WordWithSpeaker> = Vec::with_capacity(words.len());

    for word in words {
        let trimmed = word.text.trim();
        if trimmed.is_empty() {
            continue;
        }

        // A pure-punctuation token belongs to the preceding word; if there is no
        // preceding word (start of transcript) it is dropped.
        if is_punctuation_only(trimmed) {
            if let Some(previous) = result.last_mut() {
                previous.text.push_str(trimmed);
                previous.end = previous.end.max(word.end);
            }
            continue;
        }

        // A leading punctuation prefix is peeled onto the preceding word so the
        // current word starts clean.
        if let Some((prefix, remainder)) = split_leading_punctuation(trimmed) {
            if let Some(previous) = result.last_mut() {
                previous.text.push_str(&prefix);
            }
            result.push(WordWithSpeaker {
                text: remainder,
                ..word.clone()
            });
            continue;
        }

        result.push(word.clone());
    }

    result
}

fn finalize_segment(words: &[WordWithSpeaker]) -> Option<TranscriptSegment> {
    let first = words.first()?;
    let last = words.last()?;
    let mut text = String::new();
    let mut transcript_words = Vec::with_capacity(words.len());

    for word in words {
        append_token_text(&mut text, &word.text);
        transcript_words.push(TranscriptWord {
            start: format_timestamp(word.start),
            end: format_timestamp(word.end),
            text: word.text.clone(),
            speaker: Some(word.speaker.clone()),
        });
    }

    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    Some(TranscriptSegment {
        start: format_timestamp(first.start),
        end: format_timestamp(last.end),
        speaker: first.speaker.clone(),
        text,
        words: Some(transcript_words),
        alternatives: None,
        merge_status: None,
        active_source: None,
        similarity_score: None,
    })
}

fn speaker_label_for_word(token: &TimedToken, diarization: &[SpeakerSegment]) -> String {
    let mut best_overlap = 0.0f32;
    let mut best_speaker_id = None;

    for segment in diarization {
        let seg_start = segment.start as f32 / SAMPLE_RATE as f32;
        let seg_end = segment.end as f32 / SAMPLE_RATE as f32;
        let overlap_start = token.start.max(seg_start);
        let overlap_end = token.end.min(seg_end);
        let overlap = (overlap_end - overlap_start).max(0.0);

        if overlap > best_overlap {
            best_overlap = overlap;
            best_speaker_id = Some(segment.speaker_id);
        }
    }

    if let Some(speaker_id) = best_speaker_id {
        return format!("Speaker {}", speaker_id + 1);
    }

    let token_midpoint = (token.start + token.end) / 2.0;
    let nearest = diarization
        .iter()
        .map(|segment| {
            let seg_midpoint = ((segment.start + segment.end) as f32 / 2.0) / SAMPLE_RATE as f32;
            (segment.speaker_id, (token_midpoint - seg_midpoint).abs())
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    if let Some((speaker_id, distance)) = nearest {
        if distance <= 1.5 {
            return format!("Speaker {}", speaker_id + 1);
        }
    }

    "Speaker Unknown".to_string()
}

fn build_transcript_segments(words: &[WordWithSpeaker]) -> Vec<TranscriptSegment> {
    let words = glue_punctuation_to_previous(words);
    let mut segments = Vec::new();
    let mut current_words: Vec<WordWithSpeaker> = Vec::new();
    let mut current_text = String::new();

    for word in &words {
        let long_pause_before_word = current_words
            .last()
            .map(|previous_word| (word.start - previous_word.end) >= PAUSE_BREAK_SECONDS)
            .unwrap_or(false);
        let speaker_changed = current_words
            .first()
            .map(|segment_word| segment_word.speaker != word.speaker)
            .unwrap_or(false);

        if speaker_changed
            || (long_pause_before_word && current_text.len() >= MIN_SEGMENT_CHARS_FOR_PAUSE_BREAK)
        {
            if let Some(segment) = finalize_segment(&current_words) {
                segments.push(segment);
            }
            current_words.clear();
            current_text.clear();
        }

        let mut next_text = current_text.clone();
        append_token_text(&mut next_text, &word.text);
        current_words.push(word.clone());
        current_text = next_text;
        let lexical_word_count = current_words
            .iter()
            .filter(|current_word| !is_punctuation_only(current_word.text.trim()))
            .count();

        let should_close = current_text.len() >= MAX_SEGMENT_CHARS
            || (matches!(word.text.chars().last(), Some('.' | '!' | '?'))
                && (current_text.len() >= MIN_SEGMENT_CHARS_FOR_PUNCT_BREAK
                    || lexical_word_count >= MIN_SEGMENT_WORDS_FOR_PUNCT_BREAK));

        if should_close {
            if let Some(segment) = finalize_segment(&current_words) {
                segments.push(segment);
            }
            current_words.clear();
            current_text.clear();
        }
    }

    if let Some(segment) = finalize_segment(&current_words) {
        segments.push(segment);
    }

    segments
}

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

fn load_audio_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let temp_wav_path = std::env::temp_dir().join(format!(
        "ai-media-cutter-parakeet-{}.wav",
        fastrand::u64(..)
    ));

    let output = std::process::Command::new(ffmpeg_path())
        .arg("-y")
        .arg("-i")
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(&temp_wav_path)
        .output()
        .with_context(|| format!("Failed to spawn FFmpeg for '{}'", path.display()))?;

    if !output.status.success() || !temp_wav_path.exists() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "Failed to normalize audio '{}' for Parakeet with FFmpeg: {}",
            path.display(),
            stderr.trim()
        ));
    }

    let result = (|| -> Result<Vec<f32>> {
        let mut reader = WavReader::open(&temp_wav_path).with_context(|| {
            format!(
                "Failed to open normalized WAV '{}'",
                temp_wav_path.display()
            )
        })?;
        let spec = reader.spec();

        if spec.sample_rate != SAMPLE_RATE as u32 || spec.channels != 1 {
            return Err(anyhow!(
                "Normalized WAV has unexpected format: {} Hz, {} channel(s)",
                spec.sample_rate,
                spec.channels
            ));
        }

        let samples = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|error| anyhow!("Failed to read float WAV samples: {error}"))?,
            hound::SampleFormat::Int => reader
                .samples::<i16>()
                .map(|sample| sample.map(|value| value as f32 / 32768.0))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|error| anyhow!("Failed to read PCM WAV samples: {error}"))?,
        };

        Ok(samples)
    })();

    let _ = std::fs::remove_file(&temp_wav_path);
    result
}

async fn download_file_if_missing(
    window: &tauri::Window,
    client: &reqwest::Client,
    destination: &Path,
    url: &str,
    label: &str,
) -> Result<()> {
    if destination.exists() {
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    emit_progress(window, &format!("Downloading {label}..."))?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to start download for {label}"))?
        .error_for_status()
        .with_context(|| format!("Failed to download {label}"))?;

    let total_bytes = response.content_length();
    let temp_path = destination.with_extension("partial");
    let mut file = tokio::fs::File::create(&temp_path).await.with_context(|| {
        format!(
            "Failed to create temporary file for {}",
            destination.display()
        )
    })?;

    let mut downloaded_bytes = 0u64;
    let mut last_reported_percent = 0u64;
    let mut response = response;

    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
        downloaded_bytes += chunk.len() as u64;

        if let Some(total_bytes) = total_bytes {
            let percent = ((downloaded_bytes as f64 / total_bytes as f64) * 100.0) as u64;
            if percent >= last_reported_percent + 10 {
                last_reported_percent = percent.min(100);
                emit_progress(
                    window,
                    &format!("Downloading {label}... {}%", last_reported_percent),
                )?;
            }
        }
    }

    file.flush().await?;
    drop(file);
    tokio::fs::rename(&temp_path, destination)
        .await
        .with_context(|| {
            format!(
                "Failed to finalize downloaded file '{}'",
                destination.display()
            )
        })?;

    emit_progress(window, &format!("Downloaded {label}"))?;
    Ok(())
}

fn default_model_root(window: &tauri::Window) -> Result<PathBuf> {
    window
        .path()
        .app_data_dir()
        .map_err(|error| anyhow!(error.to_string()))
        .map(|path| path.join("models").join("parakeet-rs"))
}

async fn resolve_model_paths(
    window: &tauri::Window,
    parakeet_model_path: &str,
    sortformer_model_path: &str,
) -> Result<(PathBuf, PathBuf)> {
    let mut parakeet_dir = parakeet_model_path
        .trim()
        .is_empty()
        .then(|| default_model_root(window).map(|root| root.join(DEFAULT_TDT_DIR_NAME)))
        .transpose()?;
    let mut sortformer_file = sortformer_model_path
        .trim()
        .is_empty()
        .then(|| default_model_root(window).map(|root| root.join(DEFAULT_SORTFORMER_FILE_NAME)))
        .transpose()?;

    if parakeet_dir.is_none() {
        parakeet_dir = Some(PathBuf::from(parakeet_model_path.trim()));
    }

    if sortformer_file.is_none() {
        sortformer_file = Some(default_model_root(window)?.join(DEFAULT_SORTFORMER_FILE_NAME));
    } else if !sortformer_model_path.trim().is_empty() {
        sortformer_file = Some(PathBuf::from(sortformer_model_path.trim()));
    }

    let parakeet_dir = parakeet_dir.expect("parakeet path must be resolved");
    let sortformer_file = sortformer_file.expect("sortformer path must be resolved");

    if parakeet_model_path.trim().is_empty() || sortformer_model_path.trim().is_empty() {
        let root = default_model_root(window)?;
        tokio::fs::create_dir_all(&root).await?;
    }

    if parakeet_model_path.trim().is_empty() {
        let client = reqwest::Client::new();
        tokio::fs::create_dir_all(&parakeet_dir).await?;
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
    }

    if sortformer_model_path.trim().is_empty() {
        let client = reqwest::Client::new();
        let url = format!("{HF_RESOLVE_BASE}/{DEFAULT_SORTFORMER_FILE_NAME}?download=1");
        download_file_if_missing(
            window,
            &client,
            &sortformer_file,
            &url,
            DEFAULT_SORTFORMER_FILE_NAME,
        )
        .await?;
    }

    Ok((parakeet_dir, sortformer_file))
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

        let sortformer_file = resolved_sortformer_file.as_path();
        if !sortformer_file.is_file() {
            return Err(anyhow!(
                "Sortformer model path must be a file: {}",
                sortformer_file.display()
            ));
        }

        emit_progress(&window, "Loading local audio...")?;
        let audio = load_audio_16k_mono(audio_file)
            .with_context(|| format!("Failed to load audio '{}'", audio_file.display()))?;

        emit_progress(&window, "Running Sortformer diarization...")?;
        let mut sortformer =
            Sortformer::with_config(sortformer_file, None, DiarizationConfig::callhome())
                .with_context(|| {
                    format!(
                        "Failed to load Sortformer model '{}'",
                        sortformer_file.display()
                    )
                })?;
        let diarization = sortformer
            .diarize(audio.clone(), SAMPLE_RATE as u32, 1)
            .context("Sortformer diarization failed")?;

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
                speaker: speaker_label_for_word(&word, &diarization),
                start: word.start,
                end: word.end,
                text: word.text,
            })
            .collect::<Vec<_>>();

        Ok(build_transcript_segments(&speaker_words))
    };

    run().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_token_text_handles_spacing() {
        let mut text = String::new();
        append_token_text(&mut text, "Hello");
        append_token_text(&mut text, ",");
        append_token_text(&mut text, "world");
        append_token_text(&mut text, "!");

        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn build_transcript_segments_splits_on_speaker_and_length() {
        let words = vec![
            WordWithSpeaker {
                start: 0.0,
                end: 0.3,
                text: "Hello".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.3,
                end: 0.5,
                text: "there.".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.6,
                end: 0.9,
                text: "General".into(),
                speaker: "Speaker 2".into(),
            },
            WordWithSpeaker {
                start: 0.9,
                end: 1.2,
                text: "Kenobi.".into(),
                speaker: "Speaker 2".into(),
            },
        ];

        let segments = build_transcript_segments(&words);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].speaker, "Speaker 1");
        assert_eq!(segments[0].text, "Hello there.");
        assert_eq!(segments[1].speaker, "Speaker 2");
        assert_eq!(segments[1].text, "General Kenobi.");
        assert_eq!(segments[0].words.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn build_transcript_segments_does_not_split_short_punctuation_runs_too_early() {
        let words = vec![
            WordWithSpeaker {
                start: 0.0,
                end: 0.2,
                text: "Kurz".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.2,
                end: 0.3,
                text: ".".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.3,
                end: 0.5,
                text: "aber".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.5,
                end: 0.7,
                text: "direkt".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.7,
                end: 0.9,
                text: "weiter".into(),
                speaker: "Speaker 1".into(),
            },
        ];

        let segments = build_transcript_segments(&words);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Kurz. aber direkt weiter");
    }

    #[test]
    fn standalone_punctuation_stays_with_previous_segment_across_speaker_change() {
        // Diarization misattributes the sentence-final period to the next
        // speaker; it must not open the following segment.
        let words = vec![
            WordWithSpeaker {
                start: 0.0,
                end: 0.3,
                text: "Hello".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.3,
                end: 0.5,
                text: "world".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.5,
                end: 0.55,
                text: ".".into(),
                speaker: "Speaker 2".into(),
            },
            WordWithSpeaker {
                start: 0.6,
                end: 0.9,
                text: "General".into(),
                speaker: "Speaker 2".into(),
            },
            WordWithSpeaker {
                start: 0.9,
                end: 1.2,
                text: "Kenobi.".into(),
                speaker: "Speaker 2".into(),
            },
        ];

        let segments = build_transcript_segments(&words);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "Hello world.");
        assert_eq!(segments[1].text, "General Kenobi.");
        assert!(!segments[1].text.starts_with('.'));
    }

    #[test]
    fn leading_punctuation_prefix_is_peeled_onto_previous_segment() {
        let words = vec![
            WordWithSpeaker {
                start: 0.0,
                end: 0.35,
                text: "unterwegs".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.35,
                end: 0.7,
                text: "für".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.7,
                end: 1.0,
                text: "IAV".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 1.0,
                end: 1.4,
                text: ". Kümmern".into(),
                speaker: "Speaker 2".into(),
            },
            WordWithSpeaker {
                start: 1.4,
                end: 1.7,
                text: "wir".into(),
                speaker: "Speaker 2".into(),
            },
        ];

        let segments = build_transcript_segments(&words);

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "unterwegs für IAV.");
        assert_eq!(segments[1].text, "Kümmern wir");
        assert_eq!(
            segments[1]
                .words
                .as_ref()
                .and_then(|words| words.first())
                .map(|word| word.text.as_str()),
            Some("Kümmern")
        );
        assert!(!segments[1].text.starts_with('.'));
    }

    #[test]
    fn leading_standalone_punctuation_with_no_predecessor_is_dropped() {
        let words = vec![
            WordWithSpeaker {
                start: 0.0,
                end: 0.05,
                text: ".".into(),
                speaker: "Speaker 1".into(),
            },
            WordWithSpeaker {
                start: 0.1,
                end: 0.4,
                text: "Hello".into(),
                speaker: "Speaker 1".into(),
            },
        ];

        let segments = build_transcript_segments(&words);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello");
    }

    #[test]
    fn format_timestamp_normalizes_hour_boundaries() {
        assert_eq!(format_timestamp(61.5), "01:01.500");
        assert_eq!(format_timestamp(3600.0), "01:00:00.000");
        assert_eq!(format_timestamp(3666.72), "01:01:06.720");
    }
}
