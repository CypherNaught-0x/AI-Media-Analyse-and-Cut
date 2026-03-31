use log::info;
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
const ALIGNMENT_BAND: usize = 32;
const MAX_WORD_RESEGMENT_WORDS: usize = 64;

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
    let primary_len = primary.len();
    let reference_len = reference.len();
    let mut costs = vec![vec![f32::INFINITY; reference_len + 1]; primary_len + 1];
    let mut previous = vec![vec![None; reference_len + 1]; primary_len + 1];
    let dynamic_band = ALIGNMENT_BAND.max(primary_len.abs_diff(reference_len) + 6);

    costs[0][0] = 0.0;

    for primary_index in 0..=primary_len {
        if primary_len > 0 && primary_index < primary_len && primary_index % 8 == 0 {
            let progress = 5.0 + (primary_index as f32 / primary_len as f32) * 72.0;
            on_progress(progress, "Aligning Parakeet and remote transcripts...");
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

    merged
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
                if let Some(split_ranges) = split_word_ranges_by_reference(&words, reference_group) {
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
    let google_speaker = (!reference_segment.speaker.trim().is_empty())
        .then_some(reference_segment.speaker.clone());
    let resolved_speaker = google_speaker
        .clone()
        .unwrap_or_else(|| first.speaker.clone().unwrap_or_else(|| "Speaker Unknown".to_string()));

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
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
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
}
