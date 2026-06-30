//! Benchmarks for the hybrid transcript alignment / merge pipeline.
//!
//! These exercise `merge_transcript_hypotheses` end-to-end (anchor detection,
//! gap dynamic programming, word-level resegmentation and boundary rebalancing)
//! on synthetic transcripts of increasing size. The generator is deterministic
//! so results are comparable across runs and machines.
//!
//! Run with:
//!   cargo bench --bench transcript_merge
//! Filter to one size with e.g.:
//!   cargo bench --bench transcript_merge -- large

use std::hint::black_box;

use ai_media_cutter_lib::transcript_merge::merge_transcript_hypotheses;
use ai_media_cutter_lib::video::{TranscriptSegment, TranscriptWord};
use criterion::{
    criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};

/// Tiny deterministic xorshift PRNG so the benchmark needs no external rng and
/// produces identical input on every run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform integer in `[lo, hi)`.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        debug_assert!(hi > lo);
        lo + (self.next_u64() as usize) % (hi - lo)
    }

    /// Returns true with probability `p` (0.0..=1.0).
    fn chance(&mut self, p: f64) -> bool {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64) < p
    }
}

const VOCAB: &[&str] = &[
    "team",
    "release",
    "roadmap",
    "security",
    "operations",
    "center",
    "review",
    "update",
    "customer",
    "feedback",
    "sprint",
    "deadline",
    "architecture",
    "service",
    "deployment",
    "pipeline",
    "incident",
    "mitigation",
    "stakeholder",
    "alignment",
    "transcript",
    "analysis",
    "model",
    "inference",
    "latency",
    "throughput",
    "benchmark",
    "optimization",
    "regression",
    "coverage",
    "schedule",
    "budget",
    "scope",
    "delivery",
    "milestone",
    "retrospective",
    "backlog",
    "estimate",
    "dependency",
    "integration",
];

fn timestamp(ms: u64) -> String {
    let minutes = ms / 60_000;
    let seconds = (ms % 60_000) / 1000;
    let millis = ms % 1000;
    format!("{:02}:{:02}.{:03}", minutes, seconds, millis)
}

/// Builds a paired (primary, reference) transcript of `num_segments` segments.
///
/// The reference (remote API) transcript is derived from the primary (Parakeet)
/// one with realistic divergences:
///   - capitalization + trailing punctuation (so most segments are strong
///     anchors, exercising the anchored fast path),
///   - occasional single-word substitutions (conflicts inside gaps),
///   - occasional dropped segments on each side (missing-google / missing-
///     parakeet branches),
///   - occasional merge of two primary segments into one reference segment
///     (grouped matches + word-level resegmentation).
fn make_pair(num_segments: usize, seed: u64) -> (Vec<TranscriptSegment>, Vec<TranscriptSegment>) {
    let mut rng = Rng::new(seed);
    let mut primary = Vec::with_capacity(num_segments);
    let mut reference = Vec::with_capacity(num_segments);
    let mut clock_ms = 0u64;
    let mut speaker_idx = 0usize;

    let mut pending_merge: Option<String> = None;

    for index in 0..num_segments {
        let word_count = rng.range(5, 13);
        let mut words = Vec::with_capacity(word_count);
        let mut tokens = Vec::with_capacity(word_count);
        let seg_start = clock_ms;

        for _ in 0..word_count {
            let token = VOCAB[rng.range(0, VOCAB.len())];
            let dur = rng.range(180, 520) as u64;
            let w_start = clock_ms;
            clock_ms += dur;
            words.push(TranscriptWord {
                start: timestamp(w_start),
                end: timestamp(clock_ms),
                text: token.to_string(),
                speaker: Some(format!("Speaker {}", speaker_idx % 3 + 1)),
            });
            tokens.push(token);
        }

        // Small inter-segment gap.
        clock_ms += rng.range(40, 200) as u64;

        let primary_text = tokens.join(" ");
        let speaker = format!("Speaker {}", speaker_idx % 3 + 1);
        if rng.chance(0.15) {
            speaker_idx += 1;
        }

        let primary_segment = TranscriptSegment {
            start: timestamp(seg_start),
            end: timestamp(clock_ms.saturating_sub(40)),
            speaker: speaker.clone(),
            text: primary_text.clone(),
            words: Some(words),
            ..Default::default()
        };

        // Reference text: capitalize, add punctuation, maybe perturb one word.
        let mut ref_tokens = tokens.clone();
        if rng.chance(0.12) && !ref_tokens.is_empty() {
            let pos = rng.range(0, ref_tokens.len());
            ref_tokens[pos] = VOCAB[rng.range(0, VOCAB.len())];
        }
        let mut ref_text = ref_tokens.join(" ");
        if let Some(first) = ref_text.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        ref_text.push('.');

        let ref_speaker = format!("Reference Speaker {}", speaker_idx % 3 + 1);
        let reference_segment = TranscriptSegment {
            start: primary_segment.start.clone(),
            end: primary_segment.end.clone(),
            speaker: ref_speaker,
            text: ref_text.clone(),
            ..Default::default()
        };

        // Drop ~4% of reference segments => MissingGoogle branch in a gap.
        let drop_reference = rng.chance(0.04);
        // Drop ~4% of primary segments => MissingParakeet branch in a gap.
        let drop_primary = rng.chance(0.04);

        if let Some(prev_ref) = pending_merge.take() {
            // Merge this reference into the previous one: two primary segments
            // align to a single reference segment (grouped match path).
            let merged = format!("{} {}", prev_ref, ref_text);
            if !drop_reference {
                reference.push(TranscriptSegment {
                    text: merged,
                    ..reference_segment.clone()
                });
            }
        } else if !drop_reference {
            reference.push(reference_segment.clone());
        }

        if !drop_primary {
            primary.push(primary_segment);
        }

        // Occasionally defer a reference segment to merge into the next one.
        if rng.chance(0.05) && index + 1 < num_segments {
            pending_merge = Some(ref_text);
        }
    }

    (primary, reference)
}

fn bench_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("hybrid_alignment");
    // Alignment is super-linear; keep sample counts modest for the big inputs.
    group.sample_size(20);

    for &size in &[100usize, 500, 1000, 2000, 4000] {
        let (primary, reference) = make_pair(size, 0xC0FFEE ^ size as u64);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            // `merge_transcript_hypotheses` consumes its inputs, so clone them in the
            // (untimed) setup step; only the merge itself is measured.
            b.iter_batched(
                || (primary.clone(), reference.clone()),
                |(primary, reference)| {
                    let merged =
                        merge_transcript_hypotheses(black_box(primary), black_box(reference));
                    black_box(merged.len())
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_merge);
criterion_main!(benches);
