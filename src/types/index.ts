export interface TranscriptSegment {
  start: string;
  end: string;
  text: string;
  speaker: string;
}

export interface AudioInfo {
  path: string;
  size: number;
}
