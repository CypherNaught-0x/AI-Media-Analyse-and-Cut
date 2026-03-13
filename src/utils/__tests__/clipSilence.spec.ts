import { describe, expect, it } from 'vitest';
import { trimClipBoundarySilence } from '../clipSilence';

describe('trimClipBoundarySilence', () => {
  it('trims leading silence when the clip starts inside a silence interval', () => {
    expect(
      trimClipBoundarySilence(
        [{ start: '00:05.000', end: '00:15.000' }],
        [{ start: 5, end: 7.5, duration: 2.5 }]
      )
    ).toEqual([{ start: '00:07.500', end: '00:15.000' }]);
  });

  it('trims trailing silence when the clip ends inside a silence interval', () => {
    expect(
      trimClipBoundarySilence(
        [{ start: '00:05.000', end: '00:15.000' }],
        [{ start: 12, end: 15, duration: 3 }]
      )
    ).toEqual([{ start: '00:05.000', end: '00:12.000' }]);
  });

  it('only trims the first and last segment for spliced clips', () => {
    expect(
      trimClipBoundarySilence(
        [
          { start: '00:05.000', end: '00:10.000' },
          { start: '00:20.000', end: '00:25.000' },
        ],
        [
          { start: 4, end: 6, duration: 2 },
          { start: 9, end: 10, duration: 1 },
          { start: 24, end: 25, duration: 1 },
        ]
      )
    ).toEqual([
      { start: '00:06.000', end: '00:10.000' },
      { start: '00:20.000', end: '00:24.000' },
    ]);
  });

  it('keeps the original boundary when trimming would collapse the segment', () => {
    expect(
      trimClipBoundarySilence(
        [{ start: '00:05.000', end: '00:05.400' }],
        [{ start: 5, end: 5.5, duration: 0.5 }]
      )
    ).toEqual([{ start: '00:05.000', end: '00:05.400' }]);
  });
});
