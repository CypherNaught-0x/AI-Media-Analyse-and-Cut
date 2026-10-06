<script lang="ts">
// Module-scoped guard so FFmpeg is initialized only once per app session
// rather than on every remount of the Home view (e.g. Home -> Settings -> Home).
let ffmpegInitialized = false;

// App.vue keeps this view alive by name (<keep-alive include="Home">).
export default { name: 'Home' };
</script>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { ask } from '@tauri-apps/plugin-dialog';
import { useRouter } from 'vue-router';
import ViralClipsGenerator from '../components/ViralClipsGenerator.vue';
import PodcastGenerator from '../components/PodcastGenerator.vue';
import ErrorOverlay from '../components/ErrorOverlay.vue';
import HomeSourcePanel from '../components/HomeSourcePanel.vue';
import TranscriptWorkspacePanel from '../components/TranscriptWorkspacePanel.vue';
import WorkspaceTabs from '../components/WorkspaceTabs.vue';
import type {
    AudioInfo,
    Clip,
    ClipWorkspaceState,
    LastAnalyzedSettings,
    PodcastWorkspaceState,
    ProcessedAudio,
    SilenceInterval,
    TranscriptSegment,
    TranscriptWorkspaceState,
    ViralClipsWorkspaceState,
} from '../types';
import { LOCAL_ENGINE_LABELS, usesLocalEngine, usesRemoteModel } from '../types';
import StatusBar from '../components/StatusBar.vue';
import { useSettings } from '../composables/useSettings';
import { appleSpeechLocale } from '../composables/useAppleSpeech';
import { useHomeSessionPersistence } from '../composables/useHomeSessionPersistence';
import { adjustTimestamp, formatTime, parseTime } from '../composables/useTimeFormat';
import { beginRun, isRunCancelled } from '../composables/useRunCancellation';
import { parseTranscriptResponse } from '../utils/transcriptParsing';
import { realignTranslation } from '../utils/translationAlignment';
import { buildTranscriptSidecar, parseTranscriptSidecar } from '../utils/transcriptSidecar';
import {
    createDefaultClipWorkspaceState,
    createDefaultLastAnalyzedSettings,
    createDefaultPodcastWorkspaceState,
    createDefaultViralClipsWorkspaceState,
} from '../utils/editSession';

import { adjustSegmentsWithOffsets } from '../utils/transcriptOffsets';
import { appendFileNameSuffix } from '../utils/filePath';
import { commands } from '../bindings';
import { errorMessage } from '../utils/appError';

const AUTOSAVE_DEBOUNCE_MS = 750;

const router = useRouter();
const { settings, apiKeyStored } = useSettings();

/**
 * Payloads of the backend's shared `progress` event: elapsed seconds, a plain
 * message, or an object with a percentage and/or a message.
 */
type ProgressPayload =
    | number
    | string
    | {
          percentage?: number;
          etaSeconds?: number;
          current_clip?: number;
          total_clips?: number;
          message?: string;
      };

type WorkspaceSettings = TranscriptWorkspaceState['settingsSnapshot'];
const WORKSPACE_SETTING_KEYS = [
    'glossary',
    'transcriptionBackend',
    'localEngine',
    'parakeetModelPath',
    'sortformerModelPath',
] as const satisfies readonly (keyof WorkspaceSettings)[];

function defaultWorkspaceSettings(): WorkspaceSettings {
    return {
        glossary: settings.value.glossary ?? '',
        transcriptionBackend: settings.value.transcriptionBackend ?? 'llm',
        localEngine: settings.value.localEngine ?? 'parakeet',
        parakeetModelPath: settings.value.parakeetModelPath ?? '',
        sortformerModelPath: settings.value.sortformerModelPath ?? '',
    };
}

// The open project's analysis choices. They start from the app defaults in
// Settings and are restored from a session or sidecar, but loading a file never
// writes back into the defaults.
const workspaceSettings = ref<WorkspaceSettings>(defaultWorkspaceSettings());

// Changing a default in Settings is an explicit choice, so it also applies to
// the open project.
for (const key of WORKSPACE_SETTING_KEYS) {
    watch(
        () => settings.value[key],
        (value) => {
            (workspaceSettings.value as Record<typeof key, unknown>)[key] = value;
        },
    );
}

const status = ref('Initializing...');
const isProcessing = ref(false);
const isCancelling = ref(false);
const cancelGeneration = ref(0);
const activeRunId = ref<number | null>(null);

// Error overlay state
const showErrorOverlay = ref(false);
const errorDetails = ref({
    message: '',
    rawResponse: '',
    parseError: '',
});
const progressPercentage = ref<number | null>(null);
const progressEtaSeconds = ref<number | null>(null);
const executionHistory = ref<
    { type: string; inputSize: number; duration: number; timestamp: number }[]
>([]);
const inputPath = ref('');
const inputPathExists = ref(false);
const extractedAudioPath = ref('');
const activeTab = ref('source');
const segments = ref<TranscriptSegment[]>([]);
const translations = ref<Record<string, TranscriptSegment[]>>({});
const currentLanguage = ref('Original');
const targetLanguage = ref('');
const isTranslating = ref(false);
const removeFillerWords = ref(false);
const trimSilence = ref(true);

const speakerCount = ref<number | null>(null);
const context = ref('');
const clipCount = ref(createDefaultClipWorkspaceState().count);
const clipMinDuration = ref(createDefaultClipWorkspaceState().minDuration);
const clipMaxDuration = ref(createDefaultClipWorkspaceState().maxDuration);
const clipTopic = ref(createDefaultClipWorkspaceState().topic);
const allowSplicing = ref(createDefaultClipWorkspaceState().allowSplicing);
const clips = ref<Clip[]>(createDefaultClipWorkspaceState().clips);
const lastExportPath = ref(createDefaultClipWorkspaceState().lastExportPath);
const includeSubtitles = ref(createDefaultClipWorkspaceState().includeSubtitles);
const fastMode = ref(createDefaultClipWorkspaceState().fastMode);
const clipTrimBoundarySilence = ref(createDefaultClipWorkspaceState().trimBoundarySilence);
const selectedClipIndices = ref<number[]>(createDefaultClipWorkspaceState().selectedClipIndices);
const clipExportSilenceCache = ref<{ path: string; intervals: SilenceInterval[] } | null>(null);
const speakerOrder = ref<string[]>([]);
const viralClipsState = ref<ViralClipsWorkspaceState>(createDefaultViralClipsWorkspaceState());
const podcastWorkspaceState = ref<PodcastWorkspaceState>(createDefaultPodcastWorkspaceState());

const lastAnalyzedSettings = ref<LastAnalyzedSettings>(createDefaultLastAnalyzedSettings());

// Cache of the raw (pre-offset) local-engine output so that changing only
// LLM-side inputs (context, glossary, speaker count) does not re-run the
// expensive local transcription/diarization. The cache is keyed by the
// audio-level inputs it depends on and persisted with the transcript.
//
// The `parakeet` in these names is now a misnomer — the cache holds whichever
// engine ran — but they are part of the saved session schema, so renaming them
// would need a migration for no user-visible gain.
const rawParakeetSegments = ref<TranscriptSegment[]>([]);
const parakeetCacheKey = ref<string>('');

