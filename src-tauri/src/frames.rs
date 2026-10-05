//! Decoded frames for analysis (shorts phase S2): a time range of a video,
//! at a reduced frame rate and width, as raw pixels streamed from ffmpeg.
//! ffmpeg applies the stream's rotation, so frames are display-oriented.

use crate::ffmpeg::{run_ffmpeg, FfmpegTask};
use crate::run_control::RunControl;
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PixelFormat {
    /// 3 bytes per pixel.
    Rgb24,
    /// 1 byte per pixel (luma); enough for motion and shot analysis.
    Gray,
}

impl PixelFormat {
    fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Rgb24 => "rgb24",
            Self::Gray => "gray",
        }
    }
}

pub(crate) struct FrameRequest<'a> {
    pub path: &'a Path,
    /// Source range in seconds.
    pub start: f64,
    pub end: f64,
    /// Frames per second to sample.
    pub fps: f64,
    /// Output width; height follows the aspect ratio (rounded to even).
    pub width: u32,
    pub format: PixelFormat,
}

#[derive(Debug, Clone)]
pub(crate) struct Frame {
    /// Position on the source timeline, in seconds.
    pub time: f64,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Decode `request`, handing each frame to `on_frame` as it arrives. Blocks;
/// run it on the blocking pool.
pub(crate) fn decode_frames(
    request: &FrameRequest<'_>,
    run: Option<(u64, &RunControl)>,
    mut on_frame: impl FnMut(Frame),
) -> Result<(), String> {
    if request.end <= request.start {
        return Err("The frame range is empty".to_string());
    }
    let mut command = FfmpegCommand::new();
    command
        .args(["-ss", &format!("{:.6}", request.start)])
        .input(request.path.to_string_lossy())
        .args(["-t", &format!("{:.6}", request.end - request.start)])
        .args(["-an", "-sn"])
        .filter(format!(
            "fps={},scale={}:-2:flags=bilinear",
            request.fps, request.width
        ))
        .format("rawvideo")
        .pix_fmt(request.format.ffmpeg_name())
        .pipe_stdout();

    run_ffmpeg(
        command,
        FfmpegTask {
            operation: "decode frames for analysis",
            input: request.path,
            output: None,
            run,
        },
        |event| {
            if let FfmpegEvent::OutputFrame(frame) = event {
                on_frame(Frame {
                    time: request.start + f64::from(frame.frame_num) / request.fps,
                    width: frame.width,
                    height: frame.height,
                    data: frame.data.clone(),
                });
            }
        },
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One second of solid colour per entry of `colours`, 320x180 at 25 fps.
    /// Needs ffmpeg on PATH.
    pub(crate) fn colour_video(dir: &Path, colours: &[&str]) -> std::path::PathBuf {
        let path = dir.join("colours.mp4");
        let n = colours.len();
        let inputs = (0..n).map(|i| format!("[c{i}]")).collect::<String>();
        let colours = colours
            .iter()
            .enumerate()
            .map(|(i, c)| format!("color=c={c}:s=320x180:r=25:d=1[c{i}];"))
            .collect::<String>();
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-filter_complex",
                &format!("{colours}{inputs}concat=n={n}:v=1:a=0,format=yuv420p[v]"),
            ])
            .args(["-map", "[v]", "-c:v", "libx264"])
            .arg(&path)
            .status()
            .expect("ffmpeg must be on PATH for this test");
        assert!(status.success());
        path
    }

    #[test]
    fn frames_arrive_at_the_requested_rate_size_and_times() {
        let dir = tempfile::tempdir().unwrap();
        let video = colour_video(dir.path(), &["red", "green", "blue", "white"]);
        let mut frames = Vec::new();
        decode_frames(
            &FrameRequest {
                path: &video,
                start: 1.0,
                end: 3.0,
                fps: 4.0,
                width: 160,
                format: PixelFormat::Rgb24,
            },
            None,
            |frame| frames.push(frame),
        )
        .unwrap();

        assert_eq!(frames.len(), 8);
        assert_eq!((frames[0].width, frames[0].height), (160, 90));
        assert_eq!(frames[0].data.len(), 160 * 90 * 3);
        assert_eq!(frames[0].time, 1.0);
        assert_eq!(frames[7].time, 2.75);
        // 1-2 s is green, 2-3 s blue (centre pixel).
        let centre = |frame: &Frame| {
            let i = ((45 * 160 + 80) * 3) as usize;
            (frame.data[i], frame.data[i + 1], frame.data[i + 2])
        };
        assert!(centre(&frames[1]).1 > 100 && centre(&frames[1]).2 < 60);
        assert!(centre(&frames[6]).2 > 200 && centre(&frames[6]).0 < 60);
    }

    #[test]
    fn grayscale_frames_have_one_byte_per_pixel() {
        let dir = tempfile::tempdir().unwrap();
        let video = colour_video(dir.path(), &["red", "green", "blue", "white"]);
        let mut sizes = Vec::new();
        decode_frames(
            &FrameRequest {
                path: &video,
                start: 0.0,
                end: 1.0,
                fps: 2.0,
                width: 64,
                format: PixelFormat::Gray,
            },
            None,
            |frame| sizes.push(frame.data.len()),
        )
        .unwrap();
        assert_eq!(sizes, [64 * 36, 64 * 36]);
    }
}
