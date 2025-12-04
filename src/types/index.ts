export interface TranscriptSegment {
  start: string;
  end: string;
  text: string;
  speaker: string;
}

export interface Clip {
  start: string;
  end: string;
  title: string;
  reason: string;
}

export interface AudioInfo {
  path: string;
  size: number;
}

export interface SilenceInterval {
  start: number;
  end: number;
  duration: number;
}
