import type {
    FaceSearch,
    LastAnalyzedSettings,
    ShortClip,
    LocalEngine,
    TranscriptSegment,
    TranscriptWorkspaceState,
    TranscriptionBackend,
} from '../types';
import { isLocalEngine, migrateTranscriptionBackend } from '../types';
import type { FaceOverride } from '../bindings';
import { normalizeFaceOverrides, normalizeFaceSearch, normalizeShortClips } from './shortClips';

/**
 * The viral clips of a recording, saved with it: the clips as found and
 * edited, the user's word on faces and the last face search.
 */
export interface SavedViralClips {
    clips: ShortClip[];
    faces: FaceOverride[];
    faceSearch: FaceSearch | null;
}

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
    speakerOrder?: string[];
    lastAnalyzedSettings?: LastAnalyzedSettings;
    rawParakeetSegments?: TranscriptSegment[];
    parakeetCacheKey?: string;
    transcriptionBackend?: TranscriptionBackend;
    localEngine?: LocalEngine;
    parakeetModelPath?: string;
    sortformerModelPath?: string;
    viralClips?: SavedViralClips;
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
        segments: Array.isArray(sidecar.segments)
            ? (sidecar.segments as TranscriptSegment[])
            : undefined,
        translations:
            sidecar.translations && typeof sidecar.translations === 'object'
                ? (sidecar.translations as Record<string, TranscriptSegment[]>)
                : undefined,
        currentLanguage:
            typeof sidecar.currentLanguage === 'string' ? sidecar.currentLanguage : undefined,
        targetLanguage:
            typeof sidecar.targetLanguage === 'string' ? sidecar.targetLanguage : undefined,
        context: typeof sidecar.context === 'string' ? sidecar.context : undefined,
        glossary: typeof sidecar.glossary === 'string' ? sidecar.glossary : undefined,
        speakerCount:
            typeof sidecar.speakerCount === 'number' || sidecar.speakerCount === null
                ? (sidecar.speakerCount as number | null)
                : undefined,
        removeFillerWords:
            typeof sidecar.removeFillerWords === 'boolean' ? sidecar.removeFillerWords : undefined,
        trimSilence: typeof sidecar.trimSilence === 'boolean' ? sidecar.trimSilence : undefined,
        speakerOrder: Array.isArray(sidecar.speakerOrder)
            ? (sidecar.speakerOrder as string[])
            : undefined,
        lastAnalyzedSettings:
            sidecar.lastAnalyzedSettings && typeof sidecar.lastAnalyzedSettings === 'object'
                ? {
                      ...defaultLastAnalyzedSettings,
                      ...(sidecar.lastAnalyzedSettings as Partial<LastAnalyzedSettings>),
                  }
                : undefined,
        rawParakeetSegments: Array.isArray(sidecar.rawParakeetSegments)
            ? (sidecar.rawParakeetSegments as TranscriptSegment[])
            : undefined,
        parakeetCacheKey:
            typeof sidecar.parakeetCacheKey === 'string' ? sidecar.parakeetCacheKey : undefined,
        // Sidecars written before the pipeline/engine split stored the engine as
        // the pipeline ('parakeet' / 'crisper'); migrate rather than discard.
        transcriptionBackend: migrateTranscriptionBackend(sidecar.transcriptionBackend)?.backend,
        localEngine: isLocalEngine(sidecar.localEngine)
            ? sidecar.localEngine
            : migrateTranscriptionBackend(sidecar.transcriptionBackend)?.localEngine,
        parakeetModelPath:
            typeof sidecar.parakeetModelPath === 'string' ? sidecar.parakeetModelPath : undefined,
        sortformerModelPath:
            typeof sidecar.sortformerModelPath === 'string'
                ? sidecar.sortformerModelPath
                : undefined,
        viralClips: parseViralClips(sidecar.viralClips),
    };
}

function parseViralClips(raw: unknown): SavedViralClips | undefined {
    if (!raw || typeof raw !== 'object') return undefined;
    const saved = raw as Record<string, unknown>;
    return {
        clips: normalizeShortClips(saved.clips),
        faces: normalizeFaceOverrides(saved.faces),
        faceSearch: normalizeFaceSearch(saved.faceSearch),
    };
}

/**
 * The sidecar written next to a recording. `viralClips` is kept only when
 * there is something in it.
 */
export function buildTranscriptSidecar(
    transcriptWorkspace: TranscriptWorkspaceState,
    viralClips?: SavedViralClips,
) {
    const hasViralClips =
        !!viralClips &&
        (viralClips.clips.length > 0 ||
            viralClips.faces.length > 0 ||
            viralClips.faceSearch !== null);
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
        speakerOrder: transcriptWorkspace.speakerOrder,
        lastAnalyzedSettings: transcriptWorkspace.lastAnalyzedSettings,
        rawParakeetSegments: transcriptWorkspace.rawParakeetSegments,
        parakeetCacheKey: transcriptWorkspace.parakeetCacheKey,
        transcriptionBackend: transcriptWorkspace.settingsSnapshot.transcriptionBackend,
        localEngine: transcriptWorkspace.settingsSnapshot.localEngine,
        parakeetModelPath: transcriptWorkspace.settingsSnapshot.parakeetModelPath,
        sortformerModelPath: transcriptWorkspace.settingsSnapshot.sortformerModelPath,
        ...(hasViralClips ? { viralClips } : {}),
    };
}
