import type { CaptionWord, DetectedFace, FaceOverride, SpeakerTurn } from '../bindings';
import type {
    ClipCandidate,
    FaceSearch,
    ShortClip,
    ShortClipRange,
    TimeSpan,
    TranscriptSegment,
} from '../types';
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
                ...(spans(clip.cutWords).length ? { cutWords: spans(clip.cutWords) } : {}),
                ...(spans(clip.cuts).length ? { cuts: spans(clip.cuts) } : {}),
            },
        ];
    });
}

/** Saved time spans, without malformed ones. */
function spans(raw: unknown): TimeSpan[] {
    if (!Array.isArray(raw)) return [];
    return raw.filter(
        (span): span is TimeSpan =>
            !!span &&
            typeof span.start === 'number' &&
            typeof span.end === 'number' &&
            span.end > span.start,
    );
}

/** Pieces shorter than this (seconds) left between cuts are dropped. */
const MIN_PIECE = 0.05;

/** The clip's ranges without what the user cut out, in playback order. */
export function playedRanges(clip: Pick<ShortClip, 'ranges' | 'cuts'>): ShortClipRange[] {
    const cuts = [...(clip.cuts ?? [])].sort((a, b) => a.start - b.start);
    return clip.ranges.flatMap((range) => {
        const pieces: ShortClipRange[] = [];
        let start = range.start;
        for (const cut of cuts) {
            if (cut.end <= start || cut.start >= range.end) continue;
            if (cut.start - start >= MIN_PIECE) pieces.push({ ...range, start, end: cut.start });
            start = Math.max(start, cut.end);
        }
        if (range.end - start >= MIN_PIECE) pieces.push({ ...range, start, end: range.end });
        return pieces;
    });
}

/**
 * What to cut for `cutWords`, given the clip's words in source order: each
 * run of cut words from its first word's start up to the next word's start
 * (taking the pause after it along), or its last word's end when nothing
 * follows within a second.
 */
export function cutIntervals(words: ClipWord[], cutWords: TimeSpan[]): TimeSpan[] {
    const isCut = (word: ClipWord) =>
        cutWords.some((cut) => cut.start === word.start && cut.end === word.end);
    const intervals: TimeSpan[] = [];
    let index = 0;
    while (index < words.length) {
        if (!isCut(words[index])) {
            index += 1;
            continue;
        }
        const first = words[index];
        while (index + 1 < words.length && isCut(words[index + 1])) index += 1;
        const last = words[index];
        const next = words[index + 1];
        const end =
            next && next.start >= last.end && next.start - last.end <= 1 ? next.start : last.end;
        intervals.push({ start: first.start, end });
        index += 1;
    }
    return intervals;
}

/**
 * `clip` with `changed` words cut out (or restored), `words` being all of
 * its words in source order. Cutting everything that plays is refused.
 */
export function setCutWords(
    clip: ShortClip,
    words: ClipWord[],
    changed: ClipWord[],
    cut: boolean,
): ShortClip {
    const same = (a: TimeSpan, b: TimeSpan) => a.start === b.start && a.end === b.end;
    const kept = (clip.cutWords ?? []).filter((span) => !changed.some((word) => same(span, word)));
    const cutWords = cut ? [...kept, ...changed.map(({ start, end }) => ({ start, end }))] : kept;
    const cuts = cutIntervals(words, cutWords);
    if (playedRanges({ ranges: clip.ranges, cuts }).length === 0) return clip;
    const { cutWords: _old, cuts: _oldCuts, ...rest } = clip;
    return cutWords.length ? { ...rest, cutWords, cuts } : rest;
}

export function clipDuration(clip: ShortClip): number {
    return playedRanges(clip).reduce((total, range) => total + (range.end - range.start), 0);
}

