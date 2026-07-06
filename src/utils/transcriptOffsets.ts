import type { SegmentOffset, TranscriptSegment, TranscriptWord } from '../types';
import { adjustTimestamp } from '../composables/useTimeFormat';

/**
 * Remap transcript timestamps from the silence-trimmed timeline back onto the
 * original audio timeline.
 *
 * When silence trimming is enabled, transcription (Parakeet and/or the remote
 * LLM) runs against the *trimmed* audio, so every timestamp it produces is in
 * trimmed time. In the merged/hybrid pipeline both the Parakeet (primary) and
 * remote (reference) transcripts share that same trimmed timeline, the merge
 * preserves it, and the merged result is remapped exactly once — here — so it
 * lines up with the original media the user plays back and cuts.
 *
 * `offsets` is the table returned by the `remove_silence` backend command:
 * entries are sorted ascending by `min_time` (a position in the trimmed
 * timeline), and each `offset` is the amount of removed silence to add back to
 * reach the original timeline. When trimming is disabled the caller passes the
 * identity table `[{ min_time: 0, offset: 0 }]`, so these helpers are always
 * safe to call.
 */
export function adjustWordWithOffsets(
  word: TranscriptWord,
  offsets: SegmentOffset[],
): TranscriptWord {
  return {
    ...word,
    start: adjustTimestamp(word.start, offsets),
    end: adjustTimestamp(word.end, offsets),
  };
}

export function adjustSegmentsWithOffsets(
  segments: TranscriptSegment[],
  offsets: SegmentOffset[],
): TranscriptSegment[] {
  return segments.map((segment) => ({
    ...segment,
    start: adjustTimestamp(segment.start, offsets),
    end: adjustTimestamp(segment.end, offsets),
    words: segment.words?.map((word) => adjustWordWithOffsets(word, offsets)),
  }));
}
