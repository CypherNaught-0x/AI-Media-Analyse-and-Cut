import { describe, expect, it } from 'vitest';
import type { DetectedFace } from '../../bindings';
import type { ClipCandidate, ShortClip, TranscriptSegment } from '../../types';
import {
    clipDuration,
    fromCandidates,
    normalizeShortClips,
    captionWords,
    applyAutoCuts,
    clipWords,
    cutIntervals,
    editRequest,
    editWord,
    playedRanges,
    setCutWords,
    playbackStep,
    ratingTone,
    setFaceOverride,
    speakerNames,
    speakerTurns,
    toExportSegments,
    trimClip,
    wordBoundaries,
} from '../shortClips';

const candidate: ClipCandidate = {
    title: 'Why we deleted prod',
    hookLine: 'So we deleted production on purpose.',
    reason: 'Surprising reveal',
    ranges: [
        { start: 90, end: 95, role: 'loop_opener', firstSegment: 9, lastSegment: 9 },
        { start: 20, end: 40, role: 'body', firstSegment: 2, lastSegment: 3 },
    ],
    ratings: { hook: 9, standalone: 7, emotion: 6, info: 5, loopContinuity: 8 },
    signals: { speakerChanges: 2, laughs: 1, wordsPerSecond: 2.8 },
    score: 81.6,
    duration: 25,
    looped: true,
};

function clip(ranges: ShortClip['ranges']): ShortClip {
    return { ...fromCandidates([candidate])[0], ranges };
}

describe('short clips', () => {
    it('turns candidates into selected clips with stable ids', () => {
        const [a, b] = fromCandidates([candidate, candidate]);
        expect(a.id).not.toBe(b.id);
        expect(a).toMatchObject({
            title: 'Why we deleted prod',
            score: 82,
            looped: true,
            selected: true,
            ranges: [
                { start: 90, end: 95, role: 'loop_opener' },
                { start: 20, end: 40, role: 'body' },
            ],
        });
        expect(clipDuration(a)).toBe(25);
    });

    it('restores clips saved before scoring existed', () => {
        const restored = normalizeShortClips([
            {
                title: 'Old clip',
                reason: 'Saved by 0.13',
                segments: [{ start: '00:10.500', end: '00:20' }],
            },
            { title: 'Broken', segments: [{ start: 'x', end: '00:20' }] },
            'nonsense',
        ]);
        expect(restored).toHaveLength(1);
        expect(restored[0]).toMatchObject({
            title: 'Old clip',
            ranges: [{ start: 10.5, end: 20, role: 'body' }],
            score: null,
            selected: true,
        });
    });

    it('round-trips current clips unchanged', () => {
        const [original] = fromCandidates([candidate]);
        expect(normalizeShortClips(JSON.parse(JSON.stringify([original])))).toEqual([original]);
    });

    it('exports ranges as timestamp strings', () => {
        expect(toExportSegments(clip([{ start: 61.25, end: 75, role: 'body' }]))).toEqual([
            { start: '01:01.250', end: '01:15.000' },
        ]);
    });

    const segments: TranscriptSegment[] = [
        {
            start: '00:10',
            end: '00:13',
            speaker: 'A',
            text: 'one two three',
            words: [
                { start: '00:10.000', end: '00:10.800', text: 'one' },
                { start: '00:11.000', end: '00:11.900', text: 'two' },
                { start: '00:12.100', end: '00:13.000', text: 'three' },
            ],
        },
        { start: '00:14', end: '00:16', speaker: 'B', text: 'no word timings' },
    ];

    it('snaps trims to word boundaries, falling back to segments', () => {
        const boundaries = wordBoundaries(segments);
        expect(boundaries.starts).toEqual([10, 11, 12.1, 14]);
        expect(boundaries.ends).toEqual([10.8, 11.9, 13, 16]);

        const base = clip([{ start: 11, end: 13, role: 'body' }]);
        expect(trimClip(base, 'start', 1, boundaries).ranges[0].start).toBe(12.1);
        expect(trimClip(base, 'start', -1, boundaries).ranges[0].start).toBe(10);
        expect(trimClip(base, 'end', 1, boundaries).ranges[0].end).toBe(16);
        expect(trimClip(base, 'end', -1, boundaries).ranges[0].end).toBe(11.9);
    });

    it('trims only the outer edges and never collapses a range', () => {
        const boundaries = wordBoundaries(segments);
        const spliced = clip([
            { start: 14, end: 16, role: 'hook' },
            { start: 10, end: 13, role: 'body' },
        ]);
        const trimmed = trimClip(spliced, 'end', -1, boundaries);
        expect(trimmed.ranges[0]).toEqual(spliced.ranges[0]);
        expect(trimmed.ranges[1].end).toBe(11.9);

        const tiny = clip([{ start: 12.1, end: 13, role: 'body' }]);
        expect(trimClip(tiny, 'end', -1, boundaries)).toBe(tiny);
    });

    it('plays ranges back to back, then stops', () => {
        const ranges = candidate.ranges.map(({ start, end, role }) => ({ start, end, role }));
        expect(playbackStep(92, ranges, 0)).toEqual({ action: 'continue' });
        expect(playbackStep(95.1, ranges, 0)).toEqual({ action: 'seek', to: 20, rangeIndex: 1 });
        expect(playbackStep(30, ranges, 1)).toEqual({ action: 'continue' });
        expect(playbackStep(40, ranges, 1)).toEqual({ action: 'stop' });
    });
});

