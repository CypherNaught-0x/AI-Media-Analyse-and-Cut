//! Vertical (9:16) export of clips (shorts phase S2): analyse the clips'
//! source ranges for shots and faces, bind the transcript's speakers to
//! faces, plan a camera path per range and render.
//!
//! All clips of one export are analysed together before binding: panels and
//! podcasts cut back to the same camera setups, so every clip adds evidence
//! for the others.

use crate::camera::{plan_camera, CameraSettings, Frame as CameraFrame};
use crate::captions::{tightened_output_words, CaptionStyle, TimedWord};
use crate::encoders::ExportQuality;
use crate::face_tracks::{Track, Tracker};
use crate::faces::FaceDetector;
use crate::frames::{decode_frames, FrameRequest, PixelFormat};
use crate::media_probe::probe_media;
use crate::reframe::{
    output_starts, render_vertical, CropKey, Framing, Transition, VerticalRange, VerticalRender,
};
use crate::run_control::RunControl;
use crate::shots::detect_cuts;
use crate::speaker_faces::{bind_speakers, Binding, SpeechTurn};
use crate::tighten::Intensity;
use std::path::{Path, PathBuf};

pub(crate) const OUTPUT_SIZE: (u32, u32) = (1080, 1920);
/// The crop is never upscaled more than this; tighter framing would only be
/// blur. On a 720p source this allows a 480 px crop height.
const MAX_UPSCALE: f64 = 4.0;
/// Frames analysed per second.
const ANALYSIS_FPS: f64 = 10.0;
/// Faces are detected at up to this width.
const ANALYSIS_WIDTH: u32 = 1280;
/// Analysed beyond each range so tracks are established at its edges, and
/// so a later export with padded or trimmed edges reuses the analysis.
const ANALYSIS_MARGIN: f64 = 2.0;
/// Analysed ranges kept in memory (a few seconds of work each).
const CACHE_ENTRIES: usize = 64;
/// Tracks shorter than this are detection noise.
const MIN_TRACK_SECONDS: f64 = 0.5;
/// Share of the progress bar for analysis; rendering takes the rest.
const ANALYSIS_SHARE: f64 = 0.5;

/// A clip edge this close to a source cut moves onto the cut, so a clip
/// never opens or closes on a few frames of another shot (a flash frame).
const SLIVER: f64 = 0.3;

/// What analysing a source range found.
#[derive(Debug, Clone, Default)]
pub(crate) struct Analysis {
    pub tracks: Vec<Track>,
    /// Source cut times.
    pub cuts: Vec<f64>,
}

impl Analysis {
    fn within(&self, range: (f64, f64)) -> Analysis {
        Analysis {
            tracks: self
                .tracks
                .iter()
                .filter_map(|track| track.within(range.0, range.1))
                .collect(),
            cuts: self
                .cuts
                .iter()
                .copied()
                .filter(|cut| (range.0..=range.1).contains(cut))
                .collect(),
        }
    }
}

/// `range` with edges that sit just next to a source cut moved onto it.
fn trim_slivers(range: (f64, f64), cuts: &[f64]) -> (f64, f64) {
    let (mut start, mut end) = range;
    for &cut in cuts {
        if cut > start && cut - start < SLIVER && cut < end {
            start = cut;
        }
        if cut < end && end - cut < SLIVER && cut > start {
            end = cut;
        }
    }
    (start, end)
}

/// A range to plan (without flash frames) and its tracks (indices).
type RangeTracks = ((f64, f64), Vec<usize>);

/// One analysed range: source key, range (seconds) and what was found.
type CacheEntry = (String, (f64, f64), Analysis);

/// Face analysis of source ranges, kept for the session so previewing and
/// then exporting a clip (or re-exporting it) analyses it only once.
#[derive(Default)]
pub(crate) struct AnalysisCache {
    /// Most recently used last: (source key, analysed range, tracks).
    entries: std::sync::Mutex<Vec<CacheEntry>>,
}

impl AnalysisCache {
    /// The analysis of `range` from an entry that covers it.
    fn get(&self, source: &str, range: (f64, f64)) -> Option<Analysis> {
        let mut entries = self.entries.lock().expect("analysis cache poisoned");
        let index = entries.iter().rposition(|(key, covered, _)| {
            key == source && covered.0 <= range.0 + 1e-6 && range.1 <= covered.1 + 1e-6
        })?;
        let entry = entries.remove(index);
        let analysis = entry.2.within(range);
        entries.push(entry);
        Some(analysis)
    }

