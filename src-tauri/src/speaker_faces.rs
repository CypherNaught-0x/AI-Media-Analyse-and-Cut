//! Which face belongs to which speaker (shorts phase S2).
//!
//! The transcript already says who speaks when (diarized and named). What's
//! missing is which face that is, so this is a matching problem rather than
//! frame-by-frame active speaker detection: a face belongs to a speaker if
//! its mouth moves more while that speaker talks than otherwise, and more
//! than the other faces do.
//!
//! Evidence is pooled per *seat*: studio productions cut back to the same
//! camera setups again and again with everyone in the same place, so shots
//! with the same face layout form a setup and the faces at one position in
//! it are one seat. In wide shots a mouth is a few pixels and a single shot
//! rarely has enough evidence; all shots of the setup together do.

use crate::face_tracks::Track;
use std::collections::BTreeMap;

/// Least sole-speaker speech (seconds) a seat must be seen during to bind.
const MIN_EVIDENCE: f64 = 3.0;
/// Least affinity (mouth-motion contrast, in grey levels) to bind.
const MIN_AFFINITY: f32 = 0.3;
/// A face matches a seat within this many face heights of its position…
const SEAT_DISTANCE: f32 = 0.5;
/// …and with a height within this ratio.
const SEAT_SIZE_RATIO: f32 = 1.4;
/// Share of a shot's faces (and of the setup's seats) that must match for
/// the shot to belong to a setup.
const SETUP_MATCH: f32 = 0.75;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpeechTurn {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Binding {
    pub track: usize,
    pub speaker: String,
    /// Camera setup and seat the track was grouped into.
    pub setup: usize,
    pub seat: usize,
    /// How much more the mouth moves while this speaker talks.
    pub affinity: f32,
    /// Seconds of the speaker's speech the seat was observed for.
    pub evidence: f64,
}

/// The one speaker talking at `time`, or `None` during silence and overlap.
pub(crate) fn sole_speaker(turns: &[SpeechTurn], time: f64) -> Option<&str> {
    let mut speaking = turns
        .iter()
        .filter(|turn| turn.start <= time && time < turn.end)
        .map(|turn| turn.speaker.as_str());
    let first = speaking.next()?;
    speaking.all(|other| other == first).then_some(first)
}

#[derive(Debug, Clone, Copy)]
struct Placement {
    center: (f32, f32),
    height: f32,
}

impl Placement {
    fn of(track: &Track) -> Self {
        let median = |mut values: Vec<f32>| {
            values.sort_by(f32::total_cmp);
            values[values.len() / 2]
        };
        let faces = || track.observations.iter().map(|o| o.face);
        Self {
            center: (
                median(faces().map(|f| f.center().0).collect()),
                median(faces().map(|f| f.center().1).collect()),
            ),
            height: median(faces().map(|f| f.height).collect()),
        }
    }

    fn matches(&self, other: &Placement) -> bool {
        let height = self.height.max(other.height);
        let distance = (self.center.0 - other.center.0).hypot(self.center.1 - other.center.1);
        let ratio = self.height.max(other.height) / self.height.min(other.height).max(1e-3);
        distance <= SEAT_DISTANCE * height && ratio <= SEAT_SIZE_RATIO
    }
}

#[derive(Debug, Default)]
struct Setup {
    /// Where each seat's face is, from the first shot that had it.
    seats: Vec<Placement>,
}

/// (setup, seat) for every track, by index.
fn assign_seats(tracks: &[Track]) -> Vec<(usize, usize)> {
    let mut by_shot: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, track) in tracks.iter().enumerate() {
        by_shot.entry(track.shot).or_default().push(index);
    }
    let placements: Vec<Placement> = tracks.iter().map(Placement::of).collect();
    let mut setups: Vec<Setup> = Vec::new();
    let mut seats = vec![(0, 0); tracks.len()];

    for shot_tracks in by_shot.values() {
        // Match each face to a seat of a setup; a seat takes several tracks
        // of a shot only if they don't overlap in time (a face lost and
        // found again).
        let match_setup = |setup: &Setup| -> Vec<Option<usize>> {
            let mut taken: Vec<Vec<usize>> = vec![Vec::new(); setup.seats.len()];
            shot_tracks
                .iter()
                .map(|&index| {
                    let track = &tracks[index];
                    let seat = setup
                        .seats
                        .iter()
                        .enumerate()
                        .filter(|(_, seat)| seat.matches(&placements[index]))
                        .filter(|&(seat, _)| {
                            taken[seat].iter().all(|&other| {
                                let other = &tracks[other];
                                other.end() < track.start() || track.end() < other.start()
                            })
                        })
                        .min_by(|a, b| {
                            let distance = |seat: &Placement| {
                                (seat.center.0 - placements[index].center.0)
                                    .hypot(seat.center.1 - placements[index].center.1)
                            };
                            distance(a.1).total_cmp(&distance(b.1))
                        })
                        .map(|(seat, _)| seat);
                    if let Some(seat) = seat {
                        taken[seat].push(index);
                    }
                    seat
                })
                .collect()
        };
        let distinct = |matched: &[Option<usize>]| {
            let mut seats: Vec<usize> = matched.iter().flatten().copied().collect();
            seats.sort_unstable();
            seats.dedup();
            seats.len()
        };

        let best = setups
            .iter()
            .enumerate()
            .map(|(index, setup)| (index, match_setup(setup)))
            .filter(|(index, matched)| {
                let hits = distinct(matched) as f32;
                let faces = shot_tracks.len() as f32;
                let seats = setups[*index].seats.len() as f32;
                hits >= 1.0 && hits >= SETUP_MATCH * faces.max(seats)
            })
            .max_by_key(|(_, matched)| distinct(matched));
        let (setup_index, matched) = match best {
            Some(found) => found,
            None => {
                setups.push(Setup::default());
                (setups.len() - 1, vec![None; shot_tracks.len()])
            }
        };
        for (&index, seat) in shot_tracks.iter().zip(matched) {
            let seat = seat.unwrap_or_else(|| {
                setups[setup_index].seats.push(placements[index]);
                setups[setup_index].seats.len() - 1
            });
            seats[index] = (setup_index, seat);
        }
    }
    seats
}

