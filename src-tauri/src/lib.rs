use crate::error::AppError;
use ffmpeg_sidecar::command::ffmpeg_is_installed;
use ffmpeg_sidecar::download::auto_download;
use ffmpeg_sidecar::event::FfmpegEvent;
use ffmpeg_sidecar::paths::{ffmpeg_path, sidecar_path};
#[allow(unused_imports)]
use log::{error, info, warn};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tauri::State;
use tauri::{Emitter, Manager};

fn describe_ffmpeg_lookup() -> String {
    let resolved_path = ffmpeg_path();
    let sidecar = sidecar_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "<unavailable>".to_string());

    format!(
        "FFmpeg executable was not found. Expected either a sidecar binary at '{}' or an 'ffmpeg' executable on PATH (resolved command path: '{}').",
        sidecar,
        resolved_path.display()
    )
}

pub(crate) fn format_ffmpeg_spawn_error(
    operation: &str,
    input_path: &Path,
    output_path: Option<&Path>,
    err: &std::io::Error,
) -> String {
    let mut message = if err.kind() == ErrorKind::NotFound {
        format!(
            "Failed to {}. {} Input media exists at '{}'.",
            operation,
            describe_ffmpeg_lookup(),
            input_path.display()
        )
    } else {
        format!(
            "Failed to {} for input '{}': {}",
            operation,
            input_path.display(),
            err
        )
    };

    if let Some(output_path) = output_path {
        message.push_str(&format!(" Output path was '{}'.", output_path.display()));
    }

    message
}

pub(crate) fn format_path_io_error(operation: &str, path: &Path, err: &std::io::Error) -> String {
    format!("Failed to {} at '{}': {}", operation, path.display(), err)
}

#[tauri::command]
#[specta::specta]
async fn init_ffmpeg() -> Result<String, AppError> {
    // Probing spawns ffmpeg and the fallback downloads a build; both block.
    run_blocking(init_ffmpeg_blocking)
        .await
        .map_err(AppError::from)
}

fn init_ffmpeg_blocking() -> Result<String, String> {
    if ffmpeg_is_installed() {
        info!("FFmpeg is already installed.");
        return Ok("FFmpeg is already installed.".to_string());
    }

    // Try to download
    if let Err(e) = auto_download() {
        warn!("FFmpeg auto_download failed: {}", e);
        // We continue, maybe it's already there but not in PATH
    }

    if ffmpeg_is_installed() {
        info!("FFmpeg downloaded successfully.");
        return Ok("FFmpeg downloaded successfully.".to_string());
    }

    // Fallback: Add current dir to PATH if ffmpeg is there
    let current_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let filename = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };

    let mut found_path = None;

    // 1. Check direct paths
    let candidates = vec![
        current_dir.join(filename),
        current_dir.join("ffmpeg").join(filename),
        current_dir.join("bin").join(filename),
        current_dir.join("ffmpeg").join("bin").join(filename),
    ];

    for p in candidates {
        if p.exists() {
            found_path = Some(p);
            break;
        }
    }

    // 2. Search in subdirectories (e.g. ffmpeg-6.0-windows-desktop/bin/ffmpeg.exe)
    if found_path.is_none() {
        if let Ok(entries) = std::fs::read_dir(&current_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Check inside this dir
                    let p1 = path.join(filename);
                    if p1.exists() {
                        found_path = Some(p1);
                        break;
                    }

                    // Check inside bin
                    let p2 = path.join("bin").join(filename);
                    if p2.exists() {
                        found_path = Some(p2);
                        break;
                    }
                }
            }
        }
    }

    if let Some(ffmpeg_path) = found_path {
        let key = "PATH";
        unsafe {
            if let Ok(path) = std::env::var(key) {
                let separator = if cfg!(windows) { ";" } else { ":" };
                let parent = ffmpeg_path.parent().unwrap().to_string_lossy();
                let new_path = format!("{}{}{}", path, separator, parent);
                std::env::set_var(key, new_path);
            } else {
                std::env::set_var(
                    key,
                    ffmpeg_path.parent().unwrap().to_string_lossy().to_string(),
                );
            }
        }

        if ffmpeg_is_installed() {
            return Ok(format!(
                "FFmpeg found at {} and added to PATH.",
                ffmpeg_path.display()
            ));
        }
    }

    Err(format!(
        "FFmpeg initialization failed. {} You can restart the app, or install FFmpeg manually and ensure it is available on PATH.",
        describe_ffmpeg_lookup()
    ))
}

use ffmpeg_sidecar::command::FfmpegCommand;
use serde::Serialize;

#[derive(Serialize, specta::Type)]
struct AudioInfo {
    path: String,
    size: u64,
    #[specta(type = specta_typescript::Number)] // never NaN; plain `number` in TS
    duration: f64,
}