    fn put(&self, source: String, range: (f64, f64), analysis: Analysis) {
        let mut entries = self.entries.lock().expect("analysis cache poisoned");
        entries.push((source, range, analysis));
        if entries.len() > CACHE_ENTRIES {
            entries.remove(0);
        }
    }
}

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
) -> Result<Analysis, String> {
    let cuts = detect_cuts(path, range.0, range.1, run)?;
    let mut tracker = Tracker::new(cuts.clone(), ANALYSIS_FPS);
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
    Ok(Analysis {
        tracks: tracker.finish(MIN_TRACK_SECONDS),
        cuts,
    })
}

/// The planned framing of a set of clips, ready to render (or preview).
#[derive(Debug, Clone)]
pub(crate) struct VerticalPlan {
    pub source_size: (u32, u32),
    pub fps: f64,
    pub has_audio: bool,
    /// Per clip: its pieces in order, in source pixels.
    pub clips: Vec<Vec<VerticalRange>>,
    pub bindings: Vec<Binding>,
}

/// A crop key for the live preview (source pixels; time relative to the
/// piece start).
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewKey {
    #[specta(type = specta_typescript::Number)]
    pub time: f64,
    #[specta(type = specta_typescript::Number)]
    pub center_x: f64,
    #[specta(type = specta_typescript::Number)]
    pub center_y: f64,
    #[specta(type = specta_typescript::Number)]
    pub height: f64,
}

/// A stretch of a clip with one framing: `fit` shows the whole picture,
/// otherwise the crop follows `keys`.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPiece {
    #[specta(type = specta_typescript::Number)]
    pub start: f64,
    #[specta(type = specta_typescript::Number)]
    pub end: f64,
    pub fit: bool,
    pub keys: Vec<PreviewKey>,
    /// Punch-in on top of the framing (1.0 = none).
    #[specta(type = specta_typescript::Number)]
    pub zoom: f64,
}

/// The planned vertical framing of clips, as the frontend previews it.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct VerticalPreview {
    pub source_width: u32,
    pub source_height: u32,
    pub output_width: u32,
    pub output_height: u32,
    /// Per clip: its pieces in playback order.
    pub clips: Vec<Vec<PreviewPiece>>,
}

impl From<&VerticalPlan> for VerticalPreview {
    fn from(plan: &VerticalPlan) -> Self {
        let piece = |range: &VerticalRange| PreviewPiece {
            start: range.start,
            end: range.end,
            fit: matches!(range.framing, Framing::Fit),
            zoom: range.zoom,
            keys: match &range.framing {
                Framing::Fit => Vec::new(),
                Framing::Follow(keys) => keys
                    .iter()
                    .map(|key| PreviewKey {
                        time: key.time,
                        center_x: key.center_x,
                        center_y: key.center_y,
                        height: key.height,
                    })
                    .collect(),
            },
        };
        Self {
            source_width: plan.source_size.0,
            source_height: plan.source_size.1,
            output_width: OUTPUT_SIZE.0,
            output_height: OUTPUT_SIZE.1,
            clips: plan
                .clips
                .iter()
                .map(|pieces| pieces.iter().map(piece).collect())
                .collect(),
        }
    }
}

