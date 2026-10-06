//! Vertical (9:16) export of clips (shorts phase S2): analyse the clips'
//! source ranges for shots and faces, bind the transcript's speakers to
//! faces, plan a camera path per range and render.
//!
//! All clips of one export are analysed together before binding: panels and
//! podcasts cut back to the same camera setups, so every clip adds evidence
//! for the others.

use crate::camera::{listener_framing, plan_camera, CameraSettings, Frame as CameraFrame, Stretch};
use crate::captions::{chunk_words, tightened_output_words, CaptionStyle, TimedWord};
use crate::encoders::ExportQuality;
use crate::face_identity::{group_people, Embedding, FaceEmbedder, SAME_PERSON};
use crate::face_tracks::{Track, Tracker};
use crate::faces::{Face, FaceDetector};
use crate::frames::Frame;
use crate::frames::{decode_frames, FrameRequest, PixelFormat};
use crate::media_probe::probe_media;
use crate::morph::{crop, frame_at, luma_difference, Morpher, MAX_DIFFERENCE, MORPH_FRAMES};
use crate::reframe::{
    crop_at, output_starts, render_cover, render_vertical, Cover, CropKey, CropRect, Framing,
    Transition, VerticalRange, VerticalRender,
};
use crate::run_control::RunControl;
use crate::shots::detect_cuts;
use crate::speaker_faces::{assign_seats, bind_speakers, Binding, Pin, SpeechTurn};
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

/// A range to plan (without flash frames), its tracks (indices) and the
/// source cuts in it.
type RangeTracks = ((f64, f64), Vec<usize>, Vec<f64>);

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
    /// Drawn on the cover image (`<output>.jpg`); no cover without one.
    pub title: Option<String>,
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
    /// Per clip: caption chunks on the source timeline (empty without
    /// captions), as the export burns them in.
    pub captions: Vec<Vec<Vec<PreviewWord>>>,
}

/// A caption word on the source timeline.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct PreviewWord {
    #[specta(type = specta_typescript::Number)]
    pub start: f64,
    #[specta(type = specta_typescript::Number)]
    pub end: f64,
    pub text: String,
}

impl VerticalPreview {
    /// Add caption chunks for each clip from `words` (source timeline): the
    /// words shown in its pieces, chunked like the export.
    pub(crate) fn with_captions(mut self, words: &[TimedWord], style: &CaptionStyle) -> Self {
        self.captions = self
            .clips
            .iter()
            .map(|pieces| {
                let shown: Vec<TimedWord> = words
                    .iter()
                    .filter(|word| {
                        let middle = (word.start + word.end) / 2.0;
                        pieces.iter().any(|p| p.start <= middle && middle < p.end)
                    })
                    .cloned()
                    .collect();
                chunk_words(&shown, style)
                    .into_iter()
                    .map(|chunk| {
                        chunk
                            .into_iter()
                            .map(|word| PreviewWord {
                                start: word.start,
                                end: word.end,
                                text: word.text,
                            })
                            .collect()
                    })
                    .collect()
            })
            .collect();
        self
    }
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
            captions: Vec::new(),
        }
    }
}

/// A point on a face at one moment: where the user's word about the face
/// applies. Coordinates are shares (0-1) of the picture's width and height.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct FaceAnchor {
    #[specta(type = specta_typescript::Number)]
    pub time: f64,
    #[specta(type = specta_typescript::Number)]
    pub x: f64,
    #[specta(type = specta_typescript::Number)]
    pub y: f64,
}

/// Who a face is.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FaceSpeaker {
    /// As the mouth motion says.
    Auto,
    /// This transcript speaker.
    Named { name: String },
    /// Someone who doesn't speak (a listener).
    Nobody,
}

/// The user's word on a face, found again by its anchors: it applies to
/// every face of the camera setup and seat an anchor lands on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FaceOverride {
    pub anchors: Vec<FaceAnchor>,
    /// Never frame this face: a picture on a screen, a poster.
    pub ignored: bool,
    pub speaker: FaceSpeaker,
}

/// A person found in the clips, for the user to check: one face, or the
/// same face recognised in several camera setups.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DetectedFace {
    /// Where it was seen; an override with these finds it again.
    pub anchors: Vec<FaceAnchor>,
    /// A PNG data URL of the face.
    pub thumbnail: String,
    /// The speaker bound to it (pinned or from the evidence).
    pub speaker: Option<String>,
    /// The binding is trusted for framing.
    pub confident: bool,
    /// Camera setups (angles) the person was recognised in.
    pub views: u32,
    /// Seconds on screen in the clips.
    #[specta(type = specta_typescript::Number)]
    pub seconds: f64,
    /// Index of the request's override that applies to it.
    pub applied: Option<u32>,
}

/// Who is in the clips: who speaks when (the camera follows them), and the
/// user's word on faces.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cast<'a> {
    pub turns: &'a [SpeechTurn],
    pub faces: &'a [FaceOverride],
}

/// Anchors kept per person.
const MAX_ANCHORS: usize = 96;
/// Side of a face thumbnail (pixels).
const THUMBNAIL_SIZE: u32 = 96;
/// Faces on screen shorter than this (seconds) aren't listed.
const MIN_LISTED_SECONDS: f64 = 1.0;

/// Clips analysed together: their tracks with globally unique ids (equal to
/// their index) and shots.
struct ClipsAnalysis {
    source_size: (u32, u32),
    fps: f64,
    has_audio: bool,
    /// Width of the analysed frames; source pixels per analysis pixel.
    analysis_width: u32,
    scale: f64,
    tracks: Vec<Track>,
    /// Per clip, per range: the range without flash frames, and indices
    /// into `tracks`.
    range_tracks: Vec<Vec<RangeTracks>>,
}

impl ClipsAnalysis {
    /// Size of the analysed frames.
    fn analysis_size(&self) -> (f64, f64) {
        (
            f64::from(self.analysis_width),
            f64::from(self.source_size.1) / self.scale,
        )
    }
}

/// Analyse every range of `clips`. `on_progress` gets the analysed fraction
/// (0-1).
fn analyse_clips(
    input: &Path,
    clips: &[Vec<(f64, f64)>],
    cache: Option<&AnalysisCache>,
    run: Option<(u64, &RunControl)>,
    on_progress: &mut dyn FnMut(f64),
) -> Result<ClipsAnalysis, String> {
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

    let total_seconds: f64 = clips
        .iter()
        .flatten()
        .map(|(start, end)| end - start + 2.0 * ANALYSIS_MARGIN)
        .sum::<f64>()
        .max(1e-6);

    let source_key = crate::media_cache::source_key(input)?;
    let mut detector = None;
    let mut tracks: Vec<Track> = Vec::new();
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
            let cuts = local
                .cuts
                .iter()
                .copied()
                .filter(|&cut| kept.0 < cut && cut < kept.1)
                .collect();
            per_range.push((kept, indices, cuts));
        }
        range_tracks.push(per_range);
    }
    Ok(ClipsAnalysis {
        source_size: (source_w, source_h),
        fps,
        has_audio: media.audio.is_some(),
        analysis_width: analysis_w,
        scale,
        tracks,
        range_tracks,
    })
}

