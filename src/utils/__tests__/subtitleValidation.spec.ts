import { describe, it, expect } from 'vitest';
import type { TranscriptSegment } from '../../types';
import {
  splitTextIntoChunks,
  splitSubtitleEntry,
  timeToMs,
  msToTime,
  validateSubtitles,
  processSubtitlesForDisplay,
  DEFAULT_SUBTITLE_OPTIONS,
} from '../subtitleValidation';

describe('subtitleValidation', () => {
  describe('splitTextIntoChunks', () => {
    it('should return single chunk for short text', () => {
      const text = 'Hello world';
      const chunks = splitTextIntoChunks(text, 42);
      expect(chunks).toHaveLength(1);
      expect(chunks[0]).toBe('Hello world');
    });

    it('should split long text at word boundaries', () => {
      const text = 'This is a very long sentence that needs to be split into multiple chunks for comfortable reading';
      const chunks = splitTextIntoChunks(text, 42);
      expect(chunks.length).toBeGreaterThan(1);
      chunks.forEach(chunk => {
        expect(chunk.length).toBeLessThanOrEqual(42);
      });
    });

    it('should respect sentence boundaries when possible', () => {
      const text = 'First sentence here. Second sentence here. Third sentence here.';
      const chunks = splitTextIntoChunks(text, 50);
      // Should keep sentences together if they fit
      expect(chunks.length).toBeLessThanOrEqual(3);
    });

    it('should handle very long words by forcing split', () => {
      const text = 'supercalifragilisticexpialidocious is a long word';
      const chunks = splitTextIntoChunks(text, 20);
      expect(chunks.length).toBeGreaterThan(1);
      chunks.forEach(chunk => {
        expect(chunk.length).toBeLessThanOrEqual(20);
      });
    });
  });

  describe('timeToMs', () => {
    it('should convert MM:SS to milliseconds', () => {
      expect(timeToMs('1:30')).toBe(90000);
      expect(timeToMs('5:00')).toBe(300000);
    });

    it('should convert HH:MM:SS to milliseconds', () => {
      expect(timeToMs('1:30:00')).toBe(5400000);
      expect(timeToMs('0:00:30')).toBe(30000);
    });
  });

  describe('msToTime', () => {
    it('should convert milliseconds to MM:SS', () => {
      expect(msToTime(90000)).toBe('1:30');
      expect(msToTime(300000)).toBe('5:00');
    });

    it('should convert milliseconds to HH:MM:SS when needed', () => {
      expect(msToTime(5400000)).toBe('1:30:00');
      expect(msToTime(3661000)).toBe('1:01:01');
    });
  });

  describe('splitSubtitleEntry', () => {
    it('should return single segment for short text', () => {
      const segment: TranscriptSegment = {
        start: '0:00',
        end: '0:05',
        text: 'Short text',
        speaker: 'Speaker 1',
      };
      const result = splitSubtitleEntry(segment);
      expect(result).toHaveLength(1);
      expect(result[0].text).toBe('Short text');
    });

    it('should split long text into multiple segments', () => {
      const segment: TranscriptSegment = {
        start: '0:00',
        end: '0:10',
        text: 'This is a very long text that exceeds the maximum character limit for a single subtitle line and needs to be broken down into multiple segments for comfortable reading in video players',
        speaker: 'Speaker 1',
      };
      const result = splitSubtitleEntry(segment, { maxCharsPerLine: 42, maxLines: 2 });
      expect(result.length).toBeGreaterThan(1);
      
      // Check timing distribution
      expect(result[0].start).toBe('0:00');
      expect(result[result.length - 1].end).toBe('0:10');
      
      // Check that each segment respects max lines
      result.forEach(seg => {
        const lines = seg.text.split('\n');
        expect(lines.length).toBeLessThanOrEqual(2);
      });
    });

    it('should preserve speaker across split segments', () => {
      const segment: TranscriptSegment = {
        start: '0:00',
        end: '0:05',
        text: 'A'.repeat(200),
        speaker: 'John Doe',
      };
      const result = splitSubtitleEntry(segment);
      result.forEach(seg => {
        expect(seg.speaker).toBe('John Doe');
      });
    });
  });

  describe('validateSubtitles', () => {
    it('should return empty array for valid subtitles', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:05', text: 'Valid text', speaker: 'Speaker 1' },
      ];
      const errors = validateSubtitles(segments);
      expect(errors).toHaveLength(0);
    });

    it('should detect text that is too long', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:05', text: 'A'.repeat(50), speaker: 'Speaker 1' },
      ];
      const errors = validateSubtitles(segments, { maxCharsPerLine: 42, maxLines: 1 });
      const textErrors = errors.filter(e => e.field.startsWith('text'));
      expect(textErrors.length).toBeGreaterThan(0);
    });

    it('should detect too many lines', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:05', text: 'Line 1\nLine 2\nLine 3\nLine 4', speaker: 'Speaker 1' },
      ];
      const errors = validateSubtitles(segments, { maxLines: 2 });
      const lineErrors = errors.filter(e => e.field === 'text.lines');
      expect(lineErrors.length).toBeGreaterThan(0);
    });

    it('should detect short duration', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:00', text: 'Quick', speaker: 'Speaker 1' },
      ];
      const errors = validateSubtitles(segments, { minDurationMs: 500 });
      const durationErrors = errors.filter(e => e.field === 'duration');
      expect(durationErrors.length).toBeGreaterThan(0);
      expect(durationErrors[0].severity).toBe('warning');
    });

    it('should detect long duration', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:15', text: 'Text', speaker: 'Speaker 1' },
      ];
      const errors = validateSubtitles(segments, { maxDurationMs: 7000 });
      const durationErrors = errors.filter(e => e.field === 'duration');
      expect(durationErrors.length).toBeGreaterThan(0);
      expect(durationErrors[0].severity).toBe('warning');
    });

    it('should detect empty text', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:05', text: '', speaker: 'Speaker 1' },
        { start: '0:05', end: '0:10', text: '   ', speaker: 'Speaker 1' },
      ];
      const errors = validateSubtitles(segments);
      const textErrors = errors.filter(e => e.field === 'text' && e.severity === 'error');
      expect(textErrors.length).toBe(2);
    });

    it('should detect missing speaker', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:05', text: 'Text', speaker: '' },
      ];
      const errors = validateSubtitles(segments);
      const speakerErrors = errors.filter(e => e.field === 'speaker');
      expect(speakerErrors.length).toBeGreaterThan(0);
      expect(speakerErrors[0].severity).toBe('warning');
    });

    it('should detect overlapping timestamps', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:10', text: 'First', speaker: 'Speaker 1' },
        { start: '0:05', end: '0:15', text: 'Second', speaker: 'Speaker 2' },
      ];
      const errors = validateSubtitles(segments);
      const timingErrors = errors.filter(e => e.field === 'timing');
      expect(timingErrors.length).toBeGreaterThan(0);
      expect(timingErrors[0].severity).toBe('error');
    });

    it('should detect excessive characters per second', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:01', text: 'A'.repeat(30), speaker: 'Speaker 1' },
      ];
      const errors = validateSubtitles(segments, { maxCps: 20 });
      const cpsErrors = errors.filter(e => e.field === 'cps');
      expect(cpsErrors.length).toBeGreaterThan(0);
    });
  });

  describe('processSubtitlesForDisplay', () => {
    it('should process and validate subtitles', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:05', text: 'Short', speaker: 'Speaker 1' },
        { start: '0:05', end: '0:10', text: 'A'.repeat(200), speaker: 'Speaker 1' },
      ];
      const result = processSubtitlesForDisplay(segments, { maxCharsPerLine: 42, maxLines: 2 });
      
      // First segment should remain single, second should be split
      // With 200 chars and max 84 chars per subtitle (42x2), we expect 3+ segments
      expect(result.segments.length).toBeGreaterThanOrEqual(3);
      
      // Should have validation info
      expect(result.errors).toBeDefined();
    });

    it('should use default options when not provided', () => {
      const segments: TranscriptSegment[] = [
        { start: '0:00', end: '0:05', text: 'Normal text', speaker: 'Speaker 1' },
      ];
      const result = processSubtitlesForDisplay(segments);
      expect(result.segments).toHaveLength(1);
      // No errors expected for a simple valid segment
      expect(result.errors.filter(e => e.severity === 'error')).toHaveLength(0);
    });
  });

  describe('DEFAULT_SUBTITLE_OPTIONS', () => {
    it('should have reasonable defaults', () => {
      expect(DEFAULT_SUBTITLE_OPTIONS.maxCharsPerLine).toBe(42);
      expect(DEFAULT_SUBTITLE_OPTIONS.maxLines).toBe(2);
      expect(DEFAULT_SUBTITLE_OPTIONS.minDurationMs).toBe(500);
      expect(DEFAULT_SUBTITLE_OPTIONS.maxDurationMs).toBe(7000);
      expect(DEFAULT_SUBTITLE_OPTIONS.minCps).toBe(8);
      expect(DEFAULT_SUBTITLE_OPTIONS.maxCps).toBe(25);
    });
  });
});