#[tauri::command]
#[specta::specta]
async fn prepare_audio_for_ai(
    run_id: u64,
    window: tauri::Window,
    input_path: String,
    run_control: State<'_, RunControl>,
) -> Result<AudioInfo, AppError> {
    run_control.ensure_active(run_id)?;
    let input = PathBuf::from(&input_path);
    if !input.exists() {
        return Err(AppError::failed("Input file does not exist"));
    }

    let cache_root = media_cache::cache_root(window.app_handle())?;
    let run_control = run_control.inner().clone();
    run_blocking(move || {
        // Derived audio lives in the app cache, keyed by the source's path,
        // size and mtime -- never next to the user's media (where an .ogg
        // source would even have been its own output).
        let output_path = media_cache::source_dir(&cache_root, &input)?.join("analysis.ogg");
        let duration = probe_media(&input)
            .ok()
            .and_then(|info| info.duration_seconds);

        // The cache key covers the source's content, so a finished extraction
        // can be reused by every re-analysis.
        if !output_path.exists() {
            // Encode under a temporary name and rename on success, so a
            // cancelled run never leaves a truncated file to be reused.
            let partial = output_path.with_file_name("analysis.partial.ogg");
            // Normalize input to OGG/Opus for downstream silence removal and AI upload.
            let mut command = FfmpegCommand::new();
            command
                .input(input.to_str().unwrap())
                .args(["-y", "-vn", "-c:a", "libopus", "-b:a", "96k"])
                .output(partial.to_str().unwrap());
            run_ffmpeg(
                command,
                FfmpegTask {
                    operation: "prepare audio for AI analysis",
                    input: &input,
                    output: Some(&partial),
                    run: Some((run_id, &run_control)),
                },
                |event| {
                    if let FfmpegEvent::Progress(progress) = event {
                        let payload = serde_json::json!({
                            "time": progress.time,
                            "percentage": progress_percentage(&progress.time, duration),
                        });
                        let _ = window.emit("progress", payload);
                    }
                },
            )?;
            std::fs::rename(&partial, &output_path).map_err(|e| {
                format_path_io_error("finalize the extracted audio", &output_path, &e)
            })?;
        }

        // Check size
        let metadata = std::fs::metadata(&output_path).map_err(|e| {
            format_path_io_error("read generated audio file metadata", &output_path, &e)
        })?;
        let size = metadata.len();

        Ok(AudioInfo {
            path: output_path.to_string_lossy().to_string(),
            size,
            duration: duration.unwrap_or(0.0),
        })
    })
    .await
    .map_err(AppError::from)
}

/// Transcode `source_path` into a seekable `<stem>_preview.m4a` sitting next to
/// it, purely for in-app playback preview.
///
/// The analysis audio is Opus in an Ogg container, and the source media's own
/// audio track is often a codec the webview can't decode. WKWebView (macOS) can
/// neither seek Ogg/Opus (it reports a bogus duration and mis-seeks) nor decode
/// many source codecs, so segment previews need a transcoded, seekable file.
/// AAC in an MP4/M4A container with `+faststart` (moov atom at the front) plays
/// and seeks reliably. This does not touch the Ogg/Opus analysis+upload pipeline.
///
/// `source_path` is expected to be the already-extracted analysis `.ogg` (fast,
/// and on the same original timeline), but any decodable audio/video file works.
/// A cached preview is reused when it is at least as new as the source.
#[tauri::command]
#[specta::specta]
async fn prepare_preview_audio(
    run_id: u64,
    window: tauri::Window,
    source_path: String,
    run_control: State<'_, RunControl>,
) -> Result<String, AppError> {
    run_control.ensure_active(run_id)?;

    let source = PathBuf::from(&source_path);
    if !source.exists() {
        return Err(AppError::failed("Source audio file does not exist"));
    }

    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let output_path = source.with_file_name(format!("{}_preview.m4a", stem));

    // Reuse an existing preview when it is not older than its source.
    if let (Ok(out_meta), Ok(src_meta)) =
        (std::fs::metadata(&output_path), std::fs::metadata(&source))
    {
        if let (Ok(out_modified), Ok(src_modified)) = (out_meta.modified(), src_meta.modified()) {
            if out_modified >= src_modified {
                return Ok(output_path.to_string_lossy().to_string());
            }
        }
    }

    let run_control = run_control.inner().clone();
    run_blocking(move || {
        let duration = probe_media(&source)
            .ok()
            .and_then(|info| info.duration_seconds);
        let mut command = FfmpegCommand::new();
        command
            .input(source.to_str().unwrap())
            .args([
                "-y",
                "-vn",
                "-c:a",
                "aac",
                "-b:a",
                "160k",
                "-movflags",
                "+faststart",
            ])
            .output(output_path.to_str().unwrap());
        run_ffmpeg(
            command,
            FfmpegTask {
                operation: "prepare preview audio",
                input: &source,
                output: Some(&output_path),
                run: Some((run_id, &run_control)),
            },
            |event| {
                if let FfmpegEvent::Progress(progress) = event {
                    let _ = window.emit(
                        "progress",
                        serde_json::json!({
                            "time": progress.time,
                            "percentage": progress_percentage(&progress.time, duration),
                        }),
                    );
                }
            },
        )?;

        Ok(output_path.to_string_lossy().to_string())
    })
    .await
    .map_err(AppError::from)
}

mod camera;
mod captions;
pub mod chunking;
pub mod clip_selection;
pub mod crisper;
pub(crate) mod encoders;
pub mod error;
mod face_tracks;
mod faces;
pub(crate) mod ffmpeg;
mod frames;
pub mod gemini;
mod http;
mod local_asr;
mod media_cache;
pub mod media_probe;
mod media_protocol;
mod model_download;
mod morph;
mod parakeet;
mod path_guard;
mod pauses;
pub mod podcast;
mod reframe;
pub mod retry;
mod run_control;
mod secrets;
mod shots;
pub mod silence;
mod speaker_faces;
mod tighten;
pub mod time_utils;
pub mod transcript_merge;
mod upload;
mod vertical;
pub mod video;

