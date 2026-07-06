import { describe, expect, it } from 'vitest';
import type { SegmentOffset, TranscriptSegment } from '../../types';
import {
  adjustSegmentsWithOffsets,
  adjustWordWithOffsets,
} from '../transcriptOffsets';

// Scenario used across the hybrid tests:
//   original audio:  [0s .. 10s speech A][10s .. 15s SILENCE][15s .. 30s speech B]
//   trimmed audio:   [0s .. 10s speech A][10s .. 25s speech B]
// The `remove_silence` backend removes the 5s gap and returns this offset table
// (sorted by trimmed-time `min_time`; `offset` is the silence to add back).
const OFFSETS: SegmentOffset[] = [
  { min_time: 0, offset: 0 },
  { min_time: 10, offset: 5 },
];

const IDENTITY_OFFSETS: SegmentOffset[] = [{ min_time: 0, offset: 0 }];

describe('adjustWordWithOffsets', () => {
  it('remaps a word before the removed silence unchanged', () => {
    expect(
      adjustWordWithOffsets(
        { start: '00:02.000', end: '00:04.000', text: 'hello', speaker: 'Speaker 1' },
        OFFSETS,
      ),
    ).toEqual({ start: '00:02.000', end: '00:04.000', text: 'hello', speaker: 'Speaker 1' });
  });

  it('remaps a word after the removed silence back onto the original timeline', () => {
    // Trimmed 12s..14s sits in speech B, so 5s of removed silence is added back.
    expect(
      adjustWordWithOffsets(
        { start: '00:12.000', end: '00:14.000', text: 'world', speaker: 'Speaker 1' },
        OFFSETS,
      ),
    ).toEqual({ start: '00:17.000', end: '00:19.000', text: 'world', speaker: 'Speaker 1' });
  });
});

describe('adjustSegmentsWithOffsets — hybrid-merge output', () => {
  // A merged segment as produced by `merge_transcript_hypotheses`: reference
  // (Google) text is active, word-level timing comes from Parakeet, and both
  // inputs were in trimmed time. Adjusting the merged output once must move the
  // segment AND its words onto the original timeline together.
  const mergedSegment: TranscriptSegment = {
    start: '00:10.000',
    end: '00:20.000',
    speaker: 'Speaker 1',
    text: 'Reconciled reference text.',
    words: [
      { start: '00:10.000', end: '00:13.000', text: 'Reconciled', speaker: 'Speaker 1' },
      { start: '00:13.000', end: '00:20.000', text: 'reference', speaker: 'Speaker 1' },
    ],
    alternatives: [
      { source: 'parakeet', text: 'reconciled reference text', speaker: 'Speaker 1', similarityScore: 0.9 },
      { source: 'google', text: 'Reconciled reference text.', speaker: 'Speaker 1', similarityScore: 0.9 },
    ],
    mergeStatus: 'matched',
    activeSource: 'google',
    similarityScore: 0.9,
  };

  it('shifts segment and word timestamps consistently by the removed silence', () => {
    const [adjusted] = adjustSegmentsWithOffsets([mergedSegment], OFFSETS);

    expect(adjusted.start).toBe('00:15.000');
    expect(adjusted.end).toBe('00:25.000');
    expect(adjusted.words).toEqual([
      { start: '00:15.000', end: '00:18.000', text: 'Reconciled', speaker: 'Speaker 1' },
      { start: '00:18.000', end: '00:25.000', text: 'reference', speaker: 'Speaker 1' },
    ]);
  });

  it('preserves all non-timestamp fields (text, speaker, alternatives, merge metadata)', () => {
    const [adjusted] = adjustSegmentsWithOffsets([mergedSegment], OFFSETS);

    expect(adjusted.text).toBe(mergedSegment.text);
    expect(adjusted.speaker).toBe(mergedSegment.speaker);
    expect(adjusted.alternatives).toEqual(mergedSegment.alternatives);
    expect(adjusted.mergeStatus).toBe('matched');
    expect(adjusted.activeSource).toBe('google');
    expect(adjusted.similarityScore).toBe(0.9);
  });

  it('handles merged segments that carry no word-level timing (missing-parakeet)', () => {
    const referenceOnly: TranscriptSegment = {
      start: '00:12.000',
      end: '00:16.000',
      speaker: 'Speaker 2',
      text: 'Reference-only line.',
      mergeStatus: 'missing_parakeet',
      activeSource: 'google',
    };

    const [adjusted] = adjustSegmentsWithOffsets([referenceOnly], OFFSETS);

    expect(adjusted.start).toBe('00:17.000');
    expect(adjusted.end).toBe('00:21.000');
    expect(adjusted.words).toBeUndefined();
  });
});

