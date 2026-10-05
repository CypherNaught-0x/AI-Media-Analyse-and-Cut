import type { CaptionWord, SpeakerTurn } from '../bindings';
import type { ClipCandidate, ShortClip, ShortClipRange, TranscriptSegment } from '../types';
import { formatTime, parseTime } from '../composables/useTimeFormat';

let nextId = 0;
function newId(): string {
    nextId += 1;
    return `clip-${Date.now().toString(36)}-${nextId}`;
}

/** Shorts from the backend's ranked candidates; all start selected. */
export function fromCandidates(candidates: ClipCandidate[]): ShortClip[] {
    return candidates.map((candidate) => ({
        id: newId(),
        title: candidate.title,
        hookLine: candidate.hookLine,
        reason: candidate.reason,
        ranges: candidate.ranges.map(({ start, end, role }) => ({ start, end, role })),
        score: Math.round(candidate.score),
        ratings: candidate.ratings,
        signals: candidate.signals,
        looped: candidate.looped,
        selected: true,
    }));
}

function seconds(value: unknown): number | null {
    if (typeof value === 'number') return Number.isFinite(value) ? value : null;
    if (typeof value === 'string') {
        try {
            return parseTime(value);
        } catch {
            return null;
        }
    }
    return null;
}

/**
 * Restore saved clips, including ones saved before clips were scored
 * (`{ segments: [{ start, end }], title, reason }` with timestamp strings).
 * Anything unusable is dropped.
 */
export function normalizeShortClips(raw: unknown): ShortClip[] {
    if (!Array.isArray(raw)) return [];
    return raw.flatMap((item): ShortClip[] => {
        if (!item || typeof item !== 'object') return [];
        const clip = item as Record<string, unknown>;
        const rawRanges = Array.isArray(clip.ranges)
            ? clip.ranges
            : Array.isArray(clip.segments)
              ? clip.segments
              : [];
        const ranges: ShortClipRange[] = rawRanges.flatMap((range): ShortClipRange[] => {
            if (!range || typeof range !== 'object') return [];
            const r = range as Record<string, unknown>;
            const start = seconds(r.start);
            const end = seconds(r.end);
            if (start === null || end === null || end <= start) return [];
            const role = typeof r.role === 'string' ? (r.role as ShortClipRange['role']) : 'body';
            return [{ start, end, role }];
        });
        if (ranges.length === 0) return [];
        return [
            {
                id: typeof clip.id === 'string' ? clip.id : newId(),
                title: typeof clip.title === 'string' ? clip.title : '',
                hookLine: typeof clip.hookLine === 'string' ? clip.hookLine : '',
                reason: typeof clip.reason === 'string' ? clip.reason : '',
                ranges,
                score: typeof clip.score === 'number' ? clip.score : null,
                ratings: (clip.ratings as ShortClip['ratings']) ?? null,
                signals: (clip.signals as ShortClip['signals']) ?? null,
                looped: clip.looped === true,
                selected: clip.selected !== false,
            },
        ];
    });
}

export function clipDuration(clip: ShortClip): number {
    return clip.ranges.reduce((total, range) => total + (range.end - range.start), 0);
}

/** The clip's ranges as the export command takes them. */
export function toExportSegments(clip: ShortClip): { start: string; end: string }[] {
    return clip.ranges.map((range) => ({
        start: formatTime(range.start),
        end: formatTime(range.end),
    }));
}

export interface WordBoundaries {
    /** Sorted times where a word starts. */
    starts: number[];
    /** Sorted times where a word ends. */
    ends: number[];
}

/**
 * Where clip edges may sit: word boundaries when the transcript has word
 * timings, segment boundaries otherwise.
 */
export function wordBoundaries(segments: TranscriptSegment[]): WordBoundaries {
    const starts = new Set<number>();
    const ends = new Set<number>();
    for (const segment of segments) {
        const units = segment.words?.length ? segment.words : [segment];
        for (const unit of units) {
            const start = seconds(unit.start);
            const end = seconds(unit.end);
            if (start !== null) starts.add(start);
            if (end !== null) ends.add(end);
        }
    }
    const sorted = (values: Set<number>) => [...values].sort((a, b) => a - b);
    return { starts: sorted(starts), ends: sorted(ends) };
}

