//! The virtual camera's path (shorts phase S2): who to frame when, and the
//! crop keyframes that frame them.
//!
//! Framing follows the active speaker (the transcript's speaker, bound to a
//! face by `speaker_faces`). The camera holds still while the subject stays
//! inside a dead zone, pans with an ease when they leave it, and hard-cuts
//! (like a multicam switch) when the subject changes; panning between people
//! looks amateurish. Runs shorter than `min_hold` are absorbed, so quick
//! interjections don't flip the picture back and forth.
//!
//! When the speaker's face isn't known with confidence and several faces are
//! on screen, the camera shows the whole shot (`Framing::Fit`) rather than
//! guess: framing the wrong person looks worse than a wide picture.

use crate::face_tracks::{Observation, Track};
use crate::reframe::{CropKey, Framing};
use crate::speaker_faces::{sole_speaker, Binding, SpeechTurn};

/// Sampling step of the planner (seconds).
const STEP: f64 = 0.1;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CameraSettings {
    /// Face height as a share of the crop height.
    pub face_share: f64,
    /// Shortest time on one subject (seconds).
    pub min_hold: f64,
    /// Share of the crop the face may drift before the camera follows.
    pub dead_zone: f64,
    /// Duration of a follow pan (seconds).
    pub pan_seconds: f64,
    /// Pauses up to this long (seconds) keep the last speaker as subject.
    pub speech_bridge: f64,
    /// Speaker bindings weaker than this aren't trusted for framing.
    pub min_affinity: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            face_share: 0.22,
            min_hold: 1.2,
            dead_zone: 0.08,
            pan_seconds: 0.6,
            speech_bridge: 1.5,
            // Measured on the PODIUM panel: wrong wide-shot bindings scored
            // 0.5-0.7, right ones 1.2-2.8.
            min_affinity: 1.0,
        }
    }
}

/// The frame the camera works in (analysis pixels).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame {
    pub height: f64,
    /// Smallest crop height allowed (limits upscaling).
    pub min_crop_height: f64,
    /// Source frame rate: framing changes land on frame boundaries.
    pub fps: f64,
}

/// A stretch of the range with one kind of framing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Piece {
    /// Source times (seconds).
    pub start: f64,
    pub end: f64,
    /// For `Follow`, key times are relative to `start`.
    pub framing: Framing,
}

/// What to show at one sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    /// Follow this track (index into the tracks).
    Face(usize),
    /// The whole shot.
    Fit,
}

fn visible(track: &Track, time: f64) -> bool {
    track.start() - STEP * 0.6 <= time && time <= track.end() + STEP * 0.6
}

fn nearest(track: &Track, time: f64) -> &Observation {
    let index = track.observations.partition_point(|o| o.time < time);
    let after = track.observations.get(index);
    let before = index.checked_sub(1).and_then(|i| track.observations.get(i));
    match (before, after) {
        (Some(b), Some(a)) if (time - b.time) <= (a.time - time) => b,
        (_, Some(a)) => a,
        (Some(b), None) => b,
        (None, None) => unreachable!("tracks are never empty"),
    }
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// The speaker at `time`, bridging short pauses with the last one heard.
fn current_speaker(turns: &[SpeechTurn], time: f64, bridge: f64) -> Option<&str> {
    let mut back = 0.0;
    while back <= bridge {
        if let Some(speaker) = sole_speaker(turns, time - back) {
            return Some(speaker);
        }
        back += STEP;
    }
    None
}

/// What to show at each sample:
/// 1. the speaker's (confidently bound, visible) face;
/// 2. while someone else speaks: the only face on screen, or the whole shot;
/// 3. in silence: the previous subject if still visible, else as in 2.
fn choose(
    times: &[f64],
    tracks: &[Track],
    bindings: &[Binding],
    turns: &[SpeechTurn],
    settings: &CameraSettings,
) -> Vec<Choice> {
    let mut previous = Choice::Fit;
    times
        .iter()
        .map(|&time| {
            let on_screen: Vec<usize> = (0..tracks.len())
                .filter(|&index| visible(&tracks[index], time))
                .collect();
            let speaker = current_speaker(turns, time, settings.speech_bridge);
            let speaker_face = speaker.and_then(|speaker| {
                bindings
                    .iter()
                    .filter(|b| b.speaker == speaker && b.affinity >= settings.min_affinity)
                    .filter_map(|b| tracks.iter().position(|t| t.id == b.track))
                    .find(|index| on_screen.contains(index))
            });
            let unsure = || match on_screen.as_slice() {
                [only] => Choice::Face(*only),
                _ => Choice::Fit,
            };
            let choice = match (speaker_face, speaker, previous) {
                (Some(face), _, _) => Choice::Face(face),
                (None, Some(_), _) => unsure(),
                (None, None, Choice::Face(face)) if on_screen.contains(&face) => previous,
                (None, None, _) => unsure(),
            };
            previous = choice;
            choice
        })
        .collect()
}

/// Runs of one choice: (choice, first sample, last sample + 1).
fn runs(choices: &[Choice]) -> Vec<(Choice, usize, usize)> {
    let mut runs: Vec<(Choice, usize, usize)> = Vec::new();
    for (index, &choice) in choices.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if run.0 == choice => run.2 = index + 1,
            _ => runs.push((choice, index, index + 1)),
        }
    }
    runs
}

