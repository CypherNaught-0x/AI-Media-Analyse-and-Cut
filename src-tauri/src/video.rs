use crate::run_control::{RunControl, RUN_CANCELLED_MESSAGE};
use crate::time_utils::parse_timestamp_to_seconds_raw;
use anyhow::Result;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use log::{debug, error, info};
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Segment {
    pub start: String,
    pub end: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TranscriptWord {
    pub start: String,
    pub end: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptAlternativeSource {
    #[default]
    Parakeet,
    Google,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptAlternative {
    pub source: TranscriptAlternativeSource,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_score: Option<f32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptMergeStatus {
    #[default]
    Matched,
    Conflict,
    MissingGoogle,
    MissingParakeet,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub start: String,
    pub end: String,
    pub speaker: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<TranscriptWord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alternatives: Option<Vec<TranscriptAlternative>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_status: Option<TranscriptMergeStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_source: Option<TranscriptAlternativeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_score: Option<f32>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ClipSegment {
    pub segments: Vec<Segment>,
    pub label: Option<String>,
    pub reason: Option<String>,
}

pub fn cut_video<F>(
    input_path: &Path,
    segments: &[Segment],
    output_path: &Path,
    run_id: u64,
    run_control: &RunControl,
    on_progress: F,
) -> Result<()>
where
    F: Fn(String) + Send + 'static,
{
    run_control
        .ensure_active(run_id)
        .map_err(|error| anyhow::anyhow!(error))?;
    // Optimization: Use filter_complex to cut and concat in a single pass.
    // Example:
    // ffmpeg -i input.mp4 -filter_complex
    // "[0:v]trim=start=10:end=20,setpts=PTS-STARTPTS[v0];
    //  [0:a]atrim=start=10:end=20,asetpts=PTS-STARTPTS[a0];
    //  [0:v]trim=start=30:end=40,setpts=PTS-STARTPTS[v1];
    //  [0:a]atrim=start=30:end=40,asetpts=PTS-STARTPTS[a1];
    //  [v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"
    // -map "[v]" -map "[a]" output.mp4

    info!(
        "Starting cut_video: input={:?}, output={:?}, segments={}",
        input_path,
        output_path,
        segments.len()
    );

    let (filter_complex, _inputs) = build_filter_complex(segments)?;

    let mut last_error = None;

    let mut child = FfmpegCommand::new()
        .input(input_path.to_str().unwrap())
        .args([
            "-y",
            "-filter_complex",
            &filter_complex,
            "-map",
            "[v]",
            "-map",
            "[a]",
        ])
        .output(output_path.to_str().unwrap())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn ffmpeg: {}", e))?;

    let pid = child.as_inner().id();
    run_control
        .register_pid(run_id, pid)
        .map_err(|error| anyhow::anyhow!(error))?;

    child
        .iter()
        .map_err(|e| anyhow::anyhow!("Failed to iterate ffmpeg events: {}", e))?
        .for_each(|event| match event {
            FfmpegEvent::Progress(p) => on_progress(p.time),
            FfmpegEvent::Log(_level, msg) => {
                debug!("[FFmpeg Log] {}", msg);
            }
            FfmpegEvent::Error(e) => {
                error!("[FFmpeg Error] {}", e);
                last_error = Some(e);
            }
            _ => {}
        });

    run_control.clear_pid(run_id, pid);

    if run_control.is_cancelled(run_id) {
        return Err(anyhow::anyhow!(RUN_CANCELLED_MESSAGE));
    }

    if !output_path.exists() {
        let msg = last_error.unwrap_or_else(|| "Unknown error".to_string());
        return Err(anyhow::anyhow!(
            "FFmpeg failed to create output file: {:?}. Error: {}",
            output_path,
            msg
        ));
    }

    Ok(())
}

fn build_filter_complex(segments: &[Segment]) -> Result<(String, String)> {
    let mut filter_complex = String::new();
    let mut inputs = String::new();

    for (i, segment) in segments.iter().enumerate() {
        // Parse timestamps to bare numbers so untrusted strings can't be
        // injected into the ffmpeg filtergraph.
        let start = parse_timestamp_to_seconds_raw(&segment.start).map_err(|e| {
            anyhow::anyhow!("Invalid segment start timestamp '{}': {}", segment.start, e)
        })?;
        let end = parse_timestamp_to_seconds_raw(&segment.end).map_err(|e| {
            anyhow::anyhow!("Invalid segment end timestamp '{}': {}", segment.end, e)
        })?;

        // Video trim
        filter_complex.push_str(&format!(
            "[0:v]trim=start={}:end={},setpts=PTS-STARTPTS[v{}];",
            start, end, i
        ));

        // Audio trim
        filter_complex.push_str(&format!(
            "[0:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[a{}];",
            start, end, i
        ));

        inputs.push_str(&format!("[v{}][a{}]", i, i));
    }

    filter_complex.push_str(&format!(
        "{}concat=n={}:v=1:a=1[v][a]",
        inputs,
        segments.len()
    ));

    Ok((filter_complex, inputs))
}

pub fn export_clips<F>(
    input_path: &Path,
    segments: &[ClipSegment],
    output_dir: &Path,
    fast_mode: bool,
    run_id: u64,
    run_control: &RunControl,
    on_progress: F,
) -> Result<()>
where
    F: Fn(usize, usize, String) + Send + Sync + 'static + Clone,
{
    run_control
        .ensure_active(run_id)
        .map_err(|error| anyhow::anyhow!(error))?;
    if output_dir.exists() {
        if !output_dir.is_dir() {
            return Err(anyhow::anyhow!(
                "Output path exists and is not a directory: {:?}",
                output_dir
            ));
        }
    } else {
        std::fs::create_dir_all(output_dir).map_err(|e| {
            anyhow::anyhow!("Failed to create output directory {:?}: {}", output_dir, e)
        })?;
    }

    info!(
        "Starting export_clips: input={:?}, output_dir={:?}, segments={}",
        input_path,
        output_dir,
        segments.len()
    );

    let total_clips = segments.len();

    for (i, segment) in segments.iter().enumerate() {
        run_control
            .ensure_active(run_id)
            .map_err(|error| anyhow::anyhow!(error))?;
        let output_filename = build_clip_output_filename(i, segment);
        let output_path = output_dir.join(&output_filename);

        // 1. Save Metadata
        let metadata_filename = output_path.with_extension("json");
        let metadata = serde_json::json!({
            "title": segment.label,
            "reason": segment.reason,
            "segments": segment.segments
        });
        if let Ok(content) = serde_json::to_string_pretty(&metadata) {
            let _ = std::fs::write(&metadata_filename, content);
        }

        let cb = on_progress.clone();

        // 2. Cut Video
        // If single segment, use simple cut. If multiple, use cut_video logic (concat).
        if segment.segments.len() == 1 {
            let s = &segment.segments[0];
            let mut last_error = None;

            let mut cmd = FfmpegCommand::new();
            cmd.input(input_path.to_str().unwrap());

            if fast_mode {
                cmd.args(["-y", "-ss", &s.start, "-to", &s.end, "-c", "copy"]);
            } else {
                cmd.args([
                    "-y", "-ss", &s.start, "-to", &s.end, "-c:v", "libx264", "-c:a", "aac",
                ]);
            }

            let mut child = cmd.output(output_path.to_str().unwrap())
                .spawn()
                .map_err(|e| anyhow::anyhow!("Failed to spawn ffmpeg: {}", e))?;

            let pid = child.as_inner().id();
            run_control
                .register_pid(run_id, pid)
                .map_err(|error| anyhow::anyhow!(error))?;

            child
                .iter()
                .map_err(|e| anyhow::anyhow!("Failed to iterate ffmpeg events: {}", e))?
                .for_each(|event| match event {
                    FfmpegEvent::Progress(p) => cb(i, total_clips, p.time),
                    FfmpegEvent::Log(_level, msg) => {
                        debug!("[FFmpeg Log] {}", msg);
                    }
                    FfmpegEvent::Error(e) => {
                        error!("[FFmpeg Error] {}", e);
                        last_error = Some(e);
                    }
                    _ => {}
                });

            run_control.clear_pid(run_id, pid);

            if run_control.is_cancelled(run_id) {
                return Err(anyhow::anyhow!(RUN_CANCELLED_MESSAGE));
            }

            if !output_path.exists() {
                let msg = last_error.unwrap_or_else(|| "Unknown error".to_string());
                return Err(anyhow::anyhow!(
                    "FFmpeg failed to create output file: {:?}. Error: {}",
                    output_path,
                    msg
                ));
            }
        } else {
            // Use existing cut_video logic which handles concat
            cut_video(
                input_path,
                &segment.segments,
                &output_path,
                run_id,
                run_control,
                move |time| {
                    cb(i, total_clips, time);
                },
            )?;
        }
    }
    Ok(())
}

fn build_clip_output_filename(i: usize, segment: &ClipSegment) -> String {
    let suffix = segment
        .label
        .as_ref()
        .map(|l| l.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', ""))
        .unwrap_or_default();

    if suffix.is_empty() {
        format!("clip_{:03}.mp4", i + 1)
    } else {
        format!("clip_{:03}_{}.mp4", i + 1, suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_filter_complex() {
        let segments = vec![
            Segment {
                start: "00:00".to_string(),
                end: "00:10".to_string(),
            },
            Segment {
                start: "00:20".to_string(),
                end: "00:30".to_string(),
            },
        ];

        let (filter, inputs) = build_filter_complex(&segments).unwrap();

        assert!(filter.contains("[0:v]trim=start=0:end=10,setpts=PTS-STARTPTS[v0];"));
        assert!(filter.contains("[0:a]atrim=start=0:end=10,asetpts=PTS-STARTPTS[a0];"));
        assert!(filter.contains("[0:v]trim=start=20:end=30,setpts=PTS-STARTPTS[v1];"));
        assert!(filter.contains("[0:a]atrim=start=20:end=30,asetpts=PTS-STARTPTS[a1];"));
        assert!(filter.contains("concat=n=2:v=1:a=1[v][a]"));
        assert_eq!(inputs, "[v0][a0][v1][a1]");
    }

    #[test]
    fn test_build_clip_output_filename() {
        let s1 = ClipSegment {
            segments: vec![Segment {
                start: "0".into(),
                end: "10".into(),
            }],
            label: None,
            reason: None,
        };
        assert_eq!(build_clip_output_filename(0, &s1), "clip_001.mp4");

        let s2 = ClipSegment {
            segments: vec![Segment {
                start: "0".into(),
                end: "10".into(),
            }],
            label: Some("My Clip".into()),
            reason: None,
        };
        assert_eq!(build_clip_output_filename(1, &s2), "clip_002_MyClip.mp4");

        let s3 = ClipSegment {
            segments: vec![Segment {
                start: "0".into(),
                end: "10".into(),
            }],
            label: Some("Clip/With\\BadChars!".into()),
            reason: None,
        };
        assert_eq!(
            build_clip_output_filename(2, &s3),
            "clip_003_ClipWithBadChars.mp4"
        );
    }
}