#[derive(Debug, Default, Clone, Copy)]
struct Stats {
    sum: f64,
    count: usize,
    seconds: f64,
}

impl Stats {
    fn add(&mut self, value: f32, seconds: f64) {
        self.sum += f64::from(value);
        self.count += 1;
        self.seconds += seconds;
    }

    fn mean(&self) -> Option<f32> {
        (self.count > 0).then(|| (self.sum / self.count as f64) as f32)
    }
}

/// Per seat: mouth motion while each speaker talks alone, and in total.
#[derive(Default)]
struct SeatEvidence {
    by_speaker: BTreeMap<String, Stats>,
    total: Stats,
}

impl SeatEvidence {
    fn add(&mut self, track: &Track, turns: &[SpeechTurn]) {
        let samples: Vec<(f64, f32)> = track
            .observations
            .iter()
            .filter_map(|o| o.mouth_motion.map(|motion| (o.time, motion)))
            .collect();
        let interval = if samples.len() > 1 {
            (samples[samples.len() - 1].0 - samples[0].0) / (samples.len() - 1) as f64
        } else {
            0.0
        };
        for &(time, motion) in &samples {
            self.total.add(motion, interval);
            if let Some(speaker) = sole_speaker(turns, time) {
                self.by_speaker
                    .entry(speaker.to_string())
                    .or_default()
                    .add(motion, interval);
            }
        }
    }

    fn while_speaking(&self, speaker: &str) -> Option<&Stats> {
        self.by_speaker.get(speaker)
    }

