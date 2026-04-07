use log::info;
use std::collections::BTreeMap;
use std::thread;
use std::time::Instant;

use crate::video::{
    TranscriptAlternative, TranscriptAlternativeSource, TranscriptMergeStatus, TranscriptSegment,
    TranscriptWord,
};

const MAX_PRIMARY_GROUP: usize = 4;
const MAX_REFERENCE_GROUP: usize = 4;
const GROUPING_PENALTY: f32 = 0.03;
const GAP_PENALTY: f32 = 0.72;
const MIN_MATCH_SIMILARITY: f32 = 0.34;
const STRONG_MATCH_SIMILARITY: f32 = 0.82;
const ANCHOR_MATCH_SIMILARITY: f32 = 0.94;
const ANCHOR_MARGIN: f32 = 0.08;
const ANCHOR_SEARCH_PADDING: usize = 18;
const MIN_ANCHOR_TEXT_CHARS: usize = 10;
const ALIGNMENT_BAND: usize = 32;
const MAX_WORD_RESEGMENT_WORDS: usize = 64;
const MIN_PARALLEL_GAP_SIZE: usize = 24;

#[derive(Clone, Copy)]
enum AlignmentStep {
    Match {
        primary_len: usize,
        reference_len: usize,
        similarity: f32,
    },
    MissingGoogle,
    MissingParakeet,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AlignmentAnchor {
    primary_index: usize,
    reference_index: usize,
    similarity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AlignmentGap {
    primary_start: usize,
    primary_end: usize,
    reference_start: usize,
    reference_end: usize,
}

#[allow(dead_code)]
pub fn merge_transcript_hypotheses(
    primary_transcript: Vec<TranscriptSegment>,
    reference_transcript: Vec<TranscriptSegment>,
) -> Vec<TranscriptSegment> {
    merge_transcript_hypotheses_with_progress(primary_transcript, reference_transcript, |_, _| {})
}

pub fn merge_transcript_hypotheses_with_progress<F>(
    primary_transcript: Vec<TranscriptSegment>,
    reference_transcript: Vec<TranscriptSegment>,
    mut on_progress: F,
) -> Vec<TranscriptSegment>
where
    F: FnMut(f32, &str),
{
    let started_at = Instant::now();
    info!(
        "Starting transcript merge: primary_segments={}, reference_segments={}",
        primary_transcript.len(),
        reference_transcript.len()
    );
    on_progress(2.0, "Preparing transcript merge...");

    if primary_transcript.is_empty() {
        return reference_transcript
            .into_iter()
            .map(|segment| TranscriptSegment {
                start: segment.start.clone(),
                end: segment.end.clone(),
                speaker: segment.speaker.clone(),
                text: segment.text.clone(),
                words: None,
                alternatives: Some(vec![
                    TranscriptAlternative {
                        source: TranscriptAlternativeSource::Parakeet,
                        text: String::new(),
                        speaker: None,
                        similarity_score: None,
                    },
                    TranscriptAlternative {
                        source: TranscriptAlternativeSource::Google,
                        text: segment.text,
                        speaker: Some(segment.speaker),
                        similarity_score: None,
                    },
                ]),
                merge_status: Some(TranscriptMergeStatus::MissingParakeet),
                active_source: Some(TranscriptAlternativeSource::Google),
                similarity_score: None,
            })
            .collect();
    }

    if reference_transcript.is_empty() {
        return primary_transcript
            .into_iter()
            .map(|segment| build_missing_google_segment(&segment))
            .collect();
    }

    on_progress(5.0, "Aligning Parakeet and remote transcripts...");
    let alignment = compute_alignment(&primary_transcript, &reference_transcript, &mut on_progress);
    on_progress(82.0, "Building merged transcript...");
    let merged = materialize_alignment(&primary_transcript, &reference_transcript, &alignment);
    on_progress(100.0, "Transcript merge complete.");
    info!(
        "Completed transcript merge in {:.2}s with {} output segments",
        started_at.elapsed().as_secs_f32(),
        merged.len()
    );
    merged
}

fn compute_alignment<F>(
    primary: &[TranscriptSegment],
    reference: &[TranscriptSegment],
    on_progress: &mut F,
) -> Vec<AlignmentStep>
where
    F: FnMut(f32, &str),
{
    on_progress(8.0, "Finding alignment anchors...");
    let anchors = detect_alignment_anchors(primary, reference);
    if anchors.is_empty() {
        return compute_alignment_window(primary, reference, Some((on_progress, 5.0, 77.0)));
    }

    info!(
        "Using {} transcript alignment anchors to split merge windows",
        anchors.len()
    );
    on_progress(
        12.0,
        "Found strong local matches. Aligning remaining transcript windows...",
    );

    let gaps = build_alignment_gaps(primary.len(), reference.len(), &anchors);
    let total_gap_units = gaps
        .iter()
        .map(|gap| {
            (gap.primary_end - gap.primary_start) + (gap.reference_end - gap.reference_start)
        })
        .sum::<usize>()
        .max(1);

    let gap_alignments = align_gaps(primary, reference, &gaps);
    let mut completed_gap_units = 0usize;
    let mut alignment = Vec::new();

    let gap_count = gaps.len();

    for (gap_index, gap) in gaps.iter().enumerate() {
        alignment.extend(gap_alignments[gap_index].iter().copied());
        completed_gap_units +=
            (gap.primary_end - gap.primary_start) + (gap.reference_end - gap.reference_start);
        let progress = 12.0 + (completed_gap_units as f32 / total_gap_units as f32) * 65.0;
        let message = if gap_count > 1 {
            format!(
                "Aligning transcript window {}/{}...",
                gap_index + 1,
                gap_count
            )
        } else {
            "Aligning Parakeet and remote transcripts...".to_string()
        };
        on_progress(progress.min(77.0), &message);

        if let Some(anchor) = anchors.get(gap_index) {
            alignment.push(AlignmentStep::Match {
                primary_len: 1,
                reference_len: 1,
                similarity: anchor.similarity,
            });
        }
    }

    alignment
}

fn compute_alignment_window<F>(
    primary: &[TranscriptSegment],
    reference: &[TranscriptSegment],
    mut progress: Option<(&mut F, f32, f32)>,
) -> Vec<AlignmentStep>
where
    F: FnMut(f32, &str),
{
    let primary_len = primary.len();
    let reference_len = reference.len();
    let mut costs = vec![vec![f32::INFINITY; reference_len + 1]; primary_len + 1];
    let mut previous = vec![vec![None; reference_len + 1]; primary_len + 1];
    let dynamic_band = ALIGNMENT_BAND.max(primary_len.abs_diff(reference_len) + 6);

    costs[0][0] = 0.0;

    for primary_index in 0..=primary_len {
        if let Some((on_progress, progress_start, progress_span)) = progress.as_mut() {
            if primary_len > 0 && primary_index < primary_len && primary_index % 8 == 0 {
                let progress =
                    *progress_start + (primary_index as f32 / primary_len as f32) * *progress_span;
                (*on_progress)(progress, "Aligning Parakeet and remote transcripts...");
            }
        }

        let center = if primary_len == 0 {
            0
        } else {
            (primary_index * reference_len) / primary_len
        };
        let reference_start = center.saturating_sub(dynamic_band);
        let reference_end = (center + dynamic_band).min(reference_len);

        for reference_index in reference_start..=reference_end {
            let current_cost = costs[primary_index][reference_index];
            if !current_cost.is_finite() {
                continue;
            }

            if primary_index < primary_len {
                let next_cost = current_cost + GAP_PENALTY;
                if next_cost < costs[primary_index + 1][reference_index] {
                    costs[primary_index + 1][reference_index] = next_cost;
                    previous[primary_index + 1][reference_index] =
                        Some(AlignmentStep::MissingGoogle);
                }
            }

            if reference_index < reference_len {
                let next_cost = current_cost + GAP_PENALTY;
                if next_cost < costs[primary_index][reference_index + 1] {
                    costs[primary_index][reference_index + 1] = next_cost;
                    previous[primary_index][reference_index + 1] =
                        Some(AlignmentStep::MissingParakeet);
                }
            }

            for primary_group_len in 1..=MAX_PRIMARY_GROUP {
                if primary_index + primary_group_len > primary_len {
                    break;
                }

                let primary_group = &primary[primary_index..primary_index + primary_group_len];
                if primary_group_len > 1 && !group_has_single_speaker(primary_group) {
                    continue;
                }

                for reference_group_len in 1..=MAX_REFERENCE_GROUP {
                    if reference_index + reference_group_len > reference_len {
                        break;
                    }

                    let reference_group =
                        &reference[reference_index..reference_index + reference_group_len];
                    let similarity = combined_similarity(primary_group, reference_group);
                    if similarity < MIN_MATCH_SIMILARITY {
                        continue;
                    }

                    let grouping_penalty = ((primary_group_len - 1) + (reference_group_len - 1))
                        as f32
                        * GROUPING_PENALTY;
                    let speaker_penalty = speaker_penalty(primary_group, reference_group);
                    let next_cost =
                        current_cost + (1.0 - similarity) + grouping_penalty + speaker_penalty;
                    let next_primary = primary_index + primary_group_len;
                    let next_reference = reference_index + reference_group_len;

                    if next_cost < costs[next_primary][next_reference] {
                        costs[next_primary][next_reference] = next_cost;
                        previous[next_primary][next_reference] = Some(AlignmentStep::Match {
                            primary_len: primary_group_len,
                            reference_len: reference_group_len,
                            similarity,
                        });
                    }
                }
            }
        }
    }

    let mut steps = Vec::new();
    let mut primary_index = primary_len;
    let mut reference_index = reference_len;

    while primary_index > 0 || reference_index > 0 {
        let step = previous[primary_index][reference_index].unwrap_or_else(|| {
            panic!(
                "Missing alignment step at {}, {}",
                primary_index, reference_index
            )
        });
        steps.push(step);

        match step {
            AlignmentStep::Match {
                primary_len,
                reference_len,
                ..
            } => {
                primary_index -= primary_len;
                reference_index -= reference_len;
            }
            AlignmentStep::MissingGoogle => {
                primary_index -= 1;
            }
            AlignmentStep::MissingParakeet => {
                reference_index -= 1;
            }
        }
    }

    steps.reverse();
    steps
}

fn detect_alignment_anchors(
    primary: &[TranscriptSegment],
    reference: &[TranscriptSegment],
) -> Vec<AlignmentAnchor> {
    if primary.is_empty() || reference.is_empty() {
        return Vec::new();
    }

    let mut primary_candidates = vec![None::<(usize, f32, f32)>; primary.len()];
    let mut reference_candidates = vec![None::<(usize, f32, f32)>; reference.len()];
    let band = ALIGNMENT_BAND.max(primary.len().abs_diff(reference.len()) + ANCHOR_SEARCH_PADDING);

    for (primary_index, primary_segment) in primary.iter().enumerate() {
        let normalized_primary = normalize_text(primary_segment.text.trim());
        if normalized_primary.len() < MIN_ANCHOR_TEXT_CHARS {
            continue;
        }

        let center = (primary_index * reference.len()) / primary.len();
        let reference_start = center.saturating_sub(band);
        let reference_end = (center + band).min(reference.len().saturating_sub(1));

        let mut best_match = None::<(usize, f32)>;
        let mut second_best = 0.0f32;

        for (reference_index, reference_segment) in reference
            .iter()
            .enumerate()
            .take(reference_end + 1)
            .skip(reference_start)
        {
            let normalized_reference = normalize_text(reference_segment.text.trim());
            if normalized_reference.len() < MIN_ANCHOR_TEXT_CHARS {
                continue;
            }

            let similarity = combined_similarity_text(&normalized_primary, &normalized_reference);
            if similarity >= ANCHOR_MATCH_SIMILARITY {
                if let Some((_, best_similarity)) = best_match {
                    if similarity > best_similarity {
                        second_best = best_similarity;
                        best_match = Some((reference_index, similarity));
                    } else if similarity > second_best {
                        second_best = similarity;
                    }
                } else {
                    best_match = Some((reference_index, similarity));
                }
            }
        }

        if let Some((reference_index, similarity)) = best_match {
            if similarity - second_best >= ANCHOR_MARGIN {
                primary_candidates[primary_index] =
                    Some((reference_index, similarity, second_best));
            }
        }
    }

    for (reference_index, reference_segment) in reference.iter().enumerate() {
        let normalized_reference = normalize_text(reference_segment.text.trim());
        if normalized_reference.len() < MIN_ANCHOR_TEXT_CHARS {
            continue;
        }

        let center = (reference_index * primary.len()) / reference.len();
        let primary_start = center.saturating_sub(band);
        let primary_end = (center + band).min(primary.len().saturating_sub(1));

        let mut best_match = None::<(usize, f32)>;
        let mut second_best = 0.0f32;

        for (primary_index, primary_segment) in primary
            .iter()
            .enumerate()
            .take(primary_end + 1)
            .skip(primary_start)
        {
            let normalized_primary = normalize_text(primary_segment.text.trim());
            if normalized_primary.len() < MIN_ANCHOR_TEXT_CHARS {
                continue;
            }

            let similarity = combined_similarity_text(&normalized_primary, &normalized_reference);
            if similarity >= ANCHOR_MATCH_SIMILARITY {
                if let Some((_, best_similarity)) = best_match {
                    if similarity > best_similarity {
                        second_best = best_similarity;
                        best_match = Some((primary_index, similarity));
                    } else if similarity > second_best {
                        second_best = similarity;
                    }
                } else {
                    best_match = Some((primary_index, similarity));
                }
            }
        }

        if let Some((primary_index, similarity)) = best_match {
            if similarity - second_best >= ANCHOR_MARGIN {
                reference_candidates[reference_index] =
                    Some((primary_index, similarity, second_best));
            }
        }
    }

    let mut anchors = Vec::new();
    let mut last_primary = None::<usize>;
    let mut last_reference = None::<usize>;

    for (primary_index, candidate) in primary_candidates.iter().enumerate() {
        let Some((reference_index, similarity, _)) = candidate else {
            continue;
        };

        let Some((back_primary_index, _, _)) = reference_candidates[*reference_index] else {
            continue;
        };

        if back_primary_index != primary_index {
            continue;
        }

        if last_primary.is_some_and(|last| primary_index <= last)
            || last_reference.is_some_and(|last| *reference_index <= last)
        {
            continue;
        }

        anchors.push(AlignmentAnchor {
            primary_index,
            reference_index: *reference_index,
            similarity: *similarity,
        });
        last_primary = Some(primary_index);
        last_reference = Some(*reference_index);
    }

    anchors
}

fn build_alignment_gaps(
    primary_len: usize,
    reference_len: usize,
    anchors: &[AlignmentAnchor],
) -> Vec<AlignmentGap> {
    let mut gaps = Vec::with_capacity(anchors.len() + 1);
    let mut primary_start = 0usize;
    let mut reference_start = 0usize;

    for anchor in anchors {
        gaps.push(AlignmentGap {
            primary_start,
            primary_end: anchor.primary_index,
            reference_start,
            reference_end: anchor.reference_index,
        });
        primary_start = anchor.primary_index + 1;
        reference_start = anchor.reference_index + 1;
    }

    gaps.push(AlignmentGap {
        primary_start,
        primary_end: primary_len,
        reference_start,
        reference_end: reference_len,
    });

    gaps
}

fn align_gaps(
    primary: &[TranscriptSegment],
    reference: &[TranscriptSegment],
    gaps: &[AlignmentGap],
) -> Vec<Vec<AlignmentStep>> {
    let non_empty_gap_count = gaps
        .iter()
        .filter(|gap| {
            gap.primary_start < gap.primary_end || gap.reference_start < gap.reference_end
        })
        .count();
    let total_gap_size = gaps
        .iter()
        .map(|gap| {
            (gap.primary_end - gap.primary_start) + (gap.reference_end - gap.reference_start)
        })
        .sum::<usize>();

    let should_parallelize = non_empty_gap_count > 1
        && total_gap_size >= MIN_PARALLEL_GAP_SIZE
        && thread::available_parallelism()
            .map(|parallelism| parallelism.get() > 1)
            .unwrap_or(false);

    if !should_parallelize {
        return gaps
            .iter()
            .map(|gap| {
                compute_alignment_window::<fn(f32, &str)>(
                    &primary[gap.primary_start..gap.primary_end],
                    &reference[gap.reference_start..gap.reference_end],
                    None,
                )
            })
            .collect();
    }

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(gaps.len());

        for gap in gaps {
            handles.push(scope.spawn(move || {
                compute_alignment_window::<fn(f32, &str)>(
                    &primary[gap.primary_start..gap.primary_end],
                    &reference[gap.reference_start..gap.reference_end],
                    None,
                )
            }));
        }

        handles
            .into_iter()
            .map(|handle| handle.join().expect("alignment gap worker panicked"))
            .collect()
    })
}

