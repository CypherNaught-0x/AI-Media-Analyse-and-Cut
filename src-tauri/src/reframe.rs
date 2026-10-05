//! Vertical reframing (shorts phase S2): a virtual camera, given as crop
//! keyframes over the source, rendered to a 9:16 video.
//!
//! The crop is applied by ffmpeg's `crop` filter, driven per output frame by a
//! `sendcmd` script. `crop` positions are whole pixels, so with one zoom level
//! for the whole clip (the usual case: the camera holds and pans) the source
//! is scaled first and cropped in output pixels. A slow pan then moves in
//! 1-pixel steps instead of jumping by the upscale factor (2.7 output pixels
//! for 720p → 1080x1920), at the same speed: decoding and encoding dominate
//! (`profiling::sendcmd_crop_render`). Clips that zoom crop the source first.

use crate::encoders::{encode_args, preferred_h264_encoder, ExportQuality};
use crate::ffmpeg::{run_ffmpeg, FfmpegTask};
use crate::run_control::RunControl;
use ffmpeg_sidecar::command::FfmpegCommand;
use std::fmt::Write as _;
use std::path::Path;

/// Where the virtual camera looks at `time` (seconds from the clip start):
/// the crop centre and height in source pixels. Width follows the output
/// aspect ratio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CropKey {
    pub time: f64,
    pub center_x: f64,
    pub center_y: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The crop at `time`, interpolated linearly between keys and clamped to the
/// source frame. Sizes are even (yuv420p needs that).
pub(crate) fn crop_at(keys: &[CropKey], time: f64, source: (u32, u32), aspect: f64) -> CropRect {
    let key = match keys.iter().position(|key| key.time > time) {
        None => *keys.last().expect("at least one crop key"),
        Some(0) => keys[0],
        Some(next) => {
            let (a, b) = (keys[next - 1], keys[next]);
            let t = (time - a.time) / (b.time - a.time);
            let lerp = |x: f64, y: f64| x + (y - x) * t;
            CropKey {
                time,
                center_x: lerp(a.center_x, b.center_x),
                center_y: lerp(a.center_y, b.center_y),
                height: lerp(a.height, b.height),
            }
        }
    };

    let (source_width, source_height) = (f64::from(source.0), f64::from(source.1));
    let mut height = key.height.min(source_height);
    let mut width = height * aspect;
    if width > source_width {
        width = source_width;
        height = width / aspect;
    }
    let even = |value: f64| (value as u32) & !1;
    let (width, height) = (even(width).max(2), even(height).max(2));
    let x = (key.center_x - f64::from(width) / 2.0)
        .clamp(0.0, source_width - f64::from(width))
        .round() as u32;
    let y = (key.center_y - f64::from(height) / 2.0)
        .clamp(0.0, source_height - f64::from(height))
        .round() as u32;
    CropRect {
        x,
        y,
        width,
        height,
    }
}

/// A `sendcmd` script setting the crop for every output frame of a clip of
/// `duration` seconds at `fps`. Only changes are written.
pub(crate) fn sendcmd_script(
    keys: &[CropKey],
    source: (u32, u32),
    aspect: f64,
    fps: f64,
    duration: f64,
) -> String {
    let mut script = String::new();
    let mut previous: Option<CropRect> = None;
    let frames = (duration * fps).ceil() as u64;
    for frame in 0..frames {
        let time = frame as f64 / fps;
        let crop = crop_at(keys, time, source, aspect);
        if previous == Some(crop) {
            continue;
        }
        let _ = writeln!(
            script,
            "{time:.4} crop w {w}, crop h {h}, crop x {x}, crop y {y};",
            w = crop.width,
            h = crop.height,
            x = crop.x,
            y = crop.y
        );
        previous = Some(crop);
    }
    script
}

pub(crate) struct VerticalRender<'a> {
    pub input: &'a Path,
    /// Source range (seconds).
    pub start: f64,
    pub end: f64,
    pub source_size: (u32, u32),
    pub fps: f64,
    pub keys: &'a [CropKey],
    /// Output size, e.g. 1080x1920.
    pub output_size: (u32, u32),
    pub output: &'a Path,
    pub quality: ExportQuality,
}