    /// Mean motion while `speaker` is not the one talking.
    fn otherwise(&self, speaker: &str) -> Option<f32> {
        let speaking = self.while_speaking(speaker);
        let count = self.total.count - speaking.map_or(0, |s| s.count);
        let sum = self.total.sum - speaking.map_or(0.0, |s| s.sum);
        (count > 0).then(|| (sum / count as f64) as f32)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Affinity {
    pub setup: usize,
    pub seat: usize,
    pub speaker: String,
    pub affinity: f32,
    pub evidence: f64,
}

/// Every (seat, speaker) pair with enough evidence, and the seat of every
/// track.
fn affinities(tracks: &[Track], turns: &[SpeechTurn]) -> (Vec<Affinity>, Vec<(usize, usize)>) {
    let seats = assign_seats(tracks);
    let mut evidence: BTreeMap<usize, BTreeMap<usize, SeatEvidence>> = BTreeMap::new();
    for (track, &(setup, seat)) in tracks.iter().zip(&seats) {
        evidence
            .entry(setup)
            .or_default()
            .entry(seat)
            .or_default()
            .add(track, turns);
    }

    let mut table = Vec::new();
    for (&setup, setup_seats) in &evidence {
        for (&seat, own) in setup_seats {
            for (speaker, stats) in &own.by_speaker {
                if stats.seconds < MIN_EVIDENCE {
                    continue;
                }
                let speaking = stats.mean().unwrap_or(0.0);
                // Against the same face at other times...
                let over_time = own.otherwise(speaker).map_or(0.0, |other| speaking - other);
                // ...and against the other faces while this speaker talks.
                let others: Vec<f32> = setup_seats
                    .iter()
                    .filter(|&(&other, _)| other != seat)
                    .filter_map(|(_, e)| e.while_speaking(speaker).and_then(Stats::mean))
                    .collect();
                let over_faces = if others.is_empty() {
                    0.0
                } else {
                    speaking - others.iter().sum::<f32>() / others.len() as f32
                };
                table.push(Affinity {
                    setup,
                    seat,
                    speaker: speaker.clone(),
                    affinity: over_time + over_faces,
                    evidence: stats.seconds,
                });
            }
        }
    }
    (table, seats)
}

/// Speaker ↔ face bindings, one-to-one within each camera setup. Faces of
/// speakers who don't talk while on screen (or whose mouth isn't the one
/// moving) stay unbound.
pub(crate) fn bind_speakers(tracks: &[Track], turns: &[SpeechTurn]) -> Vec<Binding> {
    let (mut table, seats) = affinities(tracks, turns);
    table.sort_by(|a, b| b.affinity.total_cmp(&a.affinity));
    let mut bound: Vec<Affinity> = Vec::new();
    for candidate in table {
        if candidate.affinity < MIN_AFFINITY {
            break;
        }
        if bound.iter().any(|b| {
            b.setup == candidate.setup
                && (b.seat == candidate.seat || b.speaker == candidate.speaker)
        }) {
            continue;
        }
        bound.push(candidate);
    }

    let mut bindings = Vec::new();
    for (track, &assigned) in tracks.iter().zip(&seats) {
        if let Some(found) = bound.iter().find(|b| (b.setup, b.seat) == assigned) {
            bindings.push(Binding {
                track: track.id,
                speaker: found.speaker.clone(),
                setup: found.setup,
                seat: found.seat,
                affinity: found.affinity,
                evidence: found.evidence,
            });
        }
    }
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face_tracks::Observation;
    use crate::faces::Face;

    fn turn(start: f64, end: f64, speaker: &str) -> SpeechTurn {
        SpeechTurn {
            start,
            end,
            speaker: speaker.to_string(),
        }
    }

    fn face(x: f32) -> Face {
        Face {
            x,
            y: 0.0,
            width: 40.0,
            height: 40.0,
            score: 0.9,
            landmarks: [(0.0, 0.0); 5],
        }
    }

    /// A 10 fps track whose mouth moves by `motion(time)`, `id * 100` px from
    /// the left.
    fn track(id: usize, shot: usize, range: (f64, f64), motion: impl Fn(f64) -> f32) -> Track {
        track_at(id, shot, id as f32 * 100.0, range, motion)
    }

    fn track_at(
        id: usize,
        shot: usize,
        x: f32,
        range: (f64, f64),
        motion: impl Fn(f64) -> f32,
    ) -> Track {
        let observations = (0..)
            .map(|i| range.0 + f64::from(i) / 10.0)
            .take_while(|&time| time < range.1)
            .map(|time| Observation {
                time,
                face: face(x),
                mouth_motion: Some(motion(time)),
            })
            .collect();
        Track::for_tests(id, shot, observations)
    }

    #[test]
    fn the_sole_speaker_ignores_overlap_and_silence() {
        let turns = [turn(0.0, 2.0, "A"), turn(1.5, 3.0, "B")];
        assert_eq!(sole_speaker(&turns, 1.0), Some("A"));
        assert_eq!(sole_speaker(&turns, 1.7), None);
        assert_eq!(sole_speaker(&turns, 2.5), Some("B"));
        assert_eq!(sole_speaker(&turns, 3.5), None);
    }

    #[test]
    fn faces_are_bound_to_the_speaker_their_mouth_follows() {
        // A talks 0-5 s, B 5-10 s. Track 0 moves with A, track 1 with B,
        // track 2 (a listener) never.
        let turns = [turn(0.0, 5.0, "A"), turn(5.0, 10.0, "B")];
        let tracks = [
            track(0, 0, (0.0, 10.0), |t| if t < 5.0 { 3.0 } else { 0.5 }),
            track(1, 0, (0.0, 10.0), |t| if t >= 5.0 { 2.5 } else { 0.4 }),
            track(2, 0, (0.0, 10.0), |_| 0.3),
        ];
        let mut bindings = bind_speakers(&tracks, &turns);
        bindings.sort_by_key(|b| b.track);
        let pairs: Vec<(usize, &str)> = bindings
            .iter()
            .map(|b| (b.track, b.speaker.as_str()))
            .collect();
        assert_eq!(pairs, [(0, "A"), (1, "B")]);
        assert!(bindings.iter().all(|b| (b.evidence - 5.0).abs() < 0.2));
    }

    #[test]
    fn a_shot_with_one_talker_binds_by_comparing_faces() {
        // Only A talks; the moving face is A, the still one stays unbound.
        let turns = [turn(0.0, 10.0, "A")];
        let tracks = [
            track(0, 3, (0.0, 10.0), |_| 0.4),
            track(1, 3, (0.0, 10.0), |_| 2.0),
        ];
        let bindings = bind_speakers(&tracks, &turns);
        assert_eq!(bindings.len(), 1);
        assert_eq!((bindings[0].track, bindings[0].speaker.as_str()), (1, "A"));
    }

    #[test]
    fn evidence_is_pooled_over_shots_of_the_same_setup() {
        // A wide shot (faces at 100 and 300) cut with a close-up (one big
        // face) and back. A speaks 2 s in each wide shot: too little per
        // shot, enough together.
        let turns = [turn(0.0, 2.0, "A"), turn(10.0, 12.0, "A")];
        let speaks = |t: f64| {
            if t < 2.0 || (10.0..12.0).contains(&t) {
                2.0
            } else {
                0.2
            }
        };
        let tracks = [
            track_at(0, 0, 100.0, (0.0, 5.0), speaks),
            track_at(1, 0, 300.0, (0.0, 5.0), |_| 0.2),
            Track::for_tests(
                2,
                1,
                (50..100)
                    .map(|i| Observation {
                        time: f64::from(i) / 10.0,
                        face: Face {
                            height: 200.0,
                            width: 200.0,
                            ..face(150.0)
                        },
                        mouth_motion: Some(0.2),
                    })
                    .collect(),
            ),
            track_at(3, 2, 104.0, (10.0, 15.0), speaks),
            track_at(4, 2, 297.0, (10.0, 15.0), |_| 0.2),
        ];
        let mut bindings = bind_speakers(&tracks, &turns);
        bindings.sort_by_key(|b| b.track);
        let bound: Vec<(usize, &str)> = bindings
            .iter()
            .map(|b| (b.track, b.speaker.as_str()))
            .collect();
        assert_eq!(bound, [(0, "A"), (3, "A")]);
        assert_eq!(
            (bindings[0].setup, bindings[0].seat),
            (bindings[1].setup, bindings[1].seat)
        );
        assert!((bindings[0].evidence - 4.0).abs() < 0.3, "{bindings:?}");
    }

    #[test]
    fn little_evidence_or_contrast_binds_nothing() {
        let turns = [turn(0.0, 1.0, "A"), turn(1.0, 10.0, "B")];
        let tracks = [
            // A speaks only 1 s.
            track(0, 0, (0.0, 10.0), |t| if t < 1.0 { 3.0 } else { 0.2 }),
            // Moves all the time: no contrast for B.
            track(1, 0, (0.0, 10.0), |_| 1.0),
        ];
        let bindings = bind_speakers(&tracks, &turns);
        assert!(bindings.iter().all(|b| b.speaker != "A"), "{bindings:?}");
    }
}

#[cfg(test)]
mod evaluation {
    use super::*;
    use crate::face_tracks::Tracker;
    use crate::faces::{Face, FaceDetector};
    use crate::frames::{decode_frames, Frame, FrameRequest, PixelFormat};
    use crate::shots::detect_cuts;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    fn env_f64(name: &str, default: f64) -> f64 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    }

    fn turns_from_transcript(path: &Path) -> Vec<SpeechTurn> {
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        json["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|segment| SpeechTurn {
                start: crate::time_utils::parse_time(segment["start"].as_str().unwrap()),
                end: crate::time_utils::parse_time(segment["end"].as_str().unwrap()),
                speaker: segment["speaker"].as_str().unwrap_or("?").to_string(),
            })
            .collect()
    }