fn materialize_alignment(
    primary: &[TranscriptSegment],
    reference: &[TranscriptSegment],
    alignment: &[AlignmentStep],
) -> Vec<TranscriptSegment> {
    let mut merged = Vec::new();
    let mut primary_index = 0usize;
    let mut reference_index = 0usize;

    for step in alignment {
        match *step {
            AlignmentStep::Match {
                primary_len,
                reference_len,
                similarity,
            } => {
                let primary_group = &primary[primary_index..primary_index + primary_len];
                let reference_group = &reference[reference_index..reference_index + reference_len];
                merged.extend(build_matched_segments(
                    primary_group,
                    reference_group,
                    similarity,
                ));
                primary_index += primary_len;
                reference_index += reference_len;
            }
            AlignmentStep::MissingGoogle => {
                merged.push(build_missing_google_segment(&primary[primary_index]));
                primary_index += 1;
            }
            AlignmentStep::MissingParakeet => {
                merged.push(build_missing_parakeet_segment(
                    primary,
                    primary_index,
                    &reference[reference_index],
                ));
                reference_index += 1;
            }
        }
    }

    let merged = rebalance_adjacent_boundaries(merged);
    apply_inferred_speaker_labels(primary, merged)
}

fn rebalance_adjacent_boundaries(segments: Vec<TranscriptSegment>) -> Vec<TranscriptSegment> {
    let mut rebalanced = segments;
    let mut index = 0usize;

    while index + 1 < rebalanced.len() {
        let left = rebalanced[index].clone();
        let right = rebalanced[index + 1].clone();

        if let Some((updated_left, updated_right)) =
            optimize_boundary_between_segments(&left, &right)
        {
            rebalanced[index] = updated_left;
            rebalanced[index + 1] = updated_right;
        }

        index += 1;
    }

    rebalanced
}

