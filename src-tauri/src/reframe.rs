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

use crate::captions::{write_overlay, CaptionStyle, TimedWord};
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

/// How a stretch joins the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Transition {
    /// A plain cut.
    #[default]
    Cut,
    /// A fast slide with horizontal motion blur (a whip pan): for jumps to
    /// another moment.
    Whip,
    /// A short crossfade.
    Fade,
    /// Interpolated frames between the two sides of a jump cut
    /// (`VerticalRange::morph`); the audio is a plain cut.
    Morph,
}

impl Transition {
    /// How long the two stretches overlap (seconds).
    pub(crate) fn seconds(self) -> f64 {
        match self {
            Transition::Cut | Transition::Morph => 0.0,
            Transition::Whip => 0.24,
            Transition::Fade => 0.3,
        }
    }
}

/// One stretch of the source in a vertical clip.
#[derive(Debug, Clone)]
pub(crate) struct VerticalRange {
    /// Source range (seconds).
    pub start: f64,
    pub end: f64,
    pub framing: Framing,
    /// Punch-in on top of the framing (1.0 = none), e.g. 1.12 on every
    /// other jump cut.
    pub zoom: f64,
    /// How this stretch joins the previous one.
    pub transition: Transition,
    /// For `Transition::Morph`: the in-between frames (RGB, cropped to the
    /// framing at the cut), played over the cut.
    pub morph: Option<Vec<crate::frames::Frame>>,
}

impl VerticalRange {
    pub(crate) fn new(start: f64, end: f64, framing: Framing) -> Self {
        Self {
            start,
            end,
            framing,
            zoom: 1.0,
            transition: Transition::Cut,
            morph: None,
        }
    }
}

/// The transition actually rendered into `ranges[index]`: one that would
/// eat more than half of either stretch becomes a cut.
fn effective_transition(ranges: &[VerticalRange], index: usize) -> Transition {
    let transition = ranges[index].transition;
    if transition == Transition::Morph {
        // Each side gives up half the morph (a few frames); keep it to
        // stretches that remain clearly longer than that.
        let fits = |range: &VerticalRange| range.end - range.start > 0.5;
        return if index > 0
            && ranges[index].morph.as_ref().is_some_and(|f| !f.is_empty())
            && fits(&ranges[index])
            && fits(&ranges[index - 1])
        {
            Transition::Morph
        } else {
            Transition::Cut
        };
    }
    let seconds = transition.seconds();
    let long_enough = |range: &VerticalRange| range.end - range.start >= 2.0 * seconds;
    if index == 0 || !long_enough(&ranges[index]) || !long_enough(&ranges[index - 1]) {
        Transition::Cut
    } else {
        transition
    }
}

/// Where each stretch starts on the output timeline (transitions overlap
/// stretches, so the output is shorter than their sum).
pub(crate) fn output_starts(ranges: &[VerticalRange]) -> Vec<f64> {
    let mut starts = Vec::with_capacity(ranges.len());
    let mut time = 0.0;
    for (index, range) in ranges.iter().enumerate() {
        time -= effective_transition(ranges, index).seconds();
        starts.push(time.max(0.0));
        time += range.end - range.start;
    }
    starts
}

