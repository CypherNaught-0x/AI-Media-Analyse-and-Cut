use crate::ffmpeg::{run_ffmpeg, FfmpegTask};
use crate::run_control::RunControl;
use anyhow::Result;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use log::info;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::time_utils::parse_time;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum PodcastSegmentType {
    #[default]
    Content,
    Voiceover,
}

#[derive(Serialize, Deserialize, Debug, Clone, specta::Type)]
pub struct PodcastSegment {
    pub start: String,
    pub end: String,
    pub text: String,
    pub speaker: String,
    #[serde(default)]
    pub segment_type: PodcastSegmentType, // 'content' = actual audio, 'voiceover' = suggested transition
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub include_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub transition_note: Option<String>, // For voiceover: suggested text to bridge topics
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PodcastScript {
    pub title: String,
    pub summary: String,
    pub segments: Vec<PodcastSegment>,
    pub total_duration: f64,
}

/// Calculate total duration of podcast segments in seconds (only content segments)
pub fn calculate_segments_duration(segments: &[PodcastSegment]) -> f64 {
    segments
        .iter()
        .filter(|seg| seg.segment_type == PodcastSegmentType::Content)
        .fold(0.0, |acc, seg| {
            let start = parse_time(&seg.start);
            let end = parse_time(&seg.end);
            acc + (end - start)
        })
}

/// Calculate total duration with padding applied
pub fn calculate_segments_duration_with_padding(
    segments: &[PodcastSegment],
    start_padding: f64,
    end_padding: f64,
) -> f64 {
    let content_segments: Vec<_> = segments
        .iter()
        .filter(|seg| seg.segment_type == PodcastSegmentType::Content)
        .collect();

    content_segments.iter().fold(0.0, |acc, seg| {
        let start = parse_time(&seg.start);
        let end = parse_time(&seg.end);
        let duration = (end - start) + start_padding + end_padding;
        acc + duration.max(0.0) // Ensure non-negative
    })
}

/// Export podcast as M4A audio file with optional intro and outro
#[allow(clippy::too_many_arguments)] // one per export option; see the Tauri command
pub fn export_podcast<F>(
    input_path: &Path,
    segments: &[PodcastSegment],
    intro_path: Option<&Path>,
    outro_path: Option<&Path>,
    start_padding: f64,
    end_padding: f64,
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
    // Filter out voiceover segments - only export actual content
    let content_segments: Vec<_> = segments
        .iter()
        .filter(|seg| seg.segment_type == PodcastSegmentType::Content)
        .cloned()
        .collect();

    info!(
        "Starting podcast export: input={:?}, output={:?}, segments={} (content only), intro={:?}, outro={:?}, padding=[{}, {}]",
        input_path,
        output_path,
        content_segments.len(),
        intro_path,
        outro_path,
        start_padding,
        end_padding
    );

    let filter_complex = build_podcast_filter_complex(
        &content_segments,
        intro_path,
        outro_path,
        start_padding,
        end_padding,
    );
    let mut cmd = FfmpegCommand::new();

    // Add intro input if provided
    if let Some(intro) = intro_path {
        cmd.input(intro.to_str().unwrap());
    }

    // Main input (source video/audio)
    cmd.input(input_path.to_str().unwrap());

    // Add outro input if provided
    if let Some(outro) = outro_path {
        cmd.input(outro.to_str().unwrap());
    }

    cmd.args([
        "-y",
        "-filter_complex",
        &filter_complex,
        "-map",
        "[outa]",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
    ])
    .output(output_path.to_str().unwrap());
    run_ffmpeg(
        cmd,
        FfmpegTask {
            operation: "export the podcast",
            input: input_path,
            output: Some(output_path),
            run: Some((run_id, run_control)),
        },
        |event| {
            if let FfmpegEvent::Progress(progress) = event {
                on_progress(progress.time.clone());
            }
        },
    )
    .map_err(|error| anyhow::anyhow!(error))?;

    Ok(())
}

/// Build FFmpeg filter_complex for podcast audio concatenation
fn build_podcast_filter_complex(
    segments: &[PodcastSegment],
    intro_path: Option<&Path>,
    outro_path: Option<&Path>,
    start_padding: f64,
    end_padding: f64,
) -> String {
    let mut filter = String::new();
    let mut concat_inputs = String::new();
    let mut input_count = 0;
    let mut stream_count = 0;

    // Track which input index is the main source
    let main_input_idx = if intro_path.is_some() { 1 } else { 0 };

    // Intro audio
    if intro_path.is_some() {
        filter.push_str(&format!("[{}:a]aresample=44100[intro];", input_count));
        concat_inputs.push_str("[intro]");
        input_count += 1;
        stream_count += 1;
    }

    // Main source segments (with padding applied)
    for (i, segment) in segments.iter().enumerate() {
        let start_secs = (parse_time(&segment.start) - start_padding).max(0.0);
        let end_secs = parse_time(&segment.end) + end_padding;

        filter.push_str(&format!(
            "[{}:a]atrim=start={}:end={},asetpts=PTS-STARTPTS,aresample=44100[seg{}];",
            main_input_idx, start_secs, end_secs, i
        ));
        concat_inputs.push_str(&format!("[seg{}]", i));
        stream_count += 1;
    }

    // Update input_count to account for main input
    input_count += 1;

    // Outro audio
    if outro_path.is_some() {
        filter.push_str(&format!("[{}:a]aresample=44100[outro];", input_count));
        concat_inputs.push_str("[outro]");
        stream_count += 1;
    }

    // Concat all streams
    filter.push_str(&format!(
        "{}concat=n={}:v=0:a=1[outa]",
        concat_inputs, stream_count
    ));

    filter
}

/// Export individual podcast clips as separate M4A files
#[allow(clippy::too_many_arguments)] // one per export option; see the Tauri command
pub fn export_podcast_clips<F>(
    input_path: &Path,
    segments: &[PodcastSegment],
    start_padding: f64,
    end_padding: f64,
    output_dir: &Path,
    run_id: u64,
    run_control: &RunControl,
    on_progress: F,
) -> Result<()>
where
    F: Fn(String) + Send + Sync + 'static + Clone,
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