/// Whether `anchor` lands on `track`: a face of it at that moment covers
/// the point. `size` is the analysed picture's.
fn anchored(track: &Track, anchor: &FaceAnchor, size: (f64, f64)) -> bool {
    let (x, y) = (anchor.x * size.0, anchor.y * size.1);
    track.observations.iter().any(|o| {
        let face = &o.face;
        (o.time - anchor.time).abs() <= 1.0 / ANALYSIS_FPS
            && (f64::from(face.x)..=f64::from(face.x + face.width)).contains(&x)
            && (f64::from(face.y)..=f64::from(face.y + face.height)).contains(&y)
    })
}

/// The anchors of a face seen as `tracks`: the middle of each track (at
/// most `MAX_ANCHORS`, spread over them).
fn anchors_of(tracks: &[&Track], size: (f64, f64)) -> Vec<FaceAnchor> {
    let step = tracks.len().div_ceil(MAX_ANCHORS).max(1);
    tracks
        .iter()
        .step_by(step)
        .map(|track| {
            let middle = &track.observations[track.observations.len() / 2];
            let (x, y) = middle.face.center();
            FaceAnchor {
                time: middle.time,
                x: f64::from(x) / size.0,
                y: f64::from(y) / size.1,
            }
        })
        .collect()
}

/// The user's overrides resolved to tracks.
struct Resolved {
    /// Per track (by index): the override that applies to it.
    applied: Vec<Option<usize>>,
    /// Per track: its (setup, seat).
    seats: Vec<(usize, usize)>,
}

impl Resolved {
    /// Overrides apply to whole seats: a face behind one anchor is the same
    /// face wherever its camera setup comes back. Later overrides win.
    fn new(tracks: &[Track], overrides: &[FaceOverride], size: (f64, f64)) -> Self {
        let seats = assign_seats(tracks);
        let mut by_seat: std::collections::HashMap<(usize, usize), usize> = Default::default();
        for (index, rule) in overrides.iter().enumerate() {
            for (track, &seat) in tracks.iter().zip(&seats) {
                if rule.anchors.iter().any(|a| anchored(track, a, size)) {
                    by_seat.insert(seat, index);
                }
            }
        }
        Self {
            applied: seats
                .iter()
                .map(|seat| by_seat.get(seat).copied())
                .collect(),
            seats,
        }
    }

    fn ignored(&self, overrides: &[FaceOverride], track: usize) -> bool {
        self.applied[track].is_some_and(|rule| overrides[rule].ignored)
    }

    fn pins(&self, overrides: &[FaceOverride]) -> Vec<Pin> {
        self.applied
            .iter()
            .enumerate()
            .filter_map(|(track, rule)| {
                let rule = &overrides[(*rule)?];
                let speaker = match &rule.speaker {
                    FaceSpeaker::Auto => return None,
                    FaceSpeaker::Named { name } => Some(name.clone()),
                    FaceSpeaker::Nobody => None,
                };
                (!rule.ignored).then_some(Pin { track, speaker })
            })
            .collect()
    }
}

/// Bind speakers to the faces the user didn't rule out. Returns the
/// bindings and which tracks are ignored (by index).
fn bind_with_overrides(
    analysis: &ClipsAnalysis,
    cast: Cast<'_>,
) -> (Vec<Binding>, Resolved, Vec<bool>) {
    let Cast {
        turns,
        faces: overrides,
    } = cast;
    let resolved = Resolved::new(&analysis.tracks, overrides, analysis.analysis_size());
    let ignored: Vec<bool> = (0..analysis.tracks.len())
        .map(|track| resolved.ignored(overrides, track))
        .collect();
    let kept: Vec<Track> = analysis
        .tracks
        .iter()
        .filter(|track| !ignored[track.id])
        .cloned()
        .collect();
    let bindings = bind_speakers(&kept, turns, &resolved.pins(overrides));
    (bindings, resolved, ignored)
}

/// One seat's face as listed: its tracks and its best view.
struct SeatFace<'a> {
    setup: usize,
    tracks: Vec<&'a Track>,
    seconds: f64,
    /// The largest, most frontal view: (time, face).
    best: (f64, Face),
}

/// How far the nose sits from the middle of the eyes, in eye distances
/// (0 = looking straight at the camera).
fn turned(face: &Face) -> f32 {
    let [right, left, nose, ..] = face.landmarks;
    let eyes = (left.0 - right.0).hypot(left.1 - right.1).max(1e-3);
    (nose.0 - (right.0 + left.0) / 2.0).abs() / eyes
}

/// The view of a seat to show and recognise: the most frontal face among
/// its larger ones (any of its tracks).
fn best_view(tracks: &[&Track]) -> (f64, Face) {
    let observations = || tracks.iter().flat_map(|t| t.observations.iter());
    let tallest = observations().map(|o| o.face.height).fold(0.0f32, f32::max);
    let view = observations()
        .filter(|o| o.face.height >= 0.8 * tallest)
        .min_by(|a, b| turned(&a.face).total_cmp(&turned(&b.face)))
        .expect("a seat has tracks");
    (view.time, view.face)
}

