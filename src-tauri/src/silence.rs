use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use regex::Regex;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Debug, Clone)]
pub struct SilenceInterval {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
}

#[tauri::command]
pub async fn detect_silence(path: String) -> Result<Vec<SilenceInterval>, String> {
    let input_path = PathBuf::from(path);
    if !input_path.exists() {
        return Err("File not found".to_string());
    }

    // ffmpeg -i input.mp4 -af silencedetect=noise=-30dB:d=0.5 -f null -
    // We use a lower noise threshold (-30dB) and a minimum duration of 0.5s
    let events = FfmpegCommand::new()
        .input(input_path.to_str().unwrap())
        .args(&["-af", "silencedetect=noise=-30dB:d=0.5", "-f", "null", "-"])
        .spawn()
        .map_err(|e| e.to_string())?
        .iter()
        .map_err(|e| e.to_string())?;

    let mut intervals = Vec::new();
    let mut current_start = None;

    // Regex for start: silence_start: 12.345
    let re_start = Regex::new(r"silence_start: (\d+(\.\d+)?)").unwrap();
    // Regex for end: silence_end: 15.678
    let re_end = Regex::new(r"silence_end: (\d+(\.\d+)?)").unwrap();

    for event in events {
        if let FfmpegEvent::Log(_, line) = event {
            if let Some(caps) = re_start.captures(&line) {
                if let Some(m) = caps.get(1) {
                    if let Ok(val) = m.as_str().parse::<f64>() {
                        current_start = Some(val);
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
                            current_start = None;
                        }
                    }
                }
            }
        }
    }

    Ok(intervals)
}