describe('speakerTurns', () => {
    it('turns transcript segments into timed speaker turns', () => {
        const turns = speakerTurns([
            { start: '01:00.500', end: '01:04.000', speaker: 'Annette', text: 'Hi' },
            { start: '01:04.000', end: '01:03.000', speaker: 'Bob', text: 'backwards' },
            { start: '01:05.000', end: '01:06.000', speaker: '  ', text: 'nobody' },
            { start: 'later', end: '01:09.000', speaker: 'Bob', text: 'bad time' },
            { start: '1:01:10.000', end: '1:01:12.250', speaker: 'Bob', text: 'Hello' },
        ]);
        expect(turns).toEqual([
            { start: 60.5, end: 64, speaker: 'Annette' },
            { start: 3670, end: 3672.25, speaker: 'Bob' },
        ]);
    });
});

describe('captionWords', () => {
    it('takes timed words inside the clip and spreads untimed segments', () => {
        const words = captionWords(
            [
                {
                    start: '00:10.000',
                    end: '00:12.000',
                    speaker: 'A',
                    text: 'one two',
                    words: [
                        { start: '00:10.000', end: '00:10.900', text: 'one' },
                        { start: '00:11.000', end: '00:12.000', text: ' two ' },
                    ],
                },
                { start: '00:20.000', end: '00:22.000', speaker: 'B', text: 'three four' },
                { start: '00:40.000', end: '00:41.000', speaker: 'B', text: 'outside' },
            ],
            [
                { start: 10.5, end: 15 },
                { start: 20, end: 30 },
            ],
        );
        expect(words).toEqual([
            { start: 10, end: 10.9, text: 'one' },
            { start: 11, end: 12, text: 'two' },
            { start: 20, end: 21, text: 'three' },
            { start: 21, end: 22, text: 'four' },
        ]);
    });
});

describe('setFaceOverride', () => {
    const face = (applied: number | null, time: number): DetectedFace => ({
        anchors: [{ time, x: 0.5, y: 0.3 }],
        thumbnail: '',
        speaker: null,
        confident: false,
        views: 1,
        seconds: 4,
        applied,
    });

    it('adds an override for a face none applied to', () => {
        const overrides = setFaceOverride([], face(null, 10), { ignored: true });
        expect(overrides).toEqual([
            { anchors: [{ time: 10, x: 0.5, y: 0.3 }], ignored: true, speaker: { kind: 'auto' } },
        ]);
    });

    it('changes the applied override in place, keeping its anchors', () => {
        const first = setFaceOverride([], face(null, 10), { ignored: true });
        const other = setFaceOverride(first, face(null, 50), {
            speaker: { kind: 'named', name: 'Host' },
        });
        const changed = setFaceOverride(other, face(0, 30), { ignored: false });
        expect(changed).toHaveLength(2);
        expect(changed[0].ignored).toBe(false);
        expect(changed[0].anchors.map((a) => a.time)).toEqual([10, 30]);
        expect(changed[1]).toBe(other[1]);
    });
});