/// The people in `clips`, most seen first, with what binding (under the
/// cast's overrides) made of them. A person is a seat (a face's place in a
/// camera setup) or, with `embedder`, every seat whose face looks like
/// theirs: the same person in the wide shot and in their close-up.
pub(crate) fn detect_faces(
    input: &Path,
    clips: &[Vec<(f64, f64)>],
    cast: Cast<'_>,
    mut embedder: Option<&mut FaceEmbedder>,
    cache: Option<&AnalysisCache>,
    run: Option<(u64, &RunControl)>,
    on_progress: &mut dyn FnMut(f64),
) -> Result<Vec<DetectedFace>, String> {
    let analysis = analyse_clips(input, clips, cache, run, on_progress)?;
    let (bindings, resolved, _) = bind_with_overrides(&analysis, cast);
    let size = analysis.analysis_size();
    let min_affinity = CameraSettings::default().min_affinity;

    let mut seats: Vec<(usize, usize)> = resolved.seats.clone();
    seats.sort_unstable();
    seats.dedup();
    let mut faces: Vec<SeatFace> = Vec::new();
    for seat in seats {
        let tracks: Vec<&Track> = analysis
            .tracks
            .iter()
            .filter(|track| resolved.seats[track.id] == seat)
            .collect();
        let seconds: f64 = tracks.iter().map(|t| t.end() - t.start()).sum();
        if seconds >= MIN_LISTED_SECONDS {
            let best = best_view(&tracks);
            faces.push(SeatFace {
                setup: seat.0,
                tracks,
                seconds,
                best,
            });
        }
    }

    // One frame per seat: the thumbnail, and the embedding.
    let mut thumbnails = Vec::with_capacity(faces.len());
    let mut embeddings: Vec<Option<Embedding>> = Vec::with_capacity(faces.len());
    for face in &faces {
        let (time, view) = face.best;
        match frame_at(input, time, analysis.analysis_width, run) {
            Ok(frame) => {
                thumbnails.push(face_thumbnail(&frame, &view).unwrap_or_else(|error| {
                    log::warn!("vertical: no face thumbnail: {error}");
                    String::new()
                }));
                embeddings.push(embedder.as_mut().and_then(|embedder| {
                    embedder
                        .embed(&frame, &view)
                        .inspect_err(|error| log::warn!("vertical: no face embedding: {error}"))
                        .ok()
                }));
            }
            Err(error) => {
                log::warn!("vertical: no frame for a face: {error}");
                thumbnails.push(String::new());
                embeddings.push(None);
            }
        }
    }
    let people = group_people(
        &faces
            .iter()
            .zip(&embeddings)
            .map(|(face, embedding)| (face.setup, embedding.as_deref()))
            .collect::<Vec<_>>(),
        SAME_PERSON,
    );

    let count = people.iter().map(|p| p + 1).max().unwrap_or(0);
    let mut listed: Vec<DetectedFace> = (0..count)
        .map(|person| {
            let members: Vec<usize> = (0..faces.len()).filter(|&i| people[i] == person).collect();
            let tracks: Vec<&Track> = members
                .iter()
                .flat_map(|&i| faces[i].tracks.iter().copied())
                .collect();
            // Shown where the face is largest.
            let shown = *members
                .iter()
                .max_by(|&&a, &&b| faces[a].best.1.height.total_cmp(&faces[b].best.1.height))
                .expect("people have faces");
            // A trusted binding over an unsure one.
            let binding = bindings
                .iter()
                .filter(|b| tracks.iter().any(|t| t.id == b.track))
                .max_by(|a, b| a.affinity.total_cmp(&b.affinity));
            DetectedFace {
                anchors: anchors_of(&tracks, size),
                thumbnail: thumbnails[shown].clone(),
                speaker: binding.map(|b| b.speaker.clone()),
                confident: binding.is_some_and(|b| b.affinity >= min_affinity),
                views: members.len() as u32,
                seconds: members.iter().map(|&i| faces[i].seconds).sum(),
                applied: tracks
                    .iter()
                    .find_map(|t| resolved.applied[t.id])
                    .map(|rule| rule as u32),
            }
        })
        .collect();
    listed.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    Ok(listed)
}

/// A square PNG (data URL) of `face` in `frame` with some room around it.
fn face_thumbnail(frame: &Frame, face: &Face) -> Result<String, String> {
    use base64::Engine;
    let side = (face.width.max(face.height) * 1.6).min(frame.width.min(frame.height) as f32);
    let (cx, cy) = face.center();
    let clamp = |centre: f32, limit: u32| {
        (centre - side / 2.0).clamp(0.0, (limit as f32 - side).max(0.0)) as u32
    };
    let rect = CropRect {
        x: clamp(cx, frame.width),
        y: clamp(cy, frame.height),
        width: side as u32,
        height: side as u32,
    };
    let square = crop(frame, rect);
    let small = resize(&square, THUMBNAIL_SIZE);
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, small.width, small.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .and_then(|mut writer| writer.write_image_data(&small.data))
            .map_err(|e| format!("Failed to encode a face thumbnail: {e}"))?;
    }
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    ))
}

/// An RGB frame scaled to `side`x`side` by averaging boxes of pixels.
fn resize(frame: &Frame, side: u32) -> Frame {
    let (w, h) = (frame.width.max(1), frame.height.max(1));
    let mut data = Vec::with_capacity((side * side * 3) as usize);
    for y in 0..side {
        let (y0, y1) = (
            y * h / side,
            ((y + 1) * h / side).max(y * h / side + 1).min(h),
        );
        for x in 0..side {
            let (x0, x1) = (
                x * w / side,
                ((x + 1) * w / side).max(x * w / side + 1).min(w),
            );
            let mut sum = [0u32; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * frame.width + sx) * 3) as usize;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(frame.data[i + channel]);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)).max(1);
            data.extend(sum.iter().map(|total| (total / count) as u8));
        }
    }
    Frame {
        time: frame.time,
        width: side,
        height: side,
        data,
    }
}

/// Analyse and plan the framing of clips given as source ranges, with the
/// user's word on faces. Blocks. `on_progress` gets the analysed fraction
/// (0-1).
pub(crate) fn plan_vertical(
    input: &Path,
    clips: &[Vec<(f64, f64)>],
    cast: Cast<'_>,
    cuts: &CutStyle,
    cache: Option<&AnalysisCache>,
    run: Option<(u64, &RunControl)>,
    on_progress: &mut dyn FnMut(f64),
) -> Result<VerticalPlan, String> {
    // 1. Analyse every range.
    let analysis = analyse_clips(input, clips, cache, run, on_progress)?;
    let (source_w, source_h) = analysis.source_size;
    let (fps, scale) = (analysis.fps, analysis.scale);
    let camera_frame = CameraFrame {
        height: analysis.analysis_size().1,
        min_crop_height: f64::from(OUTPUT_SIZE.1) / MAX_UPSCALE / scale,
        fps,
    };

    // 2. Who is who, over all clips together. Ignored faces are gone from
    // here on: never framed, never a listener to cut away to.
    let (bindings, _, ignored) = bind_with_overrides(&analysis, cast);
    let turns = cast.turns;
    let tracks = &analysis.tracks;
    let range_tracks: Vec<Vec<RangeTracks>> = analysis
        .range_tracks
        .iter()
        .map(|per_range| {
            per_range
                .iter()
                .map(|(range, indices, cuts)| {
                    let kept = indices.iter().copied().filter(|&i| !ignored[i]).collect();
                    (*range, kept, cuts.clone())
                })
                .collect()
        })
        .collect();

    // 3. Camera path per range, cutaways over jump cuts, then the cuts'
    // punch-ins and transitions.
    let settings = CameraSettings::default();
    let aspect = f64::from(OUTPUT_SIZE.0) / f64::from(OUTPUT_SIZE.1);
    let to_source = |key: CropKey| CropKey {
        time: key.time,
        center_x: key.center_x * scale,
        center_y: key.center_y * scale,
        height: key.height * scale,
    };
    let planned = range_tracks
        .iter()
        .map(|per_range| {
            let range_tracks: Vec<Vec<Track>> = per_range
                .iter()
                .map(|(_, indices, _)| indices.iter().map(|&i| tracks[i].clone()).collect())
                .collect();
            // Ranges joined by jump cuts are planned together, as one shot.
            let stretches: Vec<Stretch> = per_range
                .iter()
                .zip(&range_tracks)
                .map(|((range, _, cuts), local)| Stretch {
                    range: *range,
                    tracks: local,
                    cuts,
                })
                .collect();
            let ranges: Vec<(f64, f64)> = stretches.iter().map(|s| s.range).collect();
            let mut per_piece: Vec<Vec<VerticalRange>> = jump_cut_groups(&ranges)
                .into_iter()
                .flat_map(|group| {
                    plan_camera(
                        &stretches[group],
                        &bindings,
                        turns,
                        &camera_frame,
                        aspect,
                        &settings,
                    )
                })
                .map(|pieces| {
                    pieces
                        .into_iter()
                        .map(|piece| {
                            let framing = match piece.framing {
                                Framing::Fit => Framing::Fit,
                                Framing::Follow(keys) => {
                                    Framing::Follow(keys.into_iter().map(to_source).collect())
                                }
                            };
                            VerticalRange::new(piece.start, piece.end, framing)
                        })
                        .collect()
                })
                .collect();
            if cuts.cutaways {
                insert_cutaways(
                    &mut per_piece,
                    &range_tracks,
                    &mut |a, b| morph_rect(input, a, b, (source_w, source_h), fps, run).is_some(),
                    &|tracks, window, subject_x| {
                        listener_framing(
                            tracks,
                            window,
                            subject_x / scale,
                            &camera_frame,
                            &settings,
                        )
                        .map(|((x, y, h), face)| {
                            (
                                CropKey {
                                    time: 0.0,
                                    center_x: x * scale,
                                    center_y: y * scale,
                                    height: h * scale,
                                },
                                (face.0 * scale, face.1 * scale),
                            )
                        })
                    },
                );
            }
            per_piece.into_iter().flatten().collect::<Vec<_>>()
        })
        .map(|mut pieces| {
            snap_pieces(&mut pieces, fps);
            dress_cuts(&mut pieces, cuts, &mut |a, b| {
                morph_rect(input, a, b, (source_w, source_h), fps, run).is_some()
            });
            pieces
        })
        .collect();

    Ok(VerticalPlan {
        source_size: (source_w, source_h),
        fps,
        has_audio: analysis.has_audio,
        clips: planned,
        bindings,
    })
}