/// Analyse and plan the framing of clips given as source ranges. Blocks.
/// `on_progress` gets the analysed fraction (0-1).
pub(crate) fn plan_vertical(
    input: &Path,
    clips: &[Vec<(f64, f64)>],
    turns: &[SpeechTurn],
    cuts: CutStyle,
    cache: Option<&AnalysisCache>,
    run: Option<(u64, &RunControl)>,
    on_progress: &mut dyn FnMut(f64),
) -> Result<VerticalPlan, String> {
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
        .flatten()
        .map(|(start, end)| end - start + 2.0 * ANALYSIS_MARGIN)
        .sum::<f64>()
        .max(1e-6);

    // 1. Analyse every range, giving tracks globally unique ids and shots.
    let source_key = crate::media_cache::source_key(input)?;
    let mut detector = None;
    let mut tracks: Vec<Track> = Vec::new();
    // Per clip, per range: the range without flash frames, and indices into
    // `tracks`.
    let mut range_tracks: Vec<Vec<RangeTracks>> = Vec::new();
    let mut analysed = 0.0;
    let mut next_shot = 0;
    for ranges in clips {
        let mut per_range = Vec::new();
        for &(start, end) in ranges {
            let range = ((start - ANALYSIS_MARGIN).max(0.0), end + ANALYSIS_MARGIN);
            let local = match cache.and_then(|cache| cache.get(&source_key, range)) {
                Some(analysis) => analysis,
                None => {
                    if detector.is_none() {
                        detector = Some(FaceDetector::new()?);
                    }
                    let detector = detector.as_mut().expect("just created");
                    let mut on_time =
                        |time: f64| on_progress(((analysed + time) / total_seconds).min(1.0));
                    let analysis =
                        analyse_range(input, range, analysis_w, detector, run, &mut on_time)?;
                    if let Some(cache) = cache {
                        cache.put(source_key.clone(), range, analysis.clone());
                    }
                    analysis
                }
            };
            analysed += range.1 - range.0;
            on_progress((analysed / total_seconds).min(1.0));
            let kept = trim_slivers((start, end), &local.cuts);
            let shots = local.tracks.iter().map(|t| t.shot + 1).max().unwrap_or(0);
            let mut indices = Vec::new();
            for mut track in local.tracks {
                track.id = tracks.len();
                track.shot += next_shot;
                indices.push(track.id);
                tracks.push(track);
            }
            next_shot += shots;
            per_range.push((kept, indices));
        }
        range_tracks.push(per_range);
    }

    // 2. Who is who, over all clips together.
    let bindings = bind_speakers(&tracks, turns);

    // 3. Camera path per range.
    let settings = CameraSettings::default();
    let aspect = f64::from(OUTPUT_SIZE.0) / f64::from(OUTPUT_SIZE.1);
    let planned = range_tracks
        .iter()
        .map(|per_range| {
            per_range
                .iter()
                .flat_map(|&((start, end), ref indices)| {
                    let range_tracks: Vec<Track> =
                        indices.iter().map(|&i| tracks[i].clone()).collect();
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
                .map(|piece| {
                    VerticalRange::new(
                        piece.start,
                        piece.end,
                        // Analysis pixels to source pixels.
                        match piece.framing {
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
                    )
                })
                .collect::<Vec<_>>()
        })
        .map(|mut pieces| {
            dress_cuts(&mut pieces, cuts);
            pieces
        })
        .collect();

    Ok(VerticalPlan {
        source_size: (source_w, source_h),
        fps,
        has_audio: media.audio.is_some(),
        clips: planned,
        bindings,
    })
}

/// Source jumps up to this long (seconds, forward) are jump cuts within one
/// moment, e.g. a tightened pause; longer or backward ones are splices.
const JUMP_CUT: f64 = 3.0;

/// How cuts are dressed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CutStyle {
    /// Punch-in on every other jump cut (1.0 = none).
    pub punch: f64,
    /// Transition into a spliced-in moment.
    pub splice: Transition,
}

impl From<Intensity> for CutStyle {
    fn from(intensity: Intensity) -> Self {
        let (punch, splice) = match intensity {
            Intensity::Off => (1.0, Transition::Cut),
            Intensity::Chill => (1.0, Transition::Fade),
            Intensity::Punchy => (1.12, Transition::Whip),
            Intensity::Hyper => (1.18, Transition::Whip),
        };
        Self { punch, splice }
    }
}

/// Punch-ins and transitions for a clip's pieces (in playback order).
/// Jump cuts alternate between the normal framing and the punch-in, so a
/// run of them reads as deliberate; a splice gets `style.splice` and resets
/// the punch. Pieces that continue the same source moment (a framing change)
/// keep the zoom.
pub(crate) fn dress_cuts(pieces: &mut [VerticalRange], style: CutStyle) {
    let mut punched = false;
    for i in 1..pieces.len() {
        let gap = pieces[i].start - pieces[i - 1].end;
        if gap.abs() < 1e-3 {
            pieces[i].zoom = pieces[i - 1].zoom;
        } else if gap > 0.0 && gap <= JUMP_CUT {
            punched = !punched && style.punch > 1.0;
            pieces[i].zoom = if punched { style.punch } else { 1.0 };
        } else {
            punched = false;
            pieces[i].zoom = 1.0;
            pieces[i].transition = style.splice;
        }
    }
}

/// How vertical clips are rendered.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderOptions<'a> {
    pub quality: ExportQuality,
    pub cuts: CutStyle,
    /// Burned-in captions: transcript words on the source timeline.
    pub captions: Option<(&'a [TimedWord], CaptionStyle)>,
}

/// Export `clips` as vertical videos. Blocks; run it on the blocking pool.
pub(crate) fn export_vertical(
    input: &Path,
    clips: &[VerticalClip],
    turns: &[SpeechTurn],
    options: RenderOptions<'_>,
    cache: Option<&AnalysisCache>,
    run: Option<(u64, &RunControl)>,
    on_progress: &mut Progress<'_>,
) -> Result<VerticalPlan, String> {
    let ranges: Vec<Vec<(f64, f64)>> = clips.iter().map(|clip| clip.ranges.clone()).collect();
    let plan = plan_vertical(
        input,
        &ranges,
        turns,
        options.cuts,
        cache,
        run,
        &mut |fraction| {
            on_progress(
                ANALYSIS_SHARE * fraction,
                format!("Finding faces and speakers ({:.0}%)", fraction * 100.0),
            );
        },
    )?;
    let mut seats: Vec<(usize, usize, &str, f32)> = plan
        .bindings
        .iter()
        .map(|b| (b.setup, b.seat, b.speaker.as_str(), b.affinity))
        .collect();
    seats.dedup_by(|a, b| (a.0, a.1) == (b.0, b.1));
    for (setup, seat, speaker, affinity) in seats {
        log::info!("vertical: setup {setup} seat {seat} = {speaker} (affinity {affinity:.2})");
    }

    let render_total: f64 = ranges
        .iter()
        .flatten()
        .map(|(start, end)| end - start)
        .sum::<f64>()
        .max(1e-6);
    let mut rendered = 0.0;
    for (index, (clip, pieces)) in clips.iter().zip(&plan.clips).enumerate() {
        let clip_seconds: f64 = clip.ranges.iter().map(|(s, e)| e - s).sum();
        // Captions follow what is rendered: the pieces, after flash-frame
        // trimming.
        let shown: Vec<(f64, f64)> = pieces.iter().map(|p| (p.start, p.end)).collect();
        let starts = output_starts(pieces);
        let words = options
            .captions
            .map(|(words, style)| (tightened_output_words(words, &shown, &starts), style));
        render_vertical(
            &VerticalRender {
                input,
                ranges: pieces,
                source_size: plan.source_size,
                fps: plan.fps,
                has_audio: plan.has_audio,
                output_size: OUTPUT_SIZE,
                output: &clip.output,
                quality: options.quality,
                captions: words
                    .as_ref()
                    .map(|(words, style)| (words.as_slice(), *style)),
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
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face_tracks::Observation;
    use crate::faces::Face;

    fn track(times: &[f64]) -> Track {
        let face = Face {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            score: 0.9,
            landmarks: [(0.0, 0.0); 5],
        };
        Track::for_tests(
            0,
            0,
            times
                .iter()
                .map(|&time| Observation {
                    time,
                    face,
                    mouth_motion: None,
                })
                .collect(),
        )
    }

    fn piece(start: f64, end: f64) -> VerticalRange {
        VerticalRange::new(start, end, Framing::Fit)
    }

    #[test]
    fn jump_cuts_alternate_the_punch_in_and_splices_get_a_transition() {
        let mut pieces = vec![
            piece(10.0, 12.0),
            // Tightened pause: jump cut.
            piece(12.3, 14.0),
            // Framing change at the same moment: keeps the zoom.
            piece(14.0, 15.0),
            piece(15.4, 17.0),
            piece(17.2, 18.0),
            // Back to an earlier moment: a splice.
            piece(5.0, 7.0),
            piece(7.5, 9.0),
        ];
        dress_cuts(&mut pieces, CutStyle::from(Intensity::Punchy));
        let zooms: Vec<f64> = pieces.iter().map(|p| p.zoom).collect();
        assert_eq!(zooms, [1.0, 1.12, 1.12, 1.0, 1.12, 1.0, 1.12]);
        let transitions: Vec<Transition> = pieces.iter().map(|p| p.transition).collect();
        assert_eq!(transitions[5], Transition::Whip);
        assert!(transitions
            .iter()
            .enumerate()
            .all(|(i, t)| i == 5 || *t == Transition::Cut));

        let mut calm = vec![piece(10.0, 12.0), piece(12.3, 14.0), piece(30.0, 32.0)];
        dress_cuts(&mut calm, CutStyle::from(Intensity::Chill));
        assert!(calm.iter().all(|p| p.zoom == 1.0));
        assert_eq!(calm[2].transition, Transition::Fade);
    }

    #[test]
    fn clip_edges_next_to_a_cut_move_onto_it() {
        assert_eq!(
            trim_slivers((10.0, 20.0), &[10.1, 15.0, 19.8]),
            (10.1, 19.8)
        );
        // Far from the edges: nothing changes.
        assert_eq!(trim_slivers((10.0, 20.0), &[10.5, 19.5]), (10.0, 20.0));
    }

    #[test]
    fn the_cache_answers_ranges_inside_an_analysed_one() {
        let cache = AnalysisCache::default();
        cache.put(
            "a".into(),
            (10.0, 20.0),
            Analysis {
                tracks: vec![track(&[10.0, 12.0, 15.0, 19.0])],
                cuts: vec![11.0, 17.0],
            },
        );

        let inside = cache.get("a", (11.0, 16.0)).unwrap();
        let times: Vec<f64> = inside.tracks[0]
            .observations
            .iter()
            .map(|o| o.time)
            .collect();
        assert_eq!(times, [12.0, 15.0]);
        assert_eq!(inside.cuts, [11.0]);
        // Not covered, or another source: analyse again.
        assert!(cache.get("a", (9.0, 16.0)).is_none());
        assert!(cache.get("b", (11.0, 16.0)).is_none());
    }

    #[test]
    fn the_cache_forgets_the_least_recently_used() {
        let cache = AnalysisCache::default();
        for i in 0..=CACHE_ENTRIES {
            cache.put("a".into(), (i as f64, i as f64 + 1.0), Analysis::default());
        }
        assert!(cache.get("a", (0.0, 1.0)).is_none());
        assert!(cache.get("a", (1.0, 2.0)).is_some());
    }
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
        let words: Vec<TimedWord> = json["segments"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|segment| segment["words"].as_array().cloned().unwrap_or_default())
            .map(|word| TimedWord {
                start: crate::time_utils::parse_time(word["start"].as_str().unwrap()),
                end: crate::time_utils::parse_time(word["end"].as_str().unwrap()),
                text: word["text"].as_str().unwrap_or_default().to_string(),
            })
            .collect();
        // Tighten like the app does (SHORTS_PROFILE_INTENSITY, default punchy).
        let intensity = match std::env::var("SHORTS_PROFILE_INTENSITY").as_deref() {
            Ok("off") => crate::tighten::Intensity::Off,
            Ok("chill") => crate::tighten::Intensity::Chill,
            Ok("hyper") => crate::tighten::Intensity::Hyper,
            _ => crate::tighten::Intensity::Punchy,
        };
        let ranges =
            crate::tighten::tighten_clips(Path::new(&source), &[ranges], &words, intensity, None)
                .unwrap()
                .remove(0);
        let removed = crate::tighten::removed_words(&words, intensity);
        let words: Vec<TimedWord> = words.into_iter().filter(|w| !removed.contains(w)).collect();
        println!("{intensity:?}: {} ranges", ranges.len());
        let output = PathBuf::from(keep).join("vertical_export.mp4");
        let started = std::time::Instant::now();
        let mut last = String::new();
        let plan = export_vertical(
            Path::new(&source),
            &[VerticalClip {
                ranges: ranges.clone(),
                output: output.clone(),
            }],
            &turns,
            RenderOptions {
                quality: ExportQuality::Balanced,
                cuts: CutStyle::from(intensity),
                captions: Some((&words, CaptionStyle::default())),
            },
            None,
            None,
            &mut |_, message| last = message,
        )
        .unwrap();
        for piece in &plan.clips[0] {
            match &piece.framing {
                Framing::Fit => println!("{:.1}-{:.1}: fit", piece.start, piece.end),
                Framing::Follow(keys) => {
                    let speeds: Vec<f64> = keys
                        .windows(2)
                        .filter(|w| w[1].time - w[0].time > 0.01)
                        .map(|w| (w[1].center_x - w[0].center_x) / (w[1].time - w[0].time))
                        .collect();
                    let fastest = speeds.iter().fold(0.0f64, |m, v| m.max(v.abs()));
                    println!(
                        "{:.1}-{:.1}: follow, {} keys, fastest pan {fastest:.0} px/s",
                        piece.start,
                        piece.end,
                        keys.len()
                    );
                    if std::env::var_os("SHORTS_PROFILE_KEYS").is_some() {
                        for key in keys {
                            println!(
                                "    {:6.2} x {:7.1} y {:6.1} h {:5.0}",
                                key.time, key.center_x, key.center_y, key.height
                            );
                        }
                    }
                }
            }
        }
        let mut seats: Vec<_> = plan
            .bindings
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
