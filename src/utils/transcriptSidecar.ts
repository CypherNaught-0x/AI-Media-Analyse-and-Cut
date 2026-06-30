import type {
  LastAnalyzedSettings,
  TranscriptSegment,
  TranscriptWorkspaceState,
  TranscriptionBackend,
} from '../types';

export interface ParsedTranscriptSidecar {
  segments?: TranscriptSegment[];
  translations?: Record<string, TranscriptSegment[]>;
  currentLanguage?: string;
  targetLanguage?: string;
  context?: string;
  glossary?: string;
  speakerCount?: number | null;
  removeFillerWords?: boolean;
  trimSilence?: boolean;
  useAdvancedAlignment?: boolean;
  speakerOrder?: string[];
  lastAnalyzedSettings?: LastAnalyzedSettings;
  rawParakeetSegments?: TranscriptSegment[];
  parakeetCacheKey?: string;
  transcriptionBackend?: TranscriptionBackend;
  parakeetModelPath?: string;
  sortformerModelPath?: string;
}

const TRANSCRIPTION_BACKENDS: TranscriptionBackend[] = ['llm', 'parakeet', 'hybrid', 'hybrid-merge'];

function isTranscriptionBackend(value: unknown): value is TranscriptionBackend {
  return typeof value === 'string' && TRANSCRIPTION_BACKENDS.includes(value as TranscriptionBackend);
}

export function parseTranscriptSidecar(
  rawContent: string,
  defaultLastAnalyzedSettings: LastAnalyzedSettings,
): ParsedTranscriptSidecar | null {
  const parsed = JSON.parse(rawContent);

  if (Array.isArray(parsed)) {
    return { segments: parsed };
  }

  if (!parsed || typeof parsed !== 'object') {
    return null;
  }

  const sidecar = parsed as Record<string, unknown>;
  return {
    segments: Array.isArray(sidecar.segments) ? (sidecar.segments as TranscriptSegment[]) : undefined,
    translations:
      sidecar.translations && typeof sidecar.translations === 'object'
        ? (sidecar.translations as Record<string, TranscriptSegment[]>)
        : undefined,
    currentLanguage: typeof sidecar.currentLanguage === 'string' ? sidecar.currentLanguage : undefined,
    targetLanguage: typeof sidecar.targetLanguage === 'string' ? sidecar.targetLanguage : undefined,
    context: typeof sidecar.context === 'string' ? sidecar.context : undefined,
    glossary: typeof sidecar.glossary === 'string' ? sidecar.glossary : undefined,
    speakerCount:
      typeof sidecar.speakerCount === 'number' || sidecar.speakerCount === null
        ? (sidecar.speakerCount as number | null)
        : undefined,
    removeFillerWords:
      typeof sidecar.removeFillerWords === 'boolean' ? sidecar.removeFillerWords : undefined,
    trimSilence: typeof sidecar.trimSilence === 'boolean' ? sidecar.trimSilence : undefined,
    useAdvancedAlignment:
      typeof sidecar.useAdvancedAlignment === 'boolean' ? sidecar.useAdvancedAlignment : undefined,
    speakerOrder: Array.isArray(sidecar.speakerOrder) ? (sidecar.speakerOrder as string[]) : undefined,
    lastAnalyzedSettings:
      sidecar.lastAnalyzedSettings && typeof sidecar.lastAnalyzedSettings === 'object'
        ? { ...defaultLastAnalyzedSettings, ...(sidecar.lastAnalyzedSettings as Partial<LastAnalyzedSettings>) }
        : undefined,
    rawParakeetSegments: Array.isArray(sidecar.rawParakeetSegments)
      ? (sidecar.rawParakeetSegments as TranscriptSegment[])
      : undefined,
    parakeetCacheKey:
      typeof sidecar.parakeetCacheKey === 'string' ? sidecar.parakeetCacheKey : undefined,
    transcriptionBackend: isTranscriptionBackend(sidecar.transcriptionBackend)
      ? sidecar.transcriptionBackend
      : undefined,
    parakeetModelPath:
      typeof sidecar.parakeetModelPath === 'string' ? sidecar.parakeetModelPath : undefined,
    sortformerModelPath:
      typeof sidecar.sortformerModelPath === 'string' ? sidecar.sortformerModelPath : undefined,
  };
}

export function buildTranscriptSidecar(transcriptWorkspace: TranscriptWorkspaceState) {
  return {
    segments: transcriptWorkspace.segments,
    translations: transcriptWorkspace.translations,
    currentLanguage: transcriptWorkspace.currentLanguage,
    targetLanguage: transcriptWorkspace.targetLanguage,
    context: transcriptWorkspace.context,
    glossary: transcriptWorkspace.settingsSnapshot.glossary,
    speakerCount: transcriptWorkspace.speakerCount,
    removeFillerWords: transcriptWorkspace.removeFillerWords,
    trimSilence: transcriptWorkspace.trimSilence,
    useAdvancedAlignment: transcriptWorkspace.useAdvancedAlignment,
    speakerOrder: transcriptWorkspace.speakerOrder,
    lastAnalyzedSettings: transcriptWorkspace.lastAnalyzedSettings,
    rawParakeetSegments: transcriptWorkspace.rawParakeetSegments,
    parakeetCacheKey: transcriptWorkspace.parakeetCacheKey,
    transcriptionBackend: transcriptWorkspace.settingsSnapshot.transcriptionBackend,
    parakeetModelPath: transcriptWorkspace.settingsSnapshot.parakeetModelPath,
    sortformerModelPath: transcriptWorkspace.settingsSnapshot.sortformerModelPath,
  };
}