/**
 * The local-engine options that change the transcript itself (CrisperWhisper's
 * model, mode, ...; Apple Speech's language). Kept as one string so it can be
 * compared and persisted without widening the session schema every time an
 * option is added; the persisted field is still named `crisperSignature`, from
 * when only CrisperWhisper had such options.
 */
function currentEngineSignature(): string {
    // Self-contained rather than reusing the computed below, so this stays safe
    // to call from anywhere during setup.
    const { transcriptionBackend, localEngine } = workspaceSettings.value;
    if (!usesLocalEngine(transcriptionBackend)) return '';
    if (localEngine === 'apple-speech') {
        return JSON.stringify({
            locale: appleSpeechLocale(settings.value.appleSpeechLocale),
            diarize: settings.value.appleSpeechDiarize,
        });
    }
    if (localEngine !== 'crisper') return '';
    return JSON.stringify({
        model: settings.value.crisperModel,
        language: settings.value.crisperLanguage,
        mode: settings.value.crisperMode,
        removeFillers: removeFillerWords.value,
        removeVocalEvents: settings.value.crisperRemoveVocalEvents,
        diarize: settings.value.crisperDiarize,
    });
}

function currentParakeetCacheKey(): string {
    return JSON.stringify({
        inputPath: inputPath.value,
        trimSilence: trimSilence.value,
        // The raw local transcript depends on the engine and its options, not
        // on which LLM stage runs afterwards — so switching between local and
        // the hybrids reuses the cache instead of re-transcribing.
        localEngine: workspaceSettings.value.localEngine,
        parakeetModelPath: workspaceSettings.value.parakeetModelPath,
        sortformerModelPath: workspaceSettings.value.sortformerModelPath,
        crisperSignature: currentEngineSignature(),
    });
}

const isLlmOnlyBackend = computed(() => workspaceSettings.value.transcriptionBackend === 'llm');
const hasApiKey = computed(() => apiKeyStored.value);
const localEngineLabel = computed(() => LOCAL_ENGINE_LABELS[workspaceSettings.value.localEngine]);

const hasBackendConfiguration = computed(() => {
    // Local engines need no configuration up front: Parakeet auto-downloads its
    // models and CrisperWhisper reports a fixable error when its Python
    // environment is missing. Only the LLM stages need a key.
    return usesRemoteModel(workspaceSettings.value.transcriptionBackend) ? hasApiKey.value : true;
});

/** Short description of the local engine and its notable settings. */
const localEngineDisplay = computed(() => {
    if (workspaceSettings.value.localEngine === 'crisper') {
        const language = settings.value.crisperLanguage === 'de' ? 'DE' : 'EN';
        return `CrisperWhisper ${settings.value.crisperModel} (${settings.value.crisperMode}, ${language})`;
    }
    if (workspaceSettings.value.localEngine === 'apple-speech') {
        return `Apple Speech (${appleSpeechLocale(settings.value.appleSpeechLocale)})`;
    }
    const usesCustomPaths =
        workspaceSettings.value.parakeetModelPath.trim() ||
        workspaceSettings.value.sortformerModelPath.trim();
    return usesCustomPaths ? 'Parakeet-RS (local)' : 'Parakeet-RS (auto-download)';
});

const currentModelDisplay = computed(() => {
    const backend = workspaceSettings.value.transcriptionBackend;

    if (backend === 'llm') {
        return hasApiKey.value ? settings.value.model : 'No API Key configured';
    }
    if (backend === 'local') {
        return localEngineDisplay.value;
    }

    const prefix = backend === 'hybrid' ? 'Hybrid' : 'Hybrid Merge';
    if (!hasApiKey.value) return `${prefix} (missing API key)`;
    return `${prefix}: ${localEngineLabel.value} + ${settings.value.model}`;
});

const currentEngineLabel = computed(() => {
    return workspaceSettings.value.transcriptionBackend === 'llm'
        ? 'Current Model'
        : 'Current Pipeline';
});
const hasTranscript = computed(() => segments.value.length > 0);
const hasMediaFile = computed(() => inputPath.value.length > 0 && inputPathExists.value);

const workspaceTabs = computed(() => [
    { id: 'source', label: 'Source & Analysis', disabled: false },
    { id: 'transcript', label: 'Transcript', disabled: !hasTranscript.value },
    { id: 'clips', label: 'Viral Clips', disabled: !hasTranscript.value },
    { id: 'podcast', label: 'Podcast', disabled: !hasTranscript.value },
]);

async function refreshExtractedAudioPath() {
    const input = inputPath.value;
    if (!input) {
        extractedAudioPath.value = '';
        return;
    }
    // The seekable preview audio lives in the app's media cache (see
    // prepare_preview_audio). Reuse it if a previous analysis of this exact
    // file produced it; otherwise it is created during analysis.
    try {
        const cached = await commands.cachedPreviewAudio(input);
        if (cached) {
            await grantMediaAccess(cached);
        }
        extractedAudioPath.value = cached ?? '';
    } catch (error) {
        console.error('Failed to check preview audio path:', error);
        extractedAudioPath.value = '';
    }
}
const settingsChanged = computed(() => {
    return (
        workspaceSettings.value.transcriptionBackend !==
            lastAnalyzedSettings.value.transcriptionBackend ||
        workspaceSettings.value.localEngine !== lastAnalyzedSettings.value.localEngine ||
        workspaceSettings.value.parakeetModelPath !==
            lastAnalyzedSettings.value.parakeetModelPath ||
        workspaceSettings.value.sortformerModelPath !==
            lastAnalyzedSettings.value.sortformerModelPath ||
        currentEngineSignature() !== lastAnalyzedSettings.value.crisperSignature ||
        context.value !== lastAnalyzedSettings.value.context ||
        workspaceSettings.value.glossary !== lastAnalyzedSettings.value.glossary ||
        speakerCount.value !== lastAnalyzedSettings.value.speakerCount ||
        removeFillerWords.value !== lastAnalyzedSettings.value.removeFillerWords ||
        trimSilence.value !== lastAnalyzedSettings.value.trimSilence
    );
});

const clipWorkspaceState = computed<ClipWorkspaceState>(() => ({
    count: clipCount.value,
    minDuration: clipMinDuration.value,
    maxDuration: clipMaxDuration.value,
    topic: clipTopic.value,
    allowSplicing: allowSplicing.value,
    clips: clips.value,
    lastExportPath: lastExportPath.value,
    includeSubtitles: includeSubtitles.value,
    fastMode: fastMode.value,
    trimBoundarySilence: clipTrimBoundarySilence.value,
    selectedClipIndices: selectedClipIndices.value,
}));

const transcriptWorkspaceState = computed<TranscriptWorkspaceState>(() => ({
    inputPath: inputPath.value,
    segments: segments.value,
    translations: translations.value,
    currentLanguage: currentLanguage.value,
    targetLanguage: targetLanguage.value,
    context: context.value,
    speakerCount: speakerCount.value,
    removeFillerWords: removeFillerWords.value,
    trimSilence: trimSilence.value,
    speakerOrder: speakerOrder.value,
    lastAnalyzedSettings: lastAnalyzedSettings.value,
    rawParakeetSegments: rawParakeetSegments.value,
    parakeetCacheKey: parakeetCacheKey.value,
    settingsSnapshot: {
        glossary: workspaceSettings.value.glossary,
        transcriptionBackend: workspaceSettings.value.transcriptionBackend,
        localEngine: workspaceSettings.value.localEngine,
        parakeetModelPath: workspaceSettings.value.parakeetModelPath,
        sortformerModelPath: workspaceSettings.value.sortformerModelPath,
    },
}));