/** The boundary next to `current` in `direction`, or `current` at the edge. */
function step(boundaries: number[], current: number, direction: -1 | 1): number {
    const epsilon = 1e-3;
    if (direction > 0) {
        return boundaries.find((time) => time > current + epsilon) ?? current;
    }
    for (let i = boundaries.length - 1; i >= 0; i -= 1) {
        if (boundaries[i] < current - epsilon) return boundaries[i];
    }
    return current;
}

/** Shortest a range may become when trimming. */
const MIN_RANGE_SECONDS = 0.5;

/**
 * Move the clip's start (its first range) or end (its last range) by one word
 * boundary. A move that would leave the range shorter than half a second is
 * ignored.
 */
export function trimClip(
    clip: ShortClip,
    edge: 'start' | 'end',
    direction: -1 | 1,
    boundaries: WordBoundaries,
): ShortClip {
    const ranges = clip.ranges.map((range) => ({ ...range }));
    const index = edge === 'start' ? 0 : ranges.length - 1;
    const range = ranges[index];
    if (edge === 'start') {
        range.start = step(boundaries.starts, range.start, direction);
    } else {
        range.end = step(boundaries.ends, range.end, direction);
    }
    if (range.end - range.start < MIN_RANGE_SECONDS) return clip;
    return { ...clip, ranges };
}

export type PlaybackStep =
    | { action: 'continue' }
    | { action: 'seek'; to: number; rangeIndex: number }
    | { action: 'stop' };

/**
 * What a preview playing `ranges` back to back should do at `time` while in
 * `rangeIndex`: keep playing, jump to the next range, or stop after the last.
 */
export function playbackStep(
    time: number,
    ranges: ShortClipRange[],
    rangeIndex: number,
): PlaybackStep {
    const range = ranges[rangeIndex];
    if (!range || time < range.end) return { action: 'continue' };
    const next = rangeIndex + 1;
    if (next >= ranges.length) return { action: 'stop' };
    return { action: 'seek', to: ranges[next].start, rangeIndex: next };
}

/**
 * Who speaks when, for framing vertical clips: one turn per transcript
 * segment with a speaker and valid times.
 */
export function speakerTurns(segments: TranscriptSegment[]): SpeakerTurn[] {
    const turns: SpeakerTurn[] = [];
    for (const segment of segments) {
        const start = seconds(segment.start);
        const end = seconds(segment.end);
        const speaker = segment.speaker?.trim();
        if (start === null || end === null || end <= start || !speaker) continue;
        turns.push({ start, end, speaker });
    }
    return turns;
}

/**
 * Transcript words overlapping `ranges` (source seconds), for burned-in
 * captions. Segments without word timings spread their words evenly.
 */
export function captionWords(
    segments: TranscriptSegment[],
    ranges: { start: number; end: number }[],
): CaptionWord[] {
    const overlaps = (start: number, end: number) =>
        ranges.some((range) => start < range.end && end > range.start);
    const words: CaptionWord[] = [];
    for (const segment of segments) {
        const start = seconds(segment.start);
        const end = seconds(segment.end);
        if (start === null || end === null || !overlaps(start, end)) continue;
        const timed = (segment.words ?? [])
            .map((word) => ({
                start: seconds(word.start),
                end: seconds(word.end),
                text: word.text.trim(),
            }))
            .filter(
                (word): word is CaptionWord =>
                    word.start !== null && word.end !== null && word.text !== '',
            );
        if (timed.length > 0) {
            words.push(...timed.filter((word) => overlaps(word.start, word.end)));
            continue;
        }
        const texts = segment.text.split(/\s+/).filter(Boolean);
        const step = (end - start) / Math.max(texts.length, 1);
        texts.forEach((text, index) => {
            const word = { start: start + index * step, end: start + (index + 1) * step, text };
            if (overlaps(word.start, word.end)) words.push(word);
        });
    }
    return words;
}