    // Filter out voiceover segments
    let content_segments: Vec<_> = segments
        .iter()
        .filter(|seg| seg.segment_type == PodcastSegmentType::Content)
        .collect();

    info!(
        "Exporting podcast clips: input={:?}, output_dir={:?}, segments={} (content only), padding=[{}, {}]",
        input_path,
        output_dir,
        content_segments.len(),
        start_padding,
        end_padding
    );

    for (i, segment) in content_segments.iter().enumerate() {
        run_control
            .ensure_active(run_id)
            .map_err(|error| anyhow::anyhow!(error))?;
        let output_filename = format!("podcast_clip_{:03}.m4a", i + 1);
        let output_path = output_dir.join(&output_filename);

        // Save metadata
        let metadata_path = output_path.with_extension("json");
        let metadata = serde_json::json!({
            "speaker": segment.speaker,
            "text": segment.text,
            "start": segment.start,
            "end": segment.end,
            "include_reason": segment.include_reason,
            "segment_type": "content"
        });
        if let Ok(content) = serde_json::to_string_pretty(&metadata) {
            let _ = std::fs::write(&metadata_path, content);
        }

        // Export audio clip with padding
        let start_secs = (parse_time(&segment.start) - start_padding).max(0.0);
        let end_secs = parse_time(&segment.end) + end_padding;
        let cb = on_progress.clone();
        let mut command = FfmpegCommand::new();
        // Seek on the input so ffmpeg jumps to the clip instead of decoding
        // the source from the start.
        command
            .args(["-y", "-ss", &format!("{start_secs:.6}")])
            .input(input_path.to_str().unwrap())
            .args([
                "-t",
                &format!("{:.6}", (end_secs - start_secs).max(0.0)),
                "-vn",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
            ])
            .output(output_path.to_str().unwrap());
        run_ffmpeg(
            command,
            FfmpegTask {
                operation: &format!("export podcast clip {}", i + 1),
                input: input_path,
                output: Some(&output_path),
                run: Some((run_id, run_control)),
            },
            |event| {
                if let FfmpegEvent::Progress(progress) = event {
                    cb(progress.time.clone());
                }
            },
        )
        .map_err(|error| anyhow::anyhow!(error))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs real ffmpeg (on PATH, like the other export tests).
    #[test]
    fn podcast_clips_cover_the_padded_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("talk.m4a");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
            .args(["-i", "sine=frequency=440:duration=10", "-c:a", "aac"])
            .arg(&source)
            .status()
            .expect("ffmpeg must be on PATH for this test");
        assert!(status.success());

        let segment = |start: &str, end: &str| PodcastSegment {
            start: start.to_string(),
            end: end.to_string(),
            text: "text".to_string(),
            speaker: "Host".to_string(),
            segment_type: PodcastSegmentType::Content,
            include_reason: None,
            transition_note: None,
        };
        let out_dir = dir.path().join("clips");
        let control = RunControl::default();
        let run_id = control.begin_run();
        export_podcast_clips(
            &source,
            &[segment("00:02", "00:04"), segment("00:07", "00:08")],
            0.5,
            0.5,
            &out_dir,
            run_id,
            &control,
            |_| {},
        )
        .unwrap();

        for (file, expected) in [("podcast_clip_001.m4a", 3.0), ("podcast_clip_002.m4a", 2.0)] {
            let duration = crate::media_probe::probe_media(&out_dir.join(file))
                .unwrap()
                .duration_seconds
                .unwrap();
            assert!((duration - expected).abs() < 0.1, "{file}: {duration}");
        }
    }