function getSpeakerAppearanceOrder(transcriptSegments: TranscriptSegment[]): string[] {
    const seen = new Set<string>();
    const ordered: string[] = [];

    for (const segment of transcriptSegments) {
        if (!seen.has(segment.speaker)) {
            seen.add(segment.speaker);
            ordered.push(segment.speaker);
        }
    }

    return ordered;
}

function syncSpeakerOrder() {
    const appearanceOrder = getSpeakerAppearanceOrder(segments.value);
    const present = new Set(appearanceOrder);
    const preserved = speakerOrder.value.filter((speaker) => present.has(speaker));
    const additions = appearanceOrder.filter((speaker) => !preserved.includes(speaker));
    speakerOrder.value = [...preserved, ...additions];
}

const uniqueSpeakers = computed(() => {
    const present = new Set(segments.value.map((segment) => segment.speaker));
    return speakerOrder.value.filter((speaker) => present.has(speaker));
});

const displaySegments = computed({
    get: () => {
        if (currentLanguage.value === 'Original') return segments.value;
        return translations.value[currentLanguage.value] || segments.value;
    },
    set: (newSegments) => {
        if (currentLanguage.value === 'Original') {
            segments.value = newSegments;
            // Splits, merges and deletes shift indexes; refit every translation
            // onto the new structure by time so they stay aligned.
            translations.value = Object.fromEntries(
                Object.entries(translations.value).map(([lang, translated]) => [
                    lang,
                    realignTranslation(newSegments, translated),
                ]),
            );
        } else {
            // The translated view only edits wording (structure is locked in the
            // editor); timing and speakers always follow the original.
            translations.value[currentLanguage.value] = newSegments.map((segment, index) => ({
                ...segment,
                start: segments.value[index]?.start ?? segment.start,
                end: segments.value[index]?.end ?? segment.end,
                speaker: segments.value[index]?.speaker ?? segment.speaker,
            }));
        }
    },
});

async function updateInputPathExists(path: string) {
    if (!path) {
        inputPathExists.value = false;
        return false;
    }

    try {
        inputPathExists.value = await commands.pathExists(path);
    } catch (error) {
        console.error('Failed to check input path existence:', error);
        inputPathExists.value = false;
    }

    if (inputPathExists.value) {
        await grantMediaAccess(path);
    }

    return inputPathExists.value;
}

async function grantMediaAccess(path: string) {
    if (!path) return;
    // Allow the webview's asset protocol to load this file (and its sibling
    // .ogg) via convertFileSrc, even when it lives outside the static scope.
    try {
        await commands.allowMediaAccess(path);
    } catch (error) {
        console.error('Failed to grant media asset access:', error);
    }
}

function resetClipWorkspaceState() {
    const defaults = createDefaultClipWorkspaceState();
    clipCount.value = defaults.count;
    clipMinDuration.value = defaults.minDuration;
    clipMaxDuration.value = defaults.maxDuration;
    clipTopic.value = defaults.topic;
    allowSplicing.value = defaults.allowSplicing;
    clips.value = defaults.clips;
    lastExportPath.value = defaults.lastExportPath;
    includeSubtitles.value = defaults.includeSubtitles;
    fastMode.value = defaults.fastMode;
    clipTrimBoundarySilence.value = defaults.trimBoundarySilence;
    selectedClipIndices.value = defaults.selectedClipIndices;
    clipExportSilenceCache.value = null;
}

function resetTranscriptWorkspaceState() {
    segments.value = [];
    translations.value = {};
    currentLanguage.value = 'Original';
    targetLanguage.value = '';
    context.value = '';
    speakerCount.value = null;
    removeFillerWords.value = false;
    trimSilence.value = true;
    speakerOrder.value = [];
    lastAnalyzedSettings.value = createDefaultLastAnalyzedSettings();
    rawParakeetSegments.value = [];
    parakeetCacheKey.value = '';
    workspaceSettings.value = defaultWorkspaceSettings();
}

function resetDerivedWorkspaceState() {
    resetClipWorkspaceState();
    viralClipsState.value = createDefaultViralClipsWorkspaceState();
    podcastWorkspaceState.value = createDefaultPodcastWorkspaceState();
}

function applyTranscriptWorkspace(state: TranscriptWorkspaceState) {
    inputPath.value = state.inputPath;
    segments.value = state.segments;
    translations.value = state.translations;
    currentLanguage.value = state.currentLanguage;
    targetLanguage.value = state.targetLanguage;
    context.value = state.context;
    speakerCount.value = state.speakerCount;
    removeFillerWords.value = state.removeFillerWords;
    trimSilence.value = state.trimSilence;
    speakerOrder.value = state.speakerOrder;
    lastAnalyzedSettings.value = state.lastAnalyzedSettings;
    rawParakeetSegments.value = state.rawParakeetSegments ?? [];
    parakeetCacheKey.value = state.parakeetCacheKey ?? '';
    workspaceSettings.value = { ...defaultWorkspaceSettings(), ...state.settingsSnapshot };
}

function applyClipWorkspace(state: ClipWorkspaceState) {
    clipCount.value = state.count;
    clipMinDuration.value = state.minDuration;
    clipMaxDuration.value = state.maxDuration;
    clipTopic.value = state.topic;
    allowSplicing.value = state.allowSplicing;
    clips.value = state.clips;
    lastExportPath.value = state.lastExportPath;
    includeSubtitles.value = state.includeSubtitles;
    fastMode.value = state.fastMode;
    clipTrimBoundarySilence.value = state.trimBoundarySilence;
    selectedClipIndices.value = state.selectedClipIndices;
    clipExportSilenceCache.value = null;
}

const sessionPersistence = useHomeSessionPersistence({
    autosaveDebounceMs: AUTOSAVE_DEBOUNCE_MS,
    status,
    inputPath,
    inputPathExists,
    transcriptWorkspaceState,
    clipWorkspaceState,
    viralClipsState,
    podcastWorkspaceState,
    updateInputPathExists,
    saveTranscript,
    loadTranscript,
    resetTranscriptWorkspaceState,
    resetDerivedWorkspaceState,
    applyTranscriptWorkspace,
    applyClipWorkspace,
});

let unlistenProgress: UnlistenFn | null = null;