/// Pieces on the source's frame grid, so each one's video and audio are
/// the same length (whole frames); otherwise the video of every piece can
/// run up to a frame long and the audio drifts behind over many cuts.
/// Pieces that snap to nothing are dropped.
fn snap_pieces(pieces: &mut Vec<VerticalRange>, fps: f64) {
    for piece in pieces.iter_mut() {
        piece.start = crate::time_utils::snap_to_frame(piece.start, fps);
        piece.end = crate::time_utils::snap_to_frame(piece.end, fps);
    }
    pieces.retain(|piece| piece.end > piece.start);
}

/// Source jumps up to this long (seconds, forward) are jump cuts within one
/// moment, e.g. a tightened pause; longer or backward ones are splices.
const JUMP_CUT: f64 = 3.0;

/// Each side of a cutaway shows the listener this long (seconds).
const CUTAWAY_SIDE: f64 = 0.7;
/// At most one cutaway per this much output (seconds).
const CUTAWAY_SPACING: f64 = 8.0;

/// A listener framing for a window: (static crop key, face centre), in
/// source pixels, from the tracks of a range.
type ListenerFraming<'a> = dyn Fn(&[Track], (f64, f64), f64) -> Option<(CropKey, (f64, f64))> + 'a;

/// Consecutive runs of `ranges` joined by jump cuts (the next one starts at
/// most `JUMP_CUT` after the previous one ends), as index ranges.
fn jump_cut_groups(ranges: &[(f64, f64)]) -> Vec<std::ops::Range<usize>> {
    let mut groups = Vec::new();
    let mut first = 0;
    for i in 1..=ranges.len() {
        let joined = i < ranges.len() && {
            let gap = ranges[i].0 - ranges[i - 1].1;
            (-1e-3..=JUMP_CUT).contains(&gap)
        };
        if !joined {
            groups.push(first..i);
            first = i;
        }
    }
    groups
}

/// Hide jump cuts that a morph can't by cutting away to a listener: the last
/// `CUTAWAY_SIDE` before the cut and the first after it show another face
/// in the shot, the speaker's audio running on underneath. `per_range[r]`
/// are range `r`'s pieces; `tracks[r]` its face tracks.
fn insert_cutaways(
    per_range: &mut [Vec<VerticalRange>],
    tracks: &[Vec<Track>],
    morphable: &mut dyn FnMut(&VerticalRange, &VerticalRange) -> bool,
    listener: &ListenerFraming<'_>,
) {
    let mut since_last = CUTAWAY_SPACING;
    for r in 0..per_range.len().saturating_sub(1) {
        let (Some(a), Some(b)) = (per_range[r].last(), per_range[r + 1].first()) else {
            continue;
        };
        since_last += per_range[r].iter().map(|p| p.end - p.start).sum::<f64>();
        let gap = b.start - a.end;
        let jump = gap > 0.0 && gap <= JUMP_CUT;
        let long_enough = |p: &VerticalRange| p.end - p.start >= CUTAWAY_SIDE + 1.0;
        let (Framing::Follow(keys_a), Framing::Follow(keys_b)) = (&a.framing, &b.framing) else {
            continue;
        };
        if !jump || since_last < CUTAWAY_SPACING || !long_enough(a) || !long_enough(b) {
            continue;
        }
        if morphable(a, b) {
            continue;
        }
        let subject_a = keys_a.last().map_or(0.0, |k| k.center_x);
        let subject_b = keys_b.first().map_or(0.0, |k| k.center_x);
        let Some((key_a, face_a)) = listener(&tracks[r], (a.end - CUTAWAY_SIDE, a.end), subject_a)
        else {
            continue;
        };
        let Some((key_b, face_b)) =
            listener(&tracks[r + 1], (b.start, b.start + CUTAWAY_SIDE), subject_b)
        else {
            continue;
        };
        // The same person on both sides (a static camera).
        let face_height = key_a.height * 0.22;
        if (face_a.0 - face_b.0).hypot(face_a.1 - face_b.1) > 0.5 * face_height {
            continue;
        }

        let a = per_range[r].pop().expect("checked");
        let split_a = a.end - CUTAWAY_SIDE;
        let mut before = VerticalRange::new(a.start, split_a, a.framing.clone());
        before.zoom = a.zoom;
        let mut away_a = VerticalRange::new(split_a, a.end, Framing::Follow(vec![key_a]));
        away_a.cutaway = true;
        per_range[r].push(before);
        per_range[r].push(away_a);

        let b = per_range[r + 1].remove(0);
        let split_b = b.start + CUTAWAY_SIDE;
        let mut away_b = VerticalRange::new(b.start, split_b, Framing::Follow(vec![key_b]));
        away_b.cutaway = true;
        let rest = match &b.framing {
            Framing::Follow(keys) => Framing::Follow(
                keys.iter()
                    .map(|key| CropKey {
                        time: key.time - CUTAWAY_SIDE,
                        ..*key
                    })
                    .collect(),
            ),
            Framing::Fit => Framing::Fit,
        };
        per_range[r + 1].insert(0, VerticalRange::new(split_b, b.end, rest));
        per_range[r + 1].insert(0, away_b);
        since_last = -CUTAWAY_SIDE;
    }
}

