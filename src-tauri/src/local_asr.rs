//! Helpers shared by the local (on-device) ASR backends.
//!
//! Parakeet TDT and CrisperWhisper both produce a flat stream of timed words
//! and both need the same downstream treatment: 16 kHz mono audio, optional
//! Sortformer diarization, punctuation normalisation, and grouping into the
//! `TranscriptSegment`s the rest of the app edits. Everything model-agnostic
//! lives here so the two backends cannot drift apart.

use anyhow::{anyhow, Context, Result};
use ffmpeg_sidecar::paths::ffmpeg_path;
use hound::WavReader;
use parakeet_rs::sortformer::{DiarizationConfig, Sortformer, SpeakerSegment};
use std::path::{Path, PathBuf};
use tauri::{Emitter, Manager};
use tokio::io::AsyncWriteExt;

use crate::time_utils::format_time;
use crate::video::{TranscriptSegment, TranscriptWord};

pub(crate) const SAMPLE_RATE: usize = 16_000;

const MAX_SEGMENT_CHARS: usize = 120;
const MIN_SEGMENT_CHARS_FOR_PUNCT_BREAK: usize = 48;
const MIN_SEGMENT_WORDS_FOR_PUNCT_BREAK: usize = 6;
const MIN_SEGMENT_CHARS_FOR_PAUSE_BREAK: usize = 32;
const PAUSE_BREAK_SECONDS: f32 = 0.9;

/// Files hosted alongside the Parakeet ONNX exports. The Sortformer
/// diarization model lives here and is shared by every local backend.
pub(crate) const HF_RESOLVE_BASE: &str =
    "https://huggingface.co/altunenes/parakeet-rs/resolve/main";
pub(crate) const DEFAULT_SORTFORMER_FILE_NAME: &str =
    "diar_streaming_sortformer_4spk-v2.onnx";

/// A single recognised word with its speaker attribution.
#[derive(Clone, Debug)]
pub(crate) struct WordWithSpeaker {
    pub(crate) start: f32,
    pub(crate) end: f32,
    pub(crate) text: String,
    pub(crate) speaker: String,
}

pub(crate) fn emit_progress(window: &tauri::Window, message: &str) -> Result<()> {
    window
        .emit(
            "progress",
            serde_json::json!({
                "message": message,
            }),
        )
        .map_err(|e| anyhow!(e.to_string()))
}

pub(crate) fn format_timestamp(seconds: f32) -> String {
    format_time(seconds.max(0.0) as f64)
}

pub(crate) fn is_punctuation_only(text: &str) -> bool {
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

pub(crate) fn append_token_text(buffer: &mut String, token: &str) {
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
/// Local models emit punctuation either as a standalone token (e.g. `"."`) or
/// as a prefix on the following word (e.g. `". Kümmern"`). Because diarization
/// can misattribute that lone mark to the next speaker — and pause/length
/// breaks are evaluated per word — the mark would otherwise open a new segment
/// as `". ..."`. Normalising at the word level removes the problem at its
/// source instead of trying to repair already-built segments afterwards.
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

/// Resolve the speaker for a word from the diarization timeline by picking the
/// speaker segment it overlaps most, falling back to the nearest one.
pub(crate) fn speaker_label_for_word(
    start: f32,
    end: f32,
    diarization: &[SpeakerSegment],
) -> String {
    let mut best_overlap = 0.0f32;
    let mut best_speaker_id = None;

    for segment in diarization {
        let seg_start = segment.start as f32 / SAMPLE_RATE as f32;
        let seg_end = segment.end as f32 / SAMPLE_RATE as f32;
        let overlap_start = start.max(seg_start);
        let overlap_end = end.min(seg_end);
        let overlap = (overlap_end - overlap_start).max(0.0);

        if overlap > best_overlap {
            best_overlap = overlap;
            best_speaker_id = Some(segment.speaker_id);
        }
    }

    if let Some(speaker_id) = best_speaker_id {
        return format!("Speaker {}", speaker_id + 1);
    }

    let token_midpoint = (start + end) / 2.0;
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

/// Group a flat word stream into transcript segments, breaking on speaker
/// changes, long pauses, sentence-final punctuation and a hard length cap.
pub(crate) fn build_transcript_segments(words: &[WordWithSpeaker]) -> Vec<TranscriptSegment> {
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

/// Build segments from several independent word runs, keeping the runs apart.
///
/// A run boundary is a hard segment break. Callers use it to excise a time
/// span from the transcript: because the video export concatenates each
/// segment's `start`..`end` span, audio that falls *between* two segments is
/// physically dropped from the cut. Removing a word from a segment's word list
/// alone would not do that — the span would still cover it.
pub(crate) fn build_transcript_segments_from_runs(
    runs: &[Vec<WordWithSpeaker>],
) -> Vec<TranscriptSegment> {
    runs.iter()
        .flat_map(|run| build_transcript_segments(run))
        .collect()
}

/// Decode any media file to 16 kHz mono `f32` samples via FFmpeg.
pub(crate) fn load_audio_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let temp_wav_path = std::env::temp_dir()
        .join(format!("ai-media-cutter-asr-{}.wav", fastrand::u64(..)));

    let result = (|| -> Result<Vec<f32>> {
        write_wav_16k_mono(path, &temp_wav_path)?;
        read_wav_16k_mono(&temp_wav_path)
    })();

    let _ = std::fs::remove_file(&temp_wav_path);
    result
}

/// Normalise any media file into a 16 kHz mono PCM WAV at `destination`.
///
/// Backends that hand audio to an external process need a real file rather
/// than an in-memory sample buffer.
pub(crate) fn write_wav_16k_mono(source: &Path, destination: &Path) -> Result<()> {
    let output = std::process::Command::new(ffmpeg_path())
        .arg("-y")
        .arg("-i")
        .arg(source)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(destination)
        .output()
        .with_context(|| format!("Failed to spawn FFmpeg for '{}'", source.display()))?;

    if !output.status.success() || !destination.exists() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "Failed to normalize audio '{}' with FFmpeg: {}",
            source.display(),
            stderr.trim()
        ));
    }

    Ok(())
}

fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let mut reader = WavReader::open(path)
        .with_context(|| format!("Failed to open normalized WAV '{}'", path.display()))?;
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
}

pub(crate) async fn download_file_if_missing(
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

/// Root directory for on-demand model downloads, e.g.
/// `<app data>/models/<subdirectory>`.
pub(crate) fn model_root(window: &tauri::Window, subdirectory: &str) -> Result<PathBuf> {
    window
        .path()
        .app_data_dir()
        .map_err(|error| anyhow!(error.to_string()))
        .map(|path| path.join("models").join(subdirectory))
}

/// Resolve (downloading on first use) the Sortformer diarization model file.
/// An explicit user-provided path always wins.
pub(crate) async fn resolve_sortformer_file(
    window: &tauri::Window,
    sortformer_model_path: &str,
) -> Result<PathBuf> {
    let trimmed = sortformer_model_path.trim();
    if !trimmed.is_empty() {
        return Ok(PathBuf::from(trimmed));
    }

    let sortformer_file = model_root(window, "parakeet-rs")?.join(DEFAULT_SORTFORMER_FILE_NAME);
    if let Some(parent) = sortformer_file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

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

    Ok(sortformer_file)
}

/// Run Sortformer speaker diarization over 16 kHz mono samples.
pub(crate) fn diarize(sortformer_file: &Path, audio: Vec<f32>) -> Result<Vec<SpeakerSegment>> {
    if !sortformer_file.is_file() {
        return Err(anyhow!(
            "Sortformer model path must be a file: {}",
            sortformer_file.display()
        ));
    }

    let mut sortformer =
        Sortformer::with_config(sortformer_file, None, DiarizationConfig::callhome()).with_context(
            || {
                format!(
                    "Failed to load Sortformer model '{}'",
                    sortformer_file.display()
                )
            },
        )?;

    sortformer
        .diarize(audio, SAMPLE_RATE as u32, 1)
        .context("Sortformer diarization failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f32, end: f32, text: &str, speaker: &str) -> WordWithSpeaker {
        WordWithSpeaker {
            start,
            end,
            text: text.into(),
            speaker: speaker.into(),
        }
    }

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
            word(0.0, 0.3, "Hello", "Speaker 1"),
            word(0.3, 0.5, "there.", "Speaker 1"),
            word(0.6, 0.9, "General", "Speaker 2"),
            word(0.9, 1.2, "Kenobi.", "Speaker 2"),
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
            word(0.0, 0.2, "Kurz", "Speaker 1"),
            word(0.2, 0.3, ".", "Speaker 1"),
            word(0.3, 0.5, "aber", "Speaker 1"),
            word(0.5, 0.7, "direkt", "Speaker 1"),
            word(0.7, 0.9, "weiter", "Speaker 1"),
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
            word(0.0, 0.3, "Hello", "Speaker 1"),
            word(0.3, 0.5, "world", "Speaker 1"),
            word(0.5, 0.55, ".", "Speaker 2"),
            word(0.6, 0.9, "General", "Speaker 2"),
            word(0.9, 1.2, "Kenobi.", "Speaker 2"),
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
            word(0.0, 0.35, "unterwegs", "Speaker 1"),
            word(0.35, 0.7, "für", "Speaker 1"),
            word(0.7, 1.0, "IAV", "Speaker 1"),
            word(1.0, 1.4, ". Kümmern", "Speaker 2"),
            word(1.4, 1.7, "wir", "Speaker 2"),
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
            word(0.0, 0.05, ".", "Speaker 1"),
            word(0.1, 0.4, "Hello", "Speaker 1"),
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

    /// Sortformer reports speaker boundaries in samples at 16 kHz.
    fn speaker_segment(speaker_id: usize, start_seconds: f32, end_seconds: f32) -> SpeakerSegment {
        SpeakerSegment {
            speaker_id,
            start: (start_seconds * SAMPLE_RATE as f32) as u64,
            end: (end_seconds * SAMPLE_RATE as f32) as u64,
        }
    }

    #[test]
    fn speaker_label_prefers_the_most_overlapping_speaker() {
        let diarization = vec![
            speaker_segment(0, 0.0, 1.0),
            speaker_segment(1, 1.0, 3.0),
        ];

        assert_eq!(speaker_label_for_word(0.1, 0.4, &diarization), "Speaker 1");
        assert_eq!(speaker_label_for_word(1.5, 2.0, &diarization), "Speaker 2");
    }

    #[test]
    fn speaker_label_falls_back_to_unknown_when_far_from_any_speech() {
        let diarization = vec![speaker_segment(0, 0.0, 1.0)];

        assert_eq!(
            speaker_label_for_word(60.0, 60.5, &diarization),
            "Speaker Unknown"
        );
    }
}
