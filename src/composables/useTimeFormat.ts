import type { SegmentOffset } from '../types';

/**
 * Parse a time string (MM:SS or HH:MM:SS or seconds) to seconds
 */
export function parseTime(timeStr: string | number): number {
  if (typeof timeStr === 'number') {
    if (Number.isFinite(timeStr)) {
      return timeStr;
    }
    throw new Error(`Invalid timestamp number: ${timeStr}`);
  }

  if (typeof timeStr !== 'string') {
    throw new Error(`Invalid timestamp type: expected string, received ${typeof timeStr}`);
  }

  const normalized = timeStr.trim();
  if (!normalized) {
    throw new Error('Invalid timestamp: empty string');
  }

  const parts = normalized.split(':');
  if (parts.length === 3) {
    return parseFloat(parts[0]) * 3600 + parseFloat(parts[1]) * 60 + parseFloat(parts[2]);
  } else if (parts.length === 2) {
    return parseFloat(parts[0]) * 60 + parseFloat(parts[1]);
  }

  const parsed = parseFloat(normalized);
  if (Number.isNaN(parsed)) {
    throw new Error(`Invalid timestamp format: '${timeStr}'`);
  }

  return parsed;
}

/**
 * Format seconds to time string (MM:SS.mmm or HH:MM:SS.mmm)
 */
export function formatTime(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = (seconds % 60).toFixed(3);
  if (h > 0) {
    return `${h.toString().padStart(2, '0')}:${m.toString().padStart(2, '0')}:${s.padStart(6, '0')}`;
  }
  return `${m.toString().padStart(2, '0')}:${s.padStart(6, '0')}`;
}

/**
 * Adjust a timestamp based on silence offsets
 */
export function adjustTimestamp(timeStr: string, offsets: SegmentOffset[]): string {
  const t = parseTime(timeStr);
  let offset = 0;
  for (const seg of offsets) {
    if (t >= seg.min_time) {
      offset = seg.offset;
    } else {
      break;
    }
  }
  return formatTime(t + offset);
}

/**
 * Format duration in seconds to human-readable string (e.g., "10:30" or "1:05:30")
 */
export function formatDuration(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) {
    return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
  }
  return `${m}:${s.toString().padStart(2, '0')}`;
}

/**
 * Calculate total duration from an array of segments with start/end times
 */
export function calculateSegmentsDuration(segments: { start: string; end: string }[]): number {
  return segments.reduce((total, seg) => {
    const start = parseTime(seg.start);
    const end = parseTime(seg.end);
    return total + (end - start);
  }, 0);
}

/**
 * Composable for time formatting utilities
 */
export function useTimeFormat() {
  return {
    parseTime,
    formatTime,
    adjustTimestamp,
    formatDuration,
    calculateSegmentsDuration,
  };
}
