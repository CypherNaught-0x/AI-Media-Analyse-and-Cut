import { describe, expect, it } from 'vitest';

import type { TranscriptSegment } from '../../types';
import { formatSubtitleTimeRange, formatSubtitleTimestamp, generateSubtitleContent } from '../subtitle';

describe('subtitle', () => {
  it('normalizes elapsed-minute timestamps past the one-hour mark', () => {
    expect(formatSubtitleTimestamp('60:03.520', ',')).toBe('01:00:03,520');
    expect(formatSubtitleTimestamp('61:06.720', '.')).toBe('01:01:06.720');
  });

  it('generates valid SRT cues from legacy post-hour timestamps', () => {
    const segments: TranscriptSegment[] = [
      {
        start: '59:57.920',
        end: '60:03.520',
        speaker: 'Speaker 1',
        text: 'First line',
      },
      {
        start: '60:58.800',
        end: '61:06.720',
        speaker: 'Speaker 1',
        text: 'Second line',
      },
    ];

    const content = generateSubtitleContent(segments, 'srt');

    expect(content).toContain('00:59:57,920 --> 01:00:03,520');
    expect(content).toContain('01:00:58,800 --> 01:01:06,720');
    expect(content).not.toContain('00:60:');
    expect(content).not.toContain('00:61:');
  });

  it('clamps reversed cue ranges during export formatting', () => {
    expect(formatSubtitleTimeRange('34:41', '34:40.654', ',')).toEqual({
      start: '00:34:41,000',
      end: '00:34:41,000',
    });
  });
});
