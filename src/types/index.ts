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
  reviewResolved?: boolean;
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

export interface AudioChunk {
  path: string;
  start_offset: number;
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

export interface LastAnalyzedSettings {
  context: string;
  glossary: string;
  speakerCount: number | null;
  removeFillerWords: boolean;
  trimSilence: boolean;
  transcriptionBackend: TranscriptionBackend;
  parakeetModelPath: string;
  sortformerModelPath: string;
}

export interface TranscriptWorkspaceState {
  inputPath: string;
  segments: TranscriptSegment[];
  translations: Record<string, TranscriptSegment[]>;
  currentLanguage: string;
  targetLanguage: string;
  context: string;
  speakerCount: number | null;
  removeFillerWords: boolean;
  trimSilence: boolean;
  useAdvancedAlignment: boolean;
  speakerOrder: string[];
  lastAnalyzedSettings: LastAnalyzedSettings;
  rawParakeetSegments: TranscriptSegment[];
  parakeetCacheKey: string;
  settingsSnapshot: {
    glossary: string;
    transcriptionBackend: TranscriptionBackend;
    parakeetModelPath: string;
    sortformerModelPath: string;
  };
}

export interface ClipWorkspaceState {
  count: number;
  minDuration: number;
  maxDuration: number;
  topic: string;
  allowSplicing: boolean;
  clips: Clip[];
  lastExportPath: string;
  includeSubtitles: boolean;
  fastMode: boolean;
  trimBoundarySilence: boolean;
  selectedClipIndices: number[];
}

export interface ViralClipsWorkspaceState {
  count: number;
  minDuration: number;
  maxDuration: number;
  topic: string;
  allowSplicing: boolean;
  clips: Clip[];
  lastExportPath: string;
  trimBoundarySilence: boolean;
}

export interface PodcastWorkspaceState {
  minDurationMinutes: number;
  maxDurationMinutes: number;
  startPadding: number;
  endPadding: number;
  introPath: string;
  outroPath: string;
  podcastScript: PodcastScript | null;
  lastExportPath: string;
}

export interface EditSessionV1 {
  version: 1;
  savedAt: string;
  transcriptWorkspace: TranscriptWorkspaceState;
  clipWorkspace: ClipWorkspaceState;
  viralClipsWorkspace: ViralClipsWorkspaceState;
  podcastWorkspace: PodcastWorkspaceState;
}
