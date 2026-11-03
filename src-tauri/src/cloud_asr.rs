//! Cloud speech-to-text through OpenRouter's `/audio/transcriptions`.
//!
//! The counterpart of the local engines for machines that can't run
//! Nemotron: the same model (`nvidia/nemotron-3.5-transcribe`) and a dozen
//! others behind one key, returning timed words. With EU in-region routing the
//! base URL is `eu.openrouter.ai`, where requests are processed in Europe only
//! and fail rather than leave it; Nemotron is not served there, Mistral's
//! Voxtral models are.
//!
//! Audio goes up in ~10 minute MP3 chunks (a few MB each, base64 in the JSON
//! body), three at a time. Speakers come from local Sortformer over the whole
//! recording, so labels stay stable across chunks and don't depend on whether
//! the routed provider can diarize.

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use futures_util::{stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::State;

use crate::chunking::{split_audio, AudioChunk};
use crate::error::AppError;
use crate::local_asr::{
    build_transcript_segments, diarize, emit_progress, load_audio_16k_mono,
    resolve_sortformer_file, speaker_label_for_word, WordWithSpeaker,
};
use crate::run_control::{run_blocking, RunControl};
use crate::video::TranscriptSegment;

const CHUNK_SECONDS: f64 = 10.0 * 60.0;
const PARALLEL_REQUESTS: usize = 3;
const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Default, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudAsrOptions {
    /// `https://openrouter.ai/api` or `https://eu.openrouter.ai/api`.
    pub base_url: String,
    /// e.g. `nvidia/nemotron-3.5-transcribe`.
    pub model: String,
    /// ISO 639-1 code; empty lets the model detect it.
    pub language: String,
    /// Names and terms to spell right, sent as `keyterms`.
    pub glossary: String,
    /// Label speakers with Sortformer.
    pub diarize: bool,
    pub sortformer_model_path: String,
}

/// `{base}/v1/...` for a base URL with or without the `/v1` suffix.
fn endpoint(base_url: &str, path: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    format!("{base}/v1/{path}")
}

fn keyterms(glossary: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for raw in glossary.split(['\n', ',', ';']) {
        let term = raw
            .split([':', '='])
            .next()
            .unwrap_or_default()
            .trim()
            .trim_start_matches(['-', '*', '•'])
            .trim();
        if !term.is_empty() && term.len() <= 60 && !terms.iter().any(|known| known == term) {
            terms.push(term.to_string());
        }
        if terms.len() >= 100 {
            break;
        }
    }
    terms
}

fn request_body(
    model: &str,
    audio_base64: &str,
    language: &str,
    terms: &[String],
    optional_fields: bool,
) -> Value {
    let mut body = json!({
        "model": model,
        "input_audio": { "data": audio_base64, "format": "mp3" },
        "response_format": "verbose_json",
    });
    if optional_fields {
        body["timestamp_granularities"] = json!(["word", "segment"]);
        if !terms.is_empty() {
            body["keyterms"] = json!(terms);
        }
    }
    let language = language.trim();
    if !language.is_empty() {
        body["language"] = json!(language);
    }
    body
}

#[derive(Debug, Deserialize)]
struct VerboseTranscript {
    #[serde(default)]
    text: String,
    #[serde(default)]
    words: Option<Vec<ResponseWord>>,
    #[serde(default)]
    segments: Option<Vec<ResponseSegment>>,
}

#[derive(Debug, Deserialize)]
struct ResponseWord {
    word: String,
    start: f32,
    end: f32,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseSegment {
    start: f32,
    end: f32,
    text: String,
}

/// Timed words from a `verbose_json` response, shifted by `offset`. Providers
/// that time only segments get their words spread evenly over each segment,
/// which is approximate but keeps the transcript cuttable.
fn parse_transcript(body: &str, offset: f32) -> Result<Vec<WordWithSpeaker>> {
    let transcript: VerboseTranscript = serde_json::from_str(body)
        .with_context(|| format!("Unexpected transcription response: {}", preview(body)))?;
    let word = |text: &str, start: f32, end: f32| WordWithSpeaker {
        start: start + offset,
        end: end.max(start) + offset,
        text: text.trim().to_string(),
        speaker: "Speaker 1".to_string(),
    };

    if let Some(words) = transcript.words.filter(|words| !words.is_empty()) {
        return Ok(words
            .iter()
            .filter(|entry| entry.kind.as_deref() != Some("audio_event"))
            .filter(|entry| !entry.word.trim().is_empty())
            .map(|entry| word(&entry.word, entry.start, entry.end))
            .collect());
    }
    if let Some(segments) = transcript.segments.filter(|segments| !segments.is_empty()) {
        let mut words = Vec::new();
        for segment in segments {
            let tokens: Vec<&str> = segment.text.split_whitespace().collect();
            let step = (segment.end - segment.start).max(0.0) / tokens.len().max(1) as f32;
            for (index, token) in tokens.iter().enumerate() {
                let start = segment.start + step * index as f32;
                words.push(word(token, start, start + step));
            }
        }
        return Ok(words);
    }
    if transcript.text.trim().is_empty() {
        return Ok(Vec::new());
    }
    Err(anyhow!(
        "The model returned text without timings, so the transcript could not be cut. Choose a model with word timestamps (for example nvidia/nemotron-3.5-transcribe or a Voxtral model)."
    ))
}

fn preview(body: &str) -> String {
    let text: String = body.chars().take(300).collect();
    if body.len() > text.len() {
        format!("{text}...")
    } else {
        text
    }
}

/// A short, readable reason from an error body (`{"error": {"message": ...}}`).
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| preview(body))
}

