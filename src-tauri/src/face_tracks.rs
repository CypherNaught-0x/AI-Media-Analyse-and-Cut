//! Face tracks (shorts phase S2): detections linked over time within a shot,
//! each with a mouth-motion signal used to tell who is speaking.
//!
//! Tracks never cross a source cut. Association is greedy by box overlap,
//! which is enough for seated speakers; faces that vanish for longer than
//! `MAX_GAP` start a new track.
//!
//! Mouth motion is the mean absolute change of a small, brightness-normalised
//! grey patch around the mouth between consecutive samples, minus the same
//! measure for the eye region. Subtracting the eyes cancels most head motion
//! and camera noise, so the signal rises when the jaw and lips move.

use crate::faces::Face;
use crate::frames::Frame;

/// Longest time a face may go undetected and still continue its track.
const MAX_GAP: f64 = 0.5;
/// Minimum box overlap to continue a track.
const MIN_IOU: f32 = 0.3;
const PATCH_W: usize = 16;
const PATCH_H: usize = 8;

#[derive(Debug, Clone)]
pub(crate) struct Observation {
    pub time: f64,
    pub face: Face,
    /// Mouth motion since the track's previous observation; `None` for the
    /// first one or after a detection gap.
    pub mouth_motion: Option<f32>,
}

#[derive(Debug, Clone)]
pub(crate) struct Track {
    pub id: usize,
    /// Index of the shot (0 = before the first cut).
    pub shot: usize,
    pub observations: Vec<Observation>,
    mouth: Vec<f32>,
    eyes: Vec<f32>,
}

impl Track {
    pub(crate) fn start(&self) -> f64 {
        self.observations.first().map_or(0.0, |o| o.time)
    }

    pub(crate) fn end(&self) -> f64 {
        self.observations.last().map_or(0.0, |o| o.time)
    }

    /// The part of the track inside `start..=end`, if any.
    pub(crate) fn within(&self, start: f64, end: f64) -> Option<Track> {
        let observations: Vec<Observation> = self
            .observations
            .iter()
            .filter(|o| (start..=end).contains(&o.time))
            .cloned()
            .collect();
        (!observations.is_empty()).then(|| Track {
            id: self.id,
            shot: self.shot,
            observations,
            mouth: Vec::new(),
            eyes: Vec::new(),
        })
    }

    fn last(&self) -> &Observation {
        self.observations.last().expect("tracks are never empty")
    }
}

#[cfg(test)]
impl Track {
    pub(crate) fn for_tests(id: usize, shot: usize, observations: Vec<Observation>) -> Self {
        Self {
            id,
            shot,
            observations,
            mouth: Vec::new(),
            eyes: Vec::new(),
        }
    }
}

/// Builds tracks from frames handed in time order.
pub(crate) struct Tracker {
    cuts: Vec<f64>,
    sample_interval: f64,
    tracks: Vec<Track>,
}

impl Tracker {
    /// `cuts`: source cut times; `fps`: the sampling rate of the frames.
    pub(crate) fn new(mut cuts: Vec<f64>, fps: f64) -> Self {
        cuts.sort_by(f64::total_cmp);
        Self {
            cuts,
            sample_interval: 1.0 / fps,
            tracks: Vec::new(),
        }
    }

    fn shot_at(&self, time: f64) -> usize {
        self.cuts.partition_point(|&cut| cut <= time)
    }

