import { describe, expect, it } from 'vitest';

import type { TranscriptSegment } from '../../types';
import { detectTranscriptBlacklistMatches } from '../transcriptBlacklist';

describe('detectTranscriptBlacklistMatches', () => {
  it('matches blacklist entries as whole words', () => {
    const segments: TranscriptSegment[] = [
      {
        start: '00:00',
        end: '00:03',
        speaker: 'Speaker 1',
        text: 'Arschlochigkeit Arschloch',
        words: [
          { start: '00:00', end: '00:01', text: 'Arschlochigkeit' },
          { start: '00:01', end: '00:02', text: 'Arschloch' },
        ],
      },
    ];

    const result = detectTranscriptBlacklistMatches(segments, 'German');

    expect(result.languageCode).toBe('de');
    expect(result.matches).toHaveLength(1);
    expect(result.matches[0]).toMatchObject({
      matchedText: 'Arschloch',
      normalizedWord: 'arschloch',
      start: '00:01',
      end: '00:02',
    });
    expect(result.matchesBySegment[0]).toHaveLength(1);
    expect(result.uniqueWords).toEqual(['Arschloch']);
  });

  it('falls back to segment text tokenization when word timing data is unavailable', () => {
    const segments: TranscriptSegment[] = [
      {
        start: '00:10',
        end: '00:14',
        speaker: 'Speaker 2',
        text: 'Du Arsch.',
      },
    ];

    const result = detectTranscriptBlacklistMatches(segments, 'Original');

    expect(result.languageCode).toBe('de');
    expect(result.matches).toHaveLength(1);
    expect(result.matches[0]).toMatchObject({
      matchedText: 'Arsch.',
      normalizedWord: 'arsch',
      start: '00:10',
      end: '00:14',
    });
  });
});
