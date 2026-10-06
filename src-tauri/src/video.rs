use crate::encoders::{encode_args, preferred_h264_encoder, ExportQuality};
use crate::ffmpeg::{run_ffmpeg, FfmpegTask};
use crate::media_probe::probe_media;
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

#[derive(Serialize, Deserialize, Debug, Clone, specta::Type)]
pub struct ClipSegment {
    pub segments: Vec<Segment>,
    pub label: Option<String>,
    pub reason: Option<String>,
}

pub fn cut_video<F>(
    input_path: &Path,
    segments: &[Segment],
    output_path: &Path,
    quality: ExportQuality,
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

    info!(
        "Starting cut_video: input={:?}, output={:?}, segments={}",
        input_path,
        output_path,
        segments.len()
    );

    let media = probe_media(input_path).map_err(|error| anyhow::anyhow!(error))?;
    let plan = plan_cut(segments, media.video.is_some(), media.audio.is_some())?;

    let mut command = FfmpegCommand::new();
    // Seek the input to the first kept range: ffmpeg then starts decoding
    // there instead of at 0:00, and stays frame-accurate because the output is
    // re-encoded.
    command
        .args(["-y", "-ss", &format!("{:.6}", plan.seek_seconds)])
        .input(input_path.to_str().unwrap())
        .args(["-filter_complex", &plan.filter])
        .args(
            plan.maps
                .iter()
                .flat_map(|label| ["-map".to_string(), label.clone()]),
        )
        .args(encode_args_for_output(
            output_path,
            media.video.is_some(),
            quality,
        ))
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

/// How to cut and join source ranges in one ffmpeg pass.
#[derive(Debug, PartialEq)]
struct CutPlan {
    /// Input seek (seconds); trims in `filter` are relative to it.
    seek_seconds: f64,
    filter: String,
    /// Output labels to `-map`.
    maps: Vec<String>,
}

/// Plan a cut that keeps `segments` (in the given order) from a source with
/// the given streams:
///
/// `-ss <seek> -i in -filter_complex
///  "[0:v]trim=start=..:end=..,setpts=PTS-STARTPTS[v0];[0:a]atrim=..[a0];...
///   [v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"`
fn plan_cut(segments: &[Segment], has_video: bool, has_audio: bool) -> Result<CutPlan> {
    if !has_video && !has_audio {
        return Err(anyhow::anyhow!("The source has no audio or video to cut"));
    }
    if segments.is_empty() {
        return Err(anyhow::anyhow!("Nothing to cut: no segments were given"));
    }

    // Parse timestamps to bare numbers so untrusted strings can't be injected
    // into the ffmpeg filtergraph.
    let ranges = segments
        .iter()
        .map(|segment| {
            let start = parse_timestamp_to_seconds_raw(&segment.start).map_err(|e| {
                anyhow::anyhow!("Invalid segment start timestamp '{}': {}", segment.start, e)
            })?;
            let end = parse_timestamp_to_seconds_raw(&segment.end).map_err(|e| {
                anyhow::anyhow!("Invalid segment end timestamp '{}': {}", segment.end, e)
            })?;
            if end <= start {
                return Err(anyhow::anyhow!(
                    "Segment end {} must be after its start {}",
                    segment.end,
                    segment.start
                ));
            }
            Ok((start.max(0.0), end))
        })
        .collect::<Result<Vec<_>>>()?;

    let seek = ranges
        .iter()
        .map(|(start, _)| *start)
        .fold(f64::INFINITY, f64::min);

    let mut filter = String::new();
    let mut concat_inputs = String::new();
    for (i, (start, end)) in ranges.iter().enumerate() {
        let (start, end) = (start - seek, end - seek);
        if has_video {
            filter.push_str(&format!(
                "[0:v]trim=start={start:.6}:end={end:.6},setpts=PTS-STARTPTS[v{i}];"
            ));
            concat_inputs.push_str(&format!("[v{i}]"));
        }
        if has_audio {
            filter.push_str(&format!(
                "[0:a]atrim=start={start:.6}:end={end:.6},asetpts=PTS-STARTPTS[a{i}];"
            ));
            concat_inputs.push_str(&format!("[a{i}]"));
        }
    }

    let mut maps = Vec::new();
    let mut outputs = String::new();
    if has_video {
        outputs.push_str("[v]");
        maps.push("[v]".to_string());
    }
    if has_audio {
        outputs.push_str("[a]");
        maps.push("[a]".to_string());
    }
    filter.push_str(&format!(
        "{concat_inputs}concat=n={}:v={}:a={}{outputs}",
        ranges.len(),
        u8::from(has_video),
        u8::from(has_audio)
    ));

    Ok(CutPlan {
        seek_seconds: seek,
        filter,
        maps,
    })
}

/// Encoder arguments for a cut written to `output`. Containers that take
/// H.264 get the preferred (hardware when available) encoder with explicit
/// quality settings; anything else (webm, avi, audio formats) keeps ffmpeg's
/// per-container defaults, which is what made those exports work before.
fn encode_args_for_output(output: &Path, has_video: bool, quality: ExportQuality) -> Vec<String> {
    let extension = output
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if has_video && matches!(extension.as_str(), "mp4" | "mov" | "m4v" | "mkv") {
        encode_args(preferred_h264_encoder(), quality)
    } else {
        Vec::new()
    }
}

#[allow(clippy::too_many_arguments)] // one per export option; see the Tauri command
pub fn export_clips<F>(
    input_path: &Path,
    segments: &[ClipSegment],
    output_dir: &Path,
    fast_mode: bool,
    quality: ExportQuality,
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
            let (input_args, output_args) = single_clip_args(s, fast_mode, quality)?;

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
                quality,
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
fn single_clip_args(
    segment: &Segment,
    fast_mode: bool,
    quality: ExportQuality,
) -> Result<(Vec<String>, Vec<String>)> {
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
        // Clips are always .mp4.
        output_args.extend(encode_args(preferred_h264_encoder(), quality));
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

/// `clip_001_Title_9x16.mp4`: the vertical export of clip `i`.
pub(crate) fn vertical_output_filename(i: usize, segment: &ClipSegment) -> String {
    build_clip_output_filename(i, segment).replace(".mp4", "_9x16.mp4")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start: &str, end: &str) -> Segment {
        Segment {
            start: start.to_string(),
            end: end.to_string(),
        }
    }

    #[test]
    fn plan_cut_seeks_to_the_first_range_and_trims_relative_to_it() {
        let plan = plan_cut(
            &[segment("00:10", "00:20"), segment("00:30", "00:40")],
            true,
            true,
        )
        .unwrap();
        assert_eq!(plan.seek_seconds, 10.0);
        assert_eq!(
            plan.filter,
            "[0:v]trim=start=0.000000:end=10.000000,setpts=PTS-STARTPTS[v0];\
             [0:a]atrim=start=0.000000:end=10.000000,asetpts=PTS-STARTPTS[a0];\
             [0:v]trim=start=20.000000:end=30.000000,setpts=PTS-STARTPTS[v1];\
             [0:a]atrim=start=20.000000:end=30.000000,asetpts=PTS-STARTPTS[a1];\
             [v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"
        );
        assert_eq!(plan.maps, ["[v]", "[a]"]);
    }

    #[test]
    fn plan_cut_keeps_spliced_order_and_seeks_to_the_earliest_range() {
        let plan = plan_cut(
            &[segment("00:30", "00:31"), segment("00:05", "00:06")],
            true,
            false,
        )
        .unwrap();
        assert_eq!(plan.seek_seconds, 5.0);
        assert!(plan
            .filter
            .starts_with("[0:v]trim=start=25.000000:end=26.000000"));
        assert!(plan.filter.ends_with("[v0][v1]concat=n=2:v=1:a=0[v]"));
        assert_eq!(plan.maps, ["[v]"]);
    }

    #[test]
    fn plan_cut_handles_audio_only_sources() {
        let plan = plan_cut(&[segment("00:01", "00:02")], false, true).unwrap();
        assert_eq!(
            plan.filter,
            "[0:a]atrim=start=0.000000:end=1.000000,asetpts=PTS-STARTPTS[a0];\
             [a0]concat=n=1:v=0:a=1[a]"
        );
        assert_eq!(plan.maps, ["[a]"]);
    }

    #[test]
    fn plan_cut_rejects_unusable_input() {
        assert!(plan_cut(&[], true, true).is_err());
        assert!(plan_cut(&[segment("00:02", "00:01")], true, true).is_err());
        assert!(plan_cut(&[segment("x", "00:01")], true, true).is_err());
        assert!(plan_cut(&[segment("00:00", "00:01")], false, false).is_err());
    }

    #[test]
    fn only_h264_containers_get_explicit_video_encoding() {
        assert!(
            encode_args_for_output(Path::new("/a/b_cut.mp4"), true, ExportQuality::Balanced)
                .windows(2)
                .any(|w| w == ["-pix_fmt", "yuv420p"])
        );
        assert!(
            encode_args_for_output(Path::new("/a/b_cut.MOV"), true, ExportQuality::Balanced).len()
                > 2
        );
        assert!(
            encode_args_for_output(Path::new("/a/b_cut.webm"), true, ExportQuality::Balanced)
                .is_empty()
        );
        assert!(
            encode_args_for_output(Path::new("/a/b_cut.mp4"), false, ExportQuality::Balanced)
                .is_empty()
        );
        assert!(
            encode_args_for_output(Path::new("/a/b_cut.mp3"), false, ExportQuality::Balanced)
                .is_empty()
        );
    }

    #[test]
    fn single_clip_seeks_on_the_input_and_reencodes_by_default() {
        let (input, output) = single_clip_args(
            &segment("01:05.250", "01:15.750"),
            false,
            ExportQuality::Balanced,
        )
        .unwrap();
        assert_eq!(input, ["-y", "-ss", "65.250"]);
        assert_eq!(&output[..2], ["-t", "10.500"]);
        assert!(output
            .windows(2)
            .any(|w| w == ["-c:v", preferred_h264_encoder().ffmpeg_name()]));
        assert!(output.windows(2).any(|w| w == ["-pix_fmt", "yuv420p"]));
        assert!(!output.iter().any(|a| a == "copy"));
    }

    #[test]
    fn single_clip_fast_mode_stream_copies() {
        let (input, output) =
            single_clip_args(&segment("00:10", "00:20"), true, ExportQuality::Balanced).unwrap();
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
        assert!(
            single_clip_args(&segment("00:20", "00:20"), false, ExportQuality::Balanced).is_err()
        );
        assert!(
            single_clip_args(&segment("00:20", "00:10"), false, ExportQuality::Balanced).is_err()
        );
        assert!(
            single_clip_args(&segment("-ss", "00:10"), false, ExportQuality::Balanced).is_err()
        );
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
            ExportQuality::Balanced,
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

    /// 4 s of video, one solid colour per second (red, green, blue, white),
    /// with a keyframe only every 2 s, plus a tone.
    fn make_colour_source(dir: &Path) -> std::path::PathBuf {
        let source = dir.join("colours.mp4");
        let colours = ["red", "green", "blue", "white"]
            .iter()
            .enumerate()
            .map(|(i, c)| format!("color=c={c}:s=64x64:r=25:d=1[c{i}];"))
            .collect::<String>();
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-filter_complex",
                &format!("{colours}[c0][c1][c2][c3]concat=n=4:v=1:a=0,format=yuv420p[v]"),
            ])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=4"])
            .args(["-map", "[v]", "-map", "0:a"])
            .args(["-c:v", "libx264", "-g", "50", "-keyint_min", "50"])
            .args(["-sc_threshold", "0", "-c:a", "aac", "-shortest"])
            .arg(&source)
            .status()
            .expect("ffmpeg must be on PATH for this test");
        assert!(status.success());
        source
    }

    /// Average colour (r, g, b) of the frame at `seconds`.
    fn colour_at(path: &Path, seconds: f64) -> (u8, u8, u8) {
        let output = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error"])
            .args(["-ss", &seconds.to_string()])
            .arg("-i")
            .arg(path)
            .args(["-frames:v", "1", "-vf", "scale=1:1", "-f", "rawvideo"])
            .args(["-pix_fmt", "rgb24", "-"])
            .output()
            .expect("ffmpeg must be on PATH for this test");
        let rgb = output.stdout;
        assert_eq!(rgb.len(), 3, "expected one rgb pixel");
        (rgb[0], rgb[1], rgb[2])
    }

    fn is_close(actual: (u8, u8, u8), expected: (u8, u8, u8)) -> bool {
        let d = |a: u8, b: u8| (a as i16 - b as i16).abs();
        d(actual.0, expected.0) < 40 && d(actual.1, expected.1) < 40 && d(actual.2, expected.2) < 40
    }

    /// The input seek must not shift what each range shows: cutting
    /// [1.2, 1.8] (green) then [2.2, 2.8] (blue) mid-GOP must play green, then
    /// blue.
    #[test]
    fn cut_video_ranges_show_the_right_source_content() {
        let dir = tempfile::tempdir().unwrap();
        let source = make_colour_source(dir.path());
        let output = dir.path().join("cut.mp4");
        let control = RunControl::default();
        let run_id = control.begin_run();

        cut_video(
            &source,
            &[
                segment("00:01.200", "00:01.800"),
                segment("00:02.200", "00:02.800"),
            ],
            &output,
            ExportQuality::Balanced,
            run_id,
            &control,
            |_| {},
        )
        .unwrap();

        let green = (0, 128, 0);
        let blue = (0, 0, 255);
        assert!(
            is_close(colour_at(&output, 0.3), green),
            "{:?}",
            colour_at(&output, 0.3)
        );
        assert!(
            is_close(colour_at(&output, 0.9), blue),
            "{:?}",
            colour_at(&output, 0.9)
        );
        let duration = stream_duration(&output, "v:0");
        assert!((duration - 1.2).abs() < 0.08, "duration {duration}");
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
            ExportQuality::Balanced,
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
