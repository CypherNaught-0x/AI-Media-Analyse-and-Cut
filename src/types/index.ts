export interface TranscriptSegment {
  start: string;
  end: string;
  text: string;
  speaker: string;
}

export interface Clip {
  segments: { start: string; end: string }[];
  title: string;
  reason: string;
  start?: string; // Deprecated, kept for backward compatibility
  end?: string;   // Deprecated, kept for backward compatibility
}

export interface AudioInfo {
  path: string;
  size: number;
  duration: number;
}

export interface SilenceInterval {
  start: number;
  end: number;
  duration: number;
}

export interface SegmentOffset {
  min_time: number;
  offset: number;
}

export interface ProcessedAudio {
  path: string;
  silence_intervals: SilenceInterval[];
  offsets: SegmentOffset[];
}