/// Give short runs to a neighbour that can show the whole run (a face on
/// screen throughout, or the whole shot).
fn absorb_short_runs(choices: &mut [Choice], times: &[f64], tracks: &[Track], min_hold: f64) {
    let min_samples = (min_hold / STEP).round() as usize;
    let all_runs = runs(choices);
    for (position, &(_, first, end)) in all_runs.iter().enumerate() {
        if end - first >= min_samples {
            continue;
        }
        let covers = |candidate: Choice| match candidate {
            Choice::Face(track) => times[first..end]
                .iter()
                .all(|&time| visible(&tracks[track], time)),
            Choice::Fit => true,
        };
        let before = position.checked_sub(1).map(|p| choices[all_runs[p].2 - 1]);
        let after = all_runs.get(position + 1).map(|run| run.0);
        if let Some(replacement) = [before, after].into_iter().flatten().find(|&c| covers(c)) {
            choices[first..end].fill(replacement);
        }
    }
}

/// Where the crop should be to frame `observation`: centre and height.
fn target(observation: &Observation, frame: &Frame, settings: &CameraSettings) -> (f64, f64, f64) {
    let face = observation.face;
    let height = (f64::from(face.height) / settings.face_share)
        .clamp(frame.min_crop_height.min(frame.height), frame.height);
    let [right_eye, left_eye, ..] = face.landmarks;
    let eye_y = f64::from(right_eye.1 + left_eye.1) / 2.0;
    // Eye line at the upper third of the crop.
    let center_y = eye_y + height / 6.0;
    (f64::from(face.center().0), center_y, height)
}

/// Crop keys following `track` over `times` (absolute), relative to
/// `origin`, appended to `keys`.
#[allow(clippy::too_many_arguments)]
fn follow(
    keys: &mut Vec<CropKey>,
    track: &Track,
    times: &[f64],
    origin: f64,
    end: f64,
    frame: &Frame,
    aspect: f64,
    settings: &CameraSettings,
) {
    let push = |keys: &mut Vec<CropKey>, time: f64, (x, y, h): (f64, f64, f64)| {
        keys.push(CropKey {
            time: time - origin,
            center_x: x,
            center_y: y,
            height: h,
        });
    };
    let targets: Vec<(f64, f64, f64)> = times
        .iter()
        .map(|&time| target(nearest(track, time), frame, settings))
        .collect();
    // One zoom per run keeps the subject's size steady.
    let height = median(targets.iter().map(|t| t.2).collect());
    let width = height * aspect;
    let settle = |from: usize| {
        let window = &targets[from..(from + (1.0 / STEP) as usize).min(targets.len())];
        (
            median(window.iter().map(|t| t.0).collect()),
            median(window.iter().map(|t| t.1).collect()),
        )
    };

    let mut current = settle(0);
    push(keys, times[0], (current.0, current.1, height));
    let pan_samples = (settings.pan_seconds / STEP).round().max(1.0) as usize;
    let mut index = 0;
    while index < targets.len() {
        let (x, y, _) = targets[index];
        let outside = (x - current.0).abs() > settings.dead_zone * width
            || (y - current.1).abs() > settings.dead_zone * height;
        if !outside {
            index += 1;
            continue;
        }
        let next = settle(index);
        let pan_start = times[index];
        push(keys, pan_start, (current.0, current.1, height));
        // Smoothstep ease, sampled at the planner step.
        for step in 1..=pan_samples {
            let t = step as f64 / pan_samples as f64;
            let eased = t * t * (3.0 - 2.0 * t);
            push(
                keys,
                (pan_start + step as f64 * STEP).min(end),
                (
                    current.0 + (next.0 - current.0) * eased,
                    current.1 + (next.1 - current.1) * eased,
                    height,
                ),
            );
        }
        current = next;
        index += pan_samples;
    }
}

