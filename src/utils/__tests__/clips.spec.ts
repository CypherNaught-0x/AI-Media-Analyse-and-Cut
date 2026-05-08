import { describe, expect, it } from 'vitest';
import { normalizeClips, normalizeClipTimeSegments } from '../clips';

describe('clip normalization', () => {
  it('converts numeric AI clip timestamps to Tauri-safe strings', () => {
    expect(
      normalizeClipTimeSegments([
        { start: 41.744, end: 59.2 },
        { start: '01:02.500', end: '75.25' },
      ])
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
      ])
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
      normalizeClipTimeSegments([{ start: 41.744, end: Number.POSITIVE_INFINITY }])
    ).toThrow('timestamp must be finite');
  });
});