async fn transcribe_chunk(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    options: &CloudAsrOptions,
    terms: &[String],
    audio: &Path,
    offset: f32,
) -> Result<Vec<WordWithSpeaker>> {
    let bytes = tokio::fs::read(audio)
        .await
        .with_context(|| format!("Failed to read '{}'", audio.display()))?;
    let encoded = general_purpose::STANDARD.encode(bytes);

    let mut optional_fields = true;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let body = request_body(
            &options.model,
            &encoded,
            &options.language,
            terms,
            optional_fields,
        );
        let response = client
            .post(url)
            .bearer_auth(api_key)
            .header("X-Title", "AI Media Cutter")
            .json(&body)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) if attempt < MAX_ATTEMPTS => {
                log::warn!("Transcription request failed ({error}); retrying.");
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
                continue;
            }
            Err(error) => {
                return Err(anyhow!(
                    "Failed to reach the transcription service: {error}"
                ))
            }
        };
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.is_success() {
            return parse_transcript(&text, offset);
        }
        // Word timings and key terms are optional request fields some
        // providers reject; ask once more without them.
        if status == reqwest::StatusCode::BAD_REQUEST && optional_fields {
            log::warn!(
                "Transcription request rejected ({}); retrying without optional fields.",
                error_message(&text)
            );
            optional_fields = false;
            continue;
        }
        let retryable =
            status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
        if retryable && attempt < MAX_ATTEMPTS {
            tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
            continue;
        }
        let hint = match status.as_u16() {
            401 => " Check the OpenRouter API key in Settings.",
            402 => " Your OpenRouter account is out of credits.",
            404 => " The model is unknown, or no provider can serve it in the selected region.",
            _ => "",
        };
        return Err(anyhow!(
            "Transcription failed ({status}): {}.{hint}",
            error_message(&text)
        ));
    }
}

