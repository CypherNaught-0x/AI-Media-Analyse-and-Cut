import type { TranscriptSegment } from '../types';

type TimestampAdjuster = (timestamp: string) => string;

const START_KEYS = ['start', 'point', 'timestamp', 'time', 'begin'] as const;
const END_KEYS = ['end', 'stop', 'finish'] as const;
const SPEAKER_KEYS = ['speaker', 'speakerName', 'name'] as const;
const TEXT_KEYS = ['text', 'transcript', 'content'] as const;

function pickString(
  value: Record<string, unknown>,
  keys: readonly string[],
): string | undefined {
  for (const key of keys) {
    const candidate = value[key];
    if (typeof candidate === 'string' && candidate.trim().length > 0) {
      return candidate.trim();
    }
  }

  return undefined;
}

function describeKeys(value: Record<string, unknown>): string {
  const keys = Object.keys(value);
  return keys.length > 0 ? keys.join(', ') : '<none>';
}

function requireField(
  value: Record<string, unknown>,
  keys: readonly string[],
  label: string,
  index: number,
): string {
  const field = pickString(value, keys);
  if (field) {
    return field;
  }

  throw new Error(
    `Segment ${index + 1} is missing required '${label}' field. Available keys: ${describeKeys(value)}`,
  );
}

export function normalizeTranscriptSegments(
  raw: unknown,
  adjustTimestamp?: TimestampAdjuster,
): TranscriptSegment[] {
  if (!Array.isArray(raw)) {
    throw new Error('Response is not an array');
  }

  return raw.map((segment, index) => {
    if (!segment || typeof segment !== 'object' || Array.isArray(segment)) {
      throw new Error(`Segment ${index + 1} is not an object`);
    }

    const record = segment as Record<string, unknown>;
    const start = requireField(record, START_KEYS, 'start', index);
    const end = requireField(record, END_KEYS, 'end', index);
    const speaker = requireField(record, SPEAKER_KEYS, 'speaker', index);
    const text = requireField(record, TEXT_KEYS, 'text', index);

    return {
      start: adjustTimestamp ? adjustTimestamp(start) : start,
      end: adjustTimestamp ? adjustTimestamp(end) : end,
      speaker,
      text,
    };
  });
}