onMounted(async () => {
    const history = localStorage.getItem('executionHistory');
    if (history) {
        try {
            executionHistory.value = JSON.parse(history);
        } catch (e) {
            console.error('Failed to parse execution history', e);
        }
    }
    await sessionPersistence.restoreAutosavedSession();

    // Register the progress listener on every mount and keep the returned
    // unlisten handle so onUnmounted can tear it down. Without this, navigating
    // away and back stacks a new listener on each remount.
    try {
        unlistenProgress = await listen<ProgressPayload>('progress', (event) => {
            const payload = event.payload;
            if (typeof payload === 'number') {
                status.value = `Processing... ${payload.toFixed(1)}s`;
                progressEtaSeconds.value = null;
            } else if (typeof payload === 'object') {
                if (payload.percentage !== undefined) {
                    if (progressInterval) {
                        clearInterval(progressInterval);
                        progressInterval = null;
                    }
                    progressPercentage.value = payload.percentage;
                    progressEtaSeconds.value =
                        typeof payload.etaSeconds === 'number' ? payload.etaSeconds : null;
                    let statusMsg = `Processing... ${payload.percentage.toFixed(1)}%`;

                    if (payload.current_clip && payload.total_clips) {
                        statusMsg = `Exporting clip ${payload.current_clip}/${payload.total_clips} (${payload.percentage.toFixed(1)}%)`;
                    }

                    status.value = statusMsg;
                }
                if (payload.message) {
                    status.value = payload.message;
                }
            }
        });
    } catch (e) {
        console.error('Failed to register progress listener:', e);
    }

    // FFmpeg only needs to be initialized once per app session; skip the work on
    // subsequent remounts to avoid redundant re-initialization side effects.
    if (!ffmpegInitialized) {
        try {
            const res = await commands.initFfmpeg();
            status.value = res;
            ffmpegInitialized = true;
        } catch (e) {
            status.value = `Error initializing FFmpeg: ${errorMessage(e)}`;
        }
    }
});

onUnmounted(() => {
    if (unlistenProgress) {
        unlistenProgress();
        unlistenProgress = null;
    }
    sessionPersistence.dispose();
});

watch(
    inputPath,
    async (newPath, oldPath) => {
        await sessionPersistence.handleInputPathChange(newPath, oldPath);
    },
    { flush: 'sync' },
);

watch(
    segments,
    () => {
        syncSpeakerOrder();
    },
    { deep: true },
);

watch(
    inputPath,
    () => {
        void refreshExtractedAudioPath();
    },
    { immediate: true },
);

watch(hasTranscript, (ready, wasReady) => {
    // Move into the transcript flow as soon as it becomes available, and fall
    // back to the source tab when the transcript (and its dependent tabs) clear.
    if (ready && !wasReady) {
        activeTab.value = 'transcript';
    } else if (!ready && activeTab.value !== 'source') {
        activeTab.value = 'source';
    }
});

watch(
    [clipWorkspaceState, viralClipsState, podcastWorkspaceState],
    () => {
        sessionPersistence.scheduleAutosave();
    },
    { deep: true },
);

// Clips and faces are saved with the recording too.
watch(
    () => [
        viralClipsState.value.clips,
        viralClipsState.value.faces,
        viralClipsState.value.faceSearch,
    ],
    () => {
        sessionPersistence.scheduleTranscriptSave();
    },
    { deep: true },
);

watch(
    transcriptWorkspaceState,
    () => {
        sessionPersistence.scheduleAutosave();
        sessionPersistence.scheduleTranscriptSave();
    },
    { deep: true },
);

async function loadTranscript() {
    if (!inputPath.value) return;
    const transcriptPath = inputPath.value + '.transcript.json';
    try {
        const content = await commands.readTextFile(transcriptPath);
        const parsed = parseTranscriptSidecar(content, createDefaultLastAnalyzedSettings());
        if (!parsed) {
            return;
        }
        // Clips and faces saved with the recording come back with it.
        if (parsed.viralClips) {
            viralClipsState.value = { ...viralClipsState.value, ...parsed.viralClips };
        }

        if (
            parsed.segments &&
            !parsed.context &&
            !parsed.glossary &&
            parsed.currentLanguage === undefined
        ) {
            segments.value = parsed.segments;
            status.value = 'Loaded existing transcript.';
            return;
        }

        if (parsed.segments) {
            segments.value = parsed.segments;
        }
        if (parsed.context !== undefined) {
            context.value = parsed.context;
        }
        if (parsed.glossary !== undefined) {
            workspaceSettings.value.glossary = parsed.glossary;
        }
        if (parsed.speakerCount !== undefined) {
            speakerCount.value = parsed.speakerCount;
        }
        if (parsed.removeFillerWords !== undefined) {
            removeFillerWords.value = parsed.removeFillerWords;
        }
        if (parsed.trimSilence !== undefined) {
            trimSilence.value = parsed.trimSilence;
        }
        if (parsed.translations) {
            translations.value = parsed.translations;
        }
        if (parsed.currentLanguage !== undefined) {
            currentLanguage.value = parsed.currentLanguage;
        }
        if (parsed.targetLanguage !== undefined) {
            targetLanguage.value = parsed.targetLanguage;
        }
        if (parsed.speakerOrder) {
            speakerOrder.value = parsed.speakerOrder;
        }
        if (parsed.lastAnalyzedSettings) {
            lastAnalyzedSettings.value = parsed.lastAnalyzedSettings;
        } else {
            lastAnalyzedSettings.value = {
                context: context.value,
                glossary: workspaceSettings.value.glossary,
                speakerCount: speakerCount.value,
                removeFillerWords: removeFillerWords.value,
                trimSilence: trimSilence.value,
                transcriptionBackend: workspaceSettings.value.transcriptionBackend ?? 'llm',
                localEngine: workspaceSettings.value.localEngine ?? 'parakeet',
                parakeetModelPath: workspaceSettings.value.parakeetModelPath ?? '',
                sortformerModelPath: workspaceSettings.value.sortformerModelPath ?? '',
                crisperSignature: currentEngineSignature(),
            };
        }
        if (parsed.rawParakeetSegments !== undefined) {
            rawParakeetSegments.value = parsed.rawParakeetSegments;
        }
        if (parsed.parakeetCacheKey !== undefined) {
            parakeetCacheKey.value = parsed.parakeetCacheKey;
        }
        if (parsed.transcriptionBackend !== undefined) {
            workspaceSettings.value.transcriptionBackend = parsed.transcriptionBackend;
        }
        if (parsed.localEngine !== undefined) {
            workspaceSettings.value.localEngine = parsed.localEngine;
        }
        if (parsed.parakeetModelPath !== undefined) {
            workspaceSettings.value.parakeetModelPath = parsed.parakeetModelPath;
        }
        if (parsed.sortformerModelPath !== undefined) {
            workspaceSettings.value.sortformerModelPath = parsed.sortformerModelPath;
        }

        status.value = 'Loaded existing transcript and settings.';
    } catch (e) {
        // Ignore error if file doesn't exist
        console.log('No existing transcript found or error loading it.');
    }
}

async function saveTranscript() {
    if (!inputPath.value) return;
    // An empty transcript is never worth persisting, and writing one would
    // destroy the existing sidecar (e.g. after a failed load or a reset).
    if (segments.value.length === 0) return;
    const transcriptPath = inputPath.value + '.transcript.json';
    try {
        await commands.writeTextFile(
            transcriptPath,
            JSON.stringify(
                buildTranscriptSidecar(transcriptWorkspaceState.value, {
                    clips: viralClipsState.value.clips,
                    faces: viralClipsState.value.faces,
                    faceSearch: viralClipsState.value.faceSearch,
                }),
                null,
                2,
            ),
        );
        console.log('Transcript saved.');
    } catch (e) {
        console.error('Failed to save transcript:', e);
    }
}

function assertActiveRun(runId: number) {
    if (activeRunId.value !== runId) {
        throw new Error('Run cancelled.');
    }
}

