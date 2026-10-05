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
use ffmpeg_sidecar::event::FfmpegEvent;
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

/// A `sendcmd` script setting the crop filter `target` (e.g. `crop@r0`) for
/// every output frame of a clip of `duration` seconds at `fps`. Only changes
/// are written.
pub(crate) fn sendcmd_script(
    keys: &[CropKey],
    source: (u32, u32),
    aspect: f64,
    fps: f64,
    duration: f64,
    target: &str,
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
            "{time:.4} {target} w {w}, {target} h {h}, {target} x {x}, {target} y {y};",
            w = crop.width,
            h = crop.height,
            x = crop.x,
            y = crop.y
        );
        previous = Some(crop);
    }
    script
}

/// How a stretch of the source is put into the vertical frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Framing {
    /// A crop following these keys (times relative to the stretch start).
    Follow(Vec<CropKey>),
    /// The whole picture, fitted to the width over a blurred, darkened copy
    /// of itself.
    Fit,
}

/// One stretch of the source in a vertical clip.
#[derive(Debug, Clone)]
pub(crate) struct VerticalRange {
    /// Source range (seconds).
    pub start: f64,
    pub end: f64,
    pub framing: Framing,
}

pub(crate) struct VerticalRender<'a> {
    pub input: &'a Path,
    /// Played in order and joined.
    pub ranges: &'a [VerticalRange],
    pub source_size: (u32, u32),
    pub fps: f64,
    pub has_audio: bool,
    /// Output size, e.g. 1080x1920.
    pub output_size: (u32, u32),
    pub output: &'a Path,
    pub quality: ExportQuality,
}

/// The video filter chain and `sendcmd` script applying the virtual camera.
/// The script is read from `<name>.cmd` and drives the crop `crop@<name>`.
fn camera_filter(
    keys: &[CropKey],
    source: (u32, u32),
    output: (u32, u32),
    fps: f64,
    duration: f64,
    name: &str,
) -> (String, String) {
    let target = format!("crop@{name}");
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
        let script = sendcmd_script(&scaled_keys, scaled, aspect, fps, duration, &target);
        let start = crop_at(&scaled_keys, 0.0, scaled, aspect);
        let filter = format!(
            "scale={sw}:{sh}:flags=lanczos,sendcmd=f={name}.cmd,\
             {target}=w={out_w}:h={out_h}:x={x}:y={y}:exact=1,setsar=1",
            sw = scaled.0,
            sh = scaled.1,
            x = start.x,
            y = start.y
        );
        return (filter, script);
    }

    let script = sendcmd_script(keys, source, aspect, fps, duration, &target);
    let filter = format!(
        "sendcmd=f={name}.cmd,{target}=w={w}:h={h}:x={x}:y={y}:exact=1,\
         scale={out_w}:{out_h}:flags=lanczos,setsar=1",
        w = first.width,
        h = first.height,
        x = first.x,
        y = first.y
    );
    (filter, script)
}

/// The `-filter_complex` graph for `render` (trims relative to the input
/// seek) and the `sendcmd` scripts it reads, by file name.
fn render_graph(render: &VerticalRender<'_>, seek: f64) -> (String, Vec<(String, String)>) {
    let mut graph = String::new();
    let mut scripts = Vec::new();
    let mut concat_inputs = String::new();
    let (out_w, out_h) = render.output_size;
    for (i, range) in render.ranges.iter().enumerate() {
        let (start, end) = (range.start - seek, range.end - seek);
        let trim = format!("[0:v]trim=start={start:.6}:end={end:.6},setpts=PTS-STARTPTS");
        match &range.framing {
            Framing::Follow(keys) => {
                let name = format!("r{i}");
                let (camera, script) = camera_filter(
                    keys,
                    render.source_size,
                    render.output_size,
                    render.fps,
                    range.end - range.start,
                    &name,
                );
                let _ = write!(graph, "{trim},{camera}[v{i}];");
                scripts.push((format!("{name}.cmd"), script));
            }
            Framing::Fit => {
                // The background is blurred at quarter size: much cheaper
                // than blurring 1080x1920, and it's a blur anyway.
                let (bg_w, bg_h) = ((out_w / 4) & !1, (out_h / 4) & !1);
                let _ = write!(
                    graph,
                    "{trim},split=2[bg{i}][fg{i}];\
                     [bg{i}]scale={bg_w}:{bg_h}:force_original_aspect_ratio=increase,\
                     crop={bg_w}:{bg_h},boxblur=8:2,scale={out_w}:{out_h},\
                     eq=brightness=-0.08[bgb{i}];\
                     [fg{i}]scale={out_w}:{out_h}:force_original_aspect_ratio=decrease:\
                     flags=lanczos[fgs{i}];\
                     [bgb{i}][fgs{i}]overlay=(W-w)/2:(H-h)/2,setsar=1[v{i}];"
                );
            }
        }
        concat_inputs.push_str(&format!("[v{i}]"));
        if render.has_audio {
            let _ = write!(
                graph,
                "[0:a]atrim=start={start:.6}:end={end:.6},asetpts=PTS-STARTPTS[a{i}];"
            );
            concat_inputs.push_str(&format!("[a{i}]"));
        }
    }
    let _ = write!(
        graph,
        "{concat_inputs}concat=n={}:v=1:a={}[v]{}",
        render.ranges.len(),
        u8::from(render.has_audio),
        if render.has_audio { "[a]" } else { "" }
    );
    (graph, scripts)
}