fn optimize_boundary_between_segments(
    left: &TranscriptSegment,
    right: &TranscriptSegment,
) -> Option<(TranscriptSegment, TranscriptSegment)> {
    let left_google_text = google_text(left)?;
    let right_google_text = google_text(right)?;
    let left_words = left.words.as_ref()?;
    let right_words = right.words.as_ref()?;

    if left_words.is_empty() || right_words.is_empty() {
        return None;
    }

    let left_google_speaker = alternative_speaker(left, TranscriptAlternativeSource::Google);
    let right_google_speaker = alternative_speaker(right, TranscriptAlternativeSource::Google);
    let left_parakeet_speaker = alternative_speaker(left, TranscriptAlternativeSource::Parakeet);
    let right_parakeet_speaker = alternative_speaker(right, TranscriptAlternativeSource::Parakeet);

    let same_google_speaker =
        left_google_speaker.is_some() && left_google_speaker == right_google_speaker;
    let same_parakeet_speaker =
        left_parakeet_speaker.is_some() && left_parakeet_speaker == right_parakeet_speaker;

    if !(same_google_speaker || same_parakeet_speaker || left.speaker == right.speaker) {
        return None;
    }

    let combined_words = left_words
        .iter()
        .chain(right_words.iter())
        .cloned()
        .collect::<Vec<_>>();
    let original_split = left_words.len();
    let search_radius = 8usize
        .min(original_split.saturating_sub(1).max(1))
        .min(right_words.len());
    let split_start = original_split.saturating_sub(search_radius).max(1);
    let split_end = (original_split + search_radius).min(combined_words.len() - 1);

    let mut best_split = original_split;
    let mut best_score = boundary_score(
        &combined_words[..original_split],
        left_google_text,
        &combined_words[original_split..],
        right_google_text,
        original_split,
        original_split,
    );

    for split_index in split_start..=split_end {
        let score = boundary_score(
            &combined_words[..split_index],
            left_google_text,
            &combined_words[split_index..],
            right_google_text,
            split_index,
            original_split,
        );
        if score > best_score + 0.02 {
            best_score = score;
            best_split = split_index;
        }
    }

    if best_split == original_split {
        return None;
    }

    let updated_left = rebuild_segment_with_words(
        left,
        combined_words[..best_split].to_vec(),
        left_google_text,
    );
    let updated_right = rebuild_segment_with_words(
        right,
        combined_words[best_split..].to_vec(),
        right_google_text,
    );

    Some((updated_left, updated_right))
}

fn boundary_score(
    left_words: &[TranscriptWord],
    left_google_text: &str,
    right_words: &[TranscriptWord],
    right_google_text: &str,
    split_index: usize,
    original_split: usize,
) -> f32 {
    if left_words.is_empty() || right_words.is_empty() {
        return f32::NEG_INFINITY;
    }

    let left_text = join_word_text(left_words);
    let right_text = join_word_text(right_words);
    let left_similarity = combined_similarity_text(&left_text, left_google_text);
    let right_similarity = combined_similarity_text(&right_text, right_google_text);
    let distance_penalty = split_index.abs_diff(original_split) as f32 * 0.01;

    left_similarity + right_similarity - distance_penalty
}