describe('speakerNames', () => {
    it('lists each speaker once, in order of appearance', () => {
        expect(
            speakerNames([
                { start: '00:10.000', end: '00:12.000', speaker: 'Host', text: 'Hi' },
                { start: '00:20.000', end: '00:22.000', speaker: 'Guest', text: 'Hello' },
                { start: '00:30.000', end: '00:32.000', speaker: 'Host', text: 'So' },
            ]),
        ).toEqual(['Host', 'Guest']);
    });
});

describe('ratingTone', () => {
    it('makes 9-10 stand out and fades 4 or less', () => {
        expect([10, 9, 8, 7, 6, 5, 4, 0].map(ratingTone)).toEqual([
            'top',
            'top',
            'high',
            'good',
            'mid',
            'mid',
            'low',
            'low',
        ]);
    });
});

describe('clipWords and editWord', () => {
    const segments: TranscriptSegment[] = [
        {
            start: '00:10.000',
            end: '00:12.000',
            speaker: 'Annette',
            text: '60 Prozent nutzen Schatten KI.',
            words: [
                { start: '00:10.000', end: '00:10.400', text: '60' },
                { start: '00:10.400', end: '00:10.900', text: 'Prozent' },
                { start: '00:10.900', end: '00:11.300', text: 'nutzen' },
                { start: '00:11.300', end: '00:11.700', text: 'Schatten' },
                { start: '00:11.700', end: '00:12.000', text: 'KI.' },
            ],
        },
        { start: '00:12.000', end: '00:14.000', speaker: 'Tobias', text: 'Wirklich so viele?' },
    ];

    it('lists the words in the ranges with where they live', () => {
        const words = clipWords(segments, [{ start: 10.5, end: 13 }]);
        expect(words.map((w) => [w.segment, w.index, w.text, w.speaker])).toEqual([
            [0, 1, 'Prozent', 'Annette'],
            [0, 2, 'nutzen', 'Annette'],
            [0, 3, 'Schatten', 'Annette'],
            [0, 4, 'KI.', 'Annette'],
            [1, 0, 'Wirklich', 'Tobias'],
            [1, 1, 'so', 'Tobias'],
        ]);
    });

    it('edits a timed word and the segment text with it', () => {
        const edited = editWord(segments, { segment: 0, index: 3 }, 'Schatten-KI');
        expect(edited[0].words?.[3]).toEqual({
            start: '00:11.300',
            end: '00:11.700',
            text: 'Schatten-KI',
        });
        expect(edited[0].text).toBe('60 Prozent nutzen Schatten-KI KI.');
        expect(edited[1]).toBe(segments[1]);
    });

    it('edits a word of a segment without timings in its text', () => {
        const edited = editWord(segments, { segment: 1, index: 2 }, 'viele!');
        expect(edited[1].text).toBe('Wirklich so viele!');
    });

    it('ignores empty edits', () => {
        expect(editWord(segments, { segment: 0, index: 0 }, '  ')).toBe(segments);
    });
});