use crate::chunking::split_audio_for_analysis;
use crate::crisper::{
    crisper_environment_status, install_crisper_environment, transcribe_with_crisper,
};
use crate::encoders::ExportQuality;
use crate::ffmpeg::{progress_percentage, run_ffmpeg, FfmpegTask};
use crate::gemini::GeminiClient;
use crate::media_probe::probe_media;
use crate::parakeet::transcribe_with_parakeet;
use crate::podcast::{
    calculate_segments_duration as calc_duration, export_podcast as export_podcast_fn,
    export_podcast_clips as export_podcast_clips_fn, PodcastSegment,
};
use crate::run_control::{run_blocking, RunControl};
use crate::silence::{detect_silence, remove_silence};
use crate::transcript_merge::merge_transcript_hypotheses_with_progress as merge_transcript_hypotheses_fn;
use crate::upload::upload_file_and_wait;
use crate::video::{
    cut_video as cut_video_fn, export_clips as export_clips_fn, ClipSegment, Segment,
    TranscriptSegment,
};

/// Pick short-form clip candidates from the transcript (shorts phase S1).
#[tauri::command]
#[specta::specta]
async fn select_clips(
    run_id: u64,
    window: tauri::Window,
    llm: LlmConfig,
    transcript: Vec<TranscriptSegment>,
    request: clip_selection::ClipRequest,
    run_control: State<'_, RunControl>,
) -> Result<Vec<clip_selection::ClipCandidate>, AppError> {
    run_control.ensure_active(run_id)?;
    let client = GeminiClient::new(secrets::api_key().require()?, llm.base_url, llm.model);
    let on_progress = |done: usize, total: usize| {
        let _ = window.emit(
            "progress",
            serde_json::json!({
                "percentage": done as f64 * 100.0 / total.max(1) as f64,
                "message": format!("Finding clips ({done}/{total} parts analysed)..."),
            }),
        );
    };
    Ok(run_control
        .run_cancellable(
            run_id,
            clip_selection::select_clips(&client, &transcript, &request, &on_progress),
        )
        .await?)
}

/// Models available at `base_url`. Uses `api_key` when given (a key typed in
/// Settings but not saved yet), otherwise the stored key.
#[tauri::command]
#[specta::specta]
async fn list_models(
    base_url: String,
    api_key: Option<String>,
) -> Result<gemini::ModelList, AppError> {
    let api_key = match api_key.filter(|key| !key.trim().is_empty()) {
        Some(key) => key.trim().to_string(),
        None => secrets::api_key().require()?,
    };
    Ok(gemini::list_models(&http::http_client(), &base_url, &api_key).await?)
}

/// The seekable preview audio a previous analysis of `input_path` produced,
/// if it is still cached.
#[tauri::command]
#[specta::specta]
fn cached_preview_audio(app: tauri::AppHandle, input_path: String) -> Option<String> {
    let root = media_cache::cache_root(&app).ok()?;
    let dir = media_cache::existing_source_dir(&root, Path::new(&input_path))?;
    let preview = dir.join("analysis_preview.m4a");
    preview
        .exists()
        .then(|| preview.to_string_lossy().to_string())
}

#[tauri::command]
#[specta::specta]
// Parameters mirror the IPC payload the frontend sends.
#[allow(clippy::too_many_arguments)]
async fn translate_transcript(
    run_id: u64,
    base_url: String,
    model: String,
    transcript: Vec<TranscriptSegment>,
    target_language: String,
    context: String,
    run_control: State<'_, RunControl>,
) -> Result<String, AppError> {
    run_control.ensure_active(run_id)?;
    let api_key = secrets::api_key().require()?;
    let client = GeminiClient::new(api_key, base_url, model);
    run_control
        .run_cancellable(
            run_id,
            client.translate_transcript(transcript, target_language, context),
        )
        .await
        .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
async fn upload_file(
    run_id: u64,
    base_url: String,
    path: String,
    run_control: State<'_, RunControl>,
) -> Result<Option<String>, AppError> {
    run_control.ensure_active(run_id)?;
    let path_buf = PathBuf::from(path);
    run_control
        .run_cancellable(
            run_id,
            upload_file_and_wait(&secrets::api_key().require()?, &base_url, &path_buf),
        )
        .await
        .map_err(|e| {
            format!(
                "Failed to upload audio file '{}' to '{}': {}",
                path_buf.display(),
                base_url,
                e
            )
        })
        .map_err(AppError::from)
}

/// The remote LLM a request goes to, as configured in Settings. The API key
/// is not part of it: the backend reads it from the credential store.
#[derive(serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
}

