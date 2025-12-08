use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use log::{debug, info};
use regex::Regex;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Debug, Clone)]
pub struct SilenceInterval {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
}

#[derive(Serialize, Debug, Clone)]
pub struct SegmentOffset {
    pub min_time: f64,
    pub offset: f64,
}

#[derive(Serialize, Debug, Clone)]
pub struct ProcessedAudio {
    pub path: String,
    pub silence_intervals: Vec<SilenceInterval>,
    pub offsets: Vec<SegmentOffset>,
}

#[tauri::command]
pub async fn detect_silence(path: String, min_duration: Option<f64>) -> Result<Vec<SilenceInterval>, String> {
    detect_silence_internal(&path, min_duration.unwrap_or(0.5)).await
}

async fn detect_silence_internal(path: &str, min_duration: f64) -> Result<Vec<SilenceInterval>, String> {
    let input_path = PathBuf::from(path);
    if !input_path.exists() {
        return Err("File not found".to_string());
    }

    info!("Starting silence detection for {:?} with min_duration {}", input_path, min_duration);

    // ffmpeg -i input.mp4 -af silencedetect=noise=-30dB:d=min_duration -f null -
    let events = FfmpegCommand::new()
        .input(input_path.to_str().unwrap())
        .args(&["-af", &format!("silencedetect=noise=-30dB:d={}", min_duration), "-f", "null", "-"])
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
            // debug!("[FFmpeg] {}", line); // Too verbose
            if let Some(caps) = re_start.captures(&line) {
                if let Some(m) = caps.get(1) {
                    if let Ok(val) = m.as_str().parse::<f64>() {
                        current_start = Some(val);
                        debug!("Silence start detected at {}", val);
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
                            debug!("Silence interval: {} - {} (duration: {})", start_val, end_val, end_val - start_val);
                            current_start = None;
                        }
                    }
                }
            }
        }
    }

    info!("Silence detection complete. Found {} intervals.", intervals.len());
    Ok(intervals)
}

#[tauri::command]
pub async fn remove_silence(path: String, min_duration: Option<f64>) -> Result<ProcessedAudio, String> {
    let min_duration_val = min_duration.unwrap_or(10.0);
    let silence_intervals = detect_silence_internal(&path, min_duration_val).await?;
    let input_path = PathBuf::from(&path);
    
    if silence_intervals.is_empty() {
        return Ok(ProcessedAudio {
            path,
            silence_intervals,
            offsets: vec![SegmentOffset { min_time: 0.0, offset: 0.0 }],
        });
    }

    let output_path = input_path.with_file_name(format!(
        "{}_nosilence.ogg",
        input_path.file_stem().unwrap().to_string_lossy()
    ));

    // Calculate keep segments
    // Assuming audio starts at 0.0
    let mut keep_segments = Vec::new();
    let mut last_end = 0.0;

    for interval in &silence_intervals {
        if interval.start > last_end {
            keep_segments.push((last_end, interval.start));
        }
        last_end = interval.end;
    }
    
    // We don't know the total duration easily without probing, but we can assume we want to keep until the end?
    // Or we can just stop at the last silence? 
    // Ideally we should probe duration. But for now let's assume we might miss the tail if it's not silent?
    // Actually, silencedetect doesn't report the end of the file as silence end if it's silent.
    // But if there is audio after the last silence, we need to include it.
    // Without duration, we can't know for sure. 
    // However, we can use a very large number for the last segment end if we use trim?
    // Or we can probe.
    
    // Let's probe duration using ffmpeg output
    let duration = probe_duration(&path).await.unwrap_or(last_end + 3600.0); 
    
    if duration > last_end {
        keep_segments.push((last_end, duration));
    }

    info!("Removing silence. Keep segments: {:?}", keep_segments);

    // Build filter complex
    let mut filter_complex = String::new();
    let mut inputs = String::new();
    let mut offsets = Vec::new();
    let mut current_new_time = 0.0;

    for (i, (start, end)) in keep_segments.iter().enumerate() {
        // If it's the last segment and we are not sure about duration, we can omit end?
        // But we need to know the duration for the offset calculation of the *next* segment (if there was one).
        // Since this is the last segment, omitting end is fine for atrim, but we need to be careful.
        // However, we calculated duration above.
        
        filter_complex.push_str(&format!("[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{}];", start, end, i));
        inputs.push_str(&format!("[a{}]", i));
        
        offsets.push(SegmentOffset {
            min_time: current_new_time,
            offset: *start - current_new_time,
        });
        
        current_new_time += end - start;
    }

    filter_complex.push_str(&format!("{}concat=n={}:v=0:a=1[outa]", inputs, keep_segments.len()));

    info!("Running FFmpeg to remove silence...");
    
    FfmpegCommand::new()
        .input(input_path.to_str().unwrap())
        .args(&[
            "-y",
            "-filter_complex", &filter_complex,
            "-map", "[outa]",
            "-c:a", "libvorbis",
            "-q:a", "4",
        ])
        .output(output_path.to_str().unwrap())
        .spawn()
        .map_err(|e| e.to_string())?
        .iter()
        .map_err(|e| e.to_string())?
        .for_each(|event| {
             if let FfmpegEvent::Log(_, msg) = event {
                 debug!("[FFmpeg Remove Silence] {}", msg);
             }
        });

    info!("Silence removed. New file: {:?}", output_path);

    Ok(ProcessedAudio {
        path: output_path.to_string_lossy().to_string(),
        silence_intervals,
        offsets,
    })
}

