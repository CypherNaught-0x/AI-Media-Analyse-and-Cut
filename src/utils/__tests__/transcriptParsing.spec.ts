import { describe, expect, it } from 'vitest';

import { parseTime } from '../../composables/useTimeFormat';
import { normalizeTranscriptSegments } from '../transcriptParsing';

describe('normalizeTranscriptSegments', () => {
  it('normalizes point to start', () => {
    const segments = normalizeTranscriptSegments([
      {
        point: '01:09',
        end: '01:12',
        speaker: 'Speaker 2',
        text: 'Aliased start field',
      },
    ]);

    expect(segments).toEqual([
      {
        start: '01:09',
        end: '01:12',
        speaker: 'Speaker 2',
        text: 'Aliased start field',
      },
    ]);
  });

  it('applies timestamp adjustment after normalization', () => {
    const segments = normalizeTranscriptSegments(
      [
        {
          point: '01:09',
          end: '01:12',
          speaker: 'Speaker 2',
          text: 'Aliased start field',
        },
      ],
      (timestamp) => `adjusted-${timestamp}`,
    );

    expect(segments[0]).toEqual({
      start: 'adjusted-01:09',
      end: 'adjusted-01:12',
      speaker: 'Speaker 2',
      text: 'Aliased start field',
    });
  });

  it('throws a descriptive error when required timestamps are missing', () => {
    expect(() =>
      normalizeTranscriptSegments([
        {
          end: '01:12',
          speaker: 'Speaker 2',
          text: 'Missing start',
        },
      ]),
    ).toThrow("Segment 1 is missing required 'start' field");
  });
});

describe('parseTime', () => {
  it('throws a descriptive error for undefined timestamps', () => {
    expect(() => parseTime(undefined as unknown as string)).toThrow(
      'Invalid timestamp type: expected string, received undefined',
    );
  });
});
