export type TranscriptionBackend = 'llm' | 'parakeet' | 'hybrid' | 'hybrid-merge';

export interface TranscriptWord {
  start: string;
  end: string;
  text: string;
  speaker?: string;
}

export type TranscriptAlternativeSource = 'parakeet' | 'google';

export interface TranscriptAlternative {
  source: TranscriptAlternativeSource;
  text: string;
  speaker?: string;
  similarityScore?: number;
}

export type TranscriptMergeStatus = 'matched' | 'conflict' | 'missing_google' | 'missing_parakeet';

export interface TranscriptSegment {
  start: string;
  end: string;
  text: string;
  speaker: string;
  words?: TranscriptWord[];
  alternatives?: TranscriptAlternative[];
  mergeStatus?: TranscriptMergeStatus;
  activeSource?: TranscriptAlternativeSource;
  similarityScore?: number;
}

export interface Clip {
  segments: { start: string; end: string }[];
  title: string;
  reason: string;
  start?: string; // Deprecated, kept for backward compatibility
  end?: string;   // Deprecated, kept for backward compatibility
}

export interface ClipExportPayload {
  clips: Clip[];
  includeSubtitles: boolean;
  fastMode: boolean;
  trimBoundarySilence: boolean;
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

// Podcast Generator Types
export type PodcastSegmentType = 'content' | 'voiceover';

export interface PodcastSegment {
  start: string;
  end: string;
  text: string;
  speaker: string;
  type: PodcastSegmentType; // 'content' = actual audio, 'voiceover' = suggested transition text
  includeReason?: string;
  transitionNote?: string; // For voiceover segments: the suggested text to bridge topics
}

export interface PodcastScript {
  title: string;
  summary: string;
  segments: PodcastSegment[];
  totalDuration: number;
}

export interface PodcastSettings {
  minDuration: number; // in seconds
  maxDuration: number; // in seconds
  startPadding: number; // seconds to add before each segment
  endPadding: number; // seconds to add after each segment
  introPath?: string;
  outroPath?: string;
}
