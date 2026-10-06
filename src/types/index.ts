import type { ClipRatings, ClipRole, ClipSignals, FaceOverride, Intensity } from '../bindings';
// IPC payloads generated from the Rust commands (src/bindings.ts) are the
// source of truth for the shapes the backend sends and accepts.
export type {
    ClipCandidate,
    ClipRatings,
    ClipRole,
    ClipSignals,
    AppleSpeechStatus,
    AudioChunk,
    AudioInfo,
    CrisperEnvironmentStatus,
    ProcessedAudio,
    SegmentOffset,
    SilenceInterval,
} from '../bindings';

/**
 * Which pipeline produces the transcript. Orthogonal to [`LocalEngine`]: every
 * pipeline except `llm` runs the selected local engine, and the hybrid stages
 * layer an LLM pass on top of whatever that engine produced.
 */
export type TranscriptionBackend = 'llm' | 'local' | 'hybrid' | 'hybrid-merge';

/** The on-device model that produces the timed words. */
export type LocalEngine = 'parakeet' | 'crisper' | 'apple-speech';

export const TRANSCRIPTION_BACKENDS: TranscriptionBackend[] = [
    'llm',
    'local',
    'hybrid',
    'hybrid-merge',
];

export const LOCAL_ENGINES: LocalEngine[] = ['parakeet', 'crisper', 'apple-speech'];

export function isLocalEngine(value: unknown): value is LocalEngine {
    return typeof value === 'string' && (LOCAL_ENGINES as string[]).includes(value);
}

export const LOCAL_ENGINE_LABELS: Record<LocalEngine, string> = {
    parakeet: 'Parakeet',
    crisper: 'CrisperWhisper',
    'apple-speech': 'Apple Speech',
};

/** True when the pipeline needs a local engine to run. */
export function usesLocalEngine(backend: TranscriptionBackend): boolean {
    return backend !== 'llm';
}

/** True when the pipeline calls a remote LLM. */
export function usesRemoteModel(backend: TranscriptionBackend): boolean {
    return backend !== 'local';
}

/**
 * Legacy persisted values folded the engine into the pipeline: `parakeet` and
 * `crisper` were pipelines in their own right, and `hybrid`/`hybrid-merge`
 * implied Parakeet. Normalise them into the split representation.
 */
export function migrateTranscriptionBackend(value: unknown): {
    backend: TranscriptionBackend;
    localEngine?: LocalEngine;
} | null {
    if (typeof value !== 'string') return null;

    switch (value) {
        case 'parakeet':
            return { backend: 'local', localEngine: 'parakeet' };
        case 'crisper':
            return { backend: 'local', localEngine: 'crisper' };
        case 'llm':
        case 'local':
        case 'hybrid':
        case 'hybrid-merge':
            // Hybrids previously always meant Parakeet; keep that behaviour.
            return { backend: value };
        default:
            return null;
    }
}

/**
 * CrisperWhisper 2.0 is published for English and German only, so the language
 * picker is deliberately closed rather than a free-text field.
 */
export type CrisperLanguage = 'en' | 'de';

/** `verbatim` keeps what was said; `intended` returns the cleaned-up reading. */
export type CrisperMode = 'verbatim' | 'intended';

export const CRISPER_LANGUAGES: { value: CrisperLanguage; label: string }[] = [
    { value: 'en', label: 'English' },
    { value: 'de', label: 'German' },
];

export const CRISPER_MODELS = ['large', 'medium', 'turbo', 'small'] as const;

export interface TranscriptWord {
    start: string;
    end: string;
    text: string;
    speaker?: string | null;
}

/**
 * Where a merged hypothesis came from. `local` is whichever local engine ran —
 * the wire value used to be `parakeet`, which the Rust side still accepts as an
 * alias so previously saved transcripts keep loading.
 */
export type TranscriptAlternativeSource = 'local' | 'google';

export function migrateAlternativeSource(value: unknown): TranscriptAlternativeSource | null {
    if (value === 'local' || value === 'google') return value;
    if (value === 'parakeet') return 'local';
    return null;
}

export interface TranscriptAlternative {
    source: TranscriptAlternativeSource;
    text: string;
    speaker?: string | null;
    similarityScore?: number | null;
}

export type TranscriptMergeStatus = 'matched' | 'conflict' | 'missing_google' | 'missing_local';

export function migrateMergeStatus(value: unknown): TranscriptMergeStatus | null {
    if (value === 'matched' || value === 'conflict' || value === 'missing_google') return value;
    if (value === 'missing_local' || value === 'missing_parakeet') return 'missing_local';
    return null;
}

export interface TranscriptSegment {
    start: string;
    end: string;
    text: string;
    speaker: string;
    words?: TranscriptWord[] | null;
    alternatives?: TranscriptAlternative[] | null;
    mergeStatus?: TranscriptMergeStatus | null;
    activeSource?: TranscriptAlternativeSource | null;
    similarityScore?: number | null;
    reviewResolved?: boolean;
}

export interface Clip {
    segments: { start: string; end: string }[];
    title: string;
    reason: string;
    start?: string; // Deprecated, kept for backward compatibility
    end?: string; // Deprecated, kept for backward compatibility
}

export interface ClipExportPayload {
    clips: Clip[];
    includeSubtitles: boolean;
    fastMode: boolean;
    trimBoundarySilence: boolean;
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
    localEngine: LocalEngine;
    parakeetModelPath: string;
    sortformerModelPath: string;
    /**
     * Serialized CrisperWhisper options that affect the transcript. Kept as one
     * opaque string so adding a model option does not require touching session
     * persistence and its migrations.
     */
    crisperSignature: string;
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
    speakerOrder: string[];
    lastAnalyzedSettings: LastAnalyzedSettings;
    rawParakeetSegments: TranscriptSegment[];
    parakeetCacheKey: string;
    settingsSnapshot: {
        glossary: string;
        transcriptionBackend: TranscriptionBackend;
        localEngine: LocalEngine;
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

/** One source range of a short, in seconds on the source timeline. */
export interface ShortClipRange {
    start: number;
    end: number;
    role: ClipRole;
}

/** A short-form clip candidate as the Clips tab shows and edits it. */
export interface ShortClip {
    /** Stable key for rendering and selection. */
    id: string;
    title: string;
    hookLine: string;
    reason: string;
    /** In playback order. */
    ranges: ShortClipRange[];
    /** 0-100; null for clips saved before clips were scored. */
    score: number | null;
    ratings: ClipRatings | null;
    signals: ClipSignals | null;
    looped: boolean;
    /** Included in "Export selected". */
    selected: boolean;
}

export interface ViralClipsWorkspaceState {
    count: number;
    minDuration: number;
    maxDuration: number;
    topic: string;
    allowSplicing: boolean;
    /** Ask for looped shorts (opener = resolution, closing leads back to it). */
    looped: boolean;
    /** Export 9:16 videos that follow the active speaker. */
    vertical: boolean;
    /** Burn word-by-word captions into 9:16 exports. */
    captions: boolean;
    /** How hard to tighten pauses, fillers and stutters on export. */
    intensity: Intensity;
    /** The user's word on faces in 9:16 exports: ignored ones, named ones. */
    faces: FaceOverride[];
    clips: ShortClip[];
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
