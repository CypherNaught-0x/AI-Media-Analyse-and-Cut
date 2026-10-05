import { describe, expect, it } from 'vitest';
import { normalizeClips, normalizeClipTimeSegments, padClipSegments } from '../clips';

describe('clip normalization', () => {
    it('converts numeric AI clip timestamps to Tauri-safe strings', () => {
        expect(
            normalizeClipTimeSegments([
                { start: 41.744, end: 59.2 },
                { start: '01:02.500', end: '75.25' },
            ]),
        ).toEqual([
            { start: '00:41.744', end: '00:59.200' },
            { start: '01:02.500', end: '01:15.250' },
        ]);
    });

    it('normalizes legacy single-segment clip responses', () => {
        expect(
            normalizeClips([
                {
                    start: 41.744,
                    end: 59.2,
                    title: 'Legacy clip',
                    reason: 'Returned without segments',
                },
            ]),
        ).toEqual([
            {
                start: '00:41.744',
                end: '00:59.200',
                title: 'Legacy clip',
                reason: 'Returned without segments',
                segments: [{ start: '00:41.744', end: '00:59.200' }],
            },
        ]);
    });

    it('rejects malformed clip timestamps before export', () => {
        expect(() =>
            normalizeClipTimeSegments([{ start: 41.744, end: Number.POSITIVE_INFINITY }]),
        ).toThrow('timestamp must be finite');
    });
});

describe('padClipSegments', () => {
    const spliced = [
        { start: '00:10.000', end: '00:20.000' },
        { start: '00:40.000', end: '00:50.000' },
    ];

    it('pads only the outer bounds of a clip', () => {
        expect(padClipSegments(spliced, 0.5, 1.25)).toEqual([
            { start: '00:09.500', end: '00:20.000' },
            { start: '00:40.000', end: '00:51.250' },
        ]);
    });

    it('never starts before zero', () => {
        expect(padClipSegments([{ start: '00:00.200', end: '00:05.000' }], 1, 0)).toEqual([
            { start: '00:00.000', end: '00:05.000' },
        ]);
    });

    it('ignores zero, negative and invalid padding', () => {
        expect(padClipSegments(spliced, 0, 0)).toBe(spliced);
        expect(padClipSegments(spliced, -1, Number.NaN)).toBe(spliced);
    });
});