/// The video filter and `sendcmd` script applying the virtual camera.
fn camera_filter(
    keys: &[CropKey],
    source: (u32, u32),
    output: (u32, u32),
    fps: f64,
    duration: f64,
) -> (String, String) {
    let (out_w, out_h) = output;
    let aspect = f64::from(out_w) / f64::from(out_h);
    let first = crop_at(keys, 0.0, source, aspect);
    let one_zoom = keys.iter().all(|key| {
        let crop = crop_at(keys, key.time, source, aspect);
        (crop.width, crop.height) == (first.width, first.height)
    });

    if one_zoom {
        // Scale so the crop is exactly the output size, then crop in output
        // pixels.
        let factor = f64::from(out_h) / f64::from(first.height);
        let even = |value: f64| ((value.round() as u32) & !1).max(2);
        let scaled = (
            even(f64::from(source.0) * factor).max(out_w),
            even(f64::from(source.1) * factor).max(out_h),
        );
        let scaled_keys: Vec<CropKey> = keys
            .iter()
            .map(|key| CropKey {
                time: key.time,
                center_x: key.center_x * factor,
                center_y: key.center_y * factor,
                height: f64::from(out_h),
            })
            .collect();
        let script = sendcmd_script(&scaled_keys, scaled, aspect, fps, duration);
        let start = crop_at(&scaled_keys, 0.0, scaled, aspect);
        let filter = format!(
            "scale={sw}:{sh}:flags=lanczos,sendcmd=f=crop.cmd,\
             crop=w={out_w}:h={out_h}:x={x}:y={y}:exact=1,setsar=1",
            sw = scaled.0,
            sh = scaled.1,
            x = start.x,
            y = start.y
        );
        return (filter, script);
    }

    let script = sendcmd_script(keys, source, aspect, fps, duration);
    let filter = format!(
        "sendcmd=f=crop.cmd,crop=w={w}:h={h}:x={x}:y={y}:exact=1,\
         scale={out_w}:{out_h}:flags=lanczos,setsar=1",
        w = first.width,
        h = first.height,
        x = first.x,
        y = first.y
    );
    (filter, script)
}

