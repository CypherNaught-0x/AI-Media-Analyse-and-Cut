//! Vertical (9:16) export of clips (shorts phase S2): analyse the clips'
//! source ranges for shots and faces, bind the transcript's speakers to
//! faces, plan a camera path per range and render.
//!
//! All clips of one export are analysed together before binding: panels and
//! podcasts cut back to the same camera setups, so every clip adds evidence
//! for the others.

use crate::camera::{plan_camera, CameraSettings, Frame as CameraFrame};
use crate::encoders::ExportQuality;
use crate::face_tracks::{Track, Tracker};
use crate::faces::FaceDetector;
use crate::frames::{decode_frames, FrameRequest, PixelFormat};
use crate::media_probe::probe_media;
use crate::reframe::{render_vertical, CropKey, Framing, VerticalRange, VerticalRender};
use crate::run_control::RunControl;
use crate::shots::detect_cuts;
use crate::speaker_faces::{bind_speakers, Binding, SpeechTurn};
use std::path::{Path, PathBuf};

pub(crate) const OUTPUT_SIZE: (u32, u32) = (1080, 1920);
/// The crop is never upscaled more than this; tighter framing would only be
/// blur. On a 720p source this allows a 480 px crop height.
const MAX_UPSCALE: f64 = 4.0;
/// Frames analysed per second.
const ANALYSIS_FPS: f64 = 10.0;
/// Faces are detected at up to this width.
const ANALYSIS_WIDTH: u32 = 1280;
/// Analysed beyond each range so tracks are established at its edges.
const ANALYSIS_MARGIN: f64 = 0.5;
/// Tracks shorter than this are detection noise.
const MIN_TRACK_SECONDS: f64 = 0.5;
/// Share of the progress bar for analysis; rendering takes the rest.
const ANALYSIS_SHARE: f64 = 0.5;

pub(crate) struct VerticalClip {
    /// Source ranges (seconds), played in order.
    pub ranges: Vec<(f64, f64)>,
    pub output: PathBuf,
}

/// Progress: overall fraction (0-1) and what is happening.
pub(crate) type Progress<'a> = dyn FnMut(f64, String) + 'a;

/// Detects cuts and tracks faces in one range. Track ids and shots are
/// local to the range.
fn analyse_range(
    path: &Path,
    range: (f64, f64),
    width: u32,
    detector: &mut FaceDetector,
    run: Option<(u64, &RunControl)>,
    on_time: &mut dyn FnMut(f64),
) -> Result<Vec<Track>, String> {
    let cuts = detect_cuts(path, range.0, range.1, run)?;
    let mut tracker = Tracker::new(cuts, ANALYSIS_FPS);
    let mut error = None;
    decode_frames(
        &FrameRequest {
            path,
            start: range.0,
            end: range.1,
            fps: ANALYSIS_FPS,
            width,
            format: PixelFormat::Rgb24,
        },
        run,
        |frame| {
            if error.is_some() {
                return;
            }
            match detector.detect(&frame) {
                Ok(faces) => tracker.push(&frame, &faces),
                Err(e) => error = Some(e),
            }
            on_time(frame.time - range.0);
        },
    )?;
    if let Some(error) = error {
        return Err(error);
    }
    Ok(tracker.finish(MIN_TRACK_SECONDS))
}