fn rebuild_segment_with_words(
    template: &TranscriptSegment,
    words: Vec<TranscriptWord>,
    google_text: &str,
) -> TranscriptSegment {
    let first = words
        .first()
        .expect("rebuilt segment words must not be empty");
    let last = words
        .last()
        .expect("rebuilt segment words must not be empty");
    let parakeet_text = join_word_text(&words);
    let similarity = combined_similarity_text(&parakeet_text, google_text);
    let parakeet_speaker = words
        .iter()
        .find_map(|word| {
            word.speaker
                .clone()
                .filter(|speaker| !speaker.trim().is_empty())
        })
        .or_else(|| alternative_speaker(template, TranscriptAlternativeSource::Parakeet));
    let google_speaker = alternative_speaker(template, TranscriptAlternativeSource::Google);
    let resolved_speaker = google_speaker
        .clone()
        .or(parakeet_speaker.clone())
        .unwrap_or_else(|| template.speaker.clone());

    TranscriptSegment {
        start: first.start.clone(),
        end: last.end.clone(),
        speaker: resolved_speaker,
        text: google_text.to_string(),
        words: Some(words),
        alternatives: Some(vec![
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Parakeet,
                text: parakeet_text,
                speaker: parakeet_speaker,
                similarity_score: Some(similarity),
            },
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Google,
                text: google_text.to_string(),
                speaker: google_speaker,
                similarity_score: Some(similarity),
            },
        ]),
        merge_status: Some(if similarity >= STRONG_MATCH_SIMILARITY {
            TranscriptMergeStatus::Matched
        } else {
            TranscriptMergeStatus::Conflict
        }),
        active_source: Some(TranscriptAlternativeSource::Google),
        similarity_score: Some(similarity),
    }
}

fn google_text(segment: &TranscriptSegment) -> Option<&str> {
    segment
        .alternatives
        .as_ref()
        .and_then(|alternatives| {
            alternatives
                .iter()
                .find(|alternative| alternative.source == TranscriptAlternativeSource::Google)
        })
        .map(|alternative| alternative.text.trim())
        .filter(|text| !text.is_empty())
}

fn build_matched_segments(
    primary_group: &[TranscriptSegment],
    reference_group: &[TranscriptSegment],
    similarity: f32,
) -> Vec<TranscriptSegment> {
    let merged_words = merge_words(primary_group);
    if reference_group.len() > 1 {
        if let Some(words) = merged_words.clone() {
            if words.len() <= MAX_WORD_RESEGMENT_WORDS {
                if let Some(split_ranges) = split_word_ranges_by_reference(&words, reference_group)
                {
                    return split_ranges
                        .into_iter()
                        .enumerate()
                        .map(|(index, (start, end, local_similarity))| {
                            build_segment_from_word_range(
                                &words[start..=end],
                                &reference_group[index],
                                local_similarity,
                            )
                        })
                        .collect();
                }
            }
        }
    }

    vec![build_matched_segment(
        primary_group,
        reference_group,
        similarity,
        merged_words,
    )]
}

fn build_matched_segment(
    primary_group: &[TranscriptSegment],
    reference_group: &[TranscriptSegment],
    similarity: f32,
    merged_words: Option<Vec<TranscriptWord>>,
) -> TranscriptSegment {
    let first = primary_group
        .first()
        .expect("matched group must not be empty");
    let last = primary_group
        .last()
        .expect("matched group must not be empty");
    let parakeet_text = join_segment_text(primary_group);
    let google_text = join_segment_text(reference_group);
    let parakeet_speaker = first.speaker.clone();
    let google_speaker = preferred_group_speaker(reference_group);
    let resolved_speaker = google_speaker
        .clone()
        .filter(|speaker| !speaker.trim().is_empty())
        .unwrap_or_else(|| parakeet_speaker.clone());

    TranscriptSegment {
        start: first.start.clone(),
        end: last.end.clone(),
        speaker: resolved_speaker,
        text: google_text.clone(),
        words: merged_words,
        alternatives: Some(vec![
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Parakeet,
                text: parakeet_text,
                speaker: Some(parakeet_speaker),
                similarity_score: Some(similarity),
            },
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Google,
                text: google_text,
                speaker: google_speaker,
                similarity_score: Some(similarity),
            },
        ]),
        merge_status: Some(if similarity >= STRONG_MATCH_SIMILARITY {
            TranscriptMergeStatus::Matched
        } else {
            TranscriptMergeStatus::Conflict
        }),
        active_source: Some(TranscriptAlternativeSource::Google),
        similarity_score: Some(similarity),
    }
}

fn build_segment_from_word_range(
    words: &[TranscriptWord],
    reference_segment: &TranscriptSegment,
    similarity: f32,
) -> TranscriptSegment {
    let first = words.first().expect("word range must not be empty");
    let last = words.last().expect("word range must not be empty");
    let parakeet_text = join_word_text(words);
    let google_speaker =
        (!reference_segment.speaker.trim().is_empty()).then_some(reference_segment.speaker.clone());
    let resolved_speaker = google_speaker.clone().unwrap_or_else(|| {
        first
            .speaker
            .clone()
            .unwrap_or_else(|| "Speaker Unknown".to_string())
    });

    TranscriptSegment {
        start: first.start.clone(),
        end: last.end.clone(),
        speaker: resolved_speaker,
        text: reference_segment.text.clone(),
        words: Some(words.to_vec()),
        alternatives: Some(vec![
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Parakeet,
                text: parakeet_text,
                speaker: first.speaker.clone(),
                similarity_score: Some(similarity),
            },
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Google,
                text: reference_segment.text.clone(),
                speaker: google_speaker,
                similarity_score: Some(similarity),
            },
        ]),
        merge_status: Some(if similarity >= STRONG_MATCH_SIMILARITY {
            TranscriptMergeStatus::Matched
        } else {
            TranscriptMergeStatus::Conflict
        }),
        active_source: Some(TranscriptAlternativeSource::Google),
        similarity_score: Some(similarity),
    }
}

fn build_missing_google_segment(segment: &TranscriptSegment) -> TranscriptSegment {
    TranscriptSegment {
        start: segment.start.clone(),
        end: segment.end.clone(),
        speaker: segment.speaker.clone(),
        text: segment.text.clone(),
        words: segment.words.clone(),
        alternatives: Some(vec![
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Parakeet,
                text: segment.text.clone(),
                speaker: Some(segment.speaker.clone()),
                similarity_score: None,
            },
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Google,
                text: String::new(),
                speaker: None,
                similarity_score: None,
            },
        ]),
        merge_status: Some(TranscriptMergeStatus::MissingGoogle),
        active_source: Some(TranscriptAlternativeSource::Parakeet),
        similarity_score: None,
    }
}