/// Render one source range through the virtual camera. Blocks.
pub(crate) fn render_vertical(
    render: &VerticalRender<'_>,
    run: Option<(u64, &RunControl)>,
) -> Result<(), String> {
    let duration = render.end - render.start;
    let (filter, script) = camera_filter(
        render.keys,
        render.source_size,
        render.output_size,
        render.fps,
        duration,
    );

    // sendcmd reads a file; keep its path free of characters that need
    // escaping in a filtergraph (e.g. ':' in Windows paths) by running in its
    // directory.
    let dir = tempfile::tempdir().map_err(|e| format!("Failed to create a temp folder: {e}"))?;
    std::fs::write(dir.path().join("crop.cmd"), script)
        .map_err(|e| format!("Failed to write the crop script: {e}"))?;

    let mut command = FfmpegCommand::new();
    command
        .args(["-y", "-ss", &format!("{:.6}", render.start)])
        .input(render.input.to_string_lossy())
        .args(["-t", &format!("{duration:.6}")])
        // `-filter:v`, not ffmpeg-sidecar's `.filter()`: a bare `-filter`
        // also applies to the audio stream and fails.
        .arg("-filter:v")
        .arg(filter)
        .args(encode_args(preferred_h264_encoder(), render.quality))
        .output(render.output.to_string_lossy());
    command.as_inner_mut().current_dir(dir.path());

    run_ffmpeg(
        command,
        FfmpegTask {
            operation: "render the vertical clip",
            input: render.input,
            output: Some(render.output),
            run,
        },
        |_| {},
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(time: f64, center_x: f64) -> CropKey {
        CropKey {
            time,
            center_x,
            center_y: 360.0,
            height: 720.0,
        }
    }

    const SOURCE: (u32, u32) = (1280, 720);
    const PORTRAIT: f64 = 9.0 / 16.0;

    #[test]
    fn crops_are_interpolated_clamped_and_even() {
        let keys = [key(0.0, 300.0), key(2.0, 900.0)];
        let start = crop_at(&keys, 0.0, SOURCE, PORTRAIT);
        assert_eq!((start.width, start.height), (404, 720));
        assert_eq!(start.x, 98);
        assert_eq!(crop_at(&keys, 1.0, SOURCE, PORTRAIT).x, 398);
        // Past the last key: hold it. Near the edge: clamp inside the frame.
        assert_eq!(crop_at(&keys, 5.0, SOURCE, PORTRAIT).x, 698);
        let edge = crop_at(&[key(0.0, 1270.0)], 0.0, SOURCE, PORTRAIT);
        assert_eq!(edge.x + edge.width, 1280);
    }

    #[test]
    fn script_only_lists_changes() {
        let still = sendcmd_script(&[key(0.0, 640.0)], SOURCE, PORTRAIT, 30.0, 2.0);
        assert_eq!(still.lines().count(), 1);
        assert!(still.starts_with("0.0000 crop w 404, crop h 720, crop x 438, crop y 0;"));

        let pan = sendcmd_script(
            &[key(0.0, 300.0), key(1.0, 330.0)],
            SOURCE,
            PORTRAIT,
            30.0,
            1.0,
        );
        // 30 px over 30 frames: one change per frame.
        assert_eq!(pan.lines().count(), 30);
    }

    #[test]
    fn a_single_zoom_level_is_cropped_in_output_pixels() {
        // Slow pan on 720p: 0.5 source px per frame.
        let keys = [key(0.0, 500.0), key(2.0, 530.0)];
        let (filter, script) = camera_filter(&keys, SOURCE, (1080, 1920), 30.0, 2.0);
        assert!(filter.starts_with("scale=3412:1920:"), "{filter}");
        assert!(filter.contains("crop=w=1080:h=1920:"), "{filter}");
        // 80 output px over 60 frames: the crop moves on (nearly) every frame
        // instead of every second one by 2.7 px.
        assert!(script.lines().count() >= 55, "{script}");
        assert!(script
            .lines()
            .all(|line| line.contains("crop w 1080, crop h 1920")));
    }

    #[test]
    fn zooming_crops_the_source_first() {
        let mut keys = [key(0.0, 640.0), key(2.0, 640.0)];
        keys[1].height = 360.0;
        let (filter, _) = camera_filter(&keys, SOURCE, (1080, 1920), 30.0, 2.0);
        assert!(
            filter.starts_with("sendcmd=f=crop.cmd,crop=w=404:h=720:"),
            "{filter}"
        );
        assert!(filter.contains("scale=1080:1920:"), "{filter}");
    }

    /// Renders a real (generated) clip and checks the output is portrait and
    /// shows the panned-to side of the source. Needs ffmpeg on PATH.
    #[test]
    fn renders_a_portrait_clip_following_the_keys() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("wide.mp4");
        // Left half red, right half blue.
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
            .args(["-i", "color=c=red:s=640x360:r=25:d=2"])
            .args(["-f", "lavfi", "-i", "color=c=blue:s=640x360:r=25:d=2"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
            // With an audio stream: the video filter must not touch it.
            .args(["-filter_complex", "[0][1]hstack,format=yuv420p[v]"])
            .args([
                "-map", "[v]", "-map", "2:a", "-c:v", "libx264", "-c:a", "aac",
            ])
            .arg("-shortest")
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());

        let output = dir.path().join("vertical.mp4");
        let keys = [
            CropKey {
                time: 0.0,
                center_x: 200.0,
                center_y: 180.0,
                height: 360.0,
            },
            CropKey {
                time: 2.0,
                center_x: 1100.0,
                center_y: 180.0,
                height: 360.0,
            },
        ];
        render_vertical(
            &VerticalRender {
                input: &source,
                start: 0.0,
                end: 2.0,
                source_size: (1280, 360),
                fps: 25.0,
                keys: &keys,
                output_size: (360, 640),
                output: &output,
                quality: ExportQuality::Draft,
            },
            None,
        )
        .unwrap();

        let info = crate::media_probe::probe_media(&output).unwrap();
        let video = info.video.unwrap();
        assert_eq!((video.width, video.height), (360, 640));
        let colour = |time: &str| {
            let out = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-ss", time, "-i"])
                .arg(&output)
                .args(["-frames:v", "1", "-vf", "scale=1:1", "-f", "rawvideo"])
                .args(["-pix_fmt", "rgb24", "-"])
                .output()
                .unwrap()
                .stdout;
            (out[0], out[2])
        };
        let (red, blue) = colour("0.1");
        assert!(red > 150 && blue < 80, "starts on the red side");
        let (red, blue) = colour("1.9");
        assert!(blue > 150 && red < 80, "pans to the blue side");
    }
}

#[cfg(test)]
mod profiling {
    use super::*;
    use std::time::Instant;

    /// Times a 20 s vertical render of a real source with a slow pan, and
    /// reports how often the integer crop steps. Point
    /// `SHORTS_PROFILE_SOURCE` at a video:
    /// `cargo test --release --lib sendcmd_crop_render -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn sendcmd_crop_render() {
        let Some(source) = std::env::var_os("SHORTS_PROFILE_SOURCE") else {
            eprintln!("SHORTS_PROFILE_SOURCE not set; skipping");
            return;
        };
        let source = std::path::PathBuf::from(source);
        let video = crate::media_probe::probe_media(&source)
            .unwrap()
            .video
            .unwrap();
        let (w, h) = video.display_size();
        let fps = f64::from(video.fps);
        let duration = 20.0;
        // Slow pan across a third of the frame, the hardest case for integer
        // crop positions.
        let keys = [
            CropKey {
                time: 0.0,
                center_x: f64::from(w) * 0.35,
                center_y: f64::from(h) / 2.0,
                height: f64::from(h),
            },
            CropKey {
                time: duration,
                center_x: f64::from(w) * 0.65,
                center_y: f64::from(h) / 2.0,
                height: f64::from(h),
            },
        ];
        let frames = (duration * fps) as usize;
        for size in [(720, 1280), (1080, 1920)] {
            let (filter, script) = camera_filter(&keys, (w, h), size, fps, duration);
            println!(
                "{}: {} crop changes over {frames} frames",
                filter.split(',').next().unwrap_or_default(),
                script.lines().count()
            );
        }
        println!(
            "source {w}x{h} @ {fps} fps, pan {:.2} source px/frame",
            f64::from(w) * 0.3 / (duration * fps)
        );

        let dir = tempfile::tempdir().unwrap();
        for (size, quality) in [
            ((1080, 1920), ExportQuality::Balanced),
            ((720, 1280), ExportQuality::Balanced),
        ] {
            let output = dir.path().join(format!("vertical_{}.mp4", size.0));
            let started = Instant::now();
            render_vertical(
                &VerticalRender {
                    input: &source,
                    start: 600.0,
                    end: 600.0 + duration,
                    source_size: (w, h),
                    fps,
                    keys: &keys,
                    output_size: size,
                    output: &output,
                    quality,
                },
                None,
            )
            .unwrap();
            let elapsed = started.elapsed().as_secs_f64();
            println!(
                "{}x{}: {elapsed:.1} s for {duration} s ({:.1}x realtime) with {}",
                size.0,
                size.1,
                duration / elapsed,
                preferred_h264_encoder().ffmpeg_name()
            );
            if let Some(keep) = std::env::var_os("SHORTS_PROFILE_KEEP") {
                std::fs::copy(
                    &output,
                    std::path::Path::new(&keep).join(format!("sendcmd_{}.mp4", size.0)),
                )
                .unwrap();
            }
        }
    }
}