    /// Add one frame (RGB) and the faces detected in it.
    pub(crate) fn push(&mut self, frame: &Frame, faces: &[Face]) {
        let shot = self.shot_at(frame.time);
        let luma = luma(frame);
        let open: Vec<usize> = (0..self.tracks.len())
            .filter(|&i| {
                let track = &self.tracks[i];
                track.shot == shot && frame.time - track.last().time <= MAX_GAP + 1e-6
            })
            .collect();

        // Best overlaps first.
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (face_index, face) in faces.iter().enumerate() {
            for &track_index in &open {
                let iou = self.tracks[track_index].last().face.iou(face);
                if iou >= MIN_IOU {
                    pairs.push((iou, track_index, face_index));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut track_taken = vec![false; self.tracks.len()];
        let mut face_taken = vec![false; faces.len()];
        for (_, track_index, face_index) in pairs {
            if track_taken[track_index] || face_taken[face_index] {
                continue;
            }
            track_taken[track_index] = true;
            face_taken[face_index] = true;
            let face = faces[face_index];
            let mouth = mouth_patch(&luma, frame.width, frame.height, &face);
            let eyes = eye_patch(&luma, frame.width, frame.height, &face);
            let track = &mut self.tracks[track_index];
            // Only consecutive samples are comparable.
            let consecutive = frame.time - track.last().time <= self.sample_interval * 1.5;
            let mouth_motion = consecutive.then(|| {
                (patch_change(&track.mouth, &mouth) - patch_change(&track.eyes, &eyes)).max(0.0)
            });
            track.mouth = mouth;
            track.eyes = eyes;
            track.observations.push(Observation {
                time: frame.time,
                face,
                mouth_motion,
            });
        }
        for (face_index, face) in faces.iter().enumerate() {
            if face_taken[face_index] {
                continue;
            }
            self.tracks.push(Track {
                id: self.tracks.len(),
                shot,
                observations: vec![Observation {
                    time: frame.time,
                    face: *face,
                    mouth_motion: None,
                }],
                mouth: mouth_patch(&luma, frame.width, frame.height, face),
                eyes: eye_patch(&luma, frame.width, frame.height, face),
            });
        }
    }

    /// Tracks spanning at least `min_duration` seconds.
    pub(crate) fn finish(self, min_duration: f64) -> Vec<Track> {
        let mut tracks: Vec<Track> = self
            .tracks
            .into_iter()
            .filter(|track| track.end() - track.start() >= min_duration)
            .collect();
        for (id, track) in tracks.iter_mut().enumerate() {
            track.id = id;
        }
        tracks
    }
}

fn luma(frame: &Frame) -> Vec<u8> {
    if frame.data.len() == (frame.width * frame.height) as usize {
        return frame.data.clone();
    }
    frame
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[r, g, b]| ((u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29) >> 8) as u8)
        .collect()
}

/// A `PATCH_W`x`PATCH_H` bilinear sample of the region centred on
/// `(cx, cy)`, with its mean subtracted.
fn sample_patch(
    luma: &[u8],
    width: u32,
    height: u32,
    (cx, cy): (f32, f32),
    (region_w, region_h): (f32, f32),
) -> Vec<f32> {
    let (w, h) = (width as usize, height as usize);
    let pixel = |x: usize, y: usize| f32::from(luma[y.min(h - 1) * w + x.min(w - 1)]);
    let mut patch = Vec::with_capacity(PATCH_W * PATCH_H);
    for py in 0..PATCH_H {
        for px in 0..PATCH_W {
            let x = (cx - region_w / 2.0 + (px as f32 + 0.5) * region_w / PATCH_W as f32)
                .clamp(0.0, (w - 1) as f32);
            let y = (cy - region_h / 2.0 + (py as f32 + 0.5) * region_h / PATCH_H as f32)
                .clamp(0.0, (h - 1) as f32);
            let (x0, y0) = (x as usize, y as usize);
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            let top = pixel(x0, y0) * (1.0 - fx) + pixel(x0 + 1, y0) * fx;
            let bottom = pixel(x0, y0 + 1) * (1.0 - fx) + pixel(x0 + 1, y0 + 1) * fx;
            patch.push(top * (1.0 - fy) + bottom * fy);
        }
    }
    let mean = patch.iter().sum::<f32>() / patch.len() as f32;
    patch.iter_mut().for_each(|value| *value -= mean);
    patch
}

/// Lips and chin: centred a little below the mouth corners, so jaw drops
/// stay inside.
fn mouth_patch(luma: &[u8], width: u32, height: u32, face: &Face) -> Vec<f32> {
    let [_, _, _, right, left] = face.landmarks;
    let centre = (
        (right.0 + left.0) / 2.0,
        (right.1 + left.1) / 2.0 + face.height * 0.06,
    );
    sample_patch(
        luma,
        width,
        height,
        centre,
        (face.width * 0.6, face.height * 0.3),
    )
}

/// Both eyes: moves with the head but not with speech.
fn eye_patch(luma: &[u8], width: u32, height: u32, face: &Face) -> Vec<f32> {
    let [right, left, ..] = face.landmarks;
    let centre = ((right.0 + left.0) / 2.0, (right.1 + left.1) / 2.0);
    sample_patch(
        luma,
        width,
        height,
        centre,
        (face.width * 0.6, face.height * 0.3),
    )
}

fn patch_change(previous: &[f32], current: &[f32]) -> f32 {
    previous
        .iter()
        .zip(current)
        .map(|(a, b)| (a - b).abs())
        .sum::<f32>()
        / current.len().max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A face box with landmarks in the usual places.
    fn face_at(x: f32, y: f32, size: f32) -> Face {
        let at = |fx: f32, fy: f32| (x + fx * size, y + fy * size);
        Face {
            x,
            y,
            width: size,
            height: size,
            score: 0.9,
            landmarks: [
                at(0.3, 0.4),
                at(0.7, 0.4),
                at(0.5, 0.6),
                at(0.35, 0.78),
                at(0.65, 0.78),
            ],
        }
    }

    fn grey_frame(time: f64, paint: impl Fn(u32, u32) -> u8) -> Frame {
        let (width, height) = (200, 100);
        let data = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| paint(x, y))
            .collect();
        Frame {
            time,
            width,
            height,
            data,
        }
    }

    #[test]
    fn faces_are_linked_within_a_shot_and_split_at_cuts() {
        let mut tracker = Tracker::new(vec![1.0], 10.0);
        let frame = |time| grey_frame(time, |_, _| 100);
        for step in 0..15 {
            let time = f64::from(step) / 10.0;
            // Two faces drifting slowly; the right one disappears at 0.5 s.
            let mut faces = vec![face_at(10.0 + step as f32, 10.0, 40.0)];
            if step < 5 {
                faces.push(face_at(120.0, 10.0, 40.0));
            }
            tracker.push(&frame(time), &faces);
        }
        let tracks = tracker.finish(0.0);
        // Left face before and after the cut, right face before it.
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].observations.len(), 10);
        assert_eq!(tracks[0].shot, 0);
        assert_eq!(tracks[1].observations.len(), 5);
        assert_eq!(tracks[2].shot, 1);
        assert_eq!(tracks[2].start(), 1.0);
        assert_eq!(tracks[0].observations[0].mouth_motion, None);
        assert_eq!(tracks[0].observations[1].mouth_motion, Some(0.0));
    }

    #[test]
    fn a_track_can_be_cut_to_a_window() {
        let mut tracker = Tracker::new(Vec::new(), 10.0);
        for step in 0..10 {
            let frame = grey_frame(f64::from(step) / 10.0, |_, _| 100);
            tracker.push(&frame, &[face_at(10.0, 10.0, 40.0)]);
        }
        let track = &tracker.finish(0.0)[0];
        let part = track.within(0.25, 0.55).unwrap();
        let times: Vec<f64> = part.observations.iter().map(|o| o.time).collect();
        assert_eq!(times, [0.3, 0.4, 0.5]);
        assert!(track.within(2.0, 3.0).is_none());
    }

    #[test]
    fn short_tracks_are_dropped() {
        let mut tracker = Tracker::new(Vec::new(), 10.0);
        let frame = |time| grey_frame(time, |_, _| 100);
        tracker.push(&frame(0.0), &[face_at(10.0, 10.0, 40.0)]);
        tracker.push(&frame(0.1), &[face_at(10.0, 10.0, 40.0)]);
        assert!(tracker.finish(0.5).is_empty());
    }

    #[test]
    fn a_moving_mouth_scores_higher_than_a_still_face() {
        let face = face_at(50.0, 10.0, 60.0);
        // Mouth area: around (80, 57); "open" draws a dark bar there.
        let mouth_open = |open: bool| {
            move |x: u32, y: u32| {
                let in_mouth = (70..90).contains(&x) && (55..62).contains(&y);
                if open && in_mouth {
                    20
                } else {
                    150 + ((x * 7 + y * 3) % 20) as u8
                }
            }
        };
        let mut tracker = Tracker::new(Vec::new(), 10.0);
        for step in 0..6 {
            let frame = grey_frame(f64::from(step) / 10.0, mouth_open(step % 2 == 1));
            tracker.push(&frame, &[face]);
        }
        let talking: Vec<f32> = tracker.finish(0.0)[0]
            .observations
            .iter()
            .filter_map(|o| o.mouth_motion)
            .collect();
        assert!(talking.iter().all(|&motion| motion > 5.0), "{talking:?}");

        let mut tracker = Tracker::new(Vec::new(), 10.0);
        for step in 0..6 {
            let frame = grey_frame(f64::from(step) / 10.0, mouth_open(false));
            tracker.push(&frame, &[face]);
        }
        let still = tracker.finish(0.0)[0]
            .observations
            .iter()
            .filter_map(|o| o.mouth_motion)
            .fold(0.0f32, f32::max);
        assert!(still < 0.5, "{still}");
    }

    #[test]
    fn head_motion_alone_is_mostly_cancelled() {
        // The whole face region brightens and darkens (e.g. a nod into a
        // light): eyes and mouth change together.
        let face = face_at(50.0, 10.0, 60.0);
        let mut tracker = Tracker::new(Vec::new(), 10.0);
        for step in 0..6 {
            let shift = if step % 2 == 1 { 3 } else { 0 };
            let frame = grey_frame(f64::from(step) / 10.0, move |x, y| {
                150 + (((x + shift) * 7 + y * 3) % 20) as u8
            });
            tracker.push(&frame, &[face]);
        }
        let motion = tracker.finish(0.0)[0]
            .observations
            .iter()
            .filter_map(|o| o.mouth_motion)
            .fold(0.0f32, f32::max);
        assert!(motion < 1.0, "{motion}");
    }
}