/// The output duration of `ranges`.
pub(crate) fn output_duration(ranges: &[VerticalRange]) -> f64 {
    match (output_starts(ranges).last(), ranges.last()) {
        (Some(start), Some(range)) => start + range.end - range.start,
        _ => 0.0,
    }
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
    /// Burned-in captions: words on the output timeline, and their style.
    pub captions: Option<(&'a [TimedWord], CaptionStyle)>,
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

/// Integrated loudness of the delivered audio (LUFS).
const LOUDNESS: f64 = -14.0;

/// Audio fades at hard cuts (seconds): enough to avoid clicks, too short to
/// hear.
const CUT_FADE: f64 = 0.008;

/// The `-filter_complex` graph for `render` (trims relative to the input
/// seek) and the `sendcmd` scripts it reads, by file name.
/// `caption_y`: top of the caption band if captions are laid over (input 1).
/// `morph_inputs[i]`: the input holding the morph frames into stretch `i`.
fn render_graph(
    render: &VerticalRender<'_>,
    seek: f64,
    caption_y: Option<u32>,
    morph_inputs: &[Option<usize>],
) -> (String, Vec<(String, String)>) {
    let mut graph = String::new();
    let mut scripts = Vec::new();
    let (out_w, out_h) = render.output_size;
    let ranges = render.ranges;
    let transitions: Vec<Transition> = (0..ranges.len())
        .map(|index| effective_transition(ranges, index))
        .collect();
    // A morph replaces the last frames before the cut and the first ones
    // after it; the audio isn't touched, so sync holds.
    let morph_half = |i: usize| match (
        transitions.get(i),
        &ranges.get(i).and_then(|r| r.morph.as_ref()),
    ) {
        (Some(Transition::Morph), Some(frames)) => frames.len() as f64 / render.fps / 2.0,
        _ => 0.0,
    };
    for (i, range) in ranges.iter().enumerate() {
        let head = morph_half(i);
        let tail = morph_half(i + 1);
        let (start, end) = (range.start + head - seek, range.end - tail - seek);
        let duration = range.end - range.start - head - tail;
        let audio_duration = range.end - range.start;
        let (audio_start, audio_end) = (range.start - seek, range.end - seek);
        let trim = format!("[0:v]trim=start={start:.6}:end={end:.6},setpts=PTS-STARTPTS");
        // Same frame rate, time base and format for every stretch, as xfade
        // requires.
        let finish = format!(
            "setsar=1,fps={fps},settb=AVTB,format=yuv420p[v{i}];",
            fps = render.fps
        );
        let zoom = range.zoom.max(1.0);
        match &range.framing {
            Framing::Follow(keys) => {
                let name = format!("r{i}");
                // A punch-in is a tighter crop: sharper than scaling up.
                // Key times are relative to the stretch, which a morph may
                // start a little later.
                let keys: Vec<CropKey> = keys
                    .iter()
                    .map(|key| CropKey {
                        time: key.time - head,
                        height: key.height / zoom,
                        ..*key
                    })
                    .collect();
                let (camera, script) = camera_filter(
                    &keys,
                    render.source_size,
                    render.output_size,
                    render.fps,
                    duration,
                    &name,
                );
                let _ = write!(graph, "{trim},{camera},{finish}");
                scripts.push((format!("{name}.cmd"), script));
            }
            Framing::Fit => {
                // The background is blurred at quarter size: much cheaper
                // than blurring 1080x1920, and it's a blur anyway.
                let (bg_w, bg_h) = ((out_w / 4) & !1, (out_h / 4) & !1);
                let punch = if zoom > 1.0 {
                    let (w, h) = (
                        (f64::from(out_w) * zoom).round() as u32 & !1,
                        (f64::from(out_h) * zoom).round() as u32 & !1,
                    );
                    format!(",scale={w}:{h},crop={out_w}:{out_h}")
                } else {
                    String::new()
                };
                let _ = write!(
                    graph,
                    "{trim},split=2[bg{i}][fg{i}];\
                     [bg{i}]scale={bg_w}:{bg_h}:force_original_aspect_ratio=increase,\
                     crop={bg_w}:{bg_h},boxblur=8:2,scale={out_w}:{out_h},\
                     eq=brightness=-0.08[bgb{i}];\
                     [fg{i}]scale={out_w}:{out_h}:force_original_aspect_ratio=decrease:\
                     flags=lanczos[fgs{i}];\
                     [bgb{i}][fgs{i}]overlay=(W-w)/2:(H-h)/2{punch},{finish}"
                );
            }
        }
        if render.has_audio {
            // Short fades where a hard cut (or a morph, which is one for the
            // audio) meets this stretch, against clicks.
            let cut = |t: &Transition| matches!(t, Transition::Cut | Transition::Morph);
            let mut fades = String::new();
            if i > 0 && cut(&transitions[i]) {
                let _ = write!(fades, ",afade=t=in:d={CUT_FADE}");
            }
            if i + 1 < ranges.len() && cut(&transitions[i + 1]) {
                let _ = write!(
                    fades,
                    ",afade=t=out:st={:.6}:d={CUT_FADE}",
                    (audio_duration - CUT_FADE).max(0.0)
                );
            }
            let _ = write!(
                graph,
                "[0:a]atrim=start={audio_start:.6}:end={audio_end:.6},asetpts=PTS-STARTPTS{fades}[a{i}];"
            );
        }
        if let Some(input) = morph_inputs.get(i).copied().flatten() {
            // Exactly the interpolated frames: the list repeats the last one.
            let count = range.morph.as_ref().map_or(0, Vec::len);
            let _ = write!(
                graph,
                "[{input}:v]scale={out_w}:{out_h}:flags=lanczos,setsar=1,fps={fps},\
                 trim=end_frame={count},settb=AVTB,format=yuv420p[m{i}];",
                fps = render.fps
            );
        }
    }

    // Join the stretches in order: runs of cuts with concat, transitions
    // with xfade (video) and acrossfade (audio).
    let video_out = if caption_y.is_some() { "vc" } else { "v" };
    let mut current = "v0".to_string();
    let mut current_audio = "a0".to_string();
    // Output length so far (the audio's: morphs don't change it).
    let mut length = ranges[0].end - ranges[0].start;
    for i in 1..ranges.len() {
        let duration = ranges[i].end - ranges[i].start;
        let last = i + 1 == ranges.len();
        let joined = if last {
            video_out.to_string()
        } else {
            format!("j{i}")
        };
        let joined_audio = if last {
            "aj".to_string()
        } else {
            format!("ja{i}")
        };
        let seconds = transitions[i].seconds();
        let offset = (length - seconds).max(0.0);
        match transitions[i] {
            Transition::Cut => {
                let _ = write!(graph, "[{current}][v{i}]concat=n=2:v=1:a=0[{joined}];");
            }
            Transition::Morph if morph_inputs.get(i).copied().flatten().is_some() => {
                let _ = write!(
                    graph,
                    "[{current}][m{i}][v{i}]concat=n=3:v=1:a=0[{joined}];"
                );
            }
            Transition::Morph => {
                let _ = write!(graph, "[{current}][v{i}]concat=n=2:v=1:a=0[{joined}];");
            }
            Transition::Whip => {
                let _ = write!(
                    graph,
                    "[{current}][v{i}]xfade=transition=slideleft:duration={seconds}:\
                     offset={offset:.6},dblur=angle=0:radius=60:\
                     enable='between(t,{offset:.6},{:.6})'[{joined}];",
                    offset + seconds
                );
            }
            Transition::Fade => {
                let _ = write!(
                    graph,
                    "[{current}][v{i}]xfade=transition=fade:duration={seconds}:\
                     offset={offset:.6}[{joined}];"
                );
            }
        }
        if render.has_audio {
            if matches!(transitions[i], Transition::Cut | Transition::Morph) {
                let _ = write!(
                    graph,
                    "[{current_audio}][a{i}]concat=n=2:v=0:a=1[{joined_audio}];"
                );
            } else {
                let _ = write!(
                    graph,
                    "[{current_audio}][a{i}]acrossfade=d={seconds}[{joined_audio}];"
                );
            }
        }
        length = offset + duration;
        current = joined;
        current_audio = joined_audio;
    }
    if ranges.len() == 1 {
        let _ = write!(graph, "[v0]null[{video_out}];");
        if render.has_audio {
            let _ = write!(graph, "[a0]anull[aj];");
        }
    }
    if render.has_audio {
        // Shorts platforms normalise to about -14 LUFS; deliver at that
        // level so the clip isn't turned down (or sounds quiet next to
        // others). loudnorm works at 192 kHz internally.
        let _ = write!(
            graph,
            "[aj]loudnorm=I={LOUDNESS}:TP=-1.5:LRA=11,aresample=48000[a];"
        );
    }
    if let Some(y) = caption_y {
        // The caption stream ends on a blank frame; keep the video going.
        let _ = write!(
            graph,
            "[vc][1:v]overlay=x=0:y={y}:eof_action=pass:format=auto,setsar=1[v];"
        );
    }
    // No trailing separator.
    graph.pop();
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
    // sendcmd and the caption list read files; keep their paths free of
    // characters that need escaping (e.g. ':' in Windows paths) by running
    // in their directory.
    let dir = tempfile::tempdir().map_err(|e| format!("Failed to create a temp folder: {e}"))?;
    let duration = output_duration(render.ranges);
    let overlay = match render.captions {
        Some((words, style)) if !words.is_empty() => Some(write_overlay(
            dir.path(),
            words,
            duration,
            render.output_size,
            &style,
        )?),
        _ => None,
    };
    // Morph frames: one image list (an input) per morph.
    let mut morph_lists: Vec<(usize, String)> = Vec::new();
    for (i, range) in render.ranges.iter().enumerate() {
        let Some(frames) = range.morph.as_ref().filter(|frames| !frames.is_empty()) else {
            continue;
        };
        if effective_transition(render.ranges, i) != Transition::Morph {
            continue;
        }
        let mut list = String::from("ffconcat version 1.0\n");
        for (k, frame) in frames.iter().enumerate() {
            let name = format!("morph_{i}_{k}.png");
            crate::morph::write_png(&dir.path().join(&name), frame)?;
            let _ = writeln!(list, "file {name}\nduration {:.6}", 1.0 / render.fps);
        }
        // The concat demuxer ignores the last entry's duration.
        let _ = writeln!(list, "file morph_{i}_{}.png", frames.len() - 1);
        let name = format!("morph_{i}.ffconcat");
        std::fs::write(dir.path().join(&name), list)
            .map_err(|e| format!("Failed to write the morph list: {e}"))?;
        morph_lists.push((i, name));
    }
    let first_morph_input = 1 + usize::from(overlay.is_some());
    let mut morph_inputs = vec![None; render.ranges.len()];
    for (n, (i, _)) in morph_lists.iter().enumerate() {
        morph_inputs[*i] = Some(first_morph_input + n);
    }

    let (graph, scripts) = render_graph(render, seek, overlay.as_ref().map(|o| o.y), &morph_inputs);
    for (name, script) in &scripts {
        std::fs::write(dir.path().join(name), script)
            .map_err(|e| format!("Failed to write the crop script: {e}"))?;
    }

    let mut command = FfmpegCommand::new();
    command
        .args(["-y", "-ss", &format!("{seek:.6}")])
        .input(render.input.to_string_lossy());
    if let Some(overlay) = &overlay {
        command
            .args(["-f", "concat", "-safe", "0"])
            .input(&overlay.list);
    }
    for (_, list) in &morph_lists {
        command.args(["-f", "concat", "-safe", "0"]).input(list);
    }
    command.args(["-filter_complex", &graph, "-map", "[v]"]);
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
            VerticalRange::new(2.0, 4.0, Framing::Follow(vec![key(0.0, 200.0)])),
            // Pan from red to blue.
            VerticalRange::new(
                0.0,
                2.0,
                Framing::Follow(vec![key(0.0, 200.0), key(2.0, 1100.0)]),
            ),
            // The whole picture: red | blue across the middle.
            VerticalRange::new(0.0, 1.0, Framing::Fit),
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
                captions: None,
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
mod transition_tests {
    use super::*;

    #[test]
    fn transitions_overlap_stretches_on_the_output_timeline() {
        let mut ranges = vec![
            VerticalRange::new(0.0, 2.0, Framing::Fit),
            VerticalRange::new(5.0, 7.0, Framing::Fit),
            VerticalRange::new(7.3, 7.6, Framing::Fit),
        ];
        ranges[1].transition = Transition::Whip;
        // Too short for a whip: rendered as a cut.
        ranges[2].transition = Transition::Whip;
        let starts = output_starts(&ranges);
        assert!((starts[1] - 1.76).abs() < 1e-9, "{starts:?}");
        assert!((starts[2] - 3.76).abs() < 1e-9, "{starts:?}");
        assert!((output_duration(&ranges) - 4.06).abs() < 1e-9);
    }

    /// Renders a whip between two generated stretches and a punched-in jump
    /// cut: the clip is shorter by the whip's overlap, keeps its audio, and
    /// mid-whip shows both sides blurred together.
    #[test]
    fn renders_whips_and_punch_ins() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("two.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=4"])
            .args([
                "-filter_complex",
                "color=c=red:s=640x360:r=25:d=2[a];color=c=blue:s=640x360:r=25:d=2[b];\
                 [a][b]concat=n=2,format=yuv420p[v]",
            ])
            .args([
                "-map", "[v]", "-map", "0:a", "-c:v", "libx264", "-c:a", "aac",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());

        let key = CropKey {
            time: 0.0,
            center_x: 320.0,
            center_y: 180.0,
            height: 360.0,
        };
        let mut ranges = vec![
            VerticalRange::new(0.0, 1.0, Framing::Follow(vec![key])),
            VerticalRange::new(1.2, 1.9, Framing::Follow(vec![key])),
            VerticalRange::new(2.1, 3.5, Framing::Follow(vec![key])),
        ];
        ranges[1].zoom = 1.12;
        ranges[2].transition = Transition::Whip;
        let output = dir.path().join("dressed.mp4");
        render_vertical(
            &VerticalRender {
                input: &source,
                ranges: &ranges,
                source_size: (640, 360),
                fps: 25.0,
                has_audio: true,
                output_size: (360, 640),
                output: &output,
                quality: ExportQuality::Draft,
                captions: None,
            },
            None,
            |_| {},
        )
        .unwrap();

        let info = crate::media_probe::probe_media(&output).unwrap();
        assert!(info.audio.is_some());
        let expected = output_duration(&ranges);
        assert!((expected - 2.86).abs() < 1e-9);
        let duration = info.duration_seconds.unwrap();
        assert!(
            (duration - expected).abs() < 0.12,
            "{duration} vs {expected}"
        );
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
        let (red, blue) = colour("1.2");
        assert!(red > 150 && blue < 80, "red before the whip ({red},{blue})");
        let (red, blue) = colour("1.58");
        assert!(
            red > 60 && blue > 60,
            "both sides during the whip ({red},{blue})"
        );
        let (red, blue) = colour("2.5");
        assert!(blue > 150 && red < 80, "blue after it ({red},{blue})");
    }
}

#[cfg(test)]
mod morph_tests {
    use super::*;
    use crate::frames::Frame;

    /// A morph replaces frames around a jump cut without changing the
    /// clip's length, and the interpolated frames show at the cut.
    #[test]
    fn morph_frames_play_over_the_cut() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("grey.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "color=c=gray:s=640x360:r=25:d=4"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=4"])
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());

        let key = CropKey {
            time: 0.0,
            center_x: 320.0,
            center_y: 180.0,
            height: 360.0,
        };
        let green = Frame {
            time: 0.0,
            width: 202,
            height: 360,
            data: [0u8, 200, 0].repeat(202 * 360),
        };
        let mut ranges = vec![
            VerticalRange::new(0.0, 1.5, Framing::Follow(vec![key])),
            VerticalRange::new(1.8, 3.5, Framing::Follow(vec![key])),
        ];
        ranges[1].transition = Transition::Morph;
        ranges[1].morph = Some(vec![green; 6]);
        let output = dir.path().join("morphed.mp4");
        render_vertical(
            &VerticalRender {
                input: &source,
                ranges: &ranges,
                source_size: (640, 360),
                fps: 25.0,
                has_audio: true,
                output_size: (360, 640),
                output: &output,
                quality: ExportQuality::Draft,
                captions: None,
            },
            None,
            |_| {},
        )
        .unwrap();

        let duration = crate::media_probe::probe_media(&output)
            .unwrap()
            .duration_seconds
            .unwrap();
        assert!((duration - 3.2).abs() < 0.1, "{duration}");
        let green_at = |time: &str| {
            let out = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-ss", time, "-i"])
                .arg(&output)
                .args(["-frames:v", "1", "-vf", "scale=1:1", "-f", "rawvideo"])
                .args(["-pix_fmt", "rgb24", "-"])
                .output()
                .unwrap()
                .stdout;
            out[1] > 150 && out[0] < 80
        };
        // Six frames at 25 fps = 0.24 s, centred on the cut at 1.5 s.
        assert!(green_at("1.5"), "morph frames at the cut");
        assert!(!green_at("1.2"), "source before");
        assert!(!green_at("1.8"), "source after");
    }
}

