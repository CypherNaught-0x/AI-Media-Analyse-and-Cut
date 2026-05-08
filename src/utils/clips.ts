import type { Clip } from '../types';
import { formatTime, parseTime } from '../composables/useTimeFormat';

export interface ClipTimeSegment {
  start: string;
  end: string;
}

type RawClipTimeSegment = {
  start?: unknown;
  end?: unknown;
};

type RawClip = {
  segments?: unknown;
  start?: unknown;
  end?: unknown;
  title?: unknown;
  reason?: unknown;
};

function normalizeTimestamp(value: unknown, field: string): string {
  let seconds: number;

  if (typeof value === 'number') {
    seconds = value;
  } else if (typeof value === 'string') {
    seconds = parseTime(value);
  } else {
    throw new Error(`Invalid clip ${field}: expected timestamp string or number`);
  }

  if (!Number.isFinite(seconds)) {
    throw new Error(`Invalid clip ${field}: timestamp must be finite`);
  }

  return formatTime(seconds);
}

export function normalizeClipTimeSegments(segments: unknown): ClipTimeSegment[] {
  if (!Array.isArray(segments)) {
    throw new Error('Invalid clip segments: expected an array');
  }

  return segments.map((segment, index) => {
    if (!segment || typeof segment !== 'object') {
      throw new Error(`Invalid clip segment at index ${index}: expected an object`);
    }

    const { start, end } = segment as RawClipTimeSegment;
    return {
      start: normalizeTimestamp(start, `segment ${index} start`),
      end: normalizeTimestamp(end, `segment ${index} end`),
    };
  });
}

export function normalizeClip(rawClip: RawClip): Clip {
  const rawSegments = rawClip.segments ?? [{ start: rawClip.start, end: rawClip.end }];
  const clip: Clip = {
    segments: normalizeClipTimeSegments(rawSegments),
    title: typeof rawClip.title === 'string' ? rawClip.title : '',
    reason: typeof rawClip.reason === 'string' ? rawClip.reason : '',
  };

  if (rawClip.start !== undefined) {
    clip.start = normalizeTimestamp(rawClip.start, 'start');
  }

  if (rawClip.end !== undefined) {
    clip.end = normalizeTimestamp(rawClip.end, 'end');
  }

  return clip;
}

export function normalizeClips(rawClips: unknown): Clip[] {
  if (!Array.isArray(rawClips)) {
    throw new Error('Response is not an array');
  }

  return rawClips.map((clip, index) => {
    if (!clip || typeof clip !== 'object') {
      throw new Error(`Invalid clip at index ${index}: expected an object`);
    }

    return normalizeClip(clip as RawClip);
  });
}