/// How cuts are dressed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CutStyle {
    /// Punch-in on every other jump cut (1.0 = none).
    pub punch: f64,
    /// Transitions into spliced-in moments, taken in turn (none: cuts).
    pub splices: Vec<Transition>,
    /// Hide jump cuts with a morph where the two sides are similar enough.
    pub morph: bool,
    /// Otherwise hide them by cutting away to a listener.
    pub cutaways: bool,
}

impl From<Intensity> for CutStyle {
    fn from(intensity: Intensity) -> Self {
        let (punch, splice, morph, cutaways) = match intensity {
            Intensity::Off => (1.0, Transition::Cut, false, false),
            Intensity::Chill => (1.0, Transition::Fade, true, false),
            Intensity::Punchy => (1.12, Transition::Whip, true, true),
            Intensity::Hyper => (1.18, Transition::Whip, true, true),
        };
        Self {
            punch,
            splices: vec![splice],
            morph,
            cutaways,
        }
    }
}

impl CutStyle {
    /// The style for `intensity`, with the user's choice of splice
    /// transitions if they made one.
    pub(crate) fn new(intensity: Intensity, splices: Option<&[Transition]>) -> Self {
        let mut style = Self::from(intensity);
        if let Some(splices) = splices {
            // Morphs only bridge jump cuts.
            style.splices = splices
                .iter()
                .copied()
                .filter(|t| *t != Transition::Morph)
                .collect();
        }
        style
    }
}

/// Punch-ins and transitions for a clip's pieces (in playback order).
/// A jump cut becomes a morph where `morphable(before, after)` allows it
/// (keeping the zoom); otherwise jump cuts alternate between the normal
/// framing and the punch-in, so a run of them reads as deliberate. A splice
/// gets the next of `style.splices` and resets the punch. Pieces that continue the same
/// source moment (a framing change) keep the zoom.
pub(crate) fn dress_cuts(
    pieces: &mut [VerticalRange],
    style: &CutStyle,
    morphable: &mut dyn FnMut(&VerticalRange, &VerticalRange) -> bool,
) {
    let mut punched = false;
    let mut splices = style.splices.iter().cycle();
    for i in 1..pieces.len() {
        let gap = pieces[i].start - pieces[i - 1].end;
        if gap.abs() < 1e-3 {
            pieces[i].zoom = pieces[i - 1].zoom;
        } else if gap > 0.0 && gap <= JUMP_CUT {
            let mut candidate = pieces[i].clone();
            candidate.zoom = pieces[i - 1].zoom;
            if style.morph && morphable(&pieces[i - 1], &candidate) {
                pieces[i].zoom = pieces[i - 1].zoom;
                pieces[i].transition = Transition::Morph;
            } else if pieces[i].cutaway && pieces[i - 1].cutaway {
                // The cutaway hides this one; the speaker comes back at the
                // same zoom.
                pieces[i].zoom = pieces[i - 1].zoom;
            } else {
                punched = !punched && style.punch > 1.0;
                pieces[i].zoom = if punched { style.punch } else { 1.0 };
            }
        } else {
            punched = false;
            pieces[i].zoom = 1.0;
            pieces[i].transition = splices.next().copied().unwrap_or_default();
        }
    }
}

/// The crop both sides of a jump cut share at the cut, if a morph can hide
/// it: both follow a face with (nearly) the same crop, and the pictures there
/// are similar. `b` already carries the zoom it would get.
fn morph_rect(
    input: &Path,
    a: &VerticalRange,
    b: &VerticalRange,
    source: (u32, u32),
    fps: f64,
    run: Option<(u64, &RunControl)>,
) -> Option<CropRect> {
    let (Framing::Follow(keys_a), Framing::Follow(keys_b)) = (&a.framing, &b.framing) else {
        return None;
    };
    if a.end - a.start <= 0.5 || b.end - b.start <= 0.5 {
        return None;
    }
    let half = MORPH_FRAMES as f64 / fps / 2.0;
    let aspect = f64::from(OUTPUT_SIZE.0) / f64::from(OUTPUT_SIZE.1);
    let zoomed = |keys: &[CropKey], zoom: f64| -> Vec<CropKey> {
        keys.iter()
            .map(|key| CropKey {
                height: key.height / zoom.max(1.0),
                ..*key
            })
            .collect()
    };
    let rect_a = crop_at(
        &zoomed(keys_a, a.zoom),
        a.end - a.start - half,
        source,
        aspect,
    );
    let rect_b = crop_at(&zoomed(keys_b, b.zoom), half, source, aspect);
    let near = |p: u32, q: u32, size: u32| f64::from(p.abs_diff(q)) <= 0.05 * f64::from(size);
    if !near(rect_a.x, rect_b.x, rect_a.width)
        || !near(rect_a.y, rect_b.y, rect_a.height)
        || !near(rect_a.height, rect_b.height, rect_a.height)
    {
        return None;
    }
    let (before, after) = morph_frames_around(input, a, b, rect_a, source, fps, run).ok()?;
    (luma_difference(&before, &after) <= MAX_DIFFERENCE).then_some(rect_a)
}

/// The last frame kept before a morph and the first one after it, cropped.
fn morph_frames_around(
    input: &Path,
    a: &VerticalRange,
    b: &VerticalRange,
    rect: CropRect,
    source: (u32, u32),
    fps: f64,
    run: Option<(u64, &RunControl)>,
) -> Result<(Frame, Frame), String> {
    let half = MORPH_FRAMES as f64 / fps / 2.0;
    let before = frame_at(input, a.end - half - 1.0 / fps, source.0, run)?;
    let after = frame_at(input, b.start + half, source.0, run)?;
    Ok((crop(&before, rect), crop(&after, rect)))
}

/// How vertical clips are rendered.
#[derive(Debug, Clone)]
pub(crate) struct RenderOptions<'a> {
    pub quality: ExportQuality,
    pub cuts: CutStyle,
    /// The morph model; without it morph cuts render as plain cuts.
    pub morph_model: Option<&'a Path>,
    /// Burned-in captions: transcript words on the source timeline.
    pub captions: Option<(&'a [TimedWord], CaptionStyle)>,
}