    #[test]
    fn test_calculate_segments_duration() {
        let segments = vec![
            PodcastSegment {
                start: "00:00".to_string(),
                end: "00:10".to_string(),
                text: "Test".to_string(),
                speaker: "Speaker 1".to_string(),
                segment_type: PodcastSegmentType::Content,
                include_reason: None,
                transition_note: None,
            },
            PodcastSegment {
                start: "00:20".to_string(),
                end: "00:35".to_string(),
                text: "Test 2".to_string(),
                speaker: "Speaker 2".to_string(),
                segment_type: PodcastSegmentType::Content,
                include_reason: None,
                transition_note: None,
            },
        ];

        let duration = calculate_segments_duration(&segments);
        assert!((duration - 25.0).abs() < 0.01);
    }

    #[test]
    fn test_calculate_segments_duration_ignores_voiceover() {
        let segments = vec![
            PodcastSegment {
                start: "00:00".to_string(),
                end: "00:10".to_string(),
                text: "Content".to_string(),
                speaker: "Speaker 1".to_string(),
                segment_type: PodcastSegmentType::Content,
                include_reason: None,
                transition_note: None,
            },
            PodcastSegment {
                start: "00:10".to_string(),
                end: "00:15".to_string(),
                text: "Transition".to_string(),
                speaker: "Narrator".to_string(),
                segment_type: PodcastSegmentType::Voiceover,
                include_reason: None,
                transition_note: Some("Bridge text".to_string()),
            },
        ];

        let duration = calculate_segments_duration(&segments);
        // Should only count the content segment (10 seconds), not the voiceover
        assert!((duration - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_build_podcast_filter_complex_no_intro_outro() {
        let segments = vec![PodcastSegment {
            start: "00:10".to_string(),
            end: "00:20".to_string(),
            text: "Test".to_string(),
            speaker: "Speaker 1".to_string(),
            segment_type: PodcastSegmentType::Content,
            include_reason: None,
            transition_note: None,
        }];

        // No padding
        let filter = build_podcast_filter_complex(&segments, None, None, 0.0, 0.0);
        assert!(filter.contains("[0:a]atrim=start=10:end=20"));
        assert!(filter.contains("concat=n=1:v=0:a=1[outa]"));
    }

    #[test]
    fn test_build_podcast_filter_complex_with_padding() {
        let segments = vec![PodcastSegment {
            start: "00:10".to_string(),
            end: "00:20".to_string(),
            text: "Test".to_string(),
            speaker: "Speaker 1".to_string(),
            segment_type: PodcastSegmentType::Content,
            include_reason: None,
            transition_note: None,
        }];

        // With 2 second padding on each side
        let filter = build_podcast_filter_complex(&segments, None, None, 2.0, 2.0);
        // Start should be 10 - 2 = 8, end should be 20 + 2 = 22
        assert!(filter.contains("[0:a]atrim=start=8:end=22"));
    }
}