async fn probe_duration(path: &str) -> Result<f64, String> {
    use std::process::Command;
    
    // Try using ffmpeg -i path
    // We assume ffmpeg is in PATH (which it should be if init_ffmpeg was called or if installed globally)
    // In tests, we saw it works.
    let output = Command::new("ffmpeg")
        .arg("-i")
        .arg(path)
        .output()
        .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;
        
    let stderr = String::from_utf8_lossy(&output.stderr);
    
    let re_duration = Regex::new(r"Duration: (\d{2}):(\d{2}):(\d{2}\.\d{2})").unwrap();
    
    if let Some(caps) = re_duration.captures(&stderr) {
        let hours: f64 = caps[1].parse().unwrap_or(0.0);
        let minutes: f64 = caps[2].parse().unwrap_or(0.0);
        let seconds: f64 = caps[3].parse().unwrap_or(0.0);
        return Ok(hours * 3600.0 + minutes * 60.0 + seconds);
    }
    
    Err(format!("Failed to parse duration from ffmpeg output. Stderr: {}", stderr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn get_test_file_path() -> PathBuf {
        let mut path = std::env::current_dir().unwrap();
        // If we are in src-tauri, go up one level
        if path.ends_with("src-tauri") {
            path.pop();
        }
        path.join("dev-resources").join("test-data").join("test_podcast.m4a")
    }

    #[tokio::test]
    async fn test_silence_detection_and_removal() {
        let original_path = get_test_file_path();
        assert!(original_path.exists(), "Test file not found at {:?}", original_path);

        let temp_dir = std::env::temp_dir().join("ai-media-cutter-tests");
        if !temp_dir.exists() {
            std::fs::create_dir_all(&temp_dir).unwrap();
        }

        let test_file_path = temp_dir.join("test_with_silence.m4a");
        
        // Create a file with silence prepended and appended
        // Prepend 2s silence, Append 2s silence
        // Using ffmpeg command directly as FfmpegCommand might be harder to construct for complex filter with lavfi
        // We assume ffmpeg is in PATH for tests
        
        let status = Command::new("ffmpeg")
            .args(&[
                "-y",
                "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo:d=2",
                "-i", original_path.to_str().unwrap(),
                "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo:d=2",
                "-filter_complex", "[0:a][1:a][2:a]concat=n=3:v=0:a=1[out]",
                "-map", "[out]",
                test_file_path.to_str().unwrap()
            ])
            .status()
            .expect("Failed to execute ffmpeg");
            
        assert!(status.success(), "Failed to create test file with silence");
        assert!(test_file_path.exists());

        // 1. Test Detect Silence
        let intervals = detect_silence_internal(test_file_path.to_str().unwrap(), 0.5).await.unwrap();
        
        println!("Detected intervals: {:?}", intervals);
        
        // We expect at least 2 intervals: one at start (approx 0-2s) and one at end.
        // Note: silencedetect might not be perfect at exact boundaries.
        
        let start_silence = intervals.iter().find(|i| i.start < 0.5 && i.end > 1.5);
        assert!(start_silence.is_some(), "Should detect silence at the beginning");
        
        // 2. Test Remove Silence
        let processed = remove_silence(test_file_path.to_str().unwrap().to_string(), Some(0.5)).await.unwrap();
        
        assert!(Path::new(&processed.path).exists(), "Processed file should exist");
        
        // Check duration of processed file
        let processed_duration = probe_duration(&processed.path).await.unwrap();
        
        // Calculate expected duration
        // We need the duration of the input file (test_file_path) which has the added silence
        // But we can't probe it easily here because we might have deleted it? No, we haven't.
        // But wait, remove_silence takes a path string.
        
        let test_file_duration = probe_duration(test_file_path.to_str().unwrap()).await.unwrap();
        let total_silence_duration: f64 = processed.silence_intervals.iter().map(|i| i.duration).sum();
        let expected_duration = test_file_duration - total_silence_duration;
        
        println!("Test file duration: {}", test_file_duration);
        println!("Total silence detected: {}", total_silence_duration);
        println!("Processed duration (probe): {}", processed_duration);
        println!("Expected duration (calc): {}", expected_duration);
        
        assert!((processed_duration - expected_duration).abs() < 1.0, "Processed duration should match expected duration (original - silence)");
        
        // Clean up
        // std::fs::remove_file(test_file_path).unwrap();
        // std::fs::remove_file(processed.path).unwrap();
    }
}