    /// Binds speakers on a real recording and reports, per shot, who was
    /// bound to which face. With `SHORTS_PROFILE_KEEP`, writes each shot's
    /// middle frame with boxes coloured by speaker (white = unbound).
    /// `SHORTS_PROFILE_SOURCE=… SHORTS_PROFILE_TRANSCRIPT=….transcript.json
    ///  cargo test --release --lib speaker_binding_on_a_recording -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn speaker_binding_on_a_recording() {
        let (Some(source), Some(transcript)) = (
            std::env::var_os("SHORTS_PROFILE_SOURCE"),
            std::env::var_os("SHORTS_PROFILE_TRANSCRIPT"),
        ) else {
            eprintln!("SHORTS_PROFILE_SOURCE / SHORTS_PROFILE_TRANSCRIPT not set; skipping");
            return;
        };
        let source = PathBuf::from(source);
        let keep = std::env::var_os("SHORTS_PROFILE_KEEP").map(PathBuf::from);
        let start = env_f64("SHORTS_PROFILE_START", 600.0);
        let end = start + env_f64("SHORTS_PROFILE_DURATION", 300.0);
        let fps = env_f64("SHORTS_PROFILE_FPS", 10.0);
        let width = env_f64("SHORTS_PROFILE_WIDTH", 1280.0) as u32;
        let turns = turns_from_transcript(Path::new(&transcript));
        let mut speakers: Vec<String> = turns.iter().map(|t| t.speaker.clone()).collect();
        speakers.sort();
        speakers.dedup();

