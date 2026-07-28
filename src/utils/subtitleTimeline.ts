import { formatTime, parseTime } from '../composables/useTimeFormat';
import type { TranscriptSegment } from '../types';

/**
 * Re-times subtitles for the cut export ("Export Video" / the `_cut` file).
 *
 * Transcript timestamps always live on the *source* timeline: with silence
 * trimming enabled, transcription runs on the trimmed audio and the result is
 * mapped back onto the original media (see `transcriptOffsets.ts`), so a cue
 * lines up with the file the user selected.
 *
 * `cut_video` builds a different timeline: it concatenates only the transcript
 * segment ranges, dropping everything between them (a silent intro, the tail,
 * and every inter-segment gap). In the cut file the first cue therefore starts
 * at 00:00 — subtitles exported on the source timeline run late against it by
 * the whole dropped intro.
 *
 * These helpers project source timestamps onto that concatenated timeline so an
 * exported subtitle file can match the cut media instead of the source.
 */
export interface CutTimelineRange {
  /** Where the kept range starts on the source timeline, in seconds. */
  sourceStart: number;
  /** Where the kept range ends on the source timeline, in seconds. */
  sourceEnd: number;
  /** Where the kept range starts in the cut export, in seconds. */
  cutStart: number;
}

/**
 * Build the kept ranges in the order `cut_video` concatenates them (array
 * order, which for a transcript is ascending time).
 */
export function buildCutTimeline(cutSegments: TranscriptSegment[]): CutTimelineRange[] {
  const ranges: CutTimelineRange[] = [];
  let cutStart = 0;

  for (const segment of cutSegments) {
    const sourceStart = parseTime(segment.start);
    const sourceEnd = Math.max(sourceStart, parseTime(segment.end));

    ranges.push({ sourceStart, sourceEnd, cutStart });
    cutStart += sourceEnd - sourceStart;
  }

  return ranges;
}

/**
 * Map a source-timeline position onto the cut timeline. Positions inside a gap
 * the cut drops collapse onto the following range's start (which is the same
 * instant as the previous range's end), so the mapping stays monotonic.
 */
export function mapSourceTimeToCut(seconds: number, ranges: CutTimelineRange[]): number {
  if (ranges.length === 0) {
    return seconds;
  }

  let cutEnd = 0;
  for (const range of ranges) {
    if (seconds < range.sourceStart) {
      return range.cutStart;
    }
    if (seconds <= range.sourceEnd) {
      return range.cutStart + (seconds - range.sourceStart);
    }
    cutEnd = range.cutStart + (range.sourceEnd - range.sourceStart);
  }

  return cutEnd;
}

/**
 * Re-time `segments` (translated or original) for the cut export defined by
 * `cutSegments` — the exact segment list `cut_video` receives.
 */
export function remapSegmentsToCutTimeline(
  segments: TranscriptSegment[],
  cutSegments: TranscriptSegment[],
): TranscriptSegment[] {
  const ranges = buildCutTimeline(cutSegments);

  return segments.map((segment) => {
    const start = mapSourceTimeToCut(parseTime(segment.start), ranges);
    const end = Math.max(start, mapSourceTimeToCut(parseTime(segment.end), ranges));

    return {
      ...segment,
      start: formatTime(start),
      end: formatTime(end),
      words: segment.words?.map((word) => {
        const wordStart = mapSourceTimeToCut(parseTime(word.start), ranges);
        const wordEnd = Math.max(wordStart, mapSourceTimeToCut(parseTime(word.end), ranges));

        return { ...word, start: formatTime(wordStart), end: formatTime(wordEnd) };
      }),
    };
  });
}