fn build_missing_parakeet_segment(
    primary: &[TranscriptSegment],
    insertion_index: usize,
    reference_segment: &TranscriptSegment,
) -> TranscriptSegment {
    let anchor_start = if insertion_index == 0 {
        primary
            .first()
            .map(|segment| segment.start.clone())
            .unwrap_or_else(|| "00:00.000".to_string())
    } else {
        primary[insertion_index - 1].end.clone()
    };
    let anchor_end = if insertion_index < primary.len() {
        primary[insertion_index].start.clone()
    } else {
        anchor_start.clone()
    };

    TranscriptSegment {
        start: anchor_start.clone(),
        end: anchor_end,
        speaker: if reference_segment.speaker.trim().is_empty() {
            primary
                .get(insertion_index)
                .map(|segment| segment.speaker.clone())
                .or_else(|| primary.last().map(|segment| segment.speaker.clone()))
                .unwrap_or_else(|| "Speaker Unknown".to_string())
        } else {
            reference_segment.speaker.clone()
        },
        text: reference_segment.text.clone(),
        words: None,
        alternatives: Some(vec![
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Parakeet,
                text: String::new(),
                speaker: None,
                similarity_score: None,
            },
            TranscriptAlternative {
                source: TranscriptAlternativeSource::Google,
                text: reference_segment.text.clone(),
                speaker: Some(reference_segment.speaker.clone()),
                similarity_score: None,
            },
        ]),
        merge_status: Some(TranscriptMergeStatus::MissingParakeet),
        active_source: Some(TranscriptAlternativeSource::Google),
        similarity_score: None,
    }
}

fn group_has_single_speaker(group: &[TranscriptSegment]) -> bool {
    let first_speaker = group.first().map(|segment| segment.speaker.as_str());
    group
        .iter()
        .all(|segment| Some(segment.speaker.as_str()) == first_speaker)
}

fn preferred_group_speaker(group: &[TranscriptSegment]) -> Option<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    let mut first_seen = Vec::<String>::new();

    for segment in group {
        let speaker = segment.speaker.trim();
        if speaker.is_empty() {
            continue;
        }

        let speaker = speaker.to_string();
        if !counts.contains_key(&speaker) {
            first_seen.push(speaker.clone());
        }
        *counts.entry(speaker).or_insert(0) += 1;
    }

    let mut best_speaker = None;
    let mut best_count = 0usize;

    for speaker in first_seen {
        let count = counts.get(&speaker).copied().unwrap_or_default();
        if count > best_count {
            best_count = count;
            best_speaker = Some(speaker);
        }
    }

    best_speaker
}

fn speaker_penalty(
    primary_group: &[TranscriptSegment],
    reference_group: &[TranscriptSegment],
) -> f32 {
    let primary_speaker = primary_group.first().map(|segment| segment.speaker.trim());
    let reference_speaker = reference_group
        .first()
        .map(|segment| segment.speaker.trim());

    if primary_group.len() == 1
        && reference_group.len() == 1
        && primary_speaker.is_some()
        && reference_speaker.is_some()
        && primary_speaker != reference_speaker
    {
        0.05
    } else {
        0.0
    }
}

fn join_segment_text(segments: &[TranscriptSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn merge_words(segments: &[TranscriptSegment]) -> Option<Vec<TranscriptWord>> {
    let merged = segments
        .iter()
        .filter_map(|segment| segment.words.as_ref())
        .flat_map(|words| words.iter().cloned())
        .collect::<Vec<_>>();

    (!merged.is_empty()).then_some(merged)
}

fn join_word_text(words: &[TranscriptWord]) -> String {
    let mut text = String::new();

    for word in words {
        append_token_like(&mut text, word.text.trim());
    }

    text.trim().to_string()
}

fn append_token_like(buffer: &mut String, token: &str) {
    if token.is_empty() {
        return;
    }

    if !buffer.is_empty() && !is_punctuation_only(token) {
        buffer.push(' ');
    }
    buffer.push_str(token);
}

fn combined_similarity(
    primary_group: &[TranscriptSegment],
    reference_group: &[TranscriptSegment],
) -> f32 {
    let primary = normalize_text(&join_segment_text(primary_group));
    let reference = normalize_text(&join_segment_text(reference_group));

    if primary.is_empty() || reference.is_empty() {
        return 0.0;
    }

    let levenshtein_similarity = normalized_levenshtein(&primary, &reference);
    let token_similarity = token_overlap_similarity(&primary, &reference);
    (levenshtein_similarity * 0.75) + (token_similarity * 0.25)
}

fn combined_similarity_text(left: &str, right: &str) -> f32 {
    let normalized_left = normalize_text(left);
    let normalized_right = normalize_text(right);

    if normalized_left.is_empty() || normalized_right.is_empty() {
        return 0.0;
    }

    let levenshtein_similarity = normalized_levenshtein(&normalized_left, &normalized_right);
    let token_similarity = token_overlap_similarity(&normalized_left, &normalized_right);
    (levenshtein_similarity * 0.75) + (token_similarity * 0.25)
}

fn split_word_ranges_by_reference(
    words: &[TranscriptWord],
    reference_group: &[TranscriptSegment],
) -> Option<Vec<(usize, usize, f32)>> {
    if words.len() < reference_group.len() || reference_group.is_empty() {
        return None;
    }

    let segment_count = reference_group.len();
    let word_count = words.len();
    let mut costs = vec![vec![f32::INFINITY; word_count + 1]; segment_count + 1];
    let mut previous = vec![vec![None; word_count + 1]; segment_count + 1];

    costs[0][0] = 0.0;

    for segment_index in 0..segment_count {
        for start_index in 0..word_count {
            let current_cost = costs[segment_index][start_index];
            if !current_cost.is_finite() {
                continue;
            }

            let remaining_segments = segment_count - segment_index - 1;
            let max_end = word_count.saturating_sub(remaining_segments + 1);

            for end_index in start_index..=max_end {
                let candidate_text = join_word_text(&words[start_index..=end_index]);
                let similarity = combined_similarity_text(
                    &candidate_text,
                    reference_group[segment_index].text.trim(),
                );
                let next_cost = current_cost + (1.0 - similarity);

                if next_cost < costs[segment_index + 1][end_index + 1] {
                    costs[segment_index + 1][end_index + 1] = next_cost;
                    previous[segment_index + 1][end_index + 1] =
                        Some((start_index, end_index, similarity));
                }
            }
        }
    }

    if !costs[segment_count][word_count].is_finite() {
        return None;
    }

    let mut ranges = Vec::with_capacity(segment_count);
    let mut segment_index = segment_count;
    let mut word_index = word_count;

    while segment_index > 0 {
        let (start_index, end_index, similarity) = previous[segment_index][word_index]?;
        ranges.push((start_index, end_index, similarity));
        segment_index -= 1;
        word_index = start_index;
    }

    ranges.reverse();
    Some(ranges)
}

fn normalize_text(text: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_space = false;

    for character in text.chars().flat_map(|char| char.to_lowercase()) {
        if character.is_alphanumeric() {
            normalized.push(character);
            previous_was_space = false;
        } else if character.is_whitespace() && !previous_was_space {
            normalized.push(' ');
            previous_was_space = true;
        }
    }

    normalized.trim().to_string()
}

fn is_punctuation_only(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|character| {
            matches!(
                character,
                '.' | ',' | '!' | '?' | ';' | ':' | ')' | ']' | '"'
            )
        })
}

fn normalized_levenshtein(left: &str, right: &str) -> f32 {
    let left_chars = left.chars().collect::<Vec<_>>();
    let right_chars = right.chars().collect::<Vec<_>>();

    if left_chars.is_empty() && right_chars.is_empty() {
        return 1.0;
    }

    let mut previous_row = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current_row = vec![0usize; right_chars.len() + 1];

    for (left_index, left_char) in left_chars.iter().enumerate() {
        current_row[0] = left_index + 1;

        for (right_index, right_char) in right_chars.iter().enumerate() {
            let substitution_cost = if left_char == right_char { 0 } else { 1 };
            current_row[right_index + 1] = std::cmp::min(
                std::cmp::min(
                    current_row[right_index] + 1,
                    previous_row[right_index + 1] + 1,
                ),
                previous_row[right_index] + substitution_cost,
            );
        }

        std::mem::swap(&mut previous_row, &mut current_row);
    }

    let distance = previous_row[right_chars.len()];
    let max_len = left_chars.len().max(right_chars.len()) as f32;
    1.0 - (distance as f32 / max_len)
}

fn token_overlap_similarity(left: &str, right: &str) -> f32 {
    let left_tokens = left
        .split_whitespace()
        .collect::<std::collections::BTreeSet<_>>();
    let right_tokens = right
        .split_whitespace()
        .collect::<std::collections::BTreeSet<_>>();

    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }

    let intersection = left_tokens.intersection(&right_tokens).count() as f32;
    let union = left_tokens.union(&right_tokens).count() as f32;
    intersection / union
}

