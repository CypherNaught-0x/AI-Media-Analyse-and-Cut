import { describe, expect, it } from 'vitest';
import type { DetectedFace } from '../../bindings';
import type { ClipCandidate, ShortClip, TranscriptSegment } from '../../types';
import {
    clipDuration,
    fromCandidates,
    normalizeShortClips,
    captionWords,
    playbackStep,
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