async function cancelCurrentRun() {
    if (!isProcessing.value || isCancelling.value) {
        return;
    }

    isCancelling.value = true;
    cancelGeneration.value += 1;
    activeRunId.value = null;
    stopSimulatedProgress();
    progressPercentage.value = null;
    progressEtaSeconds.value = null;
    status.value = 'Cancelling run...';

    try {
        await commands.cancelCurrentRun();
        status.value = 'Run cancelled.';
    } catch (error) {
        status.value = `Failed to cancel run: ${errorMessage(error)}`;
    } finally {
        isProcessing.value = false;
        isCancelling.value = false;
    }
}

async function translateTranscript() {
    if (isProcessing.value || !targetLanguage.value || segments.value.length === 0) return;

    const lang = targetLanguage.value.trim();
    if (translations.value[lang]) {
        currentLanguage.value = lang;
        return;
    }

    const runId = await beginRun();
    activeRunId.value = runId;
    isCancelling.value = false;
    isTranslating.value = true;
    isProcessing.value = true;
    status.value = `Translating to ${lang}...`;

    try {
        const response = await commands.translateTranscript(
            runId,
            settings.value.baseUrl,
            settings.value.model,
            segments.value,
            lang,
            context.value,
        );
        assertActiveRun(runId);

        const jsonMatch = response.match(/\[[\s\S]*\]/);
        if (jsonMatch) {
            try {
                translations.value[lang] = parseTranscriptResponse(response);
                assertActiveRun(runId);
                currentLanguage.value = lang;
                status.value = `Translation to ${lang} complete.`;
            } catch (e) {
                console.error('JSON Parse Error', e);
                showError(
                    'Failed to parse translation from AI response.',
                    response,
                    errorMessage(e),
                );
            }
        } else {
            console.error(response);
            showError('Failed to find JSON in translation response.', response);
        }
    } catch (e) {
        if (isRunCancelled(e)) {
            status.value = 'Run cancelled.';
            return;
        }
        console.error('Translation failed:', e);
        status.value = `Translation failed: ${errorMessage(e)}`;
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isTranslating.value = false;
            isProcessing.value = false;
            isCancelling.value = false;
        }
    }
}

function showError(message: string, rawResponse: string, parseError: string = '') {
    errorDetails.value = { message, rawResponse, parseError };
    showErrorOverlay.value = true;
    status.value = message;
}

function dismissError() {
    showErrorOverlay.value = false;
}

let progressInterval: number | null = null;

function startSimulatedProgress(estimatedSeconds: number) {
    if (progressInterval) clearInterval(progressInterval);
    progressPercentage.value = 0;
    progressEtaSeconds.value = estimatedSeconds;
    const startTime = Date.now();

    progressInterval = window.setInterval(() => {
        const elapsed = (Date.now() - startTime) / 1000;
        const p = (elapsed / estimatedSeconds) * 100;
        // Cap at 99% so it doesn't look finished until it actually is
        progressPercentage.value = Math.min(p, 99);
        progressEtaSeconds.value = Math.max(estimatedSeconds - elapsed, 0);
    }, 100);
}

function stopSimulatedProgress() {
    if (progressInterval) {
        clearInterval(progressInterval);
        progressInterval = null;
    }
    progressPercentage.value = 100;
    progressEtaSeconds.value = null;
}

function estimateTime(type: 'analysis' | 'generation', inputSize: number): number {
    const DEFAULT_ESTIMATE = 30;
    // Only learn from entries with a positive inputSize; a zero/negative size
    // would make duration/inputSize produce Infinity/NaN and poison the rate.
    const relevant = executionHistory.value.filter((h) => h.type === type && h.inputSize > 0);

    let estimate: number;
    if (relevant.length === 0) {
        // Default estimates
        if (type === 'analysis')
            estimate = inputSize * 0.1; // e.g. 10% of audio duration
        else if (type === 'generation')
            estimate = inputSize * 0.005; // e.g. 5ms per char
        else estimate = DEFAULT_ESTIMATE;
    } else {
        const rate =
            relevant.reduce((acc, h) => acc + h.duration / h.inputSize, 0) / relevant.length;
        estimate = inputSize * rate;
    }

    // Guard the caller against NaN/Infinity/non-positive estimates (e.g. when
    // inputSize itself is <= 0), which would otherwise poison the progress bar.
    if (!Number.isFinite(estimate) || estimate <= 0) {
        return DEFAULT_ESTIMATE;
    }
    return estimate;
}

function logExecution(type: 'analysis' | 'generation', inputSize: number, duration: number) {
    executionHistory.value.push({ type, inputSize, duration, timestamp: Date.now() });
    if (executionHistory.value.length > 20) executionHistory.value.shift();
    localStorage.setItem('executionHistory', JSON.stringify(executionHistory.value));
}

async function requestLlmTranscriptForChunk(
    runId: number,
    chunkAudioPath: string,
): Promise<string> {
    const isGoogleApi = settings.value.baseUrl.includes('generativelanguage.googleapis.com');
    let uri: string | null = null;
    let audioBase64: string | null = null;

    if (isGoogleApi) {
        uri = await commands.uploadFile(runId, settings.value.baseUrl, chunkAudioPath);
        assertActiveRun(runId);
    } else {
        audioBase64 = await commands.readFileAsBase64(chunkAudioPath);
        assertActiveRun(runId);
    }

    const response = await commands.analyzeAudio(
        runId,
        {
            baseUrl: settings.value.baseUrl,
            model: settings.value.model,
        },
        settings.value.enforceJsonSchema,
        context.value,
        workspaceSettings.value.glossary,
        speakerCount.value,
        removeFillerWords.value,
        uri,
        audioBase64,
    );
    assertActiveRun(runId);
    return response;
}

// Don't re-split below this; if a chunk this small still times out, the
// problem isn't size and splitting further won't help.
const MIN_RESPLIT_SECONDS = 120;
// Backstop against pathological recursion.
const MAX_RESPLIT_DEPTH = 4;

// A failure worth retrying by shrinking the chunk: gateway/upstream timeouts
// (the 504 case) and length-driven truncation that breaks transcript parsing.
function isResplittableError(error: unknown): boolean {
    if (isRunCancelled(error)) return false;
    const message = errorMessage(error).toLowerCase();
    return /\b50[234]\b|gateway timeout|timed out|timeout|deadline exceeded|failed to (parse|find) (transcript|json)|json/.test(
        message,
    );
}

// Builds the timestamp mapper for a chunk: shift chunk-relative timestamps by
// the chunk's absolute offset in the (trimmed) timeline, then apply any
// silence-trim offset that maps the trimmed timeline back onto the original.
function buildChunkAdjuster(
    baseOffset: number,
    silenceAdjuster?: (timestamp: string) => string,
): (timestamp: string) => string {
    return (timestamp: string) => {
        const shifted =
            baseOffset === 0 ? timestamp : formatTime(parseTime(timestamp) + baseOffset);
        return silenceAdjuster ? silenceAdjuster(shifted) : shifted;
    };
}

