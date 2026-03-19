import { describe, expect, it } from 'vitest';

import { parseTime } from '../../composables/useTimeFormat';
import {
  normalizeTranscriptSegments,
  parseTranscriptResponse,
  repairMalformedTranscriptJson,
} from '../transcriptParsing';

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

describe('repairMalformedTranscriptJson', () => {
  it('repairs obvious timestamp key typos and speaker quote corruption', () => {
    const repaired = repairMalformedTranscriptJson(
      '[{"speaker":"Speaker "1","text":"Hello","start":"00:00","00:04","speaker":"Speaker 1"},{"02:37","end":"02:41","speaker":"Speaker 3","text":"World"},{"\"\"start":"03:05","end":"03:08","speaker":"Speaker 3","text":"Again"}]',
    );

    expect(repaired).toContain('"speaker":"Speaker 1"');
    expect(repaired).toContain('"start":"00:00","end":"00:04","speaker":"Speaker 1"');
    expect(repaired).toContain('{"start":"02:37","end":"02:41"');
    expect(repaired).toContain('{"start":"03:05","end":"03:08"');
  });
});

describe('parseTranscriptResponse', () => {
  it('parses a malformed AI response after conservative repair', () => {
    const response = `\`\`\`json
[{"speaker":"Speaker "1","text":"Hello","start":"00:00","00:04","speaker":"Speaker 1"},{"02:37","end":"02:41","speaker":"Speaker 3","text":"World"},{"\"\"start":"03:05","end":"03:08","speaker":"Speaker 3","text":"Again"}]
\`\`\``;

    expect(parseTranscriptResponse(response)).toEqual([
      {
        start: '00:00',
        end: '00:04',
        speaker: 'Speaker 1',
        text: 'Hello',
      },
      {
        start: '02:37',
        end: '02:41',
        speaker: 'Speaker 3',
        text: 'World',
      },
      {
        start: '03:05',
        end: '03:08',
        speaker: 'Speaker 3',
        text: 'Again',
      },
    ]);
  });
});
