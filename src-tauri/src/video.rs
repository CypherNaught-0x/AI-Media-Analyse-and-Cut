use crate::ffmpeg::{run_ffmpeg, FfmpegTask};
use crate::run_control::RunControl;
use crate::time_utils::parse_timestamp_to_seconds_raw;
use anyhow::Result;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use log::info;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, specta::Type)]
pub struct Segment {
    pub start: String,
    pub end: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, specta::Type)]
pub struct TranscriptWord {
    pub start: String,
    pub end: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub speaker: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptAlternativeSource {
    /// Whichever local engine produced the timed words (Parakeet or
    /// CrisperWhisper). Serialised as `local`; `parakeet` is accepted as an
    /// alias so transcripts saved before the engines were split still load.
    #[default]
    #[serde(rename = "local", alias = "parakeet")]
    Local,
    Google,
}

#[derive(Serialize, Deserialize, Debug, Clone, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptAlternative {
    pub source: TranscriptAlternativeSource,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub speaker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub similarity_score: Option<f32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptMergeStatus {
    #[default]
    Matched,
    Conflict,
    MissingGoogle,
    /// Missing from the local engine's hypothesis. `missing_parakeet` is
    /// accepted as an alias for transcripts saved before the split.
    #[serde(rename = "missing_local", alias = "missing_parakeet")]
    MissingLocal,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub start: String,
    pub end: String,
    pub speaker: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub words: Option<Vec<TranscriptWord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub alternatives: Option<Vec<TranscriptAlternative>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub merge_status: Option<TranscriptMergeStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub active_source: Option<TranscriptAlternativeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub similarity_score: Option<f32>,
}

#[derive(Serialize, Deserialize, Debug, specta::Type)]
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