/** The clip's ranges as the export command takes them. */
export function toExportSegments(clip: ShortClip): { start: string; end: string }[] {
    return playedRanges(clip).map((range) => ({
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
    ranges: { start: number; end: number }[],
    rangeIndex: number,
): PlaybackStep {
    const range = ranges[rangeIndex];
    if (!range || time < range.end) return { action: 'continue' };
    const next = rangeIndex + 1;
    if (next >= ranges.length) return { action: 'stop' };
    return { action: 'seek', to: ranges[next].start, rangeIndex: next };
}

/** Saved face overrides, without malformed entries. */
export function normalizeFaceOverrides(raw: unknown): FaceOverride[] {
    if (!Array.isArray(raw)) return [];
    return raw.filter(
        (item): item is FaceOverride =>
            !!item &&
            typeof item === 'object' &&
            Array.isArray(item.anchors) &&
            typeof item.ignored === 'boolean' &&
            !!item.speaker &&
            typeof item.speaker.kind === 'string',
    );
}

/** A saved face search, or null when there is none or it is malformed. */
export function normalizeFaceSearch(raw: unknown): FaceSearch | null {
    if (!raw || typeof raw !== 'object') return null;
    const search = raw as Record<string, unknown>;
    if (typeof search.signature !== 'string' || !Array.isArray(search.faces)) return null;
    const faces = search.faces.filter(
        (face): face is DetectedFace =>
            !!face &&
            typeof face === 'object' &&
            Array.isArray(face.anchors) &&
            typeof face.thumbnail === 'string' &&
            typeof face.seconds === 'number',
    );
    return { signature: search.signature, faces };
}

/** How a 0-10 rating reads at a glance: 9+ stands out, 4 or less fades. */
export type RatingTone = 'top' | 'high' | 'good' | 'mid' | 'low';

export function ratingTone(value: number): RatingTone {
    if (value >= 9) return 'top';
    if (value >= 8) return 'high';
    if (value >= 7) return 'good';
    if (value >= 5) return 'mid';
    return 'low';
}

/** Anchors kept per face override (old ones first make way). */
const MAX_FACE_ANCHORS = 96;

/**
 * `overrides` with the user's word on `face` changed: the override that
 * applied to it, or a new one, now also anchored where the face was just
 * seen. Overrides keep their places, so `applied` indices stay valid.
 */
export function setFaceOverride(
    overrides: FaceOverride[],
    face: DetectedFace,
    change: Partial<Pick<FaceOverride, 'ignored' | 'speaker'>>,
): FaceOverride[] {
    const existing = face.applied === null ? undefined : overrides[face.applied];
    const anchors = [...(existing?.anchors ?? []), ...face.anchors].slice(-MAX_FACE_ANCHORS);
    const updated: FaceOverride = {
        ignored: existing?.ignored ?? false,
        speaker: existing?.speaker ?? { kind: 'auto' },
        ...change,
        anchors,
    };
    if (existing) {
        return overrides.map((override, i) => (i === face.applied ? updated : override));
    }
    return [...overrides, updated];
}

/** The transcript's speakers, in order of first appearance. */
export function speakerNames(segments: TranscriptSegment[]): string[] {
    return [...new Set(speakerTurns(segments).map((turn) => turn.speaker))];
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

/** A transcript word in a clip, and where it lives in the transcript. */
export interface ClipWord {
    /** Index of its segment. */
    segment: number;
    /** Index in the segment's `words`, or of its token in `text` when the segment has no word timings. */
    index: number;
    start: number;
    end: number;
    text: string;
    speaker: string;
}

/**
 * Transcript words overlapping `ranges` (source seconds), with where each
 * one lives. Segments without word timings spread their words evenly.
 */
export function clipWords(
    segments: TranscriptSegment[],
    ranges: { start: number; end: number }[],
): ClipWord[] {
    const overlaps = (start: number, end: number) =>
        ranges.some((range) => start < range.end && end > range.start);
    const words: ClipWord[] = [];
    segments.forEach((segment, segmentIndex) => {
        const start = seconds(segment.start);
        const end = seconds(segment.end);
        if (start === null || end === null || !overlaps(start, end)) return;
        const speaker = segment.speaker;
        const timed = (segment.words ?? []).flatMap((word, index): ClipWord[] => {
            const wordStart = seconds(word.start);
            const wordEnd = seconds(word.end);
            const text = word.text.trim();
            if (wordStart === null || wordEnd === null || text === '') return [];
            return [
                { segment: segmentIndex, index, start: wordStart, end: wordEnd, text, speaker },
            ];
        });
        if (timed.length > 0) {
            words.push(...timed.filter((word) => overlaps(word.start, word.end)));
            return;
        }
        const texts = segment.text.split(/\s+/).filter(Boolean);
        const step = (end - start) / Math.max(texts.length, 1);
        texts.forEach((text, index) => {
            const word = {
                segment: segmentIndex,
                index,
                start: start + index * step,
                end: start + (index + 1) * step,
                text,
                speaker,
            };
            if (overlaps(word.start, word.end)) words.push(word);
        });
    });
    return words;
}

/**
 * Transcript words overlapping `ranges` (source seconds), for burned-in
 * captions. Segments without word timings spread their words evenly.
 */
export function captionWords(
    segments: TranscriptSegment[],
    ranges: { start: number; end: number }[],
): CaptionWord[] {
    return clipWords(segments, ranges).map(({ start, end, text }) => ({ start, end, text }));
}

/**
 * `segments` with one word's text changed (timings stay). The segment's
 * text follows: its matching token is replaced when text and words line
 * up, else the old word where it occurs exactly once.
 */
export function editWord(
    segments: TranscriptSegment[],
    word: Pick<ClipWord, 'segment' | 'index'>,
    text: string,
): TranscriptSegment[] {
    const segment = segments[word.segment];
    const replacement = text.trim();
    if (!segment || replacement === '') return segments;
    const tokens = segment.text.split(/\s+/).filter(Boolean);

    let updated: TranscriptSegment;
    if (segment.words?.length) {
        const old = segment.words[word.index];
        if (!old) return segments;
        const words = segment.words.map((w, i) =>
            i === word.index ? { ...w, text: replacement } : w,
        );
        // Position among the words that have text, as tokens are.
        const position = segment.words
            .slice(0, word.index)
            .filter((w) => w.text.trim() !== '').length;
        const spoken = segment.words.filter((w) => w.text.trim() !== '').length;
        let segmentText = segment.text;
        if (tokens.length === spoken) {
            tokens[position] = replacement;
            segmentText = tokens.join(' ');
        } else if (segment.text.split(old.text.trim()).length === 2) {
            segmentText = segment.text.replace(old.text.trim(), replacement);
        }
        updated = { ...segment, words, text: segmentText };
    } else {
        if (word.index >= tokens.length) return segments;
        tokens[word.index] = replacement;
        updated = { ...segment, text: tokens.join(' ') };
    }
    return segments.map((s, i) => (i === word.segment ? updated : s));
}