/// Re-encode a chunk as 16 kHz mono MP3 for upload: the format every
/// transcription provider accepts, at a few MB per 10 minutes.
fn encode_mp3(source: &Path, destination: &Path) -> Result<()> {
    let output = std::process::Command::new(ffmpeg_sidecar::paths::ffmpeg_path())
        .arg("-y")
        .arg("-i")
        .arg(source)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "64k",
        ])
        .arg(destination)
        .output()
        .with_context(|| format!("Failed to spawn FFmpeg for '{}'", source.display()))?;
    if !output.status.success() || !destination.exists() {
        return Err(anyhow!(
            "Failed to encode '{}' for upload: {}",
            source.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// What every chunk's request shares.
struct PartContext<'a> {
    run_id: u64,
    run_control: &'a RunControl,
    window: &'a tauri::Window,
    client: &'a reqwest::Client,
    url: &'a str,
    api_key: &'a str,
    options: &'a CloudAsrOptions,
    terms: &'a [String],
    scratch: &'a Path,
    total: usize,
    done: std::sync::atomic::AtomicUsize,
}

async fn transcribe_part(
    context: &PartContext<'_>,
    index: usize,
    chunk: &AudioChunk,
) -> Result<Vec<WordWithSpeaker>> {
    context
        .run_control
        .ensure_active(context.run_id)
        .map_err(|error| anyhow!(error))?;
    let mp3 = context.scratch.join(format!("chunk{index:03}.mp3"));
    let (source, destination) = (PathBuf::from(&chunk.path), mp3.clone());
    run_blocking(move || encode_mp3(&source, &destination).map_err(|error| format!("{error:#}")))
        .await
        .map_err(|error| anyhow!(error))?;
    let words = transcribe_chunk(
        context.client,
        context.url,
        context.api_key,
        context.options,
        context.terms,
        &mp3,
        chunk.start_offset as f32,
    )
    .await?;
    let finished = context
        .done
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1;
    let _ = emit_progress(
        context.window,
        &format!(
            "Transcribed part {finished}/{} with {}...",
            context.total, context.options.model
        ),
    );
    Ok(words)
}

/// Transcribe through OpenRouter and return editor-ready segments.
#[tauri::command]
#[specta::specta]
pub async fn transcribe_with_cloud_asr(
    run_id: u64,
    window: tauri::Window,
    audio_path: String,
    options: CloudAsrOptions,
    run_control: State<'_, RunControl>,
) -> Result<Vec<TranscriptSegment>, AppError> {
    let run_control = run_control.inner();
    run_control.ensure_active(run_id)?;
    if options.model.trim().is_empty() {
        return Err(AppError::failed(
            "Choose a cloud transcription model in Settings.",
        ));
    }
    let api_key = crate::secrets::key_for_base_url(&options.base_url)?;
    let audio_file = PathBuf::from(&audio_path);
    if !audio_file.exists() {
        return Err(AppError::failed(format!(
            "Audio file not found: {}",
            audio_file.display()
        )));
    }

    let sortformer_file = if options.diarize {
        Some(resolve_sortformer_file(&window, &options.sortformer_model_path).await?)
    } else {
        None
    };

    let chunks: Vec<AudioChunk> = split_audio(
        run_id,
        &window,
        audio_path.clone(),
        CHUNK_SECONDS,
        "",
        run_control,
    )
    .await?;
    let scratch =
        std::env::temp_dir().join(format!("ai-media-cutter-cloud-asr-{}", fastrand::u64(..)));
    tokio::fs::create_dir_all(&scratch)
        .await
        .map_err(|error| AppError::failed(error.to_string()))?;

    let url = endpoint(&options.base_url, "audio/transcriptions");
    let terms = keyterms(&options.glossary);
    let client = crate::http::http_client();
    let total = chunks.len();
    let model_name = options.model.clone();
    let _ = emit_progress(
        &window,
        &format!("Transcribing with {model_name} ({total} part(s))..."),
    );

    let context = PartContext {
        run_id,
        run_control,
        window: &window,
        client: &client,
        url: &url,
        api_key: &api_key,
        options: &options,
        terms: &terms,
        scratch: &scratch,
        total,
        done: std::sync::atomic::AtomicUsize::new(0),
    };
    let parts: Vec<_> = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| transcribe_part(&context, index, chunk))
        .collect();
    let transcribe = async {
        let parts: Vec<Vec<WordWithSpeaker>> = stream::iter(parts)
            .buffered(PARALLEL_REQUESTS)
            .try_collect()
            .await?;
        Ok::<_, anyhow::Error>(parts.into_iter().flatten().collect::<Vec<_>>())
    };
    let outcome = run_control.run_cancellable(run_id, transcribe).await;
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    let mut words = outcome?;

    if let (Some(file), false) = (sortformer_file, words.is_empty()) {
        run_control.ensure_active(run_id)?;
        let _ = emit_progress(&window, "Running Sortformer diarization...");
        let source = audio_file.clone();
        let diarization = run_blocking(move || {
            load_audio_16k_mono(&source)
                .and_then(|audio| diarize(&file, audio))
                .map_err(|error| format!("{error:#}"))
        })
        .await?;
        for word in &mut words {
            word.speaker = speaker_label_for_word(word.start, word.end, &diarization);
        }
    }
    Ok(build_transcript_segments(&words))
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionModel {
    pub id: String,
    pub name: String,
    /// US dollars per minute of audio, when the endpoint reports it.
    #[specta(type = Option<specta_typescript::Number>)]
    pub usd_per_minute: Option<f64>,
}

fn parse_transcription_models(body: &Value) -> Vec<TranscriptionModel> {
    body.get("data")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter(|model| {
                    model
                        .pointer("/architecture/output_modalities")
                        .and_then(Value::as_array)
                        .is_none_or(|outputs| {
                            outputs.iter().any(|output| output == "transcription")
                        })
                })
                .filter_map(|model| {
                    let id = model.get("id")?.as_str()?.to_string();
                    let name = model
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    // Audio models are priced per second of input.
                    let usd_per_minute = model
                        .pointer("/pricing/prompt")
                        .and_then(Value::as_str)
                        .and_then(|price| price.parse::<f64>().ok())
                        .filter(|price| *price > 0.0)
                        .map(|price| price * 60.0);
                    Some(TranscriptionModel {
                        id,
                        name,
                        usd_per_minute,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Speech-to-text models served at `base_url` (OpenRouter; no key needed). On
/// `eu.openrouter.ai` this is the EU-only subset.
#[tauri::command]
#[specta::specta]
pub async fn list_transcription_models(
    base_url: String,
) -> Result<Vec<TranscriptionModel>, AppError> {
    let url = format!(
        "{}?output_modalities=transcription",
        endpoint(&base_url, "models")
    );
    let response = crate::http::http_client()
        .get(&url)
        .send()
        .await
        .map_err(|error| AppError::failed(format!("Failed to reach '{base_url}': {error}")))?;
    if !response.status().is_success() {
        return Err(AppError::failed(format!(
            "Failed to fetch transcription models: {}",
            response.status()
        )));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| AppError::failed(format!("Invalid model list: {error}")))?;
    Ok(parse_transcription_models(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_accept_bases_with_or_without_v1() {
        assert_eq!(
            endpoint("https://openrouter.ai/api", "audio/transcriptions"),
            "https://openrouter.ai/api/v1/audio/transcriptions"
        );
        assert_eq!(
            endpoint("https://eu.openrouter.ai/api/v1/", "models"),
            "https://eu.openrouter.ai/api/v1/models"
        );
    }

    #[test]
    fn the_request_asks_for_word_timings_and_key_terms() {
        let body = request_body(
            "nvidia/nemotron-3.5-transcribe",
            "AAAA",
            "de",
            &["IAV".to_string()],
            true,
        );
        assert_eq!(body["input_audio"]["format"], "mp3");
        assert_eq!(body["response_format"], "verbose_json");
        assert_eq!(body["timestamp_granularities"][0], "word");
        assert_eq!(body["keyterms"][0], "IAV");
        assert_eq!(body["language"], "de");

        let plain = request_body("m", "AAAA", "", &["IAV".to_string()], false);
        assert!(plain.get("keyterms").is_none());
        assert!(plain.get("timestamp_granularities").is_none());
        assert!(plain.get("language").is_none());
    }

    #[test]
    fn word_timings_are_offset_and_audio_events_dropped() {
        let body = r#"{"text": "Hello there", "words": [
            {"word": "Hello", "start": 0.1, "end": 0.4},
            {"word": "(laughs)", "start": 0.4, "end": 0.8, "type": "audio_event"},
            {"word": "there", "start": 0.8, "end": 1.0}
        ]}"#;
        let words = parse_transcript(body, 600.0).unwrap();
        assert_eq!(words.len(), 2);
        assert!((words[0].start - 600.1).abs() < 1e-3);
        assert_eq!(words[1].text, "there");
    }

    #[test]
    fn segment_timings_are_spread_over_their_words() {
        let body = r#"{"text": "a b", "segments": [{"id": 0, "start": 2.0, "end": 4.0, "text": " one two"}]}"#;
        let words = parse_transcript(body, 0.0).unwrap();
        assert_eq!(words.len(), 2);
        assert!((words[1].start - 3.0).abs() < 1e-4);
        assert!((words[1].end - 4.0).abs() < 1e-4);
    }

    #[test]
    fn text_without_timings_is_an_error_but_silence_is_not() {
        assert!(parse_transcript(r#"{"text": "hello"}"#, 0.0).is_err());
        assert!(parse_transcript(r#"{"text": ""}"#, 0.0).unwrap().is_empty());
    }

    #[test]
    fn error_bodies_are_summarised() {
        assert_eq!(
            error_message(r#"{"error": {"message": "No endpoints found", "code": 404}}"#),
            "No endpoints found"
        );
    }

    #[test]
    fn transcription_models_are_listed_with_per_minute_prices() {
        let body = json!({"data": [
            {"id": "nvidia/nemotron-3.5-transcribe", "name": "NVIDIA: Nemotron 3.5 Transcribe",
             "architecture": {"output_modalities": ["transcription"]},
             "pricing": {"prompt": "0.0000333"}},
            {"id": "openai/gpt-x", "architecture": {"output_modalities": ["text"]}}
        ]});
        let models = parse_transcription_models(&body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "nvidia/nemotron-3.5-transcribe");
        assert!((models[0].usd_per_minute.unwrap() - 0.001998).abs() < 1e-6);
    }

    #[test]
    fn keyterms_come_from_the_glossary() {
        assert_eq!(
            keyterms("IAV: company\nNVLink, NVLink; - TensorRT"),
            vec!["IAV", "NVLink", "TensorRT"]
        );
    }
}