#[cfg(test)]
mod caption_tests {
    use super::*;

    /// Burns a caption into a real (generated) clip: the highlighted word
    /// shows up in the caption band, and the clip keeps its length and audio.
    #[test]
    fn captions_are_burned_in_over_the_video() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("grey.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "color=c=gray:s=640x360:r=25:d=2"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2,volume=-30dB",
            ])
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());

        let words = [TimedWord {
            start: 0.2,
            end: 1.8,
            text: "Hello".to_string(),
        }];
        let style = CaptionStyle::default();
        let output = dir.path().join("captioned.mp4");
        render_vertical(
            &VerticalRender {
                input: &source,
                ranges: &[VerticalRange::new(0.0, 2.0, Framing::Fit)],
                source_size: (640, 360),
                fps: 25.0,
                has_audio: true,
                output_size: (360, 640),
                output: &output,
                quality: ExportQuality::Draft,
                captions: Some((
                    &words,
                    CaptionStyle {
                        font_px: 40.0,
                        ..style
                    },
                )),
            },
            None,
            |_| {},
        )
        .unwrap();

        let info = crate::media_probe::probe_media(&output).unwrap();
        assert!(info.audio.is_some());
        assert!((info.duration_seconds.unwrap() - 2.0).abs() < 0.15);
        // A quiet sine (-30 dBFS) comes out near -14 LUFS.
        let measured = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-nostats", "-i"])
            .arg(&output)
            .args(["-af", "ebur128", "-f", "null", "-"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&measured.stderr);
        let summary = log.rsplit("Summary:").next().unwrap_or_default();
        let loudness: f64 = summary
            .lines()
            .find_map(|line| line.trim().strip_prefix("I:"))
            .and_then(|value| value.trim().trim_end_matches("LUFS").trim().parse().ok())
            .unwrap();
        assert!((loudness - LOUDNESS).abs() < 2.0, "{loudness} LUFS");
        let yellow_pixels = |time: &str| {
            let frame = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-ss", time, "-i"])
                .arg(&output)
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
                .output()
                .unwrap()
                .stdout;
            frame
                .as_chunks::<3>()
                .0
                .iter()
                .filter(|p| p[0] > 200 && p[1] > 150 && p[2] < 90)
                .count()
        };
        assert!(yellow_pixels("1.0") > 100, "the spoken word is highlighted");
        assert_eq!(yellow_pixels("0.05"), 0, "nothing before the word");
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
                    ranges: &[VerticalRange::new(
                        600.0,
                        600.0 + duration,
                        Framing::Follow(keys.to_vec()),
                    )],
                    source_size: (w, h),
                    fps,
                    has_audio: true,
                    output_size: size,
                    output: &output,
                    quality,
                    captions: None,
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