// Transcribe one chunk; on a size-related failure (504/timeout/truncation),
// re-split that chunk into smaller pieces and retry each recursively rather
// than failing the whole run.
async function transcribeChunkWithResplit(
    runId: number,
    chunkPath: string,
    baseOffset: number,
    chunkMaxSeconds: number,
    silenceAdjuster: ((timestamp: string) => string) | undefined,
    depth: number,
    label: string,
): Promise<TranscriptSegment[]> {
    try {
        const response = await requestLlmTranscriptForChunk(runId, chunkPath);
        return parseTranscriptResponse(response, buildChunkAdjuster(baseOffset, silenceAdjuster));
    } catch (error) {
        const halfMax = chunkMaxSeconds / 2;
        if (
            depth >= MAX_RESPLIT_DEPTH ||
            halfMax < MIN_RESPLIT_SECONDS ||
            !isResplittableError(error)
        ) {
            throw error;
        }

        console.warn(
            `Chunk ${label} failed (${errorMessage(error)}); re-splitting into smaller parts and retrying.`,
        );
        status.value = `Part ${label} timed out; splitting it into smaller parts and retrying...`;
        const subChunks = await commands.splitAudioForAnalysis(
            runId,
            chunkPath,
            halfMax,
            workspaceSettings.value.parakeetModelPath,
        );
        assertActiveRun(runId);

        if (subChunks.length <= 1) {
            // The chunk couldn't be divided further (e.g. no usable boundary);
            // surface the original failure rather than looping.
            throw error;
        }

        const resplitSegments: TranscriptSegment[] = [];
        for (let index = 0; index < subChunks.length; index++) {
            const subChunk = subChunks[index];
            resplitSegments.push(
                ...(await transcribeChunkWithResplit(
                    runId,
                    subChunk.path,
                    baseOffset + subChunk.start_offset,
                    halfMax,
                    silenceAdjuster,
                    depth + 1,
                    `${label}.${index + 1}`,
                )),
            );
        }
        return resplitSegments;
    }
}

async function analyzeWithLlmTranscript(
    runId: number,
    analysisAudioPath: string,
    adjustTimestamps?: boolean,
    processedOffsets?: ProcessedAudio['offsets'],
): Promise<TranscriptSegment[]> {
    // Long audio is split into chunks so each request stays under the
    // provider's request timeout. Short audio yields a single chunk pointing at
    // the original file (no extra work).
    const maxChunkSeconds = (settings.value.maxAnalysisChunkMinutes ?? 30) * 60;
    status.value = 'Planning audio chunks...';
    const chunks = await commands.splitAudioForAnalysis(
        runId,
        analysisAudioPath,
        maxChunkSeconds,
        workspaceSettings.value.parakeetModelPath,
    );
    assertActiveRun(runId);

    // Maps a silence-trimmed timestamp back onto the original timeline.
    const silenceAdjuster =
        adjustTimestamps && processedOffsets
            ? (timestamp: string) => adjustTimestamp(timestamp, processedOffsets)
            : undefined;

    const allSegments: TranscriptSegment[] = [];
    for (let index = 0; index < chunks.length; index++) {
        const chunk = chunks[index];
        if (chunks.length > 1) {
            status.value = `Analyzing with AI (part ${index + 1}/${chunks.length})...`;
        }

        allSegments.push(
            ...(await transcribeChunkWithResplit(
                runId,
                chunk.path,
                chunk.start_offset,
                maxChunkSeconds,
                silenceAdjuster,
                0,
                `${index + 1}`,
            )),
        );
    }

    return allSegments;
}

/**
 * Produce the local (pre-offset) transcript with the selected engine, reusing
 * the cache when nothing that affects it has changed.
 *
 * This is the only place that knows which engine runs; the pipeline stages that
 * consume the result treat it as an opaque transcript, which is what lets the
 * hybrid modes work with any engine.
 */
async function transcribeWithLocalEngine(
    runId: number,
    analysisAudioPath: string,
): Promise<TranscriptSegment[]> {
    const cacheKey = currentParakeetCacheKey();
    if (parakeetCacheKey.value === cacheKey && rawParakeetSegments.value.length > 0) {
        status.value = `Reusing ${localEngineLabel.value} transcript (audio unchanged)...`;
        return rawParakeetSegments.value;
    }

    const engine = workspaceSettings.value.localEngine;
    const segments =
        engine === 'apple-speech'
            ? await commands.transcribeWithAppleSpeech(runId, analysisAudioPath, {
                  locale: appleSpeechLocale(settings.value.appleSpeechLocale),
                  diarize: settings.value.appleSpeechDiarize,
                  sortformerModelPath: workspaceSettings.value.sortformerModelPath,
              })
            : engine === 'crisper'
              ? await commands.transcribeWithCrisper(runId, analysisAudioPath, {
                    pythonPath: settings.value.crisperPythonPath,
                    model: settings.value.crisperModel,
                    language: settings.value.crisperLanguage,
                    mode: settings.value.crisperMode,
                    backend: settings.value.crisperBackend,
                    device: settings.value.crisperDevice,
                    computeType: settings.value.crisperComputeType,
                    // The editor cuts on word timings, so they are always requested
                    // (the model adds no measurable overhead for them).
                    wordTimestamps: true,
                    removeFillers: removeFillerWords.value,
                    removeVocalEvents: settings.value.crisperRemoveVocalEvents,
                    diarize: settings.value.crisperDiarize,
                    sortformerModelPath: workspaceSettings.value.sortformerModelPath,
                })
              : await commands.transcribeWithParakeet(
                    runId,
                    analysisAudioPath,
                    workspaceSettings.value.parakeetModelPath,
                    workspaceSettings.value.sortformerModelPath,
                );

    assertActiveRun(runId);
    // Cache the raw, pre-offset output so changing only LLM-side inputs (or
    // switching between local and the hybrid pipelines) does not re-transcribe.
    rawParakeetSegments.value = segments;
    parakeetCacheKey.value = cacheKey;
    return segments;
}