        let started = Instant::now();
        let cuts = detect_cuts(&source, start, end, None).unwrap();
        let cut_time = started.elapsed().as_secs_f64();
        let mut bounds = vec![start];
        bounds.extend(&cuts);
        bounds.push(end);
        let middles: Vec<f64> = bounds.windows(2).map(|w| (w[0] + w[1]) / 2.0).collect();

        let mut detector = FaceDetector::new().unwrap();
        let mut tracker = Tracker::new(cuts.clone(), fps);
        let mut snapshots: Vec<Option<(Frame, Vec<Face>)>> = vec![None; middles.len()];
        let started = Instant::now();
        let mut detect_time = 0.0;
        decode_frames(
            &FrameRequest {
                path: &source,
                start,
                end,
                fps,
                width,
                format: PixelFormat::Rgb24,
            },
            None,
            |frame| {
                let detect_start = Instant::now();
                let faces = detector.detect(&frame).unwrap();
                tracker.push(&frame, &faces);
                detect_time += detect_start.elapsed().as_secs_f64();
                let shot = cuts.partition_point(|&cut| cut <= frame.time);
                if snapshots[shot].is_none() && frame.time >= middles[shot] {
                    snapshots[shot] = Some((frame, faces));
                }
            },
        )
        .unwrap();
        let total = started.elapsed().as_secs_f64();
        let tracks = tracker.finish(1.0);
        let bindings = bind_speakers(&tracks, &turns);
        let (mut table, _) = affinities(&tracks, &turns);
        table.sort_by(|a, b| (a.setup, a.seat, &a.speaker).cmp(&(b.setup, b.seat, &b.speaker)));
        for row in &table {
            println!(
                "setup {} seat {}: {:<24} affinity {:>5.2} over {:>3.0} s",
                row.setup, row.seat, row.speaker, row.affinity, row.evidence
            );
        }
        println!(
            "{:.0} s at {fps} fps, {width} px: cuts {cut_time:.1} s, decode+detect+track {total:.1} s \
             (detect+track {detect_time:.1} s) = {:.1}x realtime; {} shots, {} tracks",
            end - start,
            (end - start) / (cut_time + total),
            middles.len(),
            tracks.len()
        );

        // Coverage: sole speech whose speaker has a bound track on screen.
        let (mut speech, mut covered) = (0usize, 0usize);
        let mut time = start;
        while time < end {
            if let Some(speaker) = sole_speaker(&turns, time) {
                speech += 1;
                let on_screen = bindings.iter().any(|b| {
                    b.speaker == speaker && {
                        let track = &tracks[b.track];
                        track.start() <= time && time <= track.end()
                    }
                });
                covered += usize::from(on_screen);
            }
            time += 0.1;
        }
        println!(
            "speaker's bound face on screen during {:.0}% of sole speech",
            100.0 * covered as f64 / speech.max(1) as f64
        );

