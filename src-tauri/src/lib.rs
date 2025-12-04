// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

use ffmpeg_sidecar::command::ffmpeg_is_installed;
use ffmpeg_sidecar::download::auto_download;
use ffmpeg_sidecar::event::FfmpegEvent;
use tauri::Emitter;

#[tauri::command]
async fn init_ffmpeg() -> Result<String, String> {
    if ffmpeg_is_installed() {
        return Ok("FFmpeg is already installed.".to_string());
    }
    
    // Try to download
    if let Err(e) = auto_download() {
        println!("FFmpeg auto_download failed: {}", e);
        // We continue, maybe it's already there but not in PATH
    }

    if ffmpeg_is_installed() {
        return Ok("FFmpeg downloaded successfully.".to_string());
    }

    // Fallback: Add current dir to PATH if ffmpeg is there
    let current_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let filename = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    
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
                    if p1.exists() { found_path = Some(p1); break; }
                    
                    // Check inside bin
                    let p2 = path.join("bin").join(filename);
                    if p2.exists() { found_path = Some(p2); break; }
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
                std::env::set_var(key, ffmpeg_path.parent().unwrap().to_string_lossy().to_string());
            }
        }

        if ffmpeg_is_installed() {
            return Ok(format!("FFmpeg found at {} and added to PATH.", ffmpeg_path.display()));
        }
    }

    Ok("FFmpeg downloaded but verification failed. Please restart the app.".to_string())
}

use ffmpeg_sidecar::command::FfmpegCommand;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct AudioInfo {
    path: String,
    size: u64,
}

#[tauri::command]
async fn prepare_audio_for_ai(
    window: tauri::Window,
    input_path: String,
) -> Result<AudioInfo, String> {
    let input = PathBuf::from(&input_path);
    if !input.exists() {
        return Err("Input file does not exist".to_string());
    }

    let output_path = input.with_extension("ogg");

    // ffmpeg -i input.mp4 -vn -c:a libvorbis -q:a 4 output.ogg
    FfmpegCommand::new()
        .input(input.to_str().unwrap())
        .args(&["-vn", "-c:a", "libvorbis", "-q:a", "4"])
        .output(output_path.to_str().unwrap())
        .spawn()
        .map_err(|e| e.to_string())?
        .iter()
        .map_err(|e| e.to_string())?
        .for_each(|event| {
            if let FfmpegEvent::Progress(progress) = event {
                let _ = window.emit("progress", progress.time);
            }
        });

    // Check size
    let metadata = std::fs::metadata(&output_path).map_err(|e| e.to_string())?;
    let size = metadata.len();

    Ok(AudioInfo {
        path: output_path.to_string_lossy().to_string(),
        size,
    })
}

mod alignment;
mod gemini;
pub mod time_utils;
mod upload;
mod video;
mod silence;

use crate::alignment::align_transcript;
use crate::gemini::GeminiClient;
use crate::upload::upload_file_and_wait;
use crate::video::{
    cut_video as cut_video_fn, export_clips as export_clips_fn, ClipSegment, Segment, TranscriptSegment
};
use crate::silence::detect_silence;

#[tauri::command]
async fn translate_transcript(
    api_key: String,
    base_url: String,
    model: String,
    transcript: Vec<TranscriptSegment>,
    target_language: String,
    context: String,
) -> Result<String, String> {
    let client = GeminiClient::new(api_key, base_url, model);
    client
        .translate_transcript(transcript, target_language, context)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn upload_file(
    api_key: String,
    base_url: String,
    path: String,
) -> Result<Option<String>, String> {
    let path_buf = PathBuf::from(path);
    upload_file_and_wait(&api_key, &base_url, &path_buf)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn analyze_audio(
    api_key: String,
    base_url: String,
    model: String,
    context: String,
    glossary: String,
    speaker_count: Option<u32>,
    audio_uri: Option<String>,
    audio_base64: Option<String>,
) -> Result<String, String> {
    let client = GeminiClient::new(api_key, base_url, model);
    client
        .analyze_audio(
            &context,
            &glossary,
            speaker_count,
            audio_uri.as_deref(),
            audio_base64.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn cut_video(
    window: tauri::Window,
    input_path: String,
    segments: Vec<Segment>,
    output_path: String,
) -> Result<(), String> {
    let input = PathBuf::from(input_path);
    let output = PathBuf::from(output_path);
    cut_video_fn(&input, &segments, &output, move |time| {
        let _ = window.emit("progress", time);
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn export_clips(
    window: tauri::Window,
    input_path: String,
    segments: Vec<ClipSegment>,
    output_dir: String,
) -> Result<(), String> {
    let input = PathBuf::from(input_path);
    let output = PathBuf::from(output_dir);
    export_clips_fn(&input, &segments, &output, move |time| {
        let _ = window.emit("progress", time);
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn read_file_as_base64(path: String) -> Result<String, String> {
    use base64::{engine::general_purpose, Engine as _};

    let content = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("Failed to read file: {}", e))?;

    Ok(general_purpose::STANDARD.encode(content))
}

#[tauri::command]
async fn generate_clips(
    api_key: String,
    base_url: String,
    model: String,
    transcript: String,
    count: u32,
    min_duration: u32,
    max_duration: u32,
    topic: Option<String>,
    splicing: bool,
) -> Result<String, String> {
    let client = GeminiClient::new(api_key, base_url, model);
    client
        .generate_clips(&transcript, count, min_duration, max_duration, topic, splicing)
        .await
        .map_err(|e| e.to_string())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            init_ffmpeg,
            prepare_audio_for_ai,
            upload_file,
            analyze_audio,
            cut_video,
            export_clips,
            read_file_as_base64,
            generate_clips,
            open_folder,
            write_text_file,
            read_text_file,
            align_transcript,
            detect_silence,
            translate_transcript
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