async function processFile() {
    // Starting a run supersedes the backend's current one, so never start a
    // second job while one is in flight.
    if (isProcessing.value) return;
    if (!inputPath.value) {
        status.value = 'Please provide a media file.';
        return;
    }

    if (!hasMediaFile.value) {
        status.value = 'Selected media file could not be found. Choose a valid file to continue.';
        return;
    }

    if (!hasBackendConfiguration.value) {
        status.value =
            workspaceSettings.value.transcriptionBackend === 'llm'
                ? 'Please provide an API key.'
                : 'Please provide an API key for the hybrid AI stage.';
        return;
    }

    const runId = await beginRun();
    activeRunId.value = runId;
    isCancelling.value = false;
    isProcessing.value = true;
    progressPercentage.value = null;
    progressEtaSeconds.value = null;
    status.value = 'Preparing audio...';
    // Keep the current transcript until the new one arrives: clearing it here
    // would let autosave persist an empty transcript over the sidecar, losing it
    // for good if this run fails or is cancelled.
    await grantMediaAccess(inputPath.value);

    try {
        const failStage = (stage: string, error: unknown) => {
            if (isRunCancelled(error)) {
                throw new Error('Run cancelled.');
            }
            const details = errorMessage(error);
            const message = `${stage} failed.`;
            showError(message, details);
            status.value = `${message} ${details}`;
            throw new Error(message);
        };

        let audioInfo: AudioInfo;
        try {
            audioInfo = await commands.prepareAudioForAi(runId, inputPath.value);
            assertActiveRun(runId);
        } catch (error) {
            failStage('Audio preparation', error);
            return;
        }
        status.value = `Audio prepared: ${audioInfo.path} (${(audioInfo.size / 1024 / 1024).toFixed(2)} MB)`;

        // Produce a seekable, webview-playable preview (AAC/m4a) from the extracted
        // stream for the in-app audio scrubber. Failure here is non-fatal: it only
        // disables the audio preview, not the transcription itself.
        try {
            const previewPath = await commands.preparePreviewAudio(runId, audioInfo.path);
            extractedAudioPath.value = previewPath;
            await grantMediaAccess(previewPath);
        } catch (error) {
            console.warn('Preview audio preparation failed; audio scrubber disabled.', error);
            extractedAudioPath.value = '';
        }

        let processedAudio: ProcessedAudio;
        if (trimSilence.value) {
            status.value = 'Removing silence...';
            try {
                processedAudio = await commands.removeSilence(
                    runId,
                    audioInfo.path,
                    null /* default minimum silence */,
                );
                assertActiveRun(runId);
            } catch (error) {
                failStage('Silence removal', error);
                return;
            }
            console.log(`Found ${processedAudio.silence_intervals.length} silence intervals.`);
        } else {
            processedAudio = {
                path: audioInfo.path,
                silence_intervals: [],
                offsets: [{ min_time: 0.0, offset: 0.0 }],
            };
        }

        // Use processed audio for upload/analysis
        const analysisAudioPath = processedAudio.path;

        const estimatedTime = estimateTime('analysis', audioInfo.duration);
        const pipelineLabel = isLlmOnlyBackend.value
            ? 'Analyzing with AI'
            : workspaceSettings.value.transcriptionBackend === 'hybrid'
              ? `Running hybrid transcription (${localEngineLabel.value} + AI cleanup)`
              : workspaceSettings.value.transcriptionBackend === 'hybrid-merge'
                ? `Running merged hybrid transcription (${localEngineLabel.value} + AI)`
                : `Transcribing with ${localEngineLabel.value}`;
        status.value = `${pipelineLabel}... (Est. ${estimatedTime.toFixed(0)}s)`;
        const startTime = Date.now();
        let hybridCleanupUsedFallback = false;

        startSimulatedProgress(estimatedTime);
        let nextSegments: TranscriptSegment[] = [];
        try {
            if (isLlmOnlyBackend.value) {
                try {
                    nextSegments = await analyzeWithLlmTranscript(
                        runId,
                        analysisAudioPath,
                        true,
                        processedAudio.offsets,
                    );
                } catch (error) {
                    failStage('AI analysis request', error);
                    return;
                }
            } else {
                // One local transcript, produced by whichever engine is
                // selected. The hybrid stages below are deliberately unaware of
                // which engine that was.
                let localSegments: TranscriptSegment[];
                try {
                    localSegments = await transcribeWithLocalEngine(runId, analysisAudioPath);
                } catch (error) {
                    failStage(`${localEngineLabel.value} transcription`, error);
                    return;
                }

                if (workspaceSettings.value.transcriptionBackend === 'hybrid') {
                    status.value = 'Cleaning transcript with AI...';
                    try {
                        nextSegments = await commands.cleanupLocalTranscript(
                            runId,
                            settings.value.baseUrl,
                            settings.value.model,
                            localSegments,
                            context.value,
                            workspaceSettings.value.glossary,
                            removeFillerWords.value,
                        );
                        assertActiveRun(runId);
                    } catch (error) {
                        console.warn(
                            `Hybrid cleanup failed, using the ${localEngineLabel.value} transcript`,
                            error,
                        );
                        nextSegments = localSegments;
                        hybridCleanupUsedFallback = true;
                    }
                } else if (workspaceSettings.value.transcriptionBackend === 'hybrid-merge') {
                    status.value = 'Querying remote transcript for merge...';
                    let referenceTranscript: TranscriptSegment[] = [];
                    try {
                        referenceTranscript = await analyzeWithLlmTranscript(
                            runId,
                            analysisAudioPath,
                        );
                    } catch (error) {
                        console.warn(
                            `Merged hybrid remote transcript failed, using the ${localEngineLabel.value} transcript`,
                            error,
                        );
                        hybridCleanupUsedFallback = true;
                    }

                    if (referenceTranscript.length > 0) {
                        status.value = `Merging ${localEngineLabel.value} and remote transcripts...`;
                        try {
                            nextSegments = await commands.mergeTranscriptHypotheses(
                                runId,
                                localSegments,
                                referenceTranscript,
                            );
                            assertActiveRun(runId);
                        } catch (error) {
                            console.warn(
                                `Merged hybrid reconciliation failed, using the ${localEngineLabel.value} transcript`,
                                error,
                            );
                            nextSegments = localSegments;
                            hybridCleanupUsedFallback = true;
                        }
                    } else {
                        nextSegments = localSegments;
                        hybridCleanupUsedFallback = true;
                    }
                } else {
                    nextSegments = localSegments;
                }

                if (trimSilence.value) {
                    nextSegments = adjustSegmentsWithOffsets(nextSegments, processedAudio.offsets);
                }
            }
        } finally {
            stopSimulatedProgress();
        }

        const duration = (Date.now() - startTime) / 1000;
        logExecution('analysis', audioInfo.duration, duration);

        assertActiveRun(runId);
        segments.value = nextSegments;
        // Translations are index-aligned to the transcript they were made from.
        translations.value = {};
        currentLanguage.value = 'Original';
        const foundSuffix = `Found ${segments.value.length} segments.`;
        const backend = workspaceSettings.value.transcriptionBackend;
        status.value =
            backend === 'llm'
                ? `Analysis complete. ${foundSuffix}`
                : backend === 'local'
                  ? `${localEngineLabel.value} transcription complete. ${foundSuffix}`
                  : hybridCleanupUsedFallback
                    ? `${backend === 'hybrid' ? 'Hybrid cleanup' : 'Hybrid merge'} failed, using the ${localEngineLabel.value} transcript. ${foundSuffix}`
                    : `${backend === 'hybrid' ? 'Hybrid transcription' : 'Hybrid merge'} complete. ${foundSuffix}`;

        lastAnalyzedSettings.value = {
            context: context.value,
            glossary: workspaceSettings.value.glossary,
            speakerCount: speakerCount.value,
            removeFillerWords: removeFillerWords.value,
            trimSilence: trimSilence.value,
            transcriptionBackend: workspaceSettings.value.transcriptionBackend,
            localEngine: workspaceSettings.value.localEngine,
            parakeetModelPath: workspaceSettings.value.parakeetModelPath,
            sortformerModelPath: workspaceSettings.value.sortformerModelPath,
            crisperSignature: currentEngineSignature(),
        };

        await saveTranscript();
        assertActiveRun(runId);
    } catch (e) {
        if (isRunCancelled(e)) {
            status.value = 'Run cancelled.';
            return;
        }
        const message = errorMessage(e);
        if (!showErrorOverlay.value) {
            showError('Analysis failed before transcription completed.', message);
        }
        status.value = message;
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isProcessing.value = false;
            isCancelling.value = false;
            progressPercentage.value = null;
            progressEtaSeconds.value = null;
        }
    }
}