/// The framing of the source range `range`, as consecutive pieces covering
/// it. `tracks` and `bindings` come from analysing the range; coordinates
/// are in `frame`'s pixels.
pub(crate) fn plan_camera(
    range: (f64, f64),
    tracks: &[Track],
    bindings: &[Binding],
    turns: &[SpeechTurn],
    frame: &Frame,
    aspect: f64,
    settings: &CameraSettings,
) -> Vec<Piece> {
    let (start, end) = range;
    let times: Vec<f64> = (0..)
        .map(|i| start + f64::from(i) * STEP)
        .take_while(|&time| time < end)
        .collect();
    if times.is_empty() {
        return vec![Piece {
            start,
            end,
            framing: Framing::Fit,
        }];
    }
    let mut choices = choose(&times, tracks, bindings, turns, settings);
    absorb_short_runs(&mut choices, &times, tracks, settings.min_hold);

    // Pieces change on frame boundaries, so trims and audio stay in sync.
    let snap = |time: f64| start + ((time - start) * frame.fps).round() / frame.fps;
    let mut pieces: Vec<Piece> = Vec::new();
    for (choice, first, run_end) in runs(&choices) {
        let run_start = if first == 0 {
            start
        } else {
            snap(times[first])
        };
        match choice {
            Choice::Fit => pieces.push(Piece {
                start: run_start,
                end,
                framing: Framing::Fit,
            }),
            Choice::Face(track) => {
                // Consecutive faces share a piece, with a hard switch.
                let continuing = matches!(
                    pieces.last(),
                    Some(Piece {
                        framing: Framing::Follow(_),
                        ..
                    })
                );
                if !continuing {
                    pieces.push(Piece {
                        start: run_start,
                        end,
                        framing: Framing::Follow(Vec::new()),
                    });
                }
                let piece = pieces.last_mut().expect("just pushed");
                let origin = piece.start;
                let Framing::Follow(keys) = &mut piece.framing else {
                    unreachable!("a follow piece");
                };
                // Hard switch: hold the previous framing until just before.
                if let Some(&last) = keys.last() {
                    keys.push(CropKey {
                        time: (run_start - origin - 1e-3).max(last.time),
                        ..last
                    });
                }
                let mut run_times = times[first..run_end].to_vec();
                run_times[0] = run_start;
                follow(
                    keys,
                    &tracks[track],
                    &run_times,
                    origin,
                    end,
                    frame,
                    aspect,
                    settings,
                );
            }
        }
        // The previous piece ends where this one starts.
        let count = pieces.len();
        if count >= 2 {
            let next_start = pieces[count - 1].start;
            pieces[count - 2].end = next_start;
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faces::Face;

    const FRAME: Frame = Frame {
        height: 720.0,
        min_crop_height: 480.0,
        fps: 25.0,
    };
    const PORTRAIT: f64 = 9.0 / 16.0;

    fn face(x: f32, y: f32, size: f32) -> Face {
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

    /// A track at 10 fps over `range` whose face is at `position(time)`.
    fn track(id: usize, range: (f64, f64), position: impl Fn(f64) -> (f32, f32)) -> Track {
        let observations = (0..)
            .map(|i| range.0 + f64::from(i) / 10.0)
            .take_while(|&time| time < range.1)
            .map(|time| {
                let (x, y) = position(time);
                Observation {
                    time,
                    face: face(x, y, 120.0),
                    mouth_motion: None,
                }
            })
            .collect();
        Track::for_tests(id, 0, observations)
    }

    fn bind(track: usize, speaker: &str) -> Binding {
        Binding {
            track,
            speaker: speaker.to_string(),
            setup: 0,
            seat: track,
            affinity: 2.0,
            evidence: 10.0,
        }
    }

    fn turn(start: f64, end: f64, speaker: &str) -> SpeechTurn {
        SpeechTurn {
            start,
            end,
            speaker: speaker.to_string(),
        }
    }

    fn pieces(
        range: (f64, f64),
        tracks: &[Track],
        bindings: &[Binding],
        turns: &[SpeechTurn],
    ) -> Vec<Piece> {
        plan_camera(
            range,
            tracks,
            bindings,
            turns,
            &FRAME,
            PORTRAIT,
            &CameraSettings::default(),
        )
    }

    /// The keys of a plan that follows faces throughout.
    fn plan(
        range: (f64, f64),
        tracks: &[Track],
        bindings: &[Binding],
        turns: &[SpeechTurn],
    ) -> Vec<CropKey> {
        let pieces = pieces(range, tracks, bindings, turns);
        assert_eq!(pieces.len(), 1, "{pieces:?}");
        assert_eq!((pieces[0].start, pieces[0].end), range);
        match &pieces[0].framing {
            Framing::Follow(keys) => keys.clone(),
            Framing::Fit => panic!("expected a followed face"),
        }
    }

    #[test]
    fn a_still_speaker_gets_one_static_framing() {
        let tracks = [track(0, (10.0, 20.0), |_| (200.0, 100.0))];
        let keys = plan(
            (10.0, 20.0),
            &tracks,
            &[bind(0, "A")],
            &[turn(10.0, 20.0, "A")],
        );
        assert_eq!(keys.len(), 1, "{keys:?}");
        let key = keys[0];
        assert_eq!(key.time, 0.0);
        assert_eq!(key.center_x, 260.0);
        // 120 px face / 0.22 = 545 px crop; eye line (148) at its upper third.
        assert!((key.height - 545.45).abs() < 0.1);
        assert!((key.center_y - (148.0 + key.height / 6.0)).abs() < 1e-6);
    }

    #[test]
    fn the_camera_cuts_to_the_next_speaker() {
        let tracks = [
            track(0, (0.0, 10.0), |_| (200.0, 100.0)),
            track(1, (0.0, 10.0), |_| (900.0, 100.0)),
        ];
        let keys = plan(
            (0.0, 10.0),
            &tracks,
            &[bind(0, "A"), bind(1, "B")],
            &[turn(0.0, 5.0, "A"), turn(5.0, 10.0, "B")],
        );
        assert_eq!(keys.len(), 3, "{keys:?}");
        assert_eq!(keys[0].center_x, 260.0);
        // Held until just before 5 s, then the other face.
        assert_eq!((keys[1].center_x, keys[2].center_x), (260.0, 960.0));
        assert!((keys[2].time - 5.0).abs() < 0.05);
        assert!(keys[2].time - keys[1].time < 0.01);
    }

    #[test]
    fn a_short_interjection_does_not_switch() {
        let tracks = [
            track(0, (0.0, 10.0), |_| (200.0, 100.0)),
            track(1, (0.0, 10.0), |_| (900.0, 100.0)),
        ];
        let keys = plan(
            (0.0, 10.0),
            &tracks,
            &[bind(0, "A"), bind(1, "B")],
            // B says "right" for 0.5 s; the bridge would keep B for 1.5 s
            // more, still under the hold once A resumes.
            &[
                turn(0.0, 4.0, "A"),
                turn(4.0, 4.5, "B"),
                turn(4.5, 10.0, "A"),
            ],
        );
        assert!(keys.iter().all(|k| k.center_x == 260.0), "{keys:?}");
    }

    #[test]
    fn small_movement_is_ignored_and_a_move_is_followed_with_a_pan() {
        // Sways by ±5 px, then shifts 200 px right at 5 s.
        let tracks = [track(0, (0.0, 10.0), |t| {
            let sway = if (t * 10.0).round() as i64 % 2 == 0 {
                5.0
            } else {
                -5.0
            };
            (if t < 5.0 { 200.0 } else { 400.0 } + sway, 100.0)
        })];
        let keys = plan(
            (0.0, 10.0),
            &tracks,
            &[bind(0, "A")],
            &[turn(0.0, 10.0, "A")],
        );
        let xs: Vec<f64> = keys.iter().map(|k| k.center_x).collect();
        // Static until the move, eased to the new place, then static.
        assert!(xs.iter().take_while(|&&x| x < 300.0).count() >= 2, "{xs:?}");
        assert!((xs.last().unwrap() - 460.0).abs() <= 5.0, "{xs:?}");
        assert!(xs.windows(2).all(|w| w[1] >= w[0]), "monotonic pan: {xs:?}");
        let pan_start = keys.iter().find(|k| k.center_x > 270.0).unwrap().time;
        assert!((5.0..5.3).contains(&pan_start), "{keys:?}");
    }

    #[test]
    fn without_faces_the_whole_shot_is_shown() {
        let plan = pieces((0.0, 5.0), &[], &[], &[turn(0.0, 5.0, "A")]);
        assert_eq!(
            plan,
            [Piece {
                start: 0.0,
                end: 5.0,
                framing: Framing::Fit
            }]
        );
    }

    #[test]
    fn a_lone_face_is_followed_within_the_zoom_limit() {
        let mut big = track(0, (0.0, 5.0), |_| (900.0, 100.0));
        for o in &mut big.observations {
            o.face = face(900.0, 100.0, 200.0);
        }
        // Nobody bound: the only face on screen is the subject.
        let keys = plan((0.0, 5.0), &[big], &[], &[turn(0.0, 5.0, "A")]);
        assert_eq!(keys[0].center_x, 1000.0);
        // 200 / 0.22 = 909 px would exceed the frame: clamped.
        assert_eq!(keys[0].height, 720.0);
    }

    #[test]
    fn an_unknown_speaker_among_several_faces_shows_the_whole_shot() {
        let tracks = [
            track(0, (0.0, 10.0), |_| (200.0, 100.0)),
            track(1, (0.0, 10.0), |_| (900.0, 100.0)),
        ];
        // A is bound but only weakly (as wide-shot guesses are).
        let weak = Binding {
            affinity: 0.6,
            ..bind(1, "A")
        };
        let plan = pieces(
            (0.0, 10.0),
            &tracks,
            &[weak, bind(0, "B")],
            &[turn(0.0, 5.0, "A"), turn(5.0, 10.0, "B")],
        );
        assert_eq!(plan.len(), 2, "{plan:?}");
        assert_eq!(plan[0].framing, Framing::Fit);
        assert_eq!((plan[0].end, plan[1].start), (5.0, 5.0));
        let Framing::Follow(keys) = &plan[1].framing else {
            panic!("B is followed");
        };
        assert_eq!((keys[0].time, keys[0].center_x), (0.0, 260.0));
        assert_eq!(plan[1].end, 10.0);
    }

    #[test]
    fn piece_boundaries_land_on_frames() {
        let tracks = [
            track(0, (0.0, 10.0), |_| (200.0, 100.0)),
            track(1, (0.0, 10.0), |_| (900.0, 100.0)),
        ];
        let plan = pieces(
            (0.0, 10.0),
            &tracks,
            &[bind(0, "B")],
            &[turn(0.0, 3.3, "A"), turn(3.3, 10.0, "B")],
        );
        // 3.3 s at 25 fps is frame 82.5: rounded to a frame time.
        let boundary = plan[1].start * 25.0;
        assert!((boundary - boundary.round()).abs() < 1e-9, "{plan:?}");
    }
}