/// Export `clips` as vertical videos. Blocks; run it on the blocking pool.
pub(crate) fn export_vertical(
    input: &Path,
    clips: &[VerticalClip],
    cast: Cast<'_>,
    options: RenderOptions<'_>,
    cache: Option<&AnalysisCache>,
    run: Option<(u64, &RunControl)>,
    on_progress: &mut Progress<'_>,
) -> Result<VerticalPlan, String> {
    let ranges: Vec<Vec<(f64, f64)>> = clips.iter().map(|clip| clip.ranges.clone()).collect();
    let plan = plan_vertical(
        input,
        &ranges,
        cast,
        &options.cuts,
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

    // Interpolate the morph cuts (or fall back to plain cuts).
    let mut plan = plan;
    let mut morpher = match options.morph_model {
        Some(model)
            if plan
                .clips
                .iter()
                .flatten()
                .any(|p| p.transition == Transition::Morph) =>
        {
            match Morpher::new(model) {
                Ok(morpher) => Some(morpher),
                Err(error) => {
                    log::warn!("vertical: morph cuts unavailable: {error}");
                    None
                }
            }
        }
        _ => None,
    };
    let (source_size, fps) = (plan.source_size, plan.fps);
    for pieces in &mut plan.clips {
        for i in 1..pieces.len() {
            if pieces[i].transition != Transition::Morph {
                continue;
            }
            let frames = morpher.as_mut().and_then(|morpher| {
                let rect = morph_rect(input, &pieces[i - 1], &pieces[i], source_size, fps, run)?;
                let (before, after) = morph_frames_around(
                    input,
                    &pieces[i - 1],
                    &pieces[i],
                    rect,
                    source_size,
                    fps,
                    run,
                )
                .ok()?;
                morpher.between(&before, &after, MORPH_FRAMES).ok()
            });
            match frames {
                Some(frames) => pieces[i].morph = Some(frames),
                None => pieces[i].transition = Transition::Cut,
            }
        }
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

        // A cover from the first stretch that follows a face (a moment with
        // a person in it), at its middle.
        if let Some(title) = clip.title.as_deref().filter(|t| !t.trim().is_empty()) {
            let framing = pieces
                .iter()
                .find(|p| matches!(p.framing, Framing::Follow(_)) && p.end - p.start >= 1.0)
                .or(pieces.first());
            if let Some(range) = framing {
                let cover = clip.output.with_extension("jpg");
                if let Err(error) = render_cover(
                    &Cover {
                        input,
                        range,
                        time: (range.start + range.end) / 2.0,
                        source_size: plan.source_size,
                        output_size: OUTPUT_SIZE,
                        title,
                        output: &cover,
                    },
                    run,
                ) {
                    log::warn!("vertical: no cover for {}: {error}", cover.display());
                }
            }
        }
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
        dress_cuts(
            &mut pieces,
            &CutStyle::from(Intensity::Punchy),
            &mut |_, _| false,
        );
        let zooms: Vec<f64> = pieces.iter().map(|p| p.zoom).collect();
        assert_eq!(zooms, [1.0, 1.12, 1.12, 1.0, 1.12, 1.0, 1.12]);
        let transitions: Vec<Transition> = pieces.iter().map(|p| p.transition).collect();
        assert_eq!(transitions[5], Transition::Whip);
        assert!(transitions
            .iter()
            .enumerate()
            .all(|(i, t)| i == 5 || *t == Transition::Cut));

        let mut calm = vec![piece(10.0, 12.0), piece(12.3, 14.0), piece(30.0, 32.0)];
        dress_cuts(&mut calm, &CutStyle::from(Intensity::Chill), &mut |_, _| {
            false
        });
        assert!(calm.iter().all(|p| p.zoom == 1.0));
        assert_eq!(calm[2].transition, Transition::Fade);
    }

    #[test]
    fn jump_cuts_group_the_ranges_planned_together() {
        let ranges = [
            (0.0, 2.0),
            (2.3, 4.0),
            // Touching: a framing change in the same moment.
            (4.0, 5.0),
            // A splice, backwards and then forwards beyond a jump cut.
            (1.0, 1.5),
            (9.0, 10.0),
        ];
        assert_eq!(jump_cut_groups(&ranges), [0..3, 3..4, 4..5]);
    }

    #[test]
    fn splices_take_the_chosen_transitions_in_turn() {
        let splices = [Transition::BlurZoom, Transition::Morph, Transition::Flash];
        let style = CutStyle::new(Intensity::Punchy, Some(&splices));
        assert_eq!(style.splices, [Transition::BlurZoom, Transition::Flash]);
        let mut pieces = vec![
            piece(10.0, 12.0),
            piece(30.0, 32.0),
            piece(50.0, 52.0),
            piece(5.0, 7.0),
            piece(7.3, 9.0),
        ];
        dress_cuts(&mut pieces, &style, &mut |_, _| false);
        let transitions: Vec<Transition> = pieces.iter().map(|p| p.transition).collect();
        assert_eq!(
            transitions,
            [
                Transition::Cut,
                Transition::BlurZoom,
                Transition::Flash,
                Transition::BlurZoom,
                Transition::Cut,
            ]
        );

        // None chosen: plain cuts.
        let mut pieces = vec![piece(10.0, 12.0), piece(30.0, 32.0)];
        dress_cuts(
            &mut pieces,
            &CutStyle::new(Intensity::Punchy, Some(&[])),
            &mut |_, _| false,
        );
        assert_eq!(pieces[1].transition, Transition::Cut);
    }

    fn follow(start: f64, end: f64, x: f64) -> VerticalRange {
        VerticalRange::new(
            start,
            end,
            Framing::Follow(vec![CropKey {
                time: 0.0,
                center_x: x,
                center_y: 300.0,
                height: 500.0,
            }]),
        )
    }

    #[test]
    fn jump_cuts_that_cannot_morph_cut_away_to_the_listener() {
        let mut per_range = vec![
            vec![follow(10.0, 14.0, 300.0)],
            vec![follow(14.4, 18.0, 300.0)],
        ];
        let listener_key = CropKey {
            time: 0.0,
            center_x: 900.0,
            center_y: 300.0,
            height: 500.0,
        };
        insert_cutaways(
            &mut per_range,
            &[Vec::new(), Vec::new()],
            &mut |_, _| false,
            &|_, _, _| Some((listener_key, (900.0, 200.0))),
        );
        let spans: Vec<Vec<(f64, f64, bool)>> = per_range
            .iter()
            .map(|pieces| pieces.iter().map(|p| (p.start, p.end, p.cutaway)).collect())
            .collect();
        assert_eq!(
            spans,
            [
                vec![(10.0, 13.3, false), (13.3, 14.0, true)],
                vec![(14.4, 15.1, true), (15.1, 18.0, false)],
            ]
        );
        // The speaker's framing continues after the cutaway, keys shifted.
        let Framing::Follow(keys) = &per_range[1][1].framing else {
            panic!("follows the speaker");
        };
        assert!((keys[0].time + CUTAWAY_SIDE).abs() < 1e-9);

        // Dressing: no punch-in across the cutaway, the speaker returns at
        // the same zoom.
        let mut pieces: Vec<VerticalRange> = per_range.into_iter().flatten().collect();
        dress_cuts(
            &mut pieces,
            &CutStyle::from(Intensity::Punchy),
            &mut |_, _| false,
        );
        assert!(
            pieces.iter().all(|p| p.zoom == 1.0),
            "{:?}",
            pieces.iter().map(|p| p.zoom).collect::<Vec<_>>()
        );
    }

    #[test]
    fn no_cutaway_where_a_morph_works_or_nobody_listens() {
        let ranges = || {
            vec![
                vec![follow(10.0, 14.0, 300.0)],
                vec![follow(14.4, 18.0, 300.0)],
            ]
        };
        let key = CropKey {
            time: 0.0,
            center_x: 900.0,
            center_y: 300.0,
            height: 500.0,
        };
        let mut morphs = ranges();
        insert_cutaways(
            &mut morphs,
            &[Vec::new(), Vec::new()],
            &mut |_, _| true,
            &|_, _, _| Some((key, (900.0, 200.0))),
        );
        assert_eq!(morphs[0].len(), 1);
        let mut alone = ranges();
        insert_cutaways(
            &mut alone,
            &[Vec::new(), Vec::new()],
            &mut |_, _| false,
            &|_, _, _| None,
        );
        assert_eq!(alone[0].len(), 1);
    }

    #[test]
    fn the_preview_carries_caption_chunks_for_what_is_shown() {
        let plan = VerticalPlan {
            source_size: (1280, 720),
            fps: 25.0,
            has_audio: true,
            clips: vec![vec![piece(10.0, 12.0), piece(13.0, 14.0)]],
            bindings: Vec::new(),
        };
        let word = |start: f64, end: f64, text: &str| TimedWord {
            start,
            end,
            text: text.to_string(),
        };
        let words = [
            word(10.0, 10.4, "Erst"),
            word(10.4, 10.8, "das."),
            // Cut away by tightening: not captioned.
            word(12.2, 12.6, "weg"),
            word(13.1, 13.5, "Dann"),
        ];
        let preview = VerticalPreview::from(&plan).with_captions(&words, &CaptionStyle::default());
        let chunks: Vec<Vec<&str>> = preview.captions[0]
            .iter()
            .map(|chunk| chunk.iter().map(|w| w.text.as_str()).collect())
            .collect();
        assert_eq!(chunks, [vec!["Erst", "das."], vec!["Dann"]]);
    }

    /// A still 60 px face at `x` in `shot`, sampled at 10 fps over `range`.
    fn face_track(id: usize, shot: usize, x: f32, range: (f64, f64)) -> Track {
        let face = Face {
            x,
            y: 100.0,
            width: 60.0,
            height: 60.0,
            score: 0.9,
            landmarks: [(x + 30.0, 130.0); 5],
        };
        let observations = (0..)
            .map(|i| range.0 + f64::from(i) / 10.0)
            .take_while(|&time| time < range.1)
            .map(|time| Observation {
                time,
                face,
                mouth_motion: Some(0.1),
            })
            .collect();
        Track::for_tests(id, shot, observations)
    }

    #[test]
    fn an_ignored_face_is_ignored_wherever_its_seat_comes_back() {
        // One camera setup cut to twice: a person at x 100, a face on a TV
        // at x 500. The user ignored the TV as seen in the first shot only.
        let analysis = ClipsAnalysis {
            source_size: (1280, 720),
            fps: 25.0,
            has_audio: true,
            analysis_width: 1280,
            scale: 1.0,
            tracks: vec![
                face_track(0, 0, 100.0, (0.0, 5.0)),
                face_track(1, 0, 500.0, (0.0, 5.0)),
                face_track(2, 1, 102.0, (20.0, 25.0)),
                face_track(3, 1, 498.0, (20.0, 25.0)),
            ],
            range_tracks: Vec::new(),
        };
        let size = analysis.analysis_size();
        let tv = anchors_of(&[&analysis.tracks[1]], size);
        assert!(anchored(&analysis.tracks[1], &tv[0], size));
        assert!(!anchored(&analysis.tracks[0], &tv[0], size));

        let overrides = [FaceOverride {
            anchors: tv,
            ignored: true,
            speaker: FaceSpeaker::Auto,
        }];
        let (_, resolved, ignored) = bind_with_overrides(
            &analysis,
            Cast {
                turns: &[],
                faces: &overrides,
            },
        );
        assert_eq!(ignored, [false, true, false, true]);
        assert_eq!(resolved.applied, [None, Some(0), None, Some(0)]);
    }

    #[test]
    fn a_named_face_is_bound_without_evidence() {
        let analysis = ClipsAnalysis {
            source_size: (1280, 720),
            fps: 25.0,
            has_audio: true,
            analysis_width: 1280,
            scale: 1.0,
            tracks: vec![
                face_track(0, 0, 100.0, (0.0, 5.0)),
                face_track(1, 0, 500.0, (0.0, 5.0)),
            ],
            range_tracks: Vec::new(),
        };
        let overrides = [FaceOverride {
            anchors: anchors_of(&[&analysis.tracks[0]], analysis.analysis_size()),
            ignored: false,
            speaker: FaceSpeaker::Named {
                name: "Host".to_string(),
            },
        }];
        let turns = [SpeechTurn {
            start: 0.0,
            end: 5.0,
            speaker: "Host".to_string(),
        }];
        let (bindings, _, _) = bind_with_overrides(
            &analysis,
            Cast {
                turns: &turns,
                faces: &overrides,
            },
        );
        assert_eq!(bindings.len(), 1);
        assert_eq!(
            (bindings[0].track, bindings[0].speaker.as_str()),
            (0, "Host")
        );
    }

    #[test]
    fn the_shown_view_is_large_and_frontal() {
        let face = |height: f32, nose_x: f32| Face {
            x: 0.0,
            y: 0.0,
            width: height,
            height,
            score: 0.9,
            landmarks: [
                (30.0, 40.0),
                (70.0, 40.0),
                (nose_x, 60.0),
                (35.0, 80.0),
                (65.0, 80.0),
            ],
        };
        let observation = |time: f64, face: Face| Observation {
            time,
            face,
            mouth_motion: None,
        };
        let a = Track::for_tests(
            0,
            0,
            vec![
                observation(1.0, face(100.0, 68.0)),
                observation(2.0, face(60.0, 50.0)),
            ],
        );
        let b = Track::for_tests(1, 0, vec![observation(5.0, face(95.0, 52.0))]);
        // Frontal but small loses; the frontal large one of the other track wins.
        assert_eq!(best_view(&[&a, &b]).0, 5.0);
    }

    #[test]
    fn face_thumbnails_are_small_squares() {
        let frame = Frame {
            time: 0.0,
            width: 320,
            height: 180,
            data: vec![128; 320 * 180 * 3],
        };
        let face = Face {
            x: 280.0,
            y: 10.0,
            width: 40.0,
            height: 50.0,
            score: 0.9,
            landmarks: [(300.0, 30.0); 5],
        };
        let small = resize(
            &crop(
                &frame,
                CropRect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 80,
                },
            ),
            96,
        );
        assert_eq!(
            (small.width, small.height, small.data.len()),
            (96, 96, 96 * 96 * 3)
        );
        assert!(small.data.iter().all(|&v| v == 128));
        let url = face_thumbnail(&frame, &face).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn pieces_are_snapped_to_frames_and_slivers_dropped() {
        let mut pieces = vec![
            piece(10.345, 12.019),
            piece(12.019, 12.03),
            piece(20.0, 21.0),
        ];
        snap_pieces(&mut pieces, 30.0);
        let bounds: Vec<(f64, f64)> = pieces.iter().map(|p| (p.start, p.end)).collect();
        assert_eq!(
            bounds,
            [(310.0 / 30.0, 361.0 / 30.0), (20.0, 21.0)],
            "the 11 ms sliver snaps to nothing"
        );
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
        // SHORTS_PROFILE_TIGHTENED: an exported clip's .json; its tightened
        // ranges are rendered as they are (to reproduce an export).
        let ranges: Vec<(f64, f64)> = match std::env::var_os("SHORTS_PROFILE_TIGHTENED") {
            Some(path) => {
                let json: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                serde_json::from_value(json["tightened"].clone()).unwrap()
            }
            None => crate::tighten::tighten_clips(
                Path::new(&source),
                &[ranges],
                &words,
                intensity,
                &[],
                None,
            )
            .unwrap()
            .remove(0),
        };
        let removed = crate::tighten::removed_words(&words, intensity);
        let words: Vec<TimedWord> = words.into_iter().filter(|w| !removed.contains(w)).collect();
        println!("{intensity:?}: {} ranges", ranges.len());
        let rife = std::env::var_os("SHORTS_RIFE")
            .map_or_else(|| PathBuf::from("/tmp/rife/rife49.onnx"), PathBuf::from);
        let output = PathBuf::from(keep).join("vertical_export.mp4");
        let started = std::time::Instant::now();
        let mut last = String::new();
        let plan = export_vertical(
            Path::new(&source),
            &[VerticalClip {
                ranges: ranges.clone(),
                output: output.clone(),
                title: Some("Wie weit vertrauen Patienten der KI?".to_string()),
            }],
            Cast {
                turns: &turns,
                faces: &[],
            },
            RenderOptions {
                quality: ExportQuality::Balanced,
                cuts: CutStyle::from(intensity),
                morph_model: Some(&rife),
                captions: Some((&words, CaptionStyle::default())),
            },
            None,
            None,
            &mut |_, message| last = message,
        )
        .unwrap();
        for piece in &plan.clips[0] {
            match &piece.framing {
                Framing::Fit => println!(
                    "{:.1}-{:.1}: fit, zoom {:.2}, {:?}{}",
                    piece.start,
                    piece.end,
                    piece.zoom,
                    piece.transition,
                    if piece.cutaway { " cutaway" } else { "" }
                ),
                Framing::Follow(keys) => {
                    let speeds: Vec<f64> = keys
                        .windows(2)
                        .filter(|w| w[1].time - w[0].time > 0.01)
                        .map(|w| (w[1].center_x - w[0].center_x) / (w[1].time - w[0].time))
                        .collect();
                    let fastest = speeds.iter().fold(0.0f64, |m, v| m.max(v.abs()));
                    println!(
                        "{:.1}-{:.1}: follow, {} keys, fastest pan {fastest:.0} px/s, zoom {:.2}, {:?}{}",
                        piece.start,
                        piece.end,
                        keys.len(),
                        piece.zoom,
                        piece.transition,
                        piece
                            .morph
                            .as_ref()
                            .map_or(String::new(), |f| format!(" ({} frames)", f.len()))
                            + if piece.cutaway { " cutaway" } else { "" }
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
        let pieces = &plan.clips[0];
        println!(
            "pieces: {} s in total, {} s planned output",
            pieces.iter().map(|p| p.end - p.start).sum::<f64>(),
            crate::reframe::output_duration(pieces)
        );
        let seconds: f64 = ranges.iter().map(|(s, e)| e - s).sum();
        println!(
            "{seconds:.0} s clip in {:.1} s ({last}) -> {}",
            started.elapsed().as_secs_f64(),
            output.display()
        );
    }

    /// Lists the faces of clips on a real recording and writes each one's
    /// thumbnail to `SHORTS_PROFILE_KEEP/face_NN.png`.
    /// `SHORTS_PROFILE_SOURCE=… SHORTS_PROFILE_TRANSCRIPT=… SHORTS_PROFILE_KEEP=…
    ///  SHORTS_PROFILE_RANGES=830-870,1200-1260
    ///  cargo test --release --lib faces_of_a_recording -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn faces_of_a_recording() {
        use base64::Engine;
        let (Some(source), Some(transcript), Some(keep)) = (
            std::env::var_os("SHORTS_PROFILE_SOURCE"),
            std::env::var_os("SHORTS_PROFILE_TRANSCRIPT"),
            std::env::var_os("SHORTS_PROFILE_KEEP"),
        ) else {
            eprintln!("SHORTS_PROFILE_SOURCE / _TRANSCRIPT / _KEEP not set; skipping");
            return;
        };
        let clips: Vec<Vec<(f64, f64)>> = std::env::var("SHORTS_PROFILE_RANGES")
            .unwrap_or_else(|_| "830-870".to_string())
            .split(',')
            .map(|range| {
                let (start, end) = range.split_once('-').unwrap();
                vec![(start.parse().unwrap(), end.parse().unwrap())]
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
        // Twice: cold, then with the analysis cached (as after a preview).
        // Faces are grouped into people with SHORTS_SFACE (the model file).
        let mut embedder = std::env::var_os("SHORTS_SFACE")
            .map(|model| FaceEmbedder::new(Path::new(&model)).unwrap());
        let cache = AnalysisCache::default();
        let mut faces = Vec::new();
        for pass in ["cold", "cached"] {
            let started = std::time::Instant::now();
            faces = detect_faces(
                Path::new(&source),
                &clips,
                Cast {
                    turns: &turns,
                    faces: &[],
                },
                embedder.as_mut(),
                Some(&cache),
                None,
                &mut |_| {},
            )
            .unwrap();
            println!(
                "{pass}: {} faces in {:.1} s",
                faces.len(),
                started.elapsed().as_secs_f64()
            );
        }
        let keep = PathBuf::from(keep);
        for (index, face) in faces.iter().enumerate() {
            println!(
                "face {index:>2}: {} views {:>5.1} s on screen, {} anchors, speaker {:?}{}",
                face.views,
                face.seconds,
                face.anchors.len(),
                face.speaker,
                if face.confident { "" } else { " (unsure)" }
            );
            if let Some(data) = face.thumbnail.strip_prefix("data:image/png;base64,") {
                let png = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap();
                std::fs::write(keep.join(format!("face_{index:02}.png")), png).unwrap();
            }
        }
    }
}