#[tauri::command]
#[specta::specta]
// Parameters mirror the IPC payload the frontend sends.
#[allow(clippy::too_many_arguments)]
async fn analyze_audio(
    run_id: u64,
    llm: LlmConfig,
    enforce_json_schema: bool,
    context: String,
    glossary: String,
    speaker_count: Option<u32>,
    remove_filler_words: bool,
    audio_uri: Option<String>,
    audio_base64: Option<String>,
    run_control: State<'_, RunControl>,
) -> Result<String, AppError> {
    run_control.ensure_active(run_id)?;
    let LlmConfig { base_url, model } = llm;
    let api_key = secrets::api_key().require()?;
    let client = GeminiClient::new(api_key, base_url.clone(), model.clone());
    run_control
        .run_cancellable(
            run_id,
            client.analyze_audio(
                &context,
                &glossary,
                speaker_count,
                remove_filler_words,
                enforce_json_schema,
                audio_uri.as_deref(),
                audio_base64.as_deref(),
            ),
        )
        .await
        .map_err(|e| {
            format!(
                "AI analysis request to '{}' with model '{}' failed: {}",
                base_url, model, e
            )
        })
        .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
// Parameters mirror the IPC payload the frontend sends.
#[allow(clippy::too_many_arguments)]
async fn cleanup_local_transcript(
    run_id: u64,
    base_url: String,
    model: String,
    transcript: Vec<TranscriptSegment>,
    context: String,
    glossary: String,
    remove_filler_words: bool,
    run_control: State<'_, RunControl>,
) -> Result<Vec<TranscriptSegment>, AppError> {
    run_control.ensure_active(run_id)?;
    let api_key = secrets::api_key().require()?;
    let client = GeminiClient::new(api_key, base_url.clone(), model.clone());
    run_control
        .run_cancellable(
            run_id,
            client.cleanup_local_transcript(transcript, &context, &glossary, remove_filler_words),
        )
        .await
        .map_err(|e| {
            format!(
                "Transcript cleanup request to '{}' with model '{}' failed: {}",
                base_url, model, e
            )
        })
        .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
async fn merge_transcript_hypotheses(
    run_id: u64,
    window: tauri::Window,
    primary_transcript: Vec<TranscriptSegment>,
    reference_transcript: Vec<TranscriptSegment>,
    run_control: State<'_, RunControl>,
) -> Result<Vec<TranscriptSegment>, AppError> {
    run_control.ensure_active(run_id)?;
    let run_control = run_control.inner().clone();
    let started_at = std::time::Instant::now();
    run_blocking(move || {
        Ok(merge_transcript_hypotheses_fn(
            primary_transcript,
            reference_transcript,
            |percentage, message| {
                if run_control.is_cancelled(run_id) {
                    return;
                }
                let elapsed_seconds = started_at.elapsed().as_secs_f32();
                let eta_seconds = if percentage > 0.0 && percentage < 100.0 {
                    Some((elapsed_seconds * ((100.0 - percentage) / percentage)).max(0.0))
                } else {
                    None
                };
                let _ = window.emit(
                    "progress",
                    serde_json::json!({
                        "percentage": percentage,
                        "message": message,
                        "elapsedSeconds": elapsed_seconds,
                        "etaSeconds": eta_seconds,
                    }),
                );
            },
        ))
    })
    .await
    .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
async fn cut_video(
    run_id: u64,
    window: tauri::Window,
    input_path: String,
    segments: Vec<Segment>,
    output_path: String,
    quality: ExportQuality,
    run_control: State<'_, RunControl>,
) -> Result<(), AppError> {
    run_control.ensure_active(run_id)?;
    use crate::time_utils::parse_timestamp_to_seconds_raw;

    let input = PathBuf::from(input_path);
    let output = PathBuf::from(output_path);

    let total_duration: f64 = segments
        .iter()
        .map(|s| {
            let start = parse_timestamp_to_seconds_raw(&s.start).unwrap_or(0.0);
            let end = parse_timestamp_to_seconds_raw(&s.end).unwrap_or(0.0);
            end - start
        })
        .sum();

    let run_control = run_control.inner().clone();
    run_blocking(move || {
        cut_video_fn(
            &input,
            &segments,
            &output,
            quality,
            run_id,
            &run_control,
            move |time| {
                let current = parse_timestamp_to_seconds_raw(&time).unwrap_or(0.0);
                let percentage = if total_duration > 0.0 {
                    (current / total_duration) * 100.0
                } else {
                    0.0
                };
                let payload = serde_json::json!({
                    "time": time,
                    "percentage": percentage
                });
                let _ = window.emit("progress", payload);
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
// Parameters mirror the IPC payload the frontend sends.
#[allow(clippy::too_many_arguments)]
async fn export_clips(
    run_id: u64,
    window: tauri::Window,
    input_path: String,
    segments: Vec<ClipSegment>,
    output_dir: String,
    fast_mode: bool,
    quality: ExportQuality,
    run_control: State<'_, RunControl>,
) -> Result<(), AppError> {
    run_control.ensure_active(run_id)?;
    use crate::time_utils::parse_timestamp_to_seconds_raw;

    let input = PathBuf::from(input_path);
    let output = PathBuf::from(output_dir);

    // Calculate duration for each clip
    let clip_durations: Vec<f64> = segments
        .iter()
        .map(|c| {
            c.segments
                .iter()
                .map(|s| {
                    let start = parse_timestamp_to_seconds_raw(&s.start).unwrap_or(0.0);
                    let end = parse_timestamp_to_seconds_raw(&s.end).unwrap_or(0.0);
                    end - start
                })
                .sum()
        })
        .collect();

    let total_duration: f64 = clip_durations.iter().sum();

    let run_control = run_control.inner().clone();
    run_blocking(move || {
        export_clips_fn(
            &input,
            &segments,
            &output,
            fast_mode,
            quality,
            run_id,
            &run_control,
            move |clip_idx, total_clips, time| {
                let current_clip_time = parse_timestamp_to_seconds_raw(&time).unwrap_or(0.0);

                // Sum duration of previous clips
                let previous_duration: f64 = clip_durations.iter().take(clip_idx).sum();

                let total_current = previous_duration + current_clip_time;

                let percentage = if total_duration > 0.0 {
                    ((total_current / total_duration) * 100.0).min(100.0)
                } else {
                    0.0
                };

                let payload = serde_json::json!({
                    "time": time,
                    "percentage": percentage,
                    "current_clip": clip_idx + 1,
                    "total_clips": total_clips
                });
                let _ = window.emit("progress", payload);
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(AppError::from)
}

/// A stretch of speech by one named speaker, for vertical framing.
#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
pub struct SpeakerTurn {
    #[specta(type = specta_typescript::Number)]
    pub start: f64,
    #[specta(type = specta_typescript::Number)]
    pub end: f64,
    pub speaker: String,
}

/// A transcript word on the source timeline, for burned-in captions.
#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
pub struct CaptionWord {
    #[specta(type = specta_typescript::Number)]
    pub start: f64,
    #[specta(type = specta_typescript::Number)]
    pub end: f64,
    pub text: String,
}

/// The source ranges of each clip, in seconds.
fn clip_ranges(segments: &[ClipSegment]) -> Result<Vec<Vec<(f64, f64)>>, String> {
    use crate::time_utils::parse_timestamp_to_seconds_raw;
    segments
        .iter()
        .map(|clip| {
            clip.segments
                .iter()
                .map(|segment| {
                    let start = parse_timestamp_to_seconds_raw(&segment.start)
                        .map_err(|e| format!("Invalid clip start '{}': {e}", segment.start))?;
                    let end = parse_timestamp_to_seconds_raw(&segment.end)
                        .map_err(|e| format!("Invalid clip end '{}': {e}", segment.end))?;
                    Ok((start.max(0.0), end))
                })
                .collect()
        })
        .collect()
}

fn speech_turns(turns: Vec<SpeakerTurn>) -> Vec<speaker_faces::SpeechTurn> {
    turns
        .into_iter()
        .map(|turn| speaker_faces::SpeechTurn {
            start: turn.start,
            end: turn.end,
            speaker: turn.speaker,
        })
        .collect()
}

fn timed_words(words: Vec<CaptionWord>) -> Vec<captions::TimedWord> {
    words
        .into_iter()
        .map(|word| captions::TimedWord {
            start: word.start,
            end: word.end,
            text: word.text,
        })
        .collect()
}

fn emit_progress(window: &tauri::Window, fraction: f64, message: String) {
    let _ = window.emit(
        "progress",
        serde_json::json!({ "percentage": fraction * 100.0, "message": message }),
    );
}

/// Clips to reframe as vertical shorts, with what's needed to frame, tighten
/// and caption them.
#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct VerticalRequest {
    pub input_path: String,
    pub segments: Vec<ClipSegment>,
    /// Who speaks when (the camera follows them).
    pub turns: Vec<SpeakerTurn>,
    /// Transcript words around the clips (fillers, captions).
    pub words: Vec<CaptionWord>,
    pub intensity: tighten::Intensity,
    /// Burn the words in as captions.
    pub captions: bool,
    /// Per clip: a looped short (its end runs into its start).
    pub looped: Vec<bool>,
}

/// Per clip, its source ranges (seconds) in playback order.
type ClipRanges = Vec<Vec<(f64, f64)>>;

/// The request's clips as tightened source ranges, and the words to caption
/// them with (without the ones tightening cut on purpose).
fn prepare_vertical(
    request: &VerticalRequest,
    input: &Path,
    run: Option<(u64, &RunControl)>,
) -> Result<(ClipRanges, Vec<captions::TimedWord>), String> {
    let words = timed_words(request.words.clone());
    let ranges = tighten::tighten_clips(
        input,
        &clip_ranges(&request.segments)?,
        &words,
        request.intensity,
        &request.looped,
        run,
    )?;
    let removed = tighten::removed_words(&words, request.intensity);
    let shown = words.into_iter().filter(|w| !removed.contains(w)).collect();
    Ok((ranges, shown))
}

/// Export clips as vertical (9:16) videos that follow the active speaker
/// (shorts phase S2), tightened (S4) and captioned (S3) as requested.
#[tauri::command]
#[specta::specta]
async fn export_vertical_clips(
    run_id: u64,
    window: tauri::Window,
    request: VerticalRequest,
    output_dir: String,
    quality: ExportQuality,
    run_control: State<'_, RunControl>,
    analysis: State<'_, std::sync::Arc<vertical::AnalysisCache>>,
) -> Result<(), AppError> {
    run_control.ensure_active(run_id)?;
    let input = PathBuf::from(&request.input_path);
    let output_dir = PathBuf::from(output_dir);
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format_path_io_error("create the output folder", &output_dir, &e))?;

    // Morph cuts need RIFE (21 MB), downloaded on first use. Without it the
    // export still works, with plain cuts.
    let cuts = vertical::CutStyle::from(request.intensity);
    let morph_model = if cuts.morph {
        match local_asr::model_root(&window, "rife") {
            Ok(dir) => {
                let path = dir.join(morph::RIFE.file_name());
                let downloaded = model_download::ensure_pinned_file(
                    &http::http_client(),
                    model_download::HUGGING_FACE,
                    &morph::RIFE,
                    &path,
                    &|message| emit_progress(&window, 0.0, message.to_string()),
                )
                .await;
                match downloaded {
                    Ok(()) => Some(path),
                    Err(error) => {
                        warn!("Morph cuts unavailable, using plain cuts: {error:#}");
                        None
                    }
                }
            }
            Err(error) => {
                warn!("Morph cuts unavailable, using plain cuts: {error:#}");
                None
            }
        }
    } else {
        None
    };
    run_control.ensure_active(run_id)?;

    let run_control = run_control.inner().clone();
    let analysis = analysis.inner().clone();
    run_blocking(move || {
        let run = Some((run_id, &run_control));
        emit_progress(&window, 0.0, "Tightening clips...".to_string());
        let (ranges, words) = prepare_vertical(&request, &input, run)?;
        let mut clips = Vec::with_capacity(ranges.len());
        for (index, (clip, ranges)) in request.segments.iter().zip(ranges).enumerate() {
            let output = output_dir.join(video::vertical_output_filename(index, clip));
            let metadata = serde_json::json!({
                "title": clip.label,
                "reason": clip.reason,
                "segments": clip.segments,
                "tightened": ranges,
                "format": "9:16",
            });
            if let Ok(content) = serde_json::to_string_pretty(&metadata) {
                let _ = std::fs::write(output.with_extension("json"), content);
            }
            clips.push(vertical::VerticalClip { ranges, output });
        }
        vertical::export_vertical(
            &input,
            &clips,
            &speech_turns(request.turns.clone()),
            vertical::RenderOptions {
                quality,
                cuts,
                morph_model: morph_model.as_deref(),
                captions: request
                    .captions
                    .then(|| (words.as_slice(), captions::CaptionStyle::default())),
            },
            Some(&analysis),
            run,
            &mut |fraction, message| emit_progress(&window, fraction, message),
        )
        .map(|_| ())
    })
    .await
    .map_err(AppError::from)
}

/// Plan the vertical framing of clips without rendering, for the live 9:16
/// preview: tightened like the export, and with the analysis cached so a
/// following export reuses it. The pieces are the playback order.
#[tauri::command]
#[specta::specta]
async fn plan_vertical_clips(
    run_id: u64,
    window: tauri::Window,
    request: VerticalRequest,
    run_control: State<'_, RunControl>,
    analysis: State<'_, std::sync::Arc<vertical::AnalysisCache>>,
) -> Result<vertical::VerticalPreview, AppError> {
    run_control.ensure_active(run_id)?;
    let input = PathBuf::from(&request.input_path);
    let run_control = run_control.inner().clone();
    let analysis = analysis.inner().clone();
    run_blocking(move || {
        let run = Some((run_id, &run_control));
        let (ranges, words) = prepare_vertical(&request, &input, run)?;
        vertical::plan_vertical(
            &input,
            &ranges,
            &speech_turns(request.turns.clone()),
            vertical::CutStyle::from(request.intensity),
            Some(&analysis),
            run,
            &mut |fraction| {
                emit_progress(
                    &window,
                    fraction,
                    format!("Finding faces and speakers ({:.0}%)", fraction * 100.0),
                )
            },
        )
        .map(|plan| {
            let preview = vertical::VerticalPreview::from(&plan);
            if request.captions {
                preview.with_captions(&words, &captions::CaptionStyle::default())
            } else {
                preview
            }
        })
    })
    .await
    .map_err(AppError::from)
}

/// Tighten clips (shorter pauses, no fillers or stutters) for the normal
/// export and its preview: the same clips with more, shorter segments.
#[tauri::command]
#[specta::specta]
async fn tighten_clips(
    run_id: u64,
    input_path: String,
    segments: Vec<ClipSegment>,
    words: Vec<CaptionWord>,
    intensity: tighten::Intensity,
    // Per clip: a looped short, whose seam keeps almost no air.
    looped: Vec<bool>,
    run_control: State<'_, RunControl>,
) -> Result<Vec<ClipSegment>, AppError> {
    run_control.ensure_active(run_id)?;
    let run_control = run_control.inner().clone();
    run_blocking(move || {
        let ranges = tighten::tighten_clips(
            Path::new(&input_path),
            &clip_ranges(&segments)?,
            &timed_words(words),
            intensity,
            &looped,
            Some((run_id, &run_control)),
        )?;
        Ok(segments
            .into_iter()
            .zip(ranges)
            .map(|(clip, ranges)| ClipSegment {
                segments: ranges
                    .into_iter()
                    .map(|(start, end)| Segment {
                        start: crate::time_utils::format_time(start),
                        end: crate::time_utils::format_time(end),
                    })
                    .collect(),
                ..clip
            })
            .collect())
    })
    .await
    .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
async fn read_file_as_base64(app: tauri::AppHandle, path: String) -> Result<String, AppError> {
    use base64::{engine::general_purpose, Engine as _};

    // Only the analysis audio the app extracted itself is ever sent this way.
    let root = media_cache::cache_root(&app)?;
    let path = path_guard::require_within(Path::new(&path), &root, "read audio")?;
    let content = tokio::fs::read(&path).await.map_err(|e| {
        format!(
            "Failed to read audio file '{}' for base64 encoding: {}",
            path.display(),
            e
        )
    })?;

    Ok(general_purpose::STANDARD.encode(content))
}

#[tauri::command]
#[specta::specta]
async fn open_folder(path: String) -> Result<(), AppError> {
    path_guard::require_directory(Path::new(&path))?;
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
async fn write_text_file(path: String, content: String) -> Result<(), AppError> {
    // Sessions and transcript sidecars (.json) and subtitles.
    path_guard::require_extension(Path::new(&path), &["json", "srt", "vtt", "txt"], "write")?;
    tokio::fs::write(path, content)
        .await
        .map_err(|e| AppError::failed(e.to_string()))
}

#[tauri::command]
#[specta::specta]
async fn read_text_file(path: String) -> Result<String, AppError> {
    // Sessions and transcript sidecars.
    path_guard::require_extension(Path::new(&path), &["json"], "read")?;
    tokio::fs::read_to_string(path)
        .await
        .map_err(|e| AppError::failed(e.to_string()))
}

#[tauri::command]
#[specta::specta]
fn path_exists(path: String) -> bool {
    Path::new(&path).exists()
}

/// Grant the webview's asset protocol access to a user-selected media file and
/// its directory at runtime. The static `assetProtocol.scope` in tauri.conf.json
/// only covers `$HOME`/`$TEMP`; this extends access to files anywhere the user
/// picks them (e.g. external volumes) so `convertFileSrc` can load the source
/// media and the extracted `.ogg` sidecar written alongside it.
#[tauri::command]
#[specta::specta]
fn allow_media_access(app: tauri::AppHandle, path: String) -> Result<(), AppError> {
    use tauri::Manager;

    let target = PathBuf::from(&path);
    // Previews load through the media:// protocol, which serves the same
    // folders.
    app.state::<media_protocol::MediaScope>()
        .allow(target.parent().unwrap_or(&target));
    let scope = app.asset_protocol_scope();

    if let Some(dir) = target.parent() {
        scope.allow_directory(dir, false).map_err(|e| {
            format!(
                "Failed to allow asset access for '{}': {}",
                dir.display(),
                e
            )
        })?;
    } else {
        scope.allow_file(&target).map_err(|e| {
            format!(
                "Failed to allow asset access for '{}': {}",
                target.display(),
                e
            )
        })?;
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
async fn zip_logs(app: tauri::AppHandle, target_path: String) -> Result<(), AppError> {
    path_guard::require_extension(Path::new(&target_path), &["zip"], "write logs to")?;
    use std::io::Write;
    use tauri::Manager;

    let log_dir = app.path().app_log_dir().map_err(|e| e.to_string())?;

    let file = std::fs::File::create(&target_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::<()>::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    if log_dir.exists() {
        for entry in std::fs::read_dir(&log_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name() {
                    let name = name.to_string_lossy();
                    zip.start_file(name, options).map_err(|e| e.to_string())?;
                    let content = std::fs::read(&path).map_err(|e| e.to_string())?;
                    zip.write_all(&content).map_err(|e| e.to_string())?;
                }
            }
        }
    }

    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
// Parameters mirror the IPC payload the frontend sends.
#[allow(clippy::too_many_arguments)]
async fn generate_podcast(
    run_id: u64,
    base_url: String,
    model: String,
    transcript: String,
    min_duration: u32,
    max_duration: u32,
    context: Option<String>,
    run_control: State<'_, RunControl>,
) -> Result<String, AppError> {
    run_control.ensure_active(run_id)?;
    let api_key = secrets::api_key().require()?;
    let client = GeminiClient::new(api_key, base_url, model);
    run_control
        .run_cancellable(
            run_id,
            client.generate_podcast(&transcript, min_duration, max_duration, context),
        )
        .await
        .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
// Parameters mirror the IPC payload the frontend sends.
#[allow(clippy::too_many_arguments)]
async fn refine_podcast(
    run_id: u64,
    base_url: String,
    model: String,
    original_transcript: String,
    current_script: String,
    current_duration: f64,
    target_min: u32,
    target_max: u32,
    run_control: State<'_, RunControl>,
) -> Result<String, AppError> {
    run_control.ensure_active(run_id)?;
    let api_key = secrets::api_key().require()?;
    let client = GeminiClient::new(api_key, base_url, model);
    run_control
        .run_cancellable(
            run_id,
            client.refine_podcast(
                &original_transcript,
                &current_script,
                current_duration,
                target_min,
                target_max,
            ),
        )
        .await
        .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
// Parameters mirror the IPC payload the frontend sends.
#[allow(clippy::too_many_arguments)]
async fn export_podcast(
    run_id: u64,
    window: tauri::Window,
    input_path: String,
    segments: Vec<PodcastSegment>,
    intro_path: Option<String>,
    outro_path: Option<String>,
    start_padding: f64,
    end_padding: f64,
    output_path: String,
    run_control: State<'_, RunControl>,
) -> Result<(), AppError> {
    run_control.ensure_active(run_id)?;
    let input = PathBuf::from(input_path);
    let output = PathBuf::from(output_path);
    let intro = intro_path.map(PathBuf::from);
    let outro = outro_path.map(PathBuf::from);

    let run_control = run_control.inner().clone();
    run_blocking(move || {
        export_podcast_fn(
            &input,
            &segments,
            intro.as_deref(),
            outro.as_deref(),
            start_padding,
            end_padding,
            &output,
            run_id,
            &run_control,
            move |time| {
                let _ = window.emit("progress", time);
            },
        )
        .map_err(|e: anyhow::Error| e.to_string())
    })
    .await
    .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
// Parameters mirror the IPC payload the frontend sends.
#[allow(clippy::too_many_arguments)]
async fn export_podcast_clips(
    run_id: u64,
    window: tauri::Window,
    input_path: String,
    segments: Vec<PodcastSegment>,
    start_padding: f64,
    end_padding: f64,
    output_dir: String,
    run_control: State<'_, RunControl>,
) -> Result<(), AppError> {
    run_control.ensure_active(run_id)?;
    let input = PathBuf::from(input_path);
    let output = PathBuf::from(output_dir);

    let run_control = run_control.inner().clone();
    run_blocking(move || {
        export_podcast_clips_fn(
            &input,
            &segments,
            start_padding,
            end_padding,
            &output,
            run_id,
            &run_control,
            move |time| {
                let _ = window.emit("progress", time);
            },
        )
        .map_err(|e: anyhow::Error| e.to_string())
    })
    .await
    .map_err(AppError::from)
}

#[tauri::command]
#[specta::specta]
fn calculate_segments_duration(segments: Vec<PodcastSegment>) -> f64 {
    calc_duration(&segments)
}

#[tauri::command]
#[specta::specta]
fn begin_run(run_control: State<'_, RunControl>) -> u64 {
    run_control.begin_run()
}

#[tauri::command]
#[specta::specta]
fn cancel_current_run(run_control: State<'_, RunControl>) -> Result<(), AppError> {
    run_control.cancel_current_run().map_err(AppError::from)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Where the generated TypeScript bindings live, relative to `src-tauri/`.
pub const IPC_BINDINGS_PATH: &str = "../src/bindings.ts";

/// The typed IPC surface: every command the frontend may call. The frontend's
/// `src/bindings.ts` is generated from it (`just bindings`), and the
/// `ipc_bindings_are_up_to_date` test fails when the two drift apart.
pub fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            begin_run,
            cancel_current_run,
            init_ffmpeg,
            prepare_audio_for_ai,
            prepare_preview_audio,
            cached_preview_audio,
            select_clips,
            list_models,
            secrets::set_api_key,
            secrets::clear_api_key,
            secrets::has_api_key,
            upload_file,
            split_audio_for_analysis,
            analyze_audio,
            cleanup_local_transcript,
            merge_transcript_hypotheses,
            transcribe_with_parakeet,
            transcribe_with_crisper,
            crisper_environment_status,
            install_crisper_environment,
            cut_video,
            export_clips,
            export_vertical_clips,
            plan_vertical_clips,
            tighten_clips,
            read_file_as_base64,
            open_folder,
            write_text_file,
            read_text_file,
            path_exists,
            allow_media_access,
            detect_silence,
            remove_silence,
            translate_transcript,
            zip_logs,
            generate_podcast,
            refine_podcast,
            export_podcast,
            export_podcast_clips,
            calculate_segments_duration
        ])
        // Keep the frontend's try/catch error handling: failed commands throw.
        .error_handling(tauri_specta::ErrorHandlingMode::Throw)
        // Run ids and sizes are far below 2^53; plain numbers keep the
        // frontend free of BigInt.
        .dangerously_cast_bigints_to_number()
}

/// Render the TypeScript bindings for [`specta_builder`].
pub fn render_ipc_bindings(path: &Path) -> Result<(), String> {
    specta_builder()
        .export(specta_typescript::Typescript::default(), path)
        .map_err(|error| format!("Failed to export TypeScript bindings: {error}"))
}

pub fn run() {
    tauri::Builder::default()
        .manage(media_protocol::MediaScope::default())
        .manage(std::sync::Arc::new(vertical::AnalysisCache::default()))
        .register_asynchronous_uri_scheme_protocol("media", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                let scope = app.state::<media_protocol::MediaScope>();
                responder.respond(media_protocol::serve(&scope, &request).await);
            });
        })
        .setup(|app| {
            // Derived preview audio lives in the media cache.
            if let Ok(root) = media_cache::cache_root(app.handle()) {
                let _ = std::fs::create_dir_all(&root);
                app.state::<media_protocol::MediaScope>().allow(&root);
            }
            // Keep the derived-media cache bounded; best effort, off the
            // startup path.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                if let Ok(root) = media_cache::cache_root(&handle) {
                    media_cache::prune(&root);
                }
            });
            Ok(())
        })
        .manage(RunControl::default())
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(specta_builder().invoke_handler())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod ipc_binding_tests {
    use super::*;

    /// Regenerate with `just bindings` (sets UPDATE_IPC_BINDINGS=1).
    #[test]
    fn ipc_bindings_are_up_to_date() {
        let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join(IPC_BINDINGS_PATH);
        if std::env::var_os("UPDATE_IPC_BINDINGS").is_some() {
            render_ipc_bindings(&committed).unwrap();
            return;
        }

        let fresh = tempfile::tempdir().unwrap().keep().join("bindings.ts");
        render_ipc_bindings(&fresh).unwrap();
        let expected = std::fs::read_to_string(&fresh).unwrap();
        let actual = std::fs::read_to_string(&committed).unwrap_or_default();
        assert!(
            expected == actual,
            "src/bindings.ts is out of date with the Tauri commands; run `just bindings`"
        );
    }
}