describe('adjustSegmentsWithOffsets — timeline coherence', () => {
  it('remaps a full transcript spanning the removed silence in one pass', () => {
    // Two segments, one each side of the removed silence, exactly as the merged
    // transcript reaches the final adjust step in the Parakeet/hybrid/hybrid-merge
    // paths.
    const trimmed: TranscriptSegment[] = [
      {
        start: '00:00.000',
        end: '00:08.000',
        speaker: 'Speaker 1',
        text: 'Before the gap.',
        words: [{ start: '00:00.000', end: '00:08.000', text: 'Before', speaker: 'Speaker 1' }],
      },
      {
        start: '00:12.000',
        end: '00:25.000',
        speaker: 'Speaker 1',
        text: 'After the gap.',
        words: [{ start: '00:20.000', end: '00:25.000', text: 'After', speaker: 'Speaker 1' }],
      },
    ];

    const adjusted = adjustSegmentsWithOffsets(trimmed, OFFSETS);

    // First segment is before the cut: unchanged.
    expect(adjusted[0].start).toBe('00:00.000');
    expect(adjusted[0].end).toBe('00:08.000');
    // Second segment is after the cut: shifted back onto original time (17s..30s).
    expect(adjusted[1].start).toBe('00:17.000');
    expect(adjusted[1].end).toBe('00:30.000');
    expect(adjusted[1].words?.[0]).toEqual({
      start: '00:25.000',
      end: '00:30.000',
      text: 'After',
      speaker: 'Speaker 1',
    });
  });

  it('maps a timestamp exactly at a silence seam forward into post-silence time', () => {
    // A timestamp landing exactly on the seam (trimmed t = min_time of the next
    // kept region) belongs to the post-silence region, so it gains that region's
    // offset. A segment that continues across the seam therefore spans the removed
    // gap in original time — the intended behavior, and identical across all
    // transcription modes.
    const spanning: TranscriptSegment[] = [
      {
        start: '00:08.000',
        end: '00:12.000',
        speaker: 'Speaker 1',
        text: 'Spans the cut.',
        words: [
          { start: '00:08.000', end: '00:10.000', text: 'Spans', speaker: 'Speaker 1' },
          { start: '00:10.000', end: '00:12.000', text: 'cut', speaker: 'Speaker 1' },
        ],
      },
    ];

    const [adjusted] = adjustSegmentsWithOffsets(spanning, OFFSETS);

    expect(adjusted.start).toBe('00:08.000');
    expect(adjusted.end).toBe('00:17.000');
    // The word ending at the seam (t=10) maps forward to 15s.
    expect(adjusted.words).toEqual([
      { start: '00:08.000', end: '00:15.000', text: 'Spans', speaker: 'Speaker 1' },
      { start: '00:15.000', end: '00:17.000', text: 'cut', speaker: 'Speaker 1' },
    ]);
  });

  it('is a no-op (up to timestamp formatting) when silence trimming is disabled', () => {
    const segments: TranscriptSegment[] = [
      {
        start: '00:05.000',
        end: '00:09.500',
        speaker: 'Speaker 1',
        text: 'No trimming applied.',
        words: [{ start: '00:05.000', end: '00:09.500', text: 'No', speaker: 'Speaker 1' }],
      },
    ];

    expect(adjustSegmentsWithOffsets(segments, IDENTITY_OFFSETS)).toEqual(segments);
  });

  it('does not mutate the input segments', () => {
    const segments: TranscriptSegment[] = [
      {
        start: '00:10.000',
        end: '00:20.000',
        speaker: 'Speaker 1',
        text: 'Immutable.',
        words: [{ start: '00:10.000', end: '00:20.000', text: 'Immutable', speaker: 'Speaker 1' }],
      },
    ];
    const snapshot = structuredClone(segments);

    adjustSegmentsWithOffsets(segments, OFFSETS);

    expect(segments).toEqual(snapshot);
  });
});