    let mut command = FfmpegCommand::new();
    command
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
        .output(output_path.to_str().unwrap());
    run_ffmpeg(
        command,
        FfmpegTask {
            operation: "cut the video",
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
            let (input_args, output_args) = single_clip_args(s, fast_mode)?;

            let mut command = FfmpegCommand::new();
            command
                .args(input_args)
                .input(input_path.to_str().unwrap())
                .args(output_args)
                .output(output_path.to_str().unwrap());
            run_ffmpeg(
                command,
                FfmpegTask {
                    operation: &format!("export clip {}", i + 1),
                    input: input_path,
                    output: Some(&output_path),
                    run: Some((run_id, run_control)),
                },
                |event| {
                    if let FfmpegEvent::Progress(progress) = event {
                        cb(i, total_clips, progress.time.clone());
                    }
                },
            )
            .map_err(|error| anyhow::anyhow!(error))?;
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

/// Encoder settings for re-encoded clip exports: broadly playable H.264/AAC
/// with the moov atom up front so the file streams and seeks immediately.
const CLIP_ENCODE_ARGS: &[(&str, &str)] = &[
    ("-c:v", "libx264"),
    ("-preset", "veryfast"),
    ("-crf", "20"),
    ("-pix_fmt", "yuv420p"),
    ("-c:a", "aac"),
    ("-b:a", "192k"),
    ("-movflags", "+faststart"),
];

/// ffmpeg arguments (before and after `-i`) for exporting one source range.
///
/// The seek is an *input* option in both modes, so ffmpeg jumps straight to the
/// range instead of decoding from the start of the file.
/// - Re-encode (default): frame-accurate, because ffmpeg decodes from the
///   preceding keyframe and discards frames up to the exact start.
/// - `fast_mode` (stream copy): a draft export. Copying can only start on a
///   keyframe, so the clip may begin slightly before `start`; seeking on the
///   input keeps it starting on that keyframe instead of on a frame that can't
///   be decoded (frozen or garbled opening).
fn single_clip_args(segment: &Segment, fast_mode: bool) -> Result<(Vec<String>, Vec<String>)> {
    let start = parse_timestamp_to_seconds_raw(&segment.start)
        .map_err(|e| anyhow::anyhow!("Invalid clip start timestamp '{}': {}", segment.start, e))?;
    let end = parse_timestamp_to_seconds_raw(&segment.end)
        .map_err(|e| anyhow::anyhow!("Invalid clip end timestamp '{}': {}", segment.end, e))?;
    if end <= start {
        return Err(anyhow::anyhow!(
            "Clip end {} must be after its start {}",
            segment.end,
            segment.start
        ));
    }

    let input_args = vec!["-y".to_string(), "-ss".to_string(), format!("{:.3}", start)];
    let mut output_args = vec!["-t".to_string(), format!("{:.3}", end - start)];
    if fast_mode {
        output_args.extend(["-c", "copy", "-avoid_negative_ts", "make_zero"].map(String::from));
    } else {
        output_args.extend(
            CLIP_ENCODE_ARGS
                .iter()
                .flat_map(|(flag, value)| [flag.to_string(), value.to_string()]),
        );
    }
    Ok((input_args, output_args))
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

    fn segment(start: &str, end: &str) -> Segment {
        Segment {
            start: start.to_string(),
            end: end.to_string(),
        }
    }

    #[test]
    fn single_clip_seeks_on_the_input_and_reencodes_by_default() {
        let (input, output) = single_clip_args(&segment("01:05.250", "01:15.750"), false).unwrap();
        assert_eq!(input, ["-y", "-ss", "65.250"]);
        assert_eq!(&output[..2], ["-t", "10.500"]);
        assert!(output.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(output.windows(2).any(|w| w == ["-pix_fmt", "yuv420p"]));
        assert!(!output.iter().any(|a| a == "copy"));
    }

    #[test]
    fn single_clip_fast_mode_stream_copies() {
        let (input, output) = single_clip_args(&segment("00:10", "00:20"), true).unwrap();
        assert_eq!(input, ["-y", "-ss", "10.000"]);
        assert_eq!(
            output,
            [
                "-t",
                "10.000",
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero"
            ]
        );
    }

    #[test]
    fn single_clip_rejects_empty_or_invalid_ranges() {
        assert!(single_clip_args(&segment("00:20", "00:20"), false).is_err());
        assert!(single_clip_args(&segment("00:20", "00:10"), false).is_err());
        assert!(single_clip_args(&segment("-ss", "00:10"), false).is_err());
    }

    /// 4 s of test video at 25 fps plus a tone, with a keyframe only every
    /// 2 s, so cut points like 0.6 s fall mid-GOP. Needs ffmpeg on PATH.
    fn make_gop_source(dir: &Path) -> std::path::PathBuf {
        let source = dir.join("source.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=25:duration=4",
            ])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=4"])
            .args([
                "-c:v",
                "libx264",
                "-g",
                "50",
                "-keyint_min",
                "50",
                "-sc_threshold",
                "0",
            ])
            .args(["-c:a", "aac", "-shortest"])
            .arg(&source)
            .status()
            .expect("ffmpeg must be on PATH for this test");
        assert!(status.success());
        source
    }

    /// Duration in seconds of one stream (`v:0` / `a:0`), via ffprobe.
    fn stream_duration(path: &Path, stream: &str) -> f64 {
        let output = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", stream])
            .args(["-show_entries", "stream=duration", "-of", "csv=p=0"])
            .arg(path)
            .output()
            .expect("ffprobe must be on PATH for this test");
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap()
    }

    #[test]
    fn cut_video_keeps_only_the_selected_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let source = make_gop_source(dir.path());
        let output = dir.path().join("cut.mp4");
        let control = RunControl::default();
        let run_id = control.begin_run();

        cut_video(
            &source,
            &[
                segment("00:00.600", "00:01.600"),
                segment("00:02.500", "00:03.000"),
            ],
            &output,
            run_id,
            &control,
            |_| {},
        )
        .unwrap();

        for stream in ["v:0", "a:0"] {
            let duration = stream_duration(&output, stream);
            assert!(
                (duration - 1.5).abs() < 0.08,
                "{stream} duration {duration}"
            );
        }
    }

    /// Runs real ffmpeg (expected on PATH, like the silence tests): a clip cut
    /// between keyframes must start on a decodable keyframe and have the exact
    /// requested length.
    #[test]
    fn exported_clip_is_frame_accurate() {
        use std::process::Command;

        let dir = tempfile::tempdir().unwrap();
        let source = make_gop_source(dir.path());

        let out_dir = dir.path().join("clips");
        let control = RunControl::default();
        let run_id = control.begin_run();
        let clip = ClipSegment {
            segments: vec![segment("00:00.600", "00:01.600")],
            label: None,
            reason: None,
        };
        export_clips(
            &source,
            &[clip],
            &out_dir,
            false,
            run_id,
            &control,
            |_, _, _| {},
        )
        .unwrap();

        let clip_path = out_dir.join("clip_001.mp4");
        let probe = |args: &[&str]| {
            let output = Command::new("ffprobe")
                .args(["-v", "error", "-select_streams", "v:0"])
                .args(args)
                .arg(&clip_path)
                .output()
                .expect("ffprobe must be on PATH for this test");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };

        let duration: f64 = probe(&["-show_entries", "stream=duration", "-of", "csv=p=0"])
            .parse()
            .unwrap();
        assert!((duration - 1.0).abs() < 0.05, "clip duration {duration}");

        let first_packet_flags = probe(&[
            "-read_intervals",
            "%+#1",
            "-show_entries",
            "packet=flags",
            "-of",
            "csv=p=0",
        ]);
        assert!(
            first_packet_flags.starts_with('K'),
            "first packet flags {first_packet_flags}"
        );
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
