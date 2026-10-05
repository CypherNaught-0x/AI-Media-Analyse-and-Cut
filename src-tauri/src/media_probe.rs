//! What a media file contains: duration, and the first video and audio
//! stream. Built on ffmpeg itself rather than ffprobe, because the ffmpeg
//! build the app downloads on macOS ships without ffprobe.

use crate::ffmpeg::{run_ffmpeg, FfmpegTask};
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::{FfmpegEvent, StreamTypeSpecificData};
use regex::Regex;
use serde::Serialize;
use std::path::Path;
use std::sync::LazyLock;

#[derive(Debug, Clone, Default, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    /// Container duration in seconds, when ffmpeg reports one.
    pub duration_seconds: Option<f64>,
    pub video: Option<VideoStreamInfo>,
    pub audio: Option<AudioStreamInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct VideoStreamInfo {
    pub codec: String,
    /// Coded size, before rotation is applied.
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub pix_fmt: String,
    /// Clockwise rotation a player applies on display (0, 90, 180 or 270),
    /// e.g. 90 for portrait phone footage stored as landscape.
    pub rotation: u32,
}

impl VideoStreamInfo {
    /// Size as displayed, after rotation.
    pub fn display_size(&self) -> (u32, u32) {
        if self.rotation % 180 == 90 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioStreamInfo {
    pub codec: String,
    pub sample_rate: u32,
    /// Channel layout as ffmpeg names it, e.g. "mono", "stereo", "5.1".
    pub channels: String,
}

/// Probe `path`. Blocks while ffmpeg runs (reads headers only, decodes
/// nothing); call from a blocking context.
pub fn probe_media(path: &Path) -> Result<MediaInfo, String> {
    if !path.exists() {
        return Err(format!("Media file not found: {}", path.display()));
    }

    let mut command = FfmpegCommand::new();
    command
        .input(path.to_string_lossy())
        .args(["-t", "0", "-f", "null", "-"]);

    let mut parser = ProbeParser::default();
    run_ffmpeg(
        command,
        FfmpegTask {
            operation: "read media information",
            input: path,
            output: None,
            run: None,
        },
        |event| parser.observe(event),
    )?;
    Ok(parser.info)
}

static ROTATION: LazyLock<Regex> = LazyLock::new(|| {
    // ffmpeg 7+: "Display Matrix: rotation of -90.00 degrees"; 5/6:
    // "displaymatrix: rotation of ..."; older builds a "rotate : 90" tag.
    Regex::new(
        r"(?i)display ?matrix: rotation of (-?\d+(?:\.\d+)?) degrees|^\s*rotate\s*:\s*(-?\d+)",
    )
    .unwrap()
});

/// Collects [`MediaInfo`] from the events of a probe run.
#[derive(Default)]
struct ProbeParser {
    info: MediaInfo,
    /// Whether log lines currently belong to the selected video stream, so its
    /// side data (rotation) is attributed to it.
    in_selected_video: bool,
}

impl ProbeParser {
    fn observe(&mut self, event: &FfmpegEvent) {
        match event {
            FfmpegEvent::ParsedDuration(duration) if duration.input_index == 0 => {
                self.info.duration_seconds = Some(duration.duration);
            }
            FfmpegEvent::ParsedInputStream(stream) if stream.parent_index == 0 => {
                self.in_selected_video = false;
                match &stream.type_specific_data {
                    StreamTypeSpecificData::Video(video) if self.info.video.is_none() => {
                        self.info.video = Some(VideoStreamInfo {
                            codec: stream.format.clone(),
                            width: video.width,
                            height: video.height,
                            fps: video.fps,
                            pix_fmt: video.pix_fmt.clone(),
                            rotation: 0,
                        });
                        self.in_selected_video = true;
                    }
                    StreamTypeSpecificData::Audio(audio) if self.info.audio.is_none() => {
                        self.info.audio = Some(AudioStreamInfo {
                            codec: stream.format.clone(),
                            sample_rate: audio.sample_rate,
                            channels: audio.channels.clone(),
                        });
                    }
                    _ => {}
                }
            }
            FfmpegEvent::ParsedOutput(_) | FfmpegEvent::ParsedStreamMapping(_) => {
                self.in_selected_video = false;
            }
            FfmpegEvent::Log(_, line) if self.in_selected_video => {
                if let Some(rotation) = parse_rotation(line) {
                    if let Some(video) = &mut self.info.video {
                        video.rotation = rotation;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Clockwise display rotation from a side-data or tag line. The display
/// matrix states it counter-clockwise, so -90 means 90 degrees clockwise.
fn parse_rotation(line: &str) -> Option<u32> {
    let captures = ROTATION.captures(line)?;
    let clockwise = if let Some(matrix) = captures.get(1) {
        -matrix.as_str().parse::<f64>().ok()?
    } else {
        captures.get(2)?.as_str().parse::<f64>().ok()?
    };
    let quarter_turns = (clockwise / 90.0).round() as i64;
    Some((quarter_turns.rem_euclid(4) * 90) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn ffmpeg(args: &[&str]) {
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(args)
            .status()
            .expect("ffmpeg must be on PATH for this test");
        assert!(status.success());
    }

    #[test]
    fn reads_duration_video_and_audio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=25:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2:sample_rate=48000",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
            path.to_str().unwrap(),
        ]);

        let info = probe_media(&path).unwrap();
        let duration = info.duration_seconds.unwrap();
        assert!((duration - 2.0).abs() < 0.1, "duration {duration}");
        let video = info.video.unwrap();
        assert_eq!(
            (video.codec.as_str(), video.width, video.height),
            ("h264", 320, 240)
        );
        assert!((video.fps - 25.0).abs() < 0.01);
        assert_eq!(video.pix_fmt, "yuv420p");
        assert_eq!(video.rotation, 0);
        let audio = info.audio.unwrap();
        assert_eq!((audio.codec.as_str(), audio.sample_rate), ("aac", 48000));
    }

    #[test]
    fn reads_rotation_of_portrait_phone_footage() {
        let dir = tempfile::tempdir().unwrap();
        let landscape = dir.path().join("landscape.mp4");
        let rotated = dir.path().join("rotated.mp4");
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x180:rate=25:duration=1",
            "-c:v",
            "libx264",
            landscape.to_str().unwrap(),
        ]);
        // Tag the stream the way phones do: stored landscape, shown portrait.
        ffmpeg(&[
            "-display_rotation",
            "-90",
            "-i",
            landscape.to_str().unwrap(),
            "-c",
            "copy",
            rotated.to_str().unwrap(),
        ]);

        let video = probe_media(&rotated).unwrap().video.unwrap();
        assert_eq!(video.rotation, 90);
        assert_eq!(video.display_size(), (180, 320));
    }

    #[test]
    fn audio_only_files_have_no_video() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.m4a");
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "aac",
            path.to_str().unwrap(),
        ]);

        let info = probe_media(&path).unwrap();
        assert!(info.video.is_none());
        assert!(info.audio.is_some());
    }

    #[test]
    fn missing_files_are_an_error() {
        assert!(probe_media(Path::new("/definitely/missing.mp4")).is_err());
    }

    #[test]
    fn rotation_lines_map_to_clockwise_quarter_turns() {
        assert_eq!(
            parse_rotation("      displaymatrix: rotation of -90.00 degrees"),
            Some(90)
        );
        assert_eq!(
            parse_rotation("      displaymatrix: rotation of 90.00 degrees"),
            Some(270)
        );
        assert_eq!(
            parse_rotation("      displaymatrix: rotation of -180.00 degrees"),
            Some(180)
        );
        assert_eq!(
            parse_rotation("      Display Matrix: rotation of -90.00 degrees"),
            Some(90)
        );
        assert_eq!(parse_rotation("    rotate          : 90"), Some(90));
        assert_eq!(parse_rotation("    encoder         : Lavc"), None);
    }
}