        for (shot, window) in bounds.windows(2).enumerate() {
            let mut talk: BTreeMap<&str, f64> = BTreeMap::new();
            let mut time = window[0];
            while time < window[1] {
                if let Some(speaker) = sole_speaker(&turns, time) {
                    *talk.entry(speaker).or_default() += 0.1;
                }
                time += 0.1;
            }
            println!(
                "shot {shot} {:.1}-{:.1}: speech {:?}",
                window[0],
                window[1],
                talk.iter()
                    .map(|(s, t)| format!("{s} {t:.0}s"))
                    .collect::<Vec<_>>()
            );
            for track in tracks.iter().filter(|t| t.shot == shot) {
                let n = track.observations.len() as f32;
                let cx = track
                    .observations
                    .iter()
                    .map(|o| o.face.center().0)
                    .sum::<f32>()
                    / n;
                let size = track
                    .observations
                    .iter()
                    .map(|o| o.face.height)
                    .sum::<f32>()
                    / n;
                let binding = bindings.iter().find(|b| b.track == track.id);
                println!(
                    "  track {:>3} x {cx:>6.0} size {size:>4.0} {:>5.1}-{:>5.1}: {}",
                    track.id,
                    track.start(),
                    track.end(),
                    binding.map_or("-".to_string(), |b| format!(
                        "{} (setup {} seat {}, affinity {:.2}, {:.0} s)",
                        b.speaker, b.setup, b.seat, b.affinity, b.evidence
                    ))
                );
            }
            if let (Some(dir), Some((frame, faces))) = (&keep, &snapshots[shot]) {
                let colour_of = |face: &Face| {
                    let track = tracks.iter().filter(|t| t.shot == shot).find(|t| {
                        t.observations
                            .iter()
                            .any(|o| (o.time - frame.time).abs() < 1e-6 && o.face == *face)
                    })?;
                    let binding = bindings.iter().find(|b| b.track == track.id)?;
                    let index = speakers.iter().position(|s| *s == binding.speaker)?;
                    Some(PALETTE[index % PALETTE.len()])
                };
                let boxes: Vec<(Face, [u8; 3])> = faces
                    .iter()
                    .map(|face| (*face, colour_of(face).unwrap_or([255, 255, 255])))
                    .collect();
                write_boxes(&dir.join(format!("shot_{shot:02}.ppm")), frame, &boxes);
            }
        }
        println!(
            "colours: {}",
            speakers
                .iter()
                .zip(["red", "green", "blue", "yellow", "magenta", "cyan"])
                .map(|(s, c)| format!("{c} = {s}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    const PALETTE: [[u8; 3]; 6] = [
        [255, 0, 0],
        [0, 255, 0],
        [0, 80, 255],
        [255, 255, 0],
        [255, 0, 255],
        [0, 255, 255],
    ];

    fn write_boxes(path: &Path, frame: &Frame, boxes: &[(Face, [u8; 3])]) {
        let (w, h) = (i64::from(frame.width), i64::from(frame.height));
        let mut data = frame.data.clone();
        for (face, colour) in boxes {
            for thickness in 0..3 {
                let t = thickness as f32;
                let (x0, y0) = (face.x - t, face.y - t);
                let (x1, y1) = (face.x + face.width + t, face.y + face.height + t);
                let steps = 200;
                for step in 0..=steps {
                    let f = step as f32 / steps as f32;
                    for (x, y) in [
                        (x0 + (x1 - x0) * f, y0),
                        (x0 + (x1 - x0) * f, y1),
                        (x0, y0 + (y1 - y0) * f),
                        (x1, y0 + (y1 - y0) * f),
                    ] {
                        let (x, y) = (x as i64, y as i64);
                        if (0..w).contains(&x) && (0..h).contains(&y) {
                            let i = ((y * w + x) * 3) as usize;
                            data[i..i + 3].copy_from_slice(colour);
                        }
                    }
                }
            }
        }
        let mut file = format!("P6\n{w} {h}\n255\n").into_bytes();
        file.extend_from_slice(&data);
        std::fs::write(path, file).unwrap();
    }
}