/// Render source ranges through the virtual camera into one clip. Blocks.
/// `on_progress` gets the output time written so far (seconds).
pub(crate) fn render_vertical(
    render: &VerticalRender<'_>,
    run: Option<(u64, &RunControl)>,
    mut on_progress: impl FnMut(f64),
) -> Result<(), String> {
    if render.ranges.is_empty() || render.ranges.iter().any(|r| r.end <= r.start) {
        return Err("A vertical clip needs non-empty source ranges".to_string());
    }
    let seek = render
        .ranges
        .iter()
        .map(|range| range.start)
        .fold(f64::INFINITY, f64::min);
    let (graph, scripts) = render_graph(render, seek);

    // sendcmd reads files; keep their paths free of characters that need
    // escaping in a filtergraph (e.g. ':' in Windows paths) by running in
    // their directory.
    let dir = tempfile::tempdir().map_err(|e| format!("Failed to create a temp folder: {e}"))?;
    for (name, script) in &scripts {
        std::fs::write(dir.path().join(name), script)
            .map_err(|e| format!("Failed to write the crop script: {e}"))?;
    }

    let mut command = FfmpegCommand::new();
    command
        .args(["-y", "-ss", &format!("{seek:.6}")])
        .input(render.input.to_string_lossy())
        .args(["-filter_complex", &graph, "-map", "[v]"]);
    if render.has_audio {
        command.args(["-map", "[a]"]);
    }
    command
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
        |event| {
            if let FfmpegEvent::Progress(progress) = event {
                if let Ok(time) = crate::time_utils::parse_timestamp_to_seconds_raw(&progress.time)
                {
                    on_progress(time);
                }
            }
        },
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
        let still = sendcmd_script(&[key(0.0, 640.0)], SOURCE, PORTRAIT, 30.0, 2.0, "crop");
        assert_eq!(still.lines().count(), 1);
        assert!(still.starts_with("0.0000 crop w 404, crop h 720, crop x 438, crop y 0;"));

        let pan = sendcmd_script(
            &[key(0.0, 300.0), key(1.0, 330.0)],
            SOURCE,
            PORTRAIT,
            30.0,
            1.0,
            "crop@r0",
        );
        assert!(pan.starts_with("0.0000 crop@r0 w 404, crop@r0 h 720,"));
        // 30 px over 30 frames: one change per frame.
        assert_eq!(pan.lines().count(), 30);
    }

    #[test]
    fn a_single_zoom_level_is_cropped_in_output_pixels() {
        // Slow pan on 720p: 0.5 source px per frame.
        let keys = [key(0.0, 500.0), key(2.0, 530.0)];
        let (filter, script) = camera_filter(&keys, SOURCE, (1080, 1920), 30.0, 2.0, "r0");
        assert!(filter.starts_with("scale=3412:1920:"), "{filter}");
        assert!(
            filter.contains("sendcmd=f=r0.cmd,crop@r0=w=1080:h=1920:"),
            "{filter}"
        );
        // 80 output px over 60 frames: the crop moves on (nearly) every frame
        // instead of every second one by 2.7 px.
        assert!(script.lines().count() >= 55, "{script}");
        assert!(script
            .lines()
            .all(|line| line.contains("crop@r0 w 1080, crop@r0 h 1920")));
    }

    #[test]
    fn zooming_crops_the_source_first() {
        let mut keys = [key(0.0, 640.0), key(2.0, 640.0)];
        keys[1].height = 360.0;
        let (filter, _) = camera_filter(&keys, SOURCE, (1080, 1920), 30.0, 2.0, "r1");
        assert!(
            filter.starts_with("sendcmd=f=r1.cmd,crop@r1=w=404:h=720:"),
            "{filter}"
        );
        assert!(filter.contains("scale=1080:1920:"), "{filter}");
    }

    /// Renders three ranges of a real (generated) clip out of order and
    /// checks the output is portrait, joined in order, and follows each
    /// range's framing. Needs ffmpeg on PATH.
    #[test]
    fn renders_ranges_in_order_following_their_keys() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("wide.mp4");
        // 0-2 s: left half red, right half blue. 2-4 s: green | yellow.
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=4"])
            .args([
                "-filter_complex",
                "color=c=red:s=640x360:r=25:d=2[a];color=c=blue:s=640x360:r=25:d=2[b];\
                 color=c=green:s=640x360:r=25:d=2[c];color=c=yellow:s=640x360:r=25:d=2[d];\
                 [a][b]hstack[ab];[c][d]hstack[cd];[ab][cd]concat=n=2,format=yuv420p[v]",
            ])
            // With an audio stream: the graph must carry it through.
            .args([
                "-map", "[v]", "-map", "0:a", "-c:v", "libx264", "-c:a", "aac",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());

        let key = |time: f64, center_x: f64| CropKey {
            time,
            center_x,
            center_y: 180.0,
            height: 360.0,
        };
        let ranges = [
            // Hold on the left (green) side.
            VerticalRange {
                start: 2.0,
                end: 4.0,
                framing: Framing::Follow(vec![key(0.0, 200.0)]),
            },
            // Pan from red to blue.
            VerticalRange {
                start: 0.0,
                end: 2.0,
                framing: Framing::Follow(vec![key(0.0, 200.0), key(2.0, 1100.0)]),
            },
            // The whole picture: red | blue across the middle.
            VerticalRange {
                start: 0.0,
                end: 1.0,
                framing: Framing::Fit,
            },
        ];
        let output = dir.path().join("vertical.mp4");
        let mut progress = Vec::new();
        render_vertical(
            &VerticalRender {
                input: &source,
                ranges: &ranges,
                source_size: (1280, 360),
                fps: 25.0,
                has_audio: true,
                output_size: (360, 640),
                output: &output,
                quality: ExportQuality::Draft,
            },
            None,
            |time| progress.push(time),
        )
        .unwrap();

        let info = crate::media_probe::probe_media(&output).unwrap();
        let video = info.video.unwrap();
        assert_eq!((video.width, video.height), (360, 640));
        assert!(info.audio.is_some());
        let duration = info.duration_seconds.unwrap();
        assert!((duration - 5.0).abs() < 0.15, "{duration}");
        assert!(progress.last().is_some_and(|&t| t > 3.0), "{progress:?}");
        let colour = |time: &str| {
            let out = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-ss", time, "-i"])
                .arg(&output)
                .args(["-frames:v", "1", "-vf", "scale=1:1", "-f", "rawvideo"])
                .args(["-pix_fmt", "rgb24", "-"])
                .output()
                .unwrap()
                .stdout;
            (out[0], out[1], out[2])
        };
        let (r, g, b) = colour("0.5");
        assert!(
            g > 100 && r < 80 && b < 80,
            "first range: green ({r},{g},{b})"
        );
        let (r, g, b) = colour("2.1");
        assert!(
            r > 150 && g < 80 && b < 80,
            "second range starts red ({r},{g},{b})"
        );
        let (r, g, b) = colour("3.9");
        assert!(
            b > 150 && r < 80 && g < 80,
            "and pans to blue ({r},{g},{b})"
        );
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
            let (filter, script) = camera_filter(&keys, (w, h), size, fps, duration, "r0");
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
                    ranges: &[VerticalRange {
                        start: 600.0,
                        end: 600.0 + duration,
                        framing: Framing::Follow(keys.to_vec()),
                    }],
                    source_size: (w, h),
                    fps,
                    has_audio: true,
                    output_size: size,
                    output: &output,
                    quality,
                },
                None,
                |_| {},
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