describe('cutting words out of a clip', () => {
    const word = (start: number, end: number, text: string) => ({
        segment: 0,
        index: 0,
        start,
        end,
        text,
        speaker: 'A',
    });
    // "So, äh, we deleted prod." with a pause after "äh".
    const words = [
        word(10, 10.3, 'So,'),
        word(10.4, 10.7, 'äh,'),
        word(11.2, 11.5, 'we'),
        word(11.5, 11.9, 'deleted'),
        word(12.0, 12.5, 'prod.'),
    ];
    const clip = fromCandidates([
        {
            title: 'T',
            hookLine: '',
            reason: '',
            ranges: [{ start: 10, end: 13, role: 'body', firstSegment: 0, lastSegment: 0 }],
            ratings: null as never,
            signals: null as never,
            score: 50,
            duration: 3,
            looped: false,
        },
    ])[0];

    it('cuts a run up to the next word, taking its pause along', () => {
        expect(cutIntervals(words, [words[1], words[2]])).toEqual([{ start: 10.4, end: 11.5 }]);
        // The last word: up to its end.
        expect(cutIntervals(words, [words[4]])).toEqual([{ start: 12.0, end: 12.5 }]);
    });

    it('plays and exports the ranges around the cuts', () => {
        const cut = setCutWords(clip, words, [words[1]], true);
        expect(cut.cutWords).toEqual([{ start: 10.4, end: 10.7 }]);
        expect(playedRanges(cut).map((r) => [r.start, r.end, r.role])).toEqual([
            [10, 10.4, 'body'],
            [11.2, 13, 'body'],
        ]);
        expect(clipDuration(cut)).toBeCloseTo(2.2);
        expect(toExportSegments(cut)).toEqual([
            { start: '00:10.000', end: '00:10.400' },
            { start: '00:11.200', end: '00:13.000' },
        ]);
        // Restoring removes the cut entirely.
        const restored = setCutWords(cut, words, [words[1]], false);
        expect(restored).not.toHaveProperty('cuts');
        expect(playedRanges(restored)).toEqual(clip.ranges);
    });

    it('refuses to cut everything and keeps cuts through saving', () => {
        const all = setCutWords(
            { ...clip, ranges: [{ start: 10, end: 12.5, role: 'body' }] },
            words,
            words,
            true,
        );
        expect(all.cuts).toBeUndefined();

        const cut = setCutWords(clip, words, [words[1]], true);
        expect(normalizeShortClips(JSON.parse(JSON.stringify([cut])))).toEqual([cut]);
    });
});

describe('auto editing', () => {
    const segments: TranscriptSegment[] = [
        {
            start: '00:00.000',
            end: '00:04.000',
            speaker: 'Host',
            text: 'a b c d',
            words: ['a', 'b', 'c', 'd'].map((text, i) => ({
                start: `00:0${i}.000`,
                end: `00:0${i}.900`,
                text,
            })),
        },
        {
            start: '00:10.000',
            end: '00:12.000',
            speaker: 'Guest',
            text: 'x y',
            words: [
                { start: '00:10.000', end: '00:10.900', text: 'x' },
                { start: '00:11.000', end: '00:11.900', text: 'y' },
            ],
        },
    ];
    // Looped: the opener (10-12 s) plays before the body (0-4 s).
    const clip = {
        ...fromCandidates([
            {
                title: 'Loop',
                hookLine: '',
                reason: '',
                ranges: [
                    { start: 10, end: 12, role: 'loop_opener', firstSegment: 1, lastSegment: 1 },
                    { start: 0, end: 4, role: 'closing', firstSegment: 0, lastSegment: 0 },
                ],
                ratings: null as never,
                signals: null as never,
                score: 50,
                duration: 6,
                looped: true,
            },
        ])[0],
    };

    it('sends the words in playback order and marks the splice', () => {
        expect(editRequest(segments, clip)).toEqual({
            title: 'Loop',
            looped: true,
            words: [
                { text: 'x', speaker: 'Guest', splice: false },
                { text: 'y', speaker: 'Guest', splice: false },
                { text: 'a', speaker: 'Host', splice: true },
                { text: 'b', speaker: 'Host', splice: false },
                { text: 'c', speaker: 'Host', splice: false },
                { text: 'd', speaker: 'Host', splice: false },
            ],
        });
    });

    it('maps playback word numbers to cuts and replaces only earlier auto cuts', () => {
        // The user cut "d" (index 5) themselves; an earlier auto edit cut "x".
        const words = clipWords(segments, clip.ranges);
        let edited = setCutWords(clip, words, [words[3]], true);
        edited = setCutWords(edited, words, [words[4]], true, { auto: true, reason: 'old' });

        // Now auto editing cuts "b c" (playback 3-4).
        const result = applyAutoCuts(segments, edited, [{ from: 3, to: 4, reason: 'Fehlstart' }]);
        expect(result.cutWords).toEqual([
            { start: 3, end: 3.9 },
            { start: 1, end: 1.9, auto: true, reason: 'Fehlstart' },
            { start: 2, end: 2.9, auto: true, reason: 'Fehlstart' },
        ]);
        expect(normalizeShortClips(JSON.parse(JSON.stringify([result])))[0].cutWords).toEqual(
            result.cutWords,
        );
    });
});