/// Export `clips` as vertical videos. Blocks; run it on the blocking pool.
pub(crate) fn export_vertical(
    input: &Path,
    clips: &[VerticalClip],
    turns: &[SpeechTurn],
    quality: ExportQuality,
    run: Option<(u64, &RunControl)>,
    on_progress: &mut Progress<'_>,
) -> Result<Vec<Binding>, String> {
    let media = probe_media(input)?;
    let video = media
        .video
        .as_ref()
        .ok_or_else(|| "The source has no video to reframe".to_string())?;
    let (source_w, source_h) = video.display_size();
    let fps = if video.fps > 0.0 {
        f64::from(video.fps)
    } else {
        30.0
    };
    let analysis_w = (source_w.min(ANALYSIS_WIDTH)) & !1;
    let scale = f64::from(source_w) / f64::from(analysis_w);
    let camera_frame = CameraFrame {
        height: f64::from(source_h) / scale,
        min_crop_height: f64::from(OUTPUT_SIZE.1) / MAX_UPSCALE / scale,
        fps,
    };

    let total_seconds: f64 = clips
        .iter()
        .flat_map(|clip| &clip.ranges)
        .map(|(start, end)| end - start + 2.0 * ANALYSIS_MARGIN)
        .sum::<f64>()
        .max(1e-6);

    // 1. Analyse every range, giving tracks globally unique ids and shots.
    let mut detector = FaceDetector::new()?;
    let mut tracks: Vec<Track> = Vec::new();
    // Per clip, per range: indices into `tracks`.
    let mut range_tracks: Vec<Vec<Vec<usize>>> = Vec::new();
    let mut analysed = 0.0;
    let mut next_shot = 0;
    for clip in clips {
        let mut per_range = Vec::new();
        for &(start, end) in &clip.ranges {
            let range = ((start - ANALYSIS_MARGIN).max(0.0), end + ANALYSIS_MARGIN);
            let mut on_time = |time: f64| {
                let fraction = (analysed + time) / total_seconds;
                on_progress(
                    ANALYSIS_SHARE * fraction.min(1.0),
                    format!("Finding faces and speakers ({:.0}%)", fraction * 100.0),
                );
            };
            let local = analyse_range(input, range, analysis_w, &mut detector, run, &mut on_time)?;
            analysed += range.1 - range.0;
            let shots = local.iter().map(|t| t.shot + 1).max().unwrap_or(0);
            let mut indices = Vec::new();
            for mut track in local {
                track.id = tracks.len();
                track.shot += next_shot;
                indices.push(track.id);
                tracks.push(track);
            }
            next_shot += shots;
            per_range.push(indices);
        }
        range_tracks.push(per_range);
    }

    // 2. Who is who, over all clips together.
    let bindings = bind_speakers(&tracks, turns);

    // 3. Camera path and render per clip.
    let settings = CameraSettings::default();
    let aspect = f64::from(OUTPUT_SIZE.0) / f64::from(OUTPUT_SIZE.1);
    let render_total: f64 = clips
        .iter()
        .flat_map(|clip| &clip.ranges)
        .map(|(start, end)| end - start)
        .sum::<f64>()
        .max(1e-6);
    let mut rendered = 0.0;
    for (index, (clip, per_range)) in clips.iter().zip(&range_tracks).enumerate() {
        let ranges: Vec<VerticalRange> = clip
            .ranges
            .iter()
            .zip(per_range)
            .flat_map(|(&(start, end), indices)| {
                let range_tracks: Vec<Track> = indices.iter().map(|&i| tracks[i].clone()).collect();
                plan_camera(
                    (start, end),
                    &range_tracks,
                    &bindings,
                    turns,
                    &camera_frame,
                    aspect,
                    &settings,
                )
            })
            .map(|piece| VerticalRange {
                start: piece.start,
                end: piece.end,
                // Analysis pixels to source pixels.
                framing: match piece.framing {
                    Framing::Fit => Framing::Fit,
                    Framing::Follow(keys) => Framing::Follow(
                        keys.into_iter()
                            .map(|key| CropKey {
                                time: key.time,
                                center_x: key.center_x * scale,
                                center_y: key.center_y * scale,
                                height: key.height * scale,
                            })
                            .collect(),
                    ),
                },
            })
            .collect();
        let clip_seconds: f64 = clip.ranges.iter().map(|(s, e)| e - s).sum();
        render_vertical(
            &VerticalRender {
                input,
                ranges: &ranges,
                source_size: (source_w, source_h),
                fps,
                has_audio: media.audio.is_some(),
                output_size: OUTPUT_SIZE,
                output: &clip.output,
                quality,
            },
            run,
            |time| {
                let fraction = (rendered + time.min(clip_seconds)) / render_total;
                on_progress(
                    ANALYSIS_SHARE + (1.0 - ANALYSIS_SHARE) * fraction.min(1.0),
                    format!(
                        "Rendering vertical clip {}/{} ({:.0}%)",
                        index + 1,
                        clips.len(),
                        fraction * 100.0
                    ),
                );
            },
        )?;
        rendered += clip_seconds;
    }
    on_progress(1.0, "Vertical export finished".to_string());
    Ok(bindings)
}

#[cfg(test)]
mod evaluation {
    use super::*;

    /// Exports one real clip vertically. `SHORTS_PROFILE_RANGES` is a list
    /// like `600-640,700-710` (one clip from several ranges).
    /// `SHORTS_PROFILE_SOURCE=… SHORTS_PROFILE_TRANSCRIPT=… SHORTS_PROFILE_KEEP=dir
    ///  cargo test --release --lib vertical_export_of_a_recording -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn vertical_export_of_a_recording() {
        let (Some(source), Some(transcript), Some(keep)) = (
            std::env::var_os("SHORTS_PROFILE_SOURCE"),
            std::env::var_os("SHORTS_PROFILE_TRANSCRIPT"),
            std::env::var_os("SHORTS_PROFILE_KEEP"),
        ) else {
            eprintln!("SHORTS_PROFILE_SOURCE / _TRANSCRIPT / _KEEP not set; skipping");
            return;
        };
        let ranges: Vec<(f64, f64)> = std::env::var("SHORTS_PROFILE_RANGES")
            .unwrap_or_else(|_| "830-870".to_string())
            .split(',')
            .map(|range| {
                let (start, end) = range.split_once('-').unwrap();
                (start.parse().unwrap(), end.parse().unwrap())
            })
            .collect();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(transcript).unwrap()).unwrap();
        let turns: Vec<SpeechTurn> = json["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|segment| SpeechTurn {
                start: crate::time_utils::parse_time(segment["start"].as_str().unwrap()),
                end: crate::time_utils::parse_time(segment["end"].as_str().unwrap()),
                speaker: segment["speaker"].as_str().unwrap_or("?").to_string(),
            })
            .collect();
        let output = PathBuf::from(keep).join("vertical_export.mp4");
        let started = std::time::Instant::now();
        let mut last = String::new();
        let bindings = export_vertical(
            Path::new(&source),
            &[VerticalClip {
                ranges: ranges.clone(),
                output: output.clone(),
            }],
            &turns,
            ExportQuality::Balanced,
            None,
            &mut |_, message| last = message,
        )
        .unwrap();
        let mut seats: Vec<_> = bindings
            .iter()
            .map(|b| (b.setup, b.seat, b.speaker.clone(), b.affinity, b.evidence))
            .collect();
        seats.sort_by_key(|seat| (seat.0, seat.1));
        seats.dedup_by(|a, b| (a.0, a.1) == (b.0, b.1));
        for (setup, seat, speaker, affinity, evidence) in seats {
            println!(
                "setup {setup} seat {seat}: {speaker} (affinity {affinity:.2}, {evidence:.0} s)"
            );
        }
        let seconds: f64 = ranges.iter().map(|(s, e)| e - s).sum();
        println!(
            "{seconds:.0} s clip in {:.1} s ({last}) -> {}",
            started.elapsed().as_secs_f64(),
            output.display()
        );
    }
}