fn apply_inferred_speaker_labels(
    primary: &[TranscriptSegment],
    merged: Vec<TranscriptSegment>,
) -> Vec<TranscriptSegment> {
    let speaker_orders = primary_speaker_orders(primary);
    let mut cooccurrence_by_parakeet = BTreeMap::<String, BTreeMap<String, usize>>::new();
    let mut cooccurrence_by_order = BTreeMap::<usize, BTreeMap<String, usize>>::new();

    merged
        .into_iter()
        .map(|mut segment| {
            let parakeet_speaker =
                alternative_speaker(&segment, TranscriptAlternativeSource::Parakeet).or_else(
                    || {
                        segment.words.as_ref().and_then(|words| {
                            words.iter().find_map(|word| {
                                word.speaker
                                    .clone()
                                    .filter(|speaker| !speaker.trim().is_empty())
                            })
                        })
                    },
                );
            let speaker_order = parakeet_speaker
                .as_ref()
                .and_then(|speaker| speaker_orders.get(speaker).copied());
            let google_speaker = alternative_speaker(&segment, TranscriptAlternativeSource::Google);

            if let Some(google_speaker) = google_speaker {
                segment.speaker = google_speaker.clone();

                if let Some(parakeet_speaker) = &parakeet_speaker {
                    record_cooccurrence(
                        &mut cooccurrence_by_parakeet,
                        parakeet_speaker.clone(),
                        google_speaker.clone(),
                    );
                }

                if let Some(speaker_order) = speaker_order {
                    record_cooccurrence(&mut cooccurrence_by_order, speaker_order, google_speaker);
                }

                return segment;
            }

            if let Some(parakeet_speaker) = parakeet_speaker {
                if let Some(inferred) = infer_speaker_from_cooccurrences(
                    &cooccurrence_by_parakeet,
                    &cooccurrence_by_order,
                    &parakeet_speaker,
                    speaker_order,
                ) {
                    segment.speaker = inferred;
                }
            }

            segment
        })
        .collect()
}

fn record_cooccurrence<K: Ord>(
    counts_by_key: &mut BTreeMap<K, BTreeMap<String, usize>>,
    key: K,
    speaker: String,
) {
    let counts = counts_by_key.entry(key).or_default();
    *counts.entry(speaker).or_insert(0) += 1;
}

fn infer_speaker_from_cooccurrences(
    counts_by_parakeet: &BTreeMap<String, BTreeMap<String, usize>>,
    counts_by_order: &BTreeMap<usize, BTreeMap<String, usize>>,
    parakeet_speaker: &str,
    speaker_order: Option<usize>,
) -> Option<String> {
    let mut combined = BTreeMap::<String, usize>::new();

    if let Some(counts) = counts_by_parakeet.get(parakeet_speaker) {
        for (speaker, count) in counts {
            *combined.entry(speaker.clone()).or_insert(0) += count * 2;
        }
    }

    if let Some(speaker_order) = speaker_order {
        if let Some(counts) = counts_by_order.get(&speaker_order) {
            for (speaker, count) in counts {
                *combined.entry(speaker.clone()).or_insert(0) += count;
            }
        }
    }

    combined
        .into_iter()
        .max_by(|(left_speaker, left_count), (right_speaker, right_count)| {
            left_count
                .cmp(right_count)
                .then_with(|| right_speaker.cmp(left_speaker))
        })
        .map(|(speaker, _)| speaker)
}

fn primary_speaker_orders(primary: &[TranscriptSegment]) -> BTreeMap<String, usize> {
    let mut orders = BTreeMap::new();
    let mut next_order = 0usize;

    for segment in primary {
        if !orders.contains_key(&segment.speaker) {
            orders.insert(segment.speaker.clone(), next_order);
            next_order += 1;
        }
    }

    orders
}

