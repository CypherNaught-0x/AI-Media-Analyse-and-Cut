use log::info;
use std::cmp::Ordering;
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

    // Normalize the text of every primary/reference group exactly once up front.
    // The inner DP loop below probes up to MAX_PRIMARY_GROUP * MAX_REFERENCE_GROUP
    // candidates per band cell; recomputing normalization, char vectors and token
    // sets on every probe (the previous behaviour) dominated the runtime. With the
    // groups precomputed, each comparison is a pure, allocation-free similarity.
    let primary_groups = build_group_table(primary, MAX_PRIMARY_GROUP);
    let reference_groups = build_group_table(reference, MAX_REFERENCE_GROUP);
    let mut scratch = LevenshteinScratch::default();

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

                let prepared_primary = &primary_groups[primary_index][primary_group_len - 1];

                for reference_group_len in 1..=MAX_REFERENCE_GROUP {
                    if reference_index + reference_group_len > reference_len {
                        break;
                    }

                    let prepared_reference =
                        &reference_groups[reference_index][reference_group_len - 1];
                    let similarity = combined_similarity_prepared(
                        prepared_primary,
                        prepared_reference,
                        MIN_MATCH_SIMILARITY,
                        &mut scratch,
                    );
                    if similarity < MIN_MATCH_SIMILARITY {
                        continue;
                    }

                    let reference_group =
                        &reference[reference_index..reference_index + reference_group_len];

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

    // Normalize each segment once instead of O(segments * band) times inside the
    // search loops below.
    let primary_prepared = prepare_segments(primary);
    let reference_prepared = prepare_segments(reference);
    let mut scratch = LevenshteinScratch::default();

    for (primary_index, prepared_primary) in primary_prepared.iter().enumerate() {
        if prepared_primary.byte_len < MIN_ANCHOR_TEXT_CHARS {
            continue;
        }

        let center = (primary_index * reference.len()) / primary.len();
        let reference_start = center.saturating_sub(band);
        let reference_end = (center + band).min(reference.len().saturating_sub(1));

        let mut best_match = None::<(usize, f32)>;
        let mut second_best = 0.0f32;

        for (reference_index, prepared_reference) in reference_prepared
            .iter()
            .enumerate()
            .take(reference_end + 1)
            .skip(reference_start)
        {
            if prepared_reference.byte_len < MIN_ANCHOR_TEXT_CHARS {
                continue;
            }

            let similarity = combined_similarity_prepared(
                &prepared_primary.group,
                &prepared_reference.group,
                ANCHOR_MATCH_SIMILARITY,
                &mut scratch,
            );
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

    for (reference_index, prepared_reference) in reference_prepared.iter().enumerate() {
        if prepared_reference.byte_len < MIN_ANCHOR_TEXT_CHARS {
            continue;
        }

        let center = (reference_index * primary.len()) / reference.len();
        let primary_start = center.saturating_sub(band);
        let primary_end = (center + band).min(primary.len().saturating_sub(1));

        let mut best_match = None::<(usize, f32)>;
        let mut second_best = 0.0f32;

        for (primary_index, prepared_primary) in primary_prepared
            .iter()
            .enumerate()
            .take(primary_end + 1)
            .skip(primary_start)
        {
            if prepared_primary.byte_len < MIN_ANCHOR_TEXT_CHARS {
                continue;
            }

            let similarity = combined_similarity_prepared(
                &prepared_primary.group,
                &prepared_reference.group,
                ANCHOR_MATCH_SIMILARITY,
                &mut scratch,
            );
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
    let mut scratch = LevenshteinScratch::default();

    while index + 1 < rebalanced.len() {
        // Borrow the pair directly; only the (rare) accepted reshuffle allocates.
        if let Some((updated_left, updated_right)) = optimize_boundary_between_segments(
            &rebalanced[index],
            &rebalanced[index + 1],
            &mut scratch,
        ) {
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
    scratch: &mut LevenshteinScratch,
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

    // The Google reference text on each side is constant across every candidate
    // split, so normalize it once instead of inside the scoring loop.
    let left_google = NormalizedText::from_text(left_google_text);
    let right_google = NormalizedText::from_text(right_google_text);

    // Reference the words rather than cloning them; only the chosen split is
    // materialized into owned word vectors at the end.
    let combined_words = left_words
        .iter()
        .chain(right_words.iter())
        .collect::<Vec<&TranscriptWord>>();
    let total = combined_words.len();
    let original_split = left_words.len();
    let search_radius = 8usize
        .min(original_split.saturating_sub(1).max(1))
        .min(right_words.len());
    let split_start = original_split.saturating_sub(search_radius).max(1);
    let split_end = (original_split + search_radius).min(total - 1);

    // Every candidate split's left side is a prefix of the combined word list and
    // its right side is a suffix. Normalize each word once and accumulate the
    // prefix/suffix normalized forms, so scoring a split is just two similarity
    // computations with no re-normalization or allocation.
    let word_norms: Vec<NormalizedText> = combined_words
        .iter()
        .map(|word| NormalizedText::from_text(word.text.trim()))
        .collect();

    let mut prefix_norms: Vec<NormalizedText> = Vec::with_capacity(total + 1);
    prefix_norms.push(NormalizedText::default());
    for word_norm in &word_norms {
        let mut next = prefix_norms.last().unwrap().clone();
        append_normalized(&mut next, word_norm);
        prefix_norms.push(next);
    }

    let mut suffix_norms: Vec<NormalizedText> = vec![NormalizedText::default(); total + 1];
    for index in (0..total).rev() {
        let mut node = word_norms[index].clone();
        append_normalized(&mut node, &suffix_norms[index + 1]);
        suffix_norms[index] = node;
    }

    let distance_penalty = |split: usize| split.abs_diff(original_split) as f32 * 0.01;

    // Exact score of a split: similarity of its left prefix to the left Google text
    // plus the same on the right, minus the distance penalty.
    let exact_score = |split: usize, scratch: &mut LevenshteinScratch| -> f32 {
        let left = combined_similarity_prepared(&prefix_norms[split], &left_google, 0.0, scratch);
        let right = combined_similarity_prepared(&suffix_norms[split], &right_google, 0.0, scratch);
        left + right - distance_penalty(split)
    };

    let mut best_split = original_split;
    let mut best_score = exact_score(original_split, scratch);

    // Each side similarity is in [0, 1], so any candidate scores at most 2.0, and
    // every non-original split carries a distance penalty of at least 0.01. A split
    // is only adopted when it beats the running best by more than 0.02. Hence if the
    // original boundary already scores >= 1.97 (= 2.0 - 0.01 - 0.02), no shift can
    // ever win, and the whole search can be skipped. Equivalent to running the loop.
    const SKIP_SEARCH_SCORE: f32 = 1.97;
    if best_score < SKIP_SEARCH_SCORE {
        for split_index in split_start..=split_end {
            let penalty = distance_penalty(split_index);
            // To beat the running best by 0.02, with each side at most 1.0, both
            // sides must exceed this floor. Pass it as the similarity threshold so
            // the cheap token/length prunes skip the edit-distance work for
            // candidates that cannot win; competitive candidates are still scored
            // exactly, so the selection is identical to the unpruned loop.
            let side_floor = (best_score + 0.02 + penalty - 1.0).max(0.0);
            let left = combined_similarity_prepared(
                &prefix_norms[split_index],
                &left_google,
                side_floor,
                scratch,
            );
            if side_floor > 0.0 && left <= 0.0 {
                continue;
            }
            let right = combined_similarity_prepared(
                &suffix_norms[split_index],
                &right_google,
                side_floor,
                scratch,
            );
            if side_floor > 0.0 && right <= 0.0 {
                continue;
            }
            let score = left + right - penalty;
            if score > best_score + 0.02 {
                best_score = score;
                best_split = split_index;
            }
        }
    }

    if best_split == original_split {
        return None;
    }

    let updated_left = rebuild_segment_with_words(
        left,
        clone_word_refs(&combined_words[..best_split]),
        left_google_text,
    );
    let updated_right = rebuild_segment_with_words(
        right,
        clone_word_refs(&combined_words[best_split..]),
        right_google_text,
    );

    Some((updated_left, updated_right))
}

fn clone_word_refs(words: &[&TranscriptWord]) -> Vec<TranscriptWord> {
    words.iter().map(|word| (*word).clone()).collect()
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

/// Levenshtein contribution weight in the combined similarity metric.
const LEVENSHTEIN_WEIGHT: f32 = 0.75;
/// Token-overlap contribution weight in the combined similarity metric.
const TOKEN_WEIGHT: f32 = 0.25;

/// Pre-normalized text of a segment (or a group of segments): the lowercased,
/// whitespace-collapsed character vector used for Levenshtein distance, plus the
/// sorted, de-duplicated token list used for the Jaccard overlap. Computing this
/// once and reusing it removes the per-comparison allocation and normalization
/// that previously dominated alignment time.
#[derive(Default, Clone)]
struct NormalizedText {
    chars: Vec<char>,
    tokens: Vec<Box<str>>,
}

impl NormalizedText {
    fn from_text(text: &str) -> Self {
        let normalized = normalize_text(text);
        Self::from_normalized(&normalized)
    }

    fn from_normalized(normalized: &str) -> Self {
        let chars = normalized.chars().collect();
        let mut tokens: Vec<Box<str>> = normalized.split_whitespace().map(Box::from).collect();
        tokens.sort_unstable();
        tokens.dedup();
        NormalizedText { chars, tokens }
    }

    fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

/// A segment prepared for anchor detection: its normalized form plus the byte
/// length of the normalized text (used to gate short, unreliable anchors exactly
/// as the original `normalize_text(..).len()` check did).
struct PreparedSegment {
    byte_len: usize,
    group: NormalizedText,
}

fn prepare_segments(segments: &[TranscriptSegment]) -> Vec<PreparedSegment> {
    segments
        .iter()
        .map(|segment| {
            let normalized = normalize_text(segment.text.trim());
            PreparedSegment {
                byte_len: normalized.len(),
                group: NormalizedText::from_normalized(&normalized),
            }
        })
        .collect()
}

/// Builds, for every start index `i`, the normalized text of each grouping
/// `segments[i..i + len]` for `len` in `1..=max_group`. `table[i][len - 1]` is the
/// group starting at `i` spanning `len` segments. Groups are accumulated
/// incrementally so the total work is `O(max_group * total_chars)`.
fn build_group_table(segments: &[TranscriptSegment], max_group: usize) -> Vec<Vec<NormalizedText>> {
    let base: Vec<NormalizedText> = segments
        .iter()
        .map(|segment| NormalizedText::from_text(segment.text.trim()))
        .collect();

    (0..segments.len())
        .map(|start| {
            let mut groups = Vec::with_capacity(max_group.min(segments.len() - start));
            let mut chars: Vec<char> = Vec::new();
            let mut tokens: Vec<Box<str>> = Vec::new();

            for offset in 0..max_group {
                let Some(member) = base.get(start + offset) else {
                    break;
                };

                // Empty members contribute no separator, matching the behaviour of
                // `normalize_text(join_segment_text(group))` which collapses the
                // whitespace around dropped (empty) segments.
                if !member.chars.is_empty() {
                    if !chars.is_empty() {
                        chars.push(' ');
                    }
                    chars.extend_from_slice(&member.chars);
                    tokens = merge_sorted_unique(&tokens, &member.tokens);
                }

                groups.push(NormalizedText {
                    chars: chars.clone(),
                    tokens: tokens.clone(),
                });
            }

            groups
        })
        .collect()
}

/// Appends `addition` onto `target` as if their source texts were joined by a
/// space, mirroring `normalize_text(join_*)`: an empty addition contributes
/// nothing (no separator), matching whitespace collapse around empty fragments.
fn append_normalized(target: &mut NormalizedText, addition: &NormalizedText) {
    if addition.chars.is_empty() {
        return;
    }
    if !target.chars.is_empty() {
        target.chars.push(' ');
    }
    target.chars.extend_from_slice(&addition.chars);
    target.tokens = merge_sorted_unique(&target.tokens, &addition.tokens);
}

fn merge_sorted_unique(left: &[Box<str>], right: &[Box<str>]) -> Vec<Box<str>> {
    let mut merged = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);

    while i < left.len() && j < right.len() {
        match left[i].as_ref().cmp(right[j].as_ref()) {
            Ordering::Less => {
                merged.push(left[i].clone());
                i += 1;
            }
            Ordering::Greater => {
                merged.push(right[j].clone());
                j += 1;
            }
            Ordering::Equal => {
                merged.push(left[i].clone());
                i += 1;
                j += 1;
            }
        }
    }

    merged.extend_from_slice(&left[i..]);
    merged.extend_from_slice(&right[j..]);
    merged
}

/// Reusable scratch buffers for the banded Levenshtein computation so the inner
/// alignment loop performs no per-comparison allocation.
#[derive(Default)]
struct LevenshteinScratch {
    previous: Vec<usize>,
    current: Vec<usize>,
}

fn combined_similarity_text(left: &str, right: &str) -> f32 {
    let left = NormalizedText::from_text(left);
    let right = NormalizedText::from_text(right);
    let mut scratch = LevenshteinScratch::default();
    combined_similarity_prepared(&left, &right, 0.0, &mut scratch)
}

/// Combined Levenshtein + token-overlap similarity over pre-normalized text.
///
/// `min_threshold` lets callers that only care whether the score clears a cutoff
/// (anchor detection, the gap DP) skip the bulk of the Levenshtein matrix: the
/// token overlap contributes at most `TOKEN_WEIGHT`, so any pair that can still
/// reach the threshold must have an edit distance within a bounded band. Pairs
/// outside that band cannot clear the threshold and are reported as `0.0`. With
/// `min_threshold == 0.0` the full, exact similarity is computed.
fn combined_similarity_prepared(
    left: &NormalizedText,
    right: &NormalizedText,
    min_threshold: f32,
    scratch: &mut LevenshteinScratch,
) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    // Token overlap is far cheaper than the edit distance and bounds the result:
    // the combined score is at most `LEVENSHTEIN_WEIGHT * 1.0 + TOKEN_WEIGHT * token`.
    // For the high thresholds used by anchor detection this rejects the vast
    // majority of candidate pairs before the expensive Levenshtein runs at all.
    let token_similarity = token_overlap_sorted(&left.tokens, &right.tokens);
    if LEVENSHTEIN_WEIGHT + TOKEN_WEIGHT * token_similarity < min_threshold {
        return 0.0;
    }

    let left_len = left.chars.len();
    let right_len = right.chars.len();
    let max_len = left_len.max(right_len);

    // Minimum Levenshtein similarity that could still reach `min_threshold`,
    // given the token overlap we just computed.
    let needed_levenshtein = (min_threshold - TOKEN_WEIGHT * token_similarity) / LEVENSHTEIN_WEIGHT;
    let max_distance = if needed_levenshtein <= 0.0 {
        max_len
    } else {
        let cap = (max_len as f32 * (1.0 - needed_levenshtein)).ceil() as usize;
        cap.max(left_len.abs_diff(right_len))
    };

    let distance = bounded_levenshtein(&left.chars, &right.chars, max_distance, scratch);
    if distance > max_distance {
        return 0.0;
    }

    let levenshtein_similarity = 1.0 - (distance as f32 / max_len as f32);
    (levenshtein_similarity * LEVENSHTEIN_WEIGHT) + (token_similarity * TOKEN_WEIGHT)
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

/// Banded Levenshtein distance with an early-out at `max_distance`.
///
/// Returns the exact edit distance whenever it is `<= max_distance`; otherwise
/// returns a value strictly greater than `max_distance` (capped at
/// `max_distance + 1`). Restricting each row to a `±max_distance` band around the
/// diagonal turns the worst case from `O(n*m)` into `O(n * max_distance)`, which
/// is the key win for the high thresholds used by anchor detection. The optimal
/// path for any distance `<= max_distance` is guaranteed to stay inside the band,
/// so banding never affects results that callers actually use.
fn bounded_levenshtein(
    left: &[char],
    right: &[char],
    max_distance: usize,
    scratch: &mut LevenshteinScratch,
) -> usize {
    let n = left.len();
    let m = right.len();

    if n == 0 {
        return m.min(max_distance + 1);
    }
    if m == 0 {
        return n.min(max_distance + 1);
    }
    if n.abs_diff(m) > max_distance {
        return max_distance + 1;
    }

    let unreachable = max_distance + 1;

    // Ensure the scratch rows are large enough, growing only when a longer input
    // is seen. Crucially we do NOT clear them per call: every cell that is read is
    // either written within the band this call or explicitly set to `unreachable`
    // at a band boundary, so stale values from previous calls are never observed.
    // This keeps the per-call cost O(n * band) instead of O(n * m).
    if scratch.previous.len() < m + 1 {
        scratch.previous.resize(m + 1, unreachable);
    }
    if scratch.current.len() < m + 1 {
        scratch.current.resize(m + 1, unreachable);
    }
    let previous = &mut scratch.previous;
    let current = &mut scratch.current;

    // Row 0 only needs to be valid across the first row's read window.
    let init_high = max_distance.min(m);
    for (j, slot) in previous.iter_mut().enumerate().take(init_high + 1) {
        *slot = j;
    }
    if init_high + 1 <= m {
        previous[init_high + 1] = unreachable;
    }

    for i in 1..=n {
        let low = i.saturating_sub(max_distance).max(1);
        let high = (i + max_distance).min(m);
        let left_char = left[i - 1];

        // Left boundary: column `low - 1` is read as the insertion source for the
        // first band cell. It is only a real value when it is column 0 (deleting
        // all `i` leading characters); otherwise it lies outside the band.
        if low == 1 {
            current[0] = i;
        } else {
            current[low - 1] = unreachable;
        }

        let mut row_min = unreachable;
        for j in low..=high {
            let substitution_cost = if left_char == right[j - 1] { 0 } else { 1 };
            let deletion = previous[j].saturating_add(1);
            let insertion = current[j - 1].saturating_add(1);
            let substitution = previous[j - 1].saturating_add(substitution_cost);
            let value = deletion.min(insertion).min(substitution);
            current[j] = value;
            row_min = row_min.min(value);
        }

        if row_min > max_distance {
            return unreachable;
        }

        // The cell just past the band must read as `unreachable` when it becomes
        // `previous[high]` for the next row (the band advances by one).
        if high + 1 <= m {
            current[high + 1] = unreachable;
        }

        std::mem::swap(previous, current);
    }

    previous[m].min(unreachable)
}

/// Jaccard token overlap over two sorted, de-duplicated token lists. Linear
/// merge, no allocation (the previous `BTreeSet` version allocated two trees per
/// comparison).
fn token_overlap_sorted(left: &[Box<str>], right: &[Box<str>]) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    let (mut i, mut j) = (0, 0);
    let mut intersection = 0usize;

    while i < left.len() && j < right.len() {
        match left[i].as_ref().cmp(right[j].as_ref()) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                intersection += 1;
                i += 1;
                j += 1;
            }
        }
    }

    let union = left.len() + right.len() - intersection;
    intersection as f32 / union as f32
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

    /// Reference full-matrix Levenshtein used to verify the banded version.
    fn naive_levenshtein(left: &[char], right: &[char]) -> usize {
        let mut previous: Vec<usize> = (0..=right.len()).collect();
        let mut current = vec![0usize; right.len() + 1];
        for (i, lc) in left.iter().enumerate() {
            current[0] = i + 1;
            for (j, rc) in right.iter().enumerate() {
                let cost = if lc == rc { 0 } else { 1 };
                current[j + 1] = (current[j] + 1)
                    .min(previous[j + 1] + 1)
                    .min(previous[j] + cost);
            }
            std::mem::swap(&mut previous, &mut current);
        }
        previous[right.len()]
    }

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn bounded_levenshtein_matches_naive_within_band() {
        let mut scratch = LevenshteinScratch::default();
        let cases = [
            ("", ""),
            ("a", ""),
            ("kitten", "sitting"),
            ("security operations center", "security operations center"),
            ("security operations center", "security operation center"),
            ("the quick brown fox", "a completely different sentence here"),
        ];
        for (left, right) in cases {
            let l = chars(left);
            let r = chars(right);
            let expected = naive_levenshtein(&l, &r);
            // A band at least as wide as the true distance must reproduce it exactly.
            let result = bounded_levenshtein(&l, &r, expected, &mut scratch);
            assert_eq!(result, expected, "exact band for {left:?} vs {right:?}");
            // A generous band must also reproduce it exactly.
            let generous = bounded_levenshtein(&l, &r, expected + 5, &mut scratch);
            assert_eq!(generous, expected, "generous band for {left:?} vs {right:?}");
        }
    }

    #[test]
    fn bounded_levenshtein_reports_overflow_beyond_band() {
        let mut scratch = LevenshteinScratch::default();
        let l = chars("kitten");
        let r = chars("sitting");
        let true_distance = naive_levenshtein(&l, &r);
        // With a band tighter than the true distance, the result must exceed the band
        // (never an under-estimate that a caller would mistake for a match).
        let capped = bounded_levenshtein(&l, &r, true_distance - 1, &mut scratch);
        assert!(capped > true_distance - 1);
    }

    fn naive_combined(left: &str, right: &str) -> f32 {
        let left = normalize_text(left);
        let right = normalize_text(right);
        if left.is_empty() || right.is_empty() {
            return 0.0;
        }
        let lc = chars(&left);
        let rc = chars(&right);
        let distance = naive_levenshtein(&lc, &rc);
        let max_len = lc.len().max(rc.len()) as f32;
        let levenshtein = 1.0 - distance as f32 / max_len;

        let left_tokens: std::collections::BTreeSet<&str> = left.split_whitespace().collect();
        let right_tokens: std::collections::BTreeSet<&str> = right.split_whitespace().collect();
        let intersection = left_tokens.intersection(&right_tokens).count() as f32;
        let union = left_tokens.union(&right_tokens).count() as f32;
        let token = if union == 0.0 { 0.0 } else { intersection / union };

        levenshtein * LEVENSHTEIN_WEIGHT + token * TOKEN_WEIGHT
    }

    #[test]
    fn combined_similarity_is_exact_with_no_threshold() {
        let cases = [
            ("Hello itemis team", "hello itemis team."),
            ("security operations center", "Security Operation Center."),
            ("totally unrelated phrase", "nothing in common whatsoever"),
            ("we reviewed the release plan", "We reviewed the release plan."),
        ];
        for (left, right) in cases {
            let expected = naive_combined(left, right);
            let actual = combined_similarity_text(left, right);
            assert!(
                (actual - expected).abs() < 1e-6,
                "{left:?} vs {right:?}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn combined_similarity_threshold_preserves_matches_and_rejects_below() {
        let mut scratch = LevenshteinScratch::default();
        let cases = [
            ("security operations center", "security operations center."),
            ("security operations center", "Security Operation Center."),
            ("alpha beta gamma delta", "alpha beta gamma delta"),
            ("alpha beta gamma delta", "completely different words entirely"),
        ];
        for threshold in [MIN_MATCH_SIMILARITY, ANCHOR_MATCH_SIMILARITY] {
            for (left, right) in cases {
                let exact = naive_combined(left, right);
                let left_norm = NormalizedText::from_text(left);
                let right_norm = NormalizedText::from_text(right);
                let pruned = combined_similarity_prepared(
                    &left_norm,
                    &right_norm,
                    threshold,
                    &mut scratch,
                );
                if exact >= threshold {
                    // Matches must be returned with the exact score.
                    assert!(
                        (pruned - exact).abs() < 1e-6,
                        "{left:?} vs {right:?} @ {threshold}: kept score {pruned} != exact {exact}"
                    );
                } else {
                    // Non-matches must stay below the threshold (pruned to 0.0 or exact).
                    assert!(
                        pruned < threshold,
                        "{left:?} vs {right:?} @ {threshold}: {pruned} should be below threshold"
                    );
                }
            }
        }
    }

    fn synth_pair(n: usize) -> (Vec<TranscriptSegment>, Vec<TranscriptSegment>) {
        const VOCAB: &[&str] = &[
            "team", "release", "roadmap", "security", "operations", "center", "review", "update",
            "customer", "feedback", "sprint", "deadline", "architecture", "service", "deployment",
            "pipeline", "incident", "mitigation", "stakeholder", "alignment", "transcript",
            "analysis", "model", "inference", "latency", "throughput", "benchmark", "optimization",
            "regression", "coverage",
        ];
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut rng = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut primary = Vec::with_capacity(n);
        let mut reference = Vec::with_capacity(n);
        let mut clock = 0u64;
        for _ in 0..n {
            let wc = 5 + (rng() as usize % 8);
            let mut words = Vec::with_capacity(wc);
            let mut toks = Vec::with_capacity(wc);
            let start = clock;
            for _ in 0..wc {
                let t = VOCAB[rng() as usize % VOCAB.len()];
                let w0 = clock;
                clock += 200 + rng() % 300;
                words.push(TranscriptWord {
                    start: format!("{:02}:{:02}.000", w0 / 60000, (w0 / 1000) % 60),
                    end: format!("{:02}:{:02}.000", clock / 60000, (clock / 1000) % 60),
                    text: t.to_string(),
                    speaker: Some("Speaker 1".into()),
                });
                toks.push(t);
            }
            clock += 80;
            let text = toks.join(" ");
            primary.push(TranscriptSegment {
                start: format!("{:02}:{:02}.000", start / 60000, (start / 1000) % 60),
                end: format!("{:02}:{:02}.000", clock / 60000, (clock / 1000) % 60),
                speaker: "Speaker 1".into(),
                text: text.clone(),
                words: Some(words),
                ..Default::default()
            });
            let mut rt = toks.clone();
            if rng() % 100 < 12 {
                let p = rng() as usize % rt.len();
                rt[p] = VOCAB[rng() as usize % VOCAB.len()];
            }
            let mut rtext = rt.join(" ");
            rtext.push('.');
            reference.push(TranscriptSegment {
                start: primary.last().unwrap().start.clone(),
                end: primary.last().unwrap().end.clone(),
                speaker: "Ref 1".into(),
                text: rtext,
                ..Default::default()
            });
        }
        (primary, reference)
    }

    #[test]
    #[ignore = "profiling harness; run with --ignored --nocapture"]
    fn profile_phases() {
        for n in [200usize, 1000, 4000] {
            let (primary, reference) = synth_pair(n);

            let t = Instant::now();
            let anchors = detect_alignment_anchors(&primary, &reference);
            let t_anchors = t.elapsed();

            let t = Instant::now();
            let gaps = build_alignment_gaps(primary.len(), reference.len(), &anchors);
            let gap_alignments = align_gaps(&primary, &reference, &gaps);
            let t_gaps = t.elapsed();

            // Reassemble the alignment exactly as compute_alignment does.
            let mut alignment = Vec::new();
            for i in 0..gaps.len() {
                alignment.extend(gap_alignments[i].iter().copied());
                if let Some(anchor) = anchors.get(i) {
                    alignment.push(AlignmentStep::Match {
                        primary_len: 1,
                        reference_len: 1,
                        similarity: anchor.similarity,
                    });
                }
            }

            let t = Instant::now();
            let merged = materialize_alignment(&primary, &reference, &alignment);
            let t_materialize = t.elapsed();

            // Break materialize into its sub-phases.
            let t = Instant::now();
            let mut assembled = Vec::new();
            {
                let mut pi = 0usize;
                let mut ri = 0usize;
                for step in &alignment {
                    match *step {
                        AlignmentStep::Match {
                            primary_len,
                            reference_len,
                            similarity,
                        } => {
                            assembled.extend(build_matched_segments(
                                &primary[pi..pi + primary_len],
                                &reference[ri..ri + reference_len],
                                similarity,
                            ));
                            pi += primary_len;
                            ri += reference_len;
                        }
                        AlignmentStep::MissingGoogle => {
                            assembled.push(build_missing_google_segment(&primary[pi]));
                            pi += 1;
                        }
                        AlignmentStep::MissingParakeet => {
                            assembled
                                .push(build_missing_parakeet_segment(&primary, pi, &reference[ri]));
                            ri += 1;
                        }
                    }
                }
            }
            let t_build = t.elapsed();
            let t = Instant::now();
            let rebalanced = rebalance_adjacent_boundaries(assembled);
            let t_rebalance = t.elapsed();
            let t = Instant::now();
            let _final = apply_inferred_speaker_labels(&primary, rebalanced);
            let t_speakers = t.elapsed();

            eprintln!(
                "n={n:5}  detect={t_anchors:>9.2?}  gaps={t_gaps:>9.2?}  materialize={t_materialize:>9.2?}  out={}  anchors={}",
                merged.len(),
                anchors.len(),
            );
            eprintln!(
                "         materialize breakdown: build={t_build:>9.2?}  rebalance={t_rebalance:>9.2?}  speakers={t_speakers:>9.2?}"
            );

            // Micro-cost of a single prepared similarity call (the inner primitive).
            let a = NormalizedText::from_text(&primary[0].text);
            let b = NormalizedText::from_text(&reference[0].text);
            let mut scratch = LevenshteinScratch::default();
            let iters = 1_000_000u32;
            let t = Instant::now();
            let mut acc = 0.0f32;
            for _ in 0..iters {
                acc += combined_similarity_prepared(
                    &a,
                    &b,
                    ANCHOR_MATCH_SIMILARITY,
                    &mut scratch,
                );
            }
            let per = t.elapsed() / iters;
            eprintln!("         per combined_similarity_prepared call (anchor thr): {per:?}  (acc={acc})");
        }
    }

    #[test]
    fn group_table_matches_joined_normalization() {
        let segments = vec![
            segment("00:00.000", "00:01.000", "S", "Hello there,"),
            segment("00:01.000", "00:02.000", "S", "general Kenobi!"),
            segment("00:02.000", "00:03.000", "S", "welcome back"),
        ];
        let table = build_group_table(&segments, MAX_PRIMARY_GROUP);
        for start in 0..segments.len() {
            for len in 1..=(segments.len() - start) {
                let joined = join_segment_text(&segments[start..start + len]);
                let expected = NormalizedText::from_text(&joined);
                let actual = &table[start][len - 1];
                assert_eq!(
                    actual.chars, expected.chars,
                    "chars mismatch at start={start} len={len}"
                );
                assert_eq!(
                    actual.tokens, expected.tokens,
                    "tokens mismatch at start={start} len={len}"
                );
            }
        }
    }
}