async function cutVideo() {
    if (isProcessing.value || segments.value.length === 0) return;
    if (!hasMediaFile.value) {
        status.value = 'Select a valid media file before exporting video.';
        return;
    }

    const runId = await beginRun();
    activeRunId.value = runId;
    isCancelling.value = false;
    status.value = 'Cutting media...';
    isProcessing.value = true;
    progressPercentage.value = null;
    progressEtaSeconds.value = null;

    try {
        const cutSegments = segments.value.map((s) => ({ start: s.start, end: s.end }));
        const outputPath = appendFileNameSuffix(inputPath.value, '_cut');

        await commands.cutVideo(
            runId,
            inputPath.value,
            cutSegments,
            outputPath,
            settings.value.exportQuality,
        );
        assertActiveRun(runId);

        status.value = `Media cut successfully to ${outputPath}`;
    } catch (e) {
        if (isRunCancelled(e)) {
            status.value = 'Run cancelled.';
            return;
        }
        status.value = `Error cutting media: ${errorMessage(e)}`;
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isProcessing.value = false;
            isCancelling.value = false;
            progressPercentage.value = null;
            progressEtaSeconds.value = null;
        }
    }
}

async function renameSpeaker(oldName: string, newName: string, inputElement: HTMLInputElement) {
    const trimmedNewName = newName.trim();
    if (oldName === trimmedNewName || !trimmedNewName) {
        inputElement.value = oldName; // Reset if empty or same
        return;
    }

    const exists = uniqueSpeakers.value.includes(trimmedNewName);

    if (exists) {
        const confirmed = await ask(
            `Speaker "${trimmedNewName}" already exists.\n\nMerging "${oldName}" into "${trimmedNewName}" is irreversible.\n\nDo you want to continue?`,
            { title: 'Merge Speakers?', kind: 'warning' },
        );

        if (!confirmed) {
            inputElement.value = oldName;
            return;
        }
    }

    // Update segments
    segments.value = segments.value.map((seg) => {
        if (seg.speaker === oldName) {
            return { ...seg, speaker: trimmedNewName };
        }
        return seg;
    });

    if (exists) {
        speakerOrder.value = speakerOrder.value.filter((speaker) => speaker !== oldName);
    } else {
        speakerOrder.value = speakerOrder.value.map((speaker) =>
            speaker === oldName ? trimmedNewName : speaker,
        );
    }

    await saveTranscript();
}

function goToSettings() {
    router.push('/settings');
}

function updateStatus(message: string) {
    status.value = message;
}

function updateProcessing(processing: boolean) {
    isProcessing.value = processing;
}
</script>

<template>
    <div
        class="min-h-screen bg-gray-900 text-gray-200 p-8 pb-24 font-sans selection:bg-blue-500/30"
    >
        <div class="mx-auto w-full max-w-[1400px]">
            <WorkspaceTabs
                :tabs="workspaceTabs"
                :activeTab="activeTab"
                @update:activeTab="activeTab = $event"
            />

            <div v-show="activeTab === 'source'">
                <HomeSourcePanel
                    :currentEngineLabel="currentEngineLabel"
                    :currentModelDisplay="currentModelDisplay"
                    :inputPath="inputPath"
                    :hasMediaFile="hasMediaFile"
                    :isProcessing="isProcessing"
                    :hasBackendConfiguration="hasBackendConfiguration"
                    :hasTranscript="hasTranscript"
                    :settingsChanged="settingsChanged"
                    :transcriptionBackend="workspaceSettings.transcriptionBackend"
                    :localEngine="workspaceSettings.localEngine"
                    :context="context"
                    :glossary="workspaceSettings.glossary"
                    :speakerCount="speakerCount"
                    :removeFillerWords="removeFillerWords"
                    :trimSilence="trimSilence"
                    @update:inputPath="inputPath = $event"
                    @update:transcriptionBackend="workspaceSettings.transcriptionBackend = $event"
                    @update:localEngine="workspaceSettings.localEngine = $event"
                    @update:context="context = $event"
                    @update:glossary="workspaceSettings.glossary = $event"
                    @update:speakerCount="speakerCount = $event"
                    @update:removeFillerWords="removeFillerWords = $event"
                    @update:trimSilence="trimSilence = $event"
                    @invalid-selection="updateStatus"
                    @save-session="sessionPersistence.handleSaveSession"
                    @load-session="sessionPersistence.handleLoadSession"
                    @open-settings="goToSettings"
                    @process="processFile"
                />
            </div>

            <!-- Editor Section -->
            <div v-show="activeTab === 'transcript'">
                <transition name="fade">
                    <TranscriptWorkspacePanel
                        v-if="hasTranscript"
                        :inputPath="inputPath"
                        :hasMediaFile="hasMediaFile"
                        :extractedAudioPath="extractedAudioPath"
                        :displaySegments="displaySegments"
                        :originalSegments="segments"
                        :translations="translations"
                        :currentLanguage="currentLanguage"
                        :targetLanguage="targetLanguage"
                        :isTranslating="isTranslating"
                        :uniqueSpeakers="uniqueSpeakers"
                        :isProcessing="isProcessing"
                        @update:currentLanguage="currentLanguage = $event"
                        @update:targetLanguage="targetLanguage = $event"
                        @translate="translateTranscript"
                        @export-video="cutVideo"
                        @rename-speaker="
                            renameSpeaker($event.oldName, $event.newName, $event.inputElement)
                        "
                        @update:segments="displaySegments = $event"
                    />
                </transition>
            </div>

            <!-- Viral Clips Generator -->
            <div v-show="activeTab === 'clips'">
                <transition name="fade">
                    <ViralClipsGenerator
                        v-if="hasTranscript"
                        :segments="segments"
                        :inputPath="inputPath"
                        :hasMediaFile="hasMediaFile"
                        :busy="isProcessing"
                        :state="viralClipsState"
                        :cancelGeneration="cancelGeneration"
                        class="mb-8"
                        @update:status="updateStatus"
                        @update:processing="updateProcessing"
                        @update:state="viralClipsState = $event"
                    />
                </transition>
            </div>

            <!-- Podcast Generator -->
            <div v-show="activeTab === 'podcast'">
                <transition name="fade">
                    <PodcastGenerator
                        v-if="hasTranscript"
                        :segments="segments"
                        :inputPath="inputPath"
                        :hasMediaFile="hasMediaFile"
                        :busy="isProcessing"
                        :context="context"
                        :state="podcastWorkspaceState"
                        :cancelGeneration="cancelGeneration"
                        class="mb-20"
                        @update:status="updateStatus"
                        @update:processing="updateProcessing"
                        @update:state="podcastWorkspaceState = $event"
                    />
                </transition>
            </div>
        </div>
    </div>

    <!-- Error Overlay -->
    <ErrorOverlay
        :show="showErrorOverlay"
        :message="errorDetails.message"
        :rawResponse="errorDetails.rawResponse"
        :parseError="errorDetails.parseError"
        @dismiss="dismissError"
        @update:status="updateStatus"
    />
    <StatusBar
        :status="status"
        :isProcessing="isProcessing"
        :progressPercentage="progressPercentage"
        :progressEtaSeconds="progressEtaSeconds"
        :isCancelling="isCancelling"
        @cancel="cancelCurrentRun"
    />
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
    transition:
        opacity 0.5s ease,
        transform 0.5s ease;
}

.fade-enter-from,
.fade-leave-to {
    opacity: 0;
    transform: translateY(20px);
}
</style>
