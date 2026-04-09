// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

use ffmpeg_sidecar::command::ffmpeg_is_installed;
use ffmpeg_sidecar::download::auto_download;
use ffmpeg_sidecar::event::FfmpegEvent;
use ffmpeg_sidecar::paths::{ffmpeg_path, sidecar_path};
#[allow(unused_imports)]
use log::{error, info, warn};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tauri::Emitter;
use tauri::State;

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
async fn init_ffmpeg() -> Result<String, String> {
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

#[derive(Serialize)]
struct AudioInfo {
    path: String,
    size: u64,
    duration: f64,
}

fn get_media_duration(input_path: &str) -> Option<f64> {
    let output = std::process::Command::new(ffmpeg_path())
        .arg("-i")
        .arg(input_path)
        .output()
        .ok()?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(pos) = stderr.find("Duration: ") {
        let s = &stderr[pos + 10..];
        if let Some(end) = s.find(',') {
            let duration_str = &s[..end];
            let parts: Vec<&str> = duration_str.split(':').collect();
            if parts.len() == 3 {
                let hours: f64 = parts[0].parse().ok()?;
                let minutes: f64 = parts[1].parse().ok()?;
                let seconds: f64 = parts[2].parse().ok()?;
                return Some(hours * 3600.0 + minutes * 60.0 + seconds);
            }
        }
    }
    None
}

#[tauri::command]
async fn prepare_audio_for_ai(
    run_id: u64,
    window: tauri::Window,
    input_path: String,
    run_control: State<'_, RunControl>,
) -> Result<AudioInfo, String> {
    run_control.ensure_active(run_id)?;
    use crate::time_utils::parse_timestamp_to_seconds_raw;

    let input = PathBuf::from(&input_path);
    if !input.exists() {
        return Err("Input file does not exist".to_string());
    }

    let output_path = input.with_extension("ogg");
    let duration = get_media_duration(input.to_str().unwrap());
    let mut last_error = None;

    // Normalize input to OGG/Opus for downstream silence removal and AI upload.
    let mut child = FfmpegCommand::new()
        .input(input.to_str().unwrap())
        .args(&["-y", "-vn", "-c:a", "libopus", "-b:a", "96k"])
        .output(output_path.to_str().unwrap())
        .spawn()
        .map_err(|e| {
            format_ffmpeg_spawn_error(
                "prepare audio for AI analysis",
                &input,
                Some(&output_path),
                &e,
            )
        })?;

    let pid = child.as_inner().id();
    run_control.register_pid(run_id, pid)?;

    child
        .iter()
        .map_err(|e| {
            format!(
                "Failed while reading FFmpeg output during audio preparation for '{}': {}",
                input.display(),
                e
            )
        })?
        .for_each(|event| match event {
            FfmpegEvent::Progress(progress) => {
                let current_seconds = parse_timestamp_to_seconds_raw(&progress.time).unwrap_or(0.0);
                let percentage = if let Some(d) = duration {
                    if d > 0.0 {
                        Some((current_seconds / d) * 100.0)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let payload = serde_json::json!({
                    "time": progress.time,
                    "percentage": percentage
                });
                let _ = window.emit("progress", payload);
            }
            FfmpegEvent::Error(err) => {
                last_error = Some(err);
            }
            FfmpegEvent::Log(level, msg) => {
                if matches!(
                    level,
                    ffmpeg_sidecar::event::LogLevel::Error | ffmpeg_sidecar::event::LogLevel::Fatal
                ) {
                    last_error = Some(msg);
                }
            }
            _ => {}
        });

    run_control.clear_pid(run_id, pid);

    if run_control.is_cancelled(run_id) {
        return Err(RUN_CANCELLED_MESSAGE.to_string());
    }

    if !output_path.exists() {
        let msg = last_error
            .unwrap_or_else(|| "FFmpeg finished without creating the output file".to_string());
        return Err(format!(
            "Audio preparation failed for '{}' -> '{}': {}",
            input.display(),
            output_path.display(),
            msg
        ));
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
}

mod alignment;
pub mod gemini;
mod parakeet;
pub mod podcast;
pub mod retry;
mod run_control;
pub mod silence;
pub mod time_utils;
mod transcript_merge;
mod upload;
pub mod video;

use crate::alignment::align_transcript;
use crate::gemini::GeminiClient;
use crate::parakeet::transcribe_with_parakeet;
use crate::podcast::{
    calculate_segments_duration as calc_duration, export_podcast as export_podcast_fn,
    export_podcast_clips as export_podcast_clips_fn, PodcastSegment,
};
use crate::run_control::{RunControl, RUN_CANCELLED_MESSAGE};
use crate::silence::{detect_silence, remove_silence};
use crate::transcript_merge::merge_transcript_hypotheses_with_progress as merge_transcript_hypotheses_fn;
use crate::upload::upload_file_and_wait;
use crate::video::{
    cut_video as cut_video_fn, export_clips as export_clips_fn, ClipSegment, Segment,
    TranscriptSegment,
};

#[tauri::command]
async fn translate_transcript(
    run_id: u64,
    api_key: String,
    base_url: String,
    model: String,
    transcript: Vec<TranscriptSegment>,
    target_language: String,
    context: String,
    run_control: State<'_, RunControl>,
) -> Result<String, String> {
    run_control.ensure_active(run_id)?;
    let client = GeminiClient::new(api_key, base_url, model);
    run_control
        .run_cancellable(
            run_id,
            client.translate_transcript(transcript, target_language, context),
        )
        .await
}

#[tauri::command]
async fn upload_file(
    run_id: u64,
    api_key: String,
    base_url: String,
    path: String,
    run_control: State<'_, RunControl>,
) -> Result<Option<String>, String> {
    run_control.ensure_active(run_id)?;
    let path_buf = PathBuf::from(path);
    run_control
        .run_cancellable(run_id, upload_file_and_wait(&api_key, &base_url, &path_buf))
        .await
        .map_err(|e| {
            format!(
                "Failed to upload audio file '{}' to '{}': {}",
                path_buf.display(),
                base_url,
                e
            )
        })
}

#[tauri::command]
async fn analyze_audio(
    run_id: u64,
    api_key: String,
    base_url: String,
    model: String,
    enforce_json_schema: bool,
    context: String,
    glossary: String,
    speaker_count: Option<u32>,
    remove_filler_words: bool,
    audio_uri: Option<String>,
    audio_base64: Option<String>,
    run_control: State<'_, RunControl>,
) -> Result<String, String> {
    run_control.ensure_active(run_id)?;
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
}

#[tauri::command]
async fn cleanup_parakeet_transcript(
    run_id: u64,
    api_key: String,
    base_url: String,
    model: String,
    transcript: Vec<TranscriptSegment>,
    context: String,
    glossary: String,
    remove_filler_words: bool,
    run_control: State<'_, RunControl>,
) -> Result<Vec<TranscriptSegment>, String> {
    run_control.ensure_active(run_id)?;
    let client = GeminiClient::new(api_key, base_url.clone(), model.clone());
    run_control
        .run_cancellable(
            run_id,
            client.cleanup_parakeet_transcript(transcript, &context, &glossary, remove_filler_words),
        )
        .await
        .map_err(|e| {
            format!(
                "Transcript cleanup request to '{}' with model '{}' failed: {}",
                base_url, model, e
            )
        })
}

#[tauri::command]
async fn merge_transcript_hypotheses(
    run_id: u64,
    window: tauri::Window,
    primary_transcript: Vec<TranscriptSegment>,
    reference_transcript: Vec<TranscriptSegment>,
    run_control: State<'_, RunControl>,
) -> Result<Vec<TranscriptSegment>, String> {
    run_control.ensure_active(run_id)?;
    let started_at = std::time::Instant::now();
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
}

#[tauri::command]
async fn cut_video(
    run_id: u64,
    window: tauri::Window,
    input_path: String,
    segments: Vec<Segment>,
    output_path: String,
    run_control: State<'_, RunControl>,
) -> Result<(), String> {
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

    cut_video_fn(&input, &segments, &output, run_id, run_control.inner(), move |time| {
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
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn export_clips(
    run_id: u64,
    window: tauri::Window,
    input_path: String,
    segments: Vec<ClipSegment>,
    output_dir: String,
    fast_mode: bool,
    run_control: State<'_, RunControl>,
) -> Result<(), String> {
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

    export_clips_fn(
        &input,
        &segments,
        &output,
        fast_mode,
        run_id,
        run_control.inner(),
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
}

#[tauri::command]
async fn read_file_as_base64(path: String) -> Result<String, String> {
    use base64::{engine::general_purpose, Engine as _};

    let content = tokio::fs::read(&path).await.map_err(|e| {
        format!(
            "Failed to read audio file '{}' for base64 encoding: {}",
            path, e
        )
    })?;

    Ok(general_purpose::STANDARD.encode(content))
}

#[tauri::command]
async fn generate_clips(
    run_id: u64,
    api_key: String,
    base_url: String,
    model: String,
    transcript: String,
    count: u32,
    min_duration: u32,
    max_duration: u32,
    topic: Option<String>,
    splicing: bool,
    run_control: State<'_, RunControl>,
) -> Result<String, String> {
    run_control.ensure_active(run_id)?;
    let client = GeminiClient::new(api_key, base_url, model);
    run_control
        .run_cancellable(
            run_id,
            client.generate_clips(
                &transcript,
                count,
                min_duration,
                max_duration,
                topic,
                splicing,
            ),
        )
        .await
}

#[tauri::command]
async fn open_folder(path: String) -> Result<(), String> {
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
async fn write_text_file(path: String, content: String) -> Result<(), String> {
    tokio::fs::write(path, content)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn read_text_file(path: String) -> Result<String, String> {
    tokio::fs::read_to_string(path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn path_exists(path: String) -> bool {
    Path::new(&path).exists()
}

#[tauri::command]
async fn zip_logs(app: tauri::AppHandle, target_path: String) -> Result<(), String> {
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
async fn generate_podcast(
    run_id: u64,
    api_key: String,
    base_url: String,
    model: String,
    transcript: String,
    min_duration: u32,
    max_duration: u32,
    context: Option<String>,
    run_control: State<'_, RunControl>,
) -> Result<String, String> {
    run_control.ensure_active(run_id)?;
    let client = GeminiClient::new(api_key, base_url, model);
    run_control
        .run_cancellable(
            run_id,
            client.generate_podcast(&transcript, min_duration, max_duration, context),
        )
        .await
}

#[tauri::command]
async fn refine_podcast(
    run_id: u64,
    api_key: String,
    base_url: String,
    model: String,
    original_transcript: String,
    current_script: String,
    current_duration: f64,
    target_min: u32,
    target_max: u32,
    run_control: State<'_, RunControl>,
) -> Result<String, String> {
    run_control.ensure_active(run_id)?;
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
}

#[tauri::command]
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
) -> Result<(), String> {
    run_control.ensure_active(run_id)?;
    let input = PathBuf::from(input_path);
    let output = PathBuf::from(output_path);
    let intro = intro_path.map(PathBuf::from);
    let outro = outro_path.map(PathBuf::from);

    export_podcast_fn(
        &input,
        &segments,
        intro.as_deref(),
        outro.as_deref(),
        start_padding,
        end_padding,
        &output,
        run_id,
        run_control.inner(),
        move |time| {
            let _ = window.emit("progress", time);
        },
    )
    .map_err(|e: anyhow::Error| e.to_string())
}

#[tauri::command]
async fn export_podcast_clips(
    run_id: u64,
    window: tauri::Window,
    input_path: String,
    segments: Vec<PodcastSegment>,
    start_padding: f64,
    end_padding: f64,
    output_dir: String,
    run_control: State<'_, RunControl>,
) -> Result<(), String> {
    run_control.ensure_active(run_id)?;
    let input = PathBuf::from(input_path);
    let output = PathBuf::from(output_dir);

    export_podcast_clips_fn(
        &input,
        &segments,
        start_padding,
        end_padding,
        &output,
        run_id,
        run_control.inner(),
        move |time| {
            let _ = window.emit("progress", time);
        },
    )
    .map_err(|e: anyhow::Error| e.to_string())
}

#[tauri::command]
fn calculate_segments_duration(segments: Vec<PodcastSegment>) -> f64 {
    calc_duration(&segments)
}

#[tauri::command]
fn begin_run(run_control: State<'_, RunControl>) -> u64 {
    run_control.begin_run()
}

#[tauri::command]
fn cancel_current_run(run_control: State<'_, RunControl>) -> Result<(), String> {
    run_control.cancel_current_run()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(RunControl::default())
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            begin_run,
            cancel_current_run,
            init_ffmpeg,
            prepare_audio_for_ai,
            upload_file,
            analyze_audio,
            cleanup_parakeet_transcript,
            merge_transcript_hypotheses,
            transcribe_with_parakeet,
            cut_video,
            export_clips,
            read_file_as_base64,
            generate_clips,
            open_folder,
            write_text_file,
            read_text_file,
            path_exists,
            align_transcript,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
