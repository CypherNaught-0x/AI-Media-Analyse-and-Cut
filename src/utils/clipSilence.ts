import { formatTime, parseTime } from '../composables/useTimeFormat';
import type { ClipTimeSegment } from './clips';
import type { SilenceInterval } from '../types';

const EPSILON = 0.001;

function findLeadingBoundarySilence(
  segmentStart: number,
  silenceIntervals: SilenceInterval[]
): SilenceInterval | undefined {
  return silenceIntervals.find(
    ({ start, end }) =>
      segmentStart >= start - EPSILON && segmentStart < end - EPSILON
  );
}

function findTrailingBoundarySilence(
  segmentEnd: number,
  silenceIntervals: SilenceInterval[]
): SilenceInterval | undefined {
  return silenceIntervals.find(
    ({ start, end }) =>
      segmentEnd > start + EPSILON && segmentEnd <= end + EPSILON
  );
}

export function trimClipBoundarySilence(
  segments: ClipTimeSegment[],
  silenceIntervals: SilenceInterval[]
): ClipTimeSegment[] {
  const trimmedSegments = segments.map((segment) => ({ ...segment }));

  if (trimmedSegments.length === 0 || silenceIntervals.length === 0) {
    return trimmedSegments;
  }

  const firstSegment = trimmedSegments[0];
  const lastSegment = trimmedSegments[trimmedSegments.length - 1];

  const firstSegmentStart = parseTime(firstSegment.start);
  const firstSegmentEnd = parseTime(firstSegment.end);
  const leadingSilence = findLeadingBoundarySilence(
    firstSegmentStart,
    silenceIntervals
  );

  if (leadingSilence) {
    const adjustedStart = leadingSilence.end;
    if (adjustedStart < firstSegmentEnd - EPSILON) {
      firstSegment.start = formatTime(adjustedStart);
    }
  }

  const lastSegmentStart = parseTime(lastSegment.start);
  const lastSegmentEnd = parseTime(lastSegment.end);
  const trailingSilence = findTrailingBoundarySilence(
    lastSegmentEnd,
    silenceIntervals
  );

  if (trailingSilence) {
    const adjustedEnd = trailingSilence.start;
    if (adjustedEnd > lastSegmentStart + EPSILON) {
      lastSegment.end = formatTime(adjustedEnd);
    }
  }

  return trimmedSegments;
}