fn alternative_speaker(
    segment: &TranscriptSegment,
    source: TranscriptAlternativeSource,
) -> Option<String> {
    segment
        .alternatives
        .as_ref()
        .and_then(|alternatives| {
            alternatives
                .iter()
                .find(|alternative| alternative.source == source)
        })
        .and_then(|alternative| alternative.speaker.as_ref())
        .map(|speaker| speaker.trim())
        .filter(|speaker| !speaker.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::{TranscriptAlternativeSource, TranscriptMergeStatus};

    fn segment(start: &str, end: &str, speaker: &str, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start: start.into(),
            end: end.into(),
            speaker: speaker.into(),
            text: text.into(),
            words: None,
            alternatives: None,
            merge_status: None,
            active_source: None,
            similarity_score: None,
        }
    }

    #[test]
    fn merges_matching_segments_and_prefers_reference_text() {
        let merged = merge_transcript_hypotheses(
            vec![segment(
                "00:00.000",
                "00:02.000",
                "Speaker 1",
                "hello itemis team",
            )],
            vec![segment(
                "00:00.000",
                "00:02.000",
                "Speaker 1",
                "Hello itemis team.",
            )],
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "Hello itemis team.");
        assert_eq!(
            merged[0].active_source,
            Some(TranscriptAlternativeSource::Google)
        );
        assert!(matches!(
            merged[0].merge_status,
            Some(TranscriptMergeStatus::Matched | TranscriptMergeStatus::Conflict)
        ));
    }

    #[test]
    fn detects_missing_google_segment() {
        let merged = merge_transcript_hypotheses(
            vec![
                segment("00:00.000", "00:01.000", "Speaker 1", "intro"),
                segment("00:01.000", "00:02.000", "Speaker 1", "roadmap update"),
            ],
            vec![segment("00:00.000", "00:01.000", "Speaker 1", "intro")],
        );

        assert_eq!(merged.len(), 2);
        assert_eq!(
            merged[1].merge_status,
            Some(TranscriptMergeStatus::MissingGoogle)
        );
        assert_eq!(
            merged[1].active_source,
            Some(TranscriptAlternativeSource::Parakeet)
        );
    }

    #[test]
    fn detects_missing_parakeet_segment() {
        let merged = merge_transcript_hypotheses(
            vec![segment("00:00.000", "00:01.000", "Speaker 1", "intro")],
            vec![
                segment("00:00.000", "00:01.000", "Speaker 1", "intro"),
                segment("00:01.000", "00:02.000", "Speaker 1", "agenda point"),
            ],
        );

        assert_eq!(merged.len(), 2);
        assert_eq!(
            merged[1].merge_status,
            Some(TranscriptMergeStatus::MissingParakeet)
        );
        assert_eq!(
            merged[1].active_source,
            Some(TranscriptAlternativeSource::Google)
        );
    }

    #[test]
    fn aligns_grouped_primary_segments_to_single_reference_segment() {
        let merged = merge_transcript_hypotheses(
            vec![
                segment("00:00.000", "00:01.000", "Speaker 1", "we reviewed"),
                segment("00:01.000", "00:02.000", "Speaker 1", "the release plan"),
            ],
            vec![segment(
                "00:00.000",
                "00:02.000",
                "Speaker 1",
                "We reviewed the release plan.",
            )],
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start, "00:00.000");
        assert_eq!(merged[0].end, "00:02.000");
        assert_eq!(merged[0].text, "We reviewed the release plan.");
    }

    #[test]
    fn prefers_google_speaker_when_alignment_matches_but_labels_differ() {
        let merged = merge_transcript_hypotheses(
            vec![segment("00:00.000", "00:02.000", "Speaker 1", "hello team")],
            vec![segment(
                "00:00.000",
                "00:02.000",
                "Speaker 2",
                "Hello team.",
            )],
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].speaker, "Speaker 2");
        assert_eq!(
            merged[0]
                .alternatives
                .as_ref()
                .and_then(|alternatives| alternatives
                    .iter()
                    .find(|item| item.source == TranscriptAlternativeSource::Google))
                .and_then(|item| item.speaker.clone()),
            Some("Speaker 2".to_string())
        );
    }

    #[test]
    fn uses_neighboring_segments_to_absorb_boundary_shifts_and_split_by_reference() {
        let merged = merge_transcript_hypotheses(
            vec![
                TranscriptSegment {
                    start: "00:00.000".into(),
                    end: "00:02.000".into(),
                    speaker: "Speaker 1".into(),
                    text: "hello there general".into(),
                    words: Some(vec![
                        TranscriptWord {
                            start: "00:00.000".into(),
                            end: "00:00.500".into(),
                            text: "hello".into(),
                            speaker: Some("Speaker 1".into()),
                        },
                        TranscriptWord {
                            start: "00:00.500".into(),
                            end: "00:01.000".into(),
                            text: "there".into(),
                            speaker: Some("Speaker 1".into()),
                        },
                        TranscriptWord {
                            start: "00:01.000".into(),
                            end: "00:02.000".into(),
                            text: "general".into(),
                            speaker: Some("Speaker 1".into()),
                        },
                    ]),
                    alternatives: None,
                    merge_status: None,
                    active_source: None,
                    similarity_score: None,
                },
                TranscriptSegment {
                    start: "00:02.000".into(),
                    end: "00:04.000".into(),
                    speaker: "Speaker 1".into(),
                    text: "kenobi welcome back".into(),
                    words: Some(vec![
                        TranscriptWord {
                            start: "00:02.000".into(),
                            end: "00:02.500".into(),
                            text: "kenobi".into(),
                            speaker: Some("Speaker 1".into()),
                        },
                        TranscriptWord {
                            start: "00:02.500".into(),
                            end: "00:03.250".into(),
                            text: "welcome".into(),
                            speaker: Some("Speaker 1".into()),
                        },
                        TranscriptWord {
                            start: "00:03.250".into(),
                            end: "00:04.000".into(),
                            text: "back".into(),
                            speaker: Some("Speaker 1".into()),
                        },
                    ]),
                    alternatives: None,
                    merge_status: None,
                    active_source: None,
                    similarity_score: None,
                },
            ],
            vec![
                segment("00:00.000", "00:01.000", "Speaker 2", "hello there"),
                segment("00:01.000", "00:02.500", "Speaker 2", "general kenobi"),
                segment("00:02.500", "00:04.000", "Speaker 2", "welcome back"),
            ],
        );

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, "hello there");
        assert_eq!(merged[0].start, "00:00.000");
        assert_eq!(merged[0].end, "00:01.000");
        assert_eq!(merged[1].text, "general kenobi");
        assert_eq!(merged[1].start, "00:01.000");
        assert_eq!(merged[1].end, "00:02.500");
        assert_eq!(merged[2].text, "welcome back");
        assert_eq!(merged[2].start, "00:02.500");
        assert_eq!(merged[2].end, "00:04.000");
    }

    #[test]
    fn infers_later_parakeet_only_speaker_from_previous_google_match() {
        let merged = merge_transcript_hypotheses(
            vec![
                segment("00:00.000", "00:01.000", "Speaker 1", "hello"),
                segment(
                    "00:01.000",
                    "00:02.000",
                    "Speaker 1",
                    "completely unrelated closing remark",
                ),
            ],
            vec![segment("00:00.000", "00:01.000", "Dirk Leopold", "hello")],
        );

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].speaker, "Dirk Leopold");
        assert_eq!(
            merged[1].merge_status,
            Some(TranscriptMergeStatus::MissingGoogle)
        );
        assert_eq!(merged[1].speaker, "Dirk Leopold");
    }

    #[test]
    fn infers_speaker_by_primary_appearance_order_when_google_named_that_slot_earlier() {
        let merged = merge_transcript_hypotheses(
            vec![
                segment("00:00.000", "00:01.000", "Speaker 1", "host intro"),
                segment("00:01.000", "00:02.000", "Speaker 2", "guest answer"),
                segment(
                    "00:02.000",
                    "00:03.000",
                    "Speaker 2",
                    "completely unrelated guest follow up",
                ),
            ],
            vec![
                segment("00:00.000", "00:01.000", "Dirk Leopold", "host intro"),
                segment("00:01.000", "00:02.000", "Alice Example", "guest answer"),
            ],
        );

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].speaker, "Dirk Leopold");
        assert_eq!(merged[1].speaker, "Alice Example");
        assert_eq!(
            merged[2].merge_status,
            Some(TranscriptMergeStatus::MissingGoogle)
        );
        assert_eq!(merged[2].speaker, "Alice Example");
    }

    #[test]
    fn prefers_highest_cooccurrence_mapping_for_missing_google_segments() {
        let mut counts_by_parakeet = BTreeMap::<String, BTreeMap<String, usize>>::new();
        let mut counts_by_order = BTreeMap::<usize, BTreeMap<String, usize>>::new();

        record_cooccurrence(
            &mut counts_by_parakeet,
            "Speaker 1".to_string(),
            "Dirk Leopold".to_string(),
        );
        record_cooccurrence(
            &mut counts_by_parakeet,
            "Speaker 1".to_string(),
            "Dirk Leopold".to_string(),
        );
        record_cooccurrence(
            &mut counts_by_parakeet,
            "Speaker 1".to_string(),
            "Moderator".to_string(),
        );
        record_cooccurrence(&mut counts_by_order, 0usize, "Dirk Leopold".to_string());

        assert_eq!(
            infer_speaker_from_cooccurrences(
                &counts_by_parakeet,
                &counts_by_order,
                "Speaker 1",
                Some(0),
            ),
            Some("Dirk Leopold".to_string())
        );
    }

    #[test]
    fn detects_mutual_anchor_matches_for_near_perfect_segments() {
        let anchors = detect_alignment_anchors(
            &[
                segment(
                    "00:00.000",
                    "00:01.000",
                    "Speaker 1",
                    "exact anchor opening",
                ),
                segment(
                    "00:01.000",
                    "00:02.000",
                    "Speaker 1",
                    "middle content differs a lot",
                ),
                segment(
                    "00:02.000",
                    "00:03.000",
                    "Speaker 1",
                    "exact anchor closing",
                ),
            ],
            &[
                segment("00:00.000", "00:01.000", "Host", "exact anchor opening"),
                segment(
                    "00:01.000",
                    "00:02.000",
                    "Host",
                    "very different middle wording",
                ),
                segment("00:02.000", "00:03.000", "Host", "exact anchor closing"),
            ],
        );

        assert_eq!(
            anchors,
            vec![
                AlignmentAnchor {
                    primary_index: 0,
                    reference_index: 0,
                    similarity: 1.0,
                },
                AlignmentAnchor {
                    primary_index: 2,
                    reference_index: 2,
                    similarity: 1.0,
                },
            ]
        );
    }

    #[test]
    fn builds_independent_alignment_gaps_around_anchors() {
        let anchors = vec![
            AlignmentAnchor {
                primary_index: 1,
                reference_index: 1,
                similarity: 0.99,
            },
            AlignmentAnchor {
                primary_index: 4,
                reference_index: 3,
                similarity: 0.98,
            },
        ];

        let gaps = build_alignment_gaps(6, 5, &anchors);

        assert_eq!(
            gaps,
            vec![
                AlignmentGap {
                    primary_start: 0,
                    primary_end: 1,
                    reference_start: 0,
                    reference_end: 1,
                },
                AlignmentGap {
                    primary_start: 2,
                    primary_end: 4,
                    reference_start: 2,
                    reference_end: 3,
                },
                AlignmentGap {
                    primary_start: 5,
                    primary_end: 6,
                    reference_start: 4,
                    reference_end: 5,
                },
            ]
        );
    }

    #[test]
    fn uses_anchor_points_to_preserve_order_while_aligning_gaps() {
        let merged = merge_transcript_hypotheses(
            vec![
                segment(
                    "00:00.000",
                    "00:01.000",
                    "Speaker 1",
                    "exact anchor opening",
                ),
                segment("00:01.000", "00:02.000", "Speaker 1", "left middle phrase"),
                segment(
                    "00:02.000",
                    "00:03.000",
                    "Speaker 1",
                    "exact anchor closing",
                ),
            ],
            vec![
                segment("00:00.000", "00:01.000", "Host", "exact anchor opening"),
                segment(
                    "00:01.000",
                    "00:02.000",
                    "Host",
                    "left middle phrase refined",
                ),
                segment("00:02.000", "00:03.000", "Host", "exact anchor closing"),
            ],
        );

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, "exact anchor opening");
        assert_eq!(merged[1].text, "left middle phrase refined");
        assert_eq!(merged[2].text, "exact anchor closing");
    }

    #[test]
    fn rebalances_adjacent_boundaries_when_a_sentence_spills_into_the_next_segment() {
        let merged = merge_transcript_hypotheses(
            vec![
                TranscriptSegment {
                    start: "02:10.000".into(),
                    end: "02:19.040".into(),
                    speaker: "Speaker 1".into(),
                    text: "In den letzten fünf Jahren dann als Seniorfachreferent".into(),
                    words: Some(vec![
                        TranscriptWord { start: "02:10.000".into(), end: "02:11.000".into(), text: "In".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:11.000".into(), end: "02:12.000".into(), text: "den".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:12.000".into(), end: "02:13.000".into(), text: "letzten".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:13.000".into(), end: "02:14.000".into(), text: "fünf".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:14.000".into(), end: "02:15.000".into(), text: "Jahren".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:15.000".into(), end: "02:16.500".into(), text: "dann".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:16.500".into(), end: "02:18.000".into(), text: "als".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:18.000".into(), end: "02:19.040".into(), text: "Seniorfachreferent".into(), speaker: Some("Speaker 1".into()) },
                    ]),
                    alternatives: None,
                    merge_status: None,
                    active_source: None,
                    similarity_score: None,
                },
                TranscriptSegment {
                    start: "02:19.040".into(),
                    end: "02:26.960".into(),
                    speaker: "Speaker 1".into(),
                    text: "unterwegs für IAV Einmal intern leite ich unser produktbezogenes Security Operations Center".into(),
                    words: Some(vec![
                        TranscriptWord { start: "02:19.040".into(), end: "02:20.000".into(), text: "unterwegs".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:20.000".into(), end: "02:21.000".into(), text: "für".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:21.000".into(), end: "02:22.000".into(), text: "IAV.".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:22.000".into(), end: "02:23.000".into(), text: "Einmal".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:23.000".into(), end: "02:24.000".into(), text: "intern".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:24.000".into(), end: "02:25.000".into(), text: "leite".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:25.000".into(), end: "02:25.800".into(), text: "ich".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:25.800".into(), end: "02:26.300".into(), text: "unser".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:26.300".into(), end: "02:26.700".into(), text: "produktbezogenes".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:26.700".into(), end: "02:26.960".into(), text: "Security".into(), speaker: Some("Speaker 1".into()) },
                    ]),
                    alternatives: None,
                    merge_status: None,
                    active_source: None,
                    similarity_score: None,
                },
                TranscriptSegment {
                    start: "02:26.960".into(),
                    end: "02:32.320".into(),
                    speaker: "Speaker 1".into(),
                    text: "Operations Center kümmern uns sozusagen".into(),
                    words: Some(vec![
                        TranscriptWord { start: "02:26.960".into(), end: "02:27.800".into(), text: "Operations".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:27.800".into(), end: "02:28.500".into(), text: "Center.".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:28.500".into(), end: "02:29.500".into(), text: "Kümmern".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:29.500".into(), end: "02:30.200".into(), text: "uns".into(), speaker: Some("Speaker 1".into()) },
                        TranscriptWord { start: "02:30.200".into(), end: "02:31.300".into(), text: "sozusagen".into(), speaker: Some("Speaker 1".into()) },
                    ]),
                    alternatives: None,
                    merge_status: None,
                    active_source: None,
                    similarity_score: None,
                },
            ],
            vec![
                segment("02:10.000", "02:22.000", "Hauke Petersen", "In den letzten fünf Jahren dann als Seniorfachreferent unterwegs für IAV."),
                segment("02:22.000", "02:28.500", "Hauke Petersen", "Einmal intern leite ich unser produktbezogenes Security Operations Center."),
                segment("02:28.500", "02:32.320", "Hauke Petersen", "Kümmern uns sozusagen."),
            ],
        );

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].end, "02:22.000");
        assert_eq!(merged[1].start, "02:22.000");
        assert_eq!(
            merged[1]
                .alternatives
                .as_ref()
                .and_then(|alternatives| {
                    alternatives.iter().find(|alternative| {
                        alternative.source == TranscriptAlternativeSource::Parakeet
                    })
                })
                .map(|alternative| alternative.text.clone()),
            Some(
                "Einmal intern leite ich unser produktbezogenes Security Operations Center."
                    .to_string()
            )
        );
        assert_eq!(merged[2].start, "02:28.500");
    }
}
