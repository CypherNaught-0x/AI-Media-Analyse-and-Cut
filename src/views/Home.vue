<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from '@tauri-apps/api/event';
import { ask } from '@tauri-apps/plugin-dialog';
import { useRouter } from 'vue-router';
import ViralClipsGenerator from "../components/ViralClipsGenerator.vue";
import PodcastGenerator from "../components/PodcastGenerator.vue";
import ErrorOverlay from "../components/ErrorOverlay.vue";
import HomeSourcePanel from "../components/HomeSourcePanel.vue";
import TranscriptWorkspacePanel from "../components/TranscriptWorkspacePanel.vue";
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
    ViralClipsWorkspaceState
} from "../types";
import ClipGenerator from "../components/ClipGenerator.vue";
import ClipList from "../components/ClipList.vue";
import StatusBar from "../components/StatusBar.vue";
import { useClipGeneration } from "../composables/useClipGeneration";
import { useSettings } from "../composables/useSettings";
import { useHomeSessionPersistence } from "../composables/useHomeSessionPersistence";
import { adjustTimestamp } from "../composables/useTimeFormat";
import { parseTranscriptResponse } from "../utils/transcriptParsing";
import { buildTranscriptSidecar, parseTranscriptSidecar } from "../utils/transcriptSidecar";
import {
    createDefaultClipWorkspaceState,
    createDefaultLastAnalyzedSettings,
    createDefaultPodcastWorkspaceState,
    createDefaultViralClipsWorkspaceState,
} from "../utils/editSession";

import type { TranscriptWord } from '../types';

const AUTOSAVE_DEBOUNCE_MS = 750;

const router = useRouter();
const { settings } = useSettings();

const status = ref("Initializing...");
const isProcessing = ref(false);

// Error overlay state
const showErrorOverlay = ref(false);
const errorDetails = ref({
    message: "",
    rawResponse: "",
    parseError: ""
});
const progressPercentage = ref<number | null>(null);
const progressEtaSeconds = ref<number | null>(null);
const executionHistory = ref<{type: string, inputSize: number, duration: number, timestamp: number}[]>([]);
const inputPath = ref("");
const inputPathExists = ref(false);
const segments = ref<TranscriptSegment[]>([]);
const translations = ref<Record<string, TranscriptSegment[]>>({});
const currentLanguage = ref("Original");
const targetLanguage = ref("");
const isTranslating = ref(false);
const removeFillerWords = ref(false);
const trimSilence = ref(true);

const speakerCount = ref<number | null>(null);
const context = ref("");
const useAdvancedAlignment = ref(false);
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

const isLlmOnlyBackend = computed(() => settings.value.transcriptionBackend === 'llm');
const hasApiKey = computed(() => settings.value.apiKey.length > 0);
const hasParakeetModels = computed(() => {
    return true;
});
const hasBackendConfiguration = computed(() => {
    if (settings.value.transcriptionBackend === 'llm') return hasApiKey.value;
    if (settings.value.transcriptionBackend === 'hybrid') return hasApiKey.value && hasParakeetModels.value;
    if (settings.value.transcriptionBackend === 'hybrid-merge') return hasApiKey.value && hasParakeetModels.value;
    return hasParakeetModels.value;
});
const currentModelDisplay = computed(() => {
    if (settings.value.transcriptionBackend === 'hybrid') {
        if (!hasApiKey.value) return "Hybrid (missing API key)";
        return `Hybrid: Parakeet + ${settings.value.model}`;
    }
    if (settings.value.transcriptionBackend === 'hybrid-merge') {
        if (!hasApiKey.value) return "Hybrid Merge (missing API key)";
        return `Hybrid Merge: Parakeet + ${settings.value.model}`;
    }
    if (settings.value.transcriptionBackend === 'parakeet') {
        if (!settings.value.parakeetModelPath.trim() && !settings.value.sortformerModelPath.trim()) {
            return "Parakeet-RS (auto-download)";
        }
        return "Parakeet-RS (local)";
    }
    if (!hasApiKey.value) return "No API Key configured";
    return `${settings.value.model}`;
});
const currentEngineLabel = computed(() => {
    return settings.value.transcriptionBackend === 'llm' ? 'Current Model' : 'Current Pipeline';
});
const hasTranscript = computed(() => segments.value.length > 0);
const hasMediaFile = computed(() => inputPath.value.length > 0 && inputPathExists.value);
const settingsChanged = computed(() => {
    return settings.value.transcriptionBackend !== lastAnalyzedSettings.value.transcriptionBackend ||
           settings.value.parakeetModelPath !== lastAnalyzedSettings.value.parakeetModelPath ||
           settings.value.sortformerModelPath !== lastAnalyzedSettings.value.sortformerModelPath ||
           context.value !== lastAnalyzedSettings.value.context ||
           settings.value.glossary !== lastAnalyzedSettings.value.glossary ||
           speakerCount.value !== lastAnalyzedSettings.value.speakerCount ||
           removeFillerWords.value !== lastAnalyzedSettings.value.removeFillerWords ||
           trimSilence.value !== lastAnalyzedSettings.value.trimSilence;
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
    useAdvancedAlignment: useAdvancedAlignment.value,
    speakerOrder: speakerOrder.value,
    lastAnalyzedSettings: lastAnalyzedSettings.value,
    settingsSnapshot: {
        glossary: settings.value.glossary,
        transcriptionBackend: settings.value.transcriptionBackend,
        parakeetModelPath: settings.value.parakeetModelPath,
        sortformerModelPath: settings.value.sortformerModelPath,
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
        if (currentLanguage.value === "Original") return segments.value;
        return translations.value[currentLanguage.value] || segments.value;
    },
    set: (newSegments) => {
        if (currentLanguage.value === "Original") {
            segments.value = newSegments;
        } else {
            translations.value[currentLanguage.value] = newSegments;
        }
    }
});

async function updateInputPathExists(path: string) {
    if (!path) {
        inputPathExists.value = false;
        return false;
    }

    try {
        inputPathExists.value = await invoke<boolean>('path_exists', { path });
    } catch (error) {
        console.error('Failed to check input path existence:', error);
        inputPathExists.value = false;
    }

    return inputPathExists.value;
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
    currentLanguage.value = "Original";
    targetLanguage.value = "";
    context.value = "";
    speakerCount.value = null;
    removeFillerWords.value = false;
    trimSilence.value = true;
    useAdvancedAlignment.value = false;
    speakerOrder.value = [];
    lastAnalyzedSettings.value = createDefaultLastAnalyzedSettings();
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
    useAdvancedAlignment.value = state.useAdvancedAlignment;
    speakerOrder.value = state.speakerOrder;
    lastAnalyzedSettings.value = state.lastAnalyzedSettings;
    settings.value.glossary = state.settingsSnapshot.glossary;
    settings.value.transcriptionBackend = state.settingsSnapshot.transcriptionBackend;
    settings.value.parakeetModelPath = state.settingsSnapshot.parakeetModelPath;
    settings.value.sortformerModelPath = state.settingsSnapshot.sortformerModelPath;
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

const clipGeneration = useClipGeneration({
    settings,
    status,
    isProcessing,
    progressPercentage,
    inputPath,
    hasMediaFile,
    segments,
    clipCount,
    clipMinDuration,
    clipMaxDuration,
    clipTopic,
    allowSplicing,
    clips,
    selectedClipIndices,
    lastExportPath,
    clipExportSilenceCache,
    estimateTime,
    logExecution,
    startSimulatedProgress,
    stopSimulatedProgress,
});

onMounted(async () => {
    const history = localStorage.getItem('executionHistory');
    if (history) {
        try {
            executionHistory.value = JSON.parse(history);
        } catch (e) {
            console.error("Failed to parse execution history", e);
        }
    }
    await sessionPersistence.restoreAutosavedSession();

    try {
        const res = await invoke<string>("init_ffmpeg");
        status.value = res;
        
        await listen<any>('progress', (event) => {
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
                     progressEtaSeconds.value = typeof payload.etaSeconds === 'number'
                        ? payload.etaSeconds
                        : null;
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
        status.value = `Error initializing FFmpeg: ${e}`;
    }
});

onUnmounted(() => {
    sessionPersistence.dispose();
});

watch(inputPath, async (newPath, oldPath) => {
    await sessionPersistence.handleInputPathChange(newPath, oldPath);
}, { flush: 'sync' });

watch(segments, () => {
    syncSpeakerOrder();
}, { deep: true });

watch(
    [
        clipWorkspaceState,
        viralClipsState,
        podcastWorkspaceState,
    ],
    () => {
        sessionPersistence.scheduleAutosave();
    },
    { deep: true }
);

watch(
    transcriptWorkspaceState,
    () => {
        sessionPersistence.scheduleAutosave();
        sessionPersistence.scheduleTranscriptSave();
    },
    { deep: true }
);

async function loadTranscript() {
    if (!inputPath.value) return;
    const transcriptPath = inputPath.value + ".transcript.json";
    try {
        const content = await invoke<string>("read_text_file", { path: transcriptPath });
        const parsed = parseTranscriptSidecar(content, createDefaultLastAnalyzedSettings());
        if (!parsed) {
            return;
        }

        if (parsed.segments && !parsed.context && !parsed.glossary && parsed.currentLanguage === undefined) {
            segments.value = parsed.segments;
            status.value = "Loaded existing transcript.";
            return;
        }

        if (parsed.segments) {
            segments.value = parsed.segments;
        }
        if (parsed.context !== undefined) {
            context.value = parsed.context;
        }
        if (parsed.glossary !== undefined) {
            settings.value.glossary = parsed.glossary;
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
        if (parsed.useAdvancedAlignment !== undefined) {
            useAdvancedAlignment.value = parsed.useAdvancedAlignment;
        }
        if (parsed.speakerOrder) {
            speakerOrder.value = parsed.speakerOrder;
        }
        if (parsed.lastAnalyzedSettings) {
            lastAnalyzedSettings.value = parsed.lastAnalyzedSettings;
        } else {
            lastAnalyzedSettings.value = {
                context: context.value,
                glossary: settings.value.glossary,
                speakerCount: speakerCount.value,
                removeFillerWords: removeFillerWords.value,
                trimSilence: trimSilence.value,
                transcriptionBackend: settings.value.transcriptionBackend ?? 'llm',
                parakeetModelPath: settings.value.parakeetModelPath ?? '',
                sortformerModelPath: settings.value.sortformerModelPath ?? '',
            };
        }
        if (parsed.transcriptionBackend !== undefined) {
            settings.value.transcriptionBackend = parsed.transcriptionBackend;
        }
        if (parsed.parakeetModelPath !== undefined) {
            settings.value.parakeetModelPath = parsed.parakeetModelPath;
        }
        if (parsed.sortformerModelPath !== undefined) {
            settings.value.sortformerModelPath = parsed.sortformerModelPath;
        }

        status.value = "Loaded existing transcript and settings.";
    } catch (e) {
        // Ignore error if file doesn't exist
        console.log("No existing transcript found or error loading it.");
    }
}

async function saveTranscript() {
    if (!inputPath.value) return;
    const transcriptPath = inputPath.value + ".transcript.json";
    try {
        await invoke("write_text_file", { 
            path: transcriptPath, 
            content: JSON.stringify(buildTranscriptSidecar(transcriptWorkspaceState.value), null, 2) 
        });
        console.log("Transcript saved.");
    } catch (e) {
        console.error("Failed to save transcript:", e);
    }
}

async function translateTranscript() {
    if (!targetLanguage.value || segments.value.length === 0) return;
    
    const lang = targetLanguage.value.trim();
    if (translations.value[lang]) {
        currentLanguage.value = lang;
        return;
    }

    isTranslating.value = true;
    status.value = `Translating to ${lang}...`;

    try {
        const response = await invoke<string>("translate_transcript", {
            transcript: segments.value,
            targetLanguage: lang,
            context: context.value,
            apiKey: settings.value.apiKey,
            baseUrl: settings.value.baseUrl,
            model: settings.value.model
        });

        const jsonMatch = response.match(/\[[\s\S]*\]/);
        if (jsonMatch) {
            try {
                translations.value[lang] = parseTranscriptResponse(response);
                currentLanguage.value = lang;
                status.value = `Translation to ${lang} complete.`;
            } catch (e) {
                console.error("JSON Parse Error", e);
                showError(
                    "Failed to parse translation from AI response.",
                    response,
                    e instanceof Error ? e.message : String(e)
                );
            }
        } else {
            console.error(response);
            showError(
                "Failed to find JSON in translation response.",
                response
            );
        }
    } catch (e) {
        console.error("Translation failed:", e);
        status.value = `Translation failed: ${e}`;
    } finally {
        isTranslating.value = false;
    }
}

function showError(message: string, rawResponse: string, parseError: string = "") {
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
    const relevant = executionHistory.value.filter(h => h.type === type);
    if (relevant.length === 0) {
        // Default estimates
        if (type === 'analysis') return inputSize * 0.1; // e.g. 10% of audio duration
        if (type === 'generation') return inputSize * 0.005; // e.g. 5ms per char
        return 30;
    }
    const rate = relevant.reduce((acc, h) => acc + (h.duration / h.inputSize), 0) / relevant.length;
    return inputSize * rate;
}

function logExecution(type: 'analysis' | 'generation', inputSize: number, duration: number) {
    executionHistory.value.push({ type, inputSize, duration, timestamp: Date.now() });
    if (executionHistory.value.length > 20) executionHistory.value.shift();
    localStorage.setItem('executionHistory', JSON.stringify(executionHistory.value));
}

async function analyzeWithLlmTranscript(
    analysisAudioPath: string,
    adjustTimestamps?: boolean,
    processedOffsets?: ProcessedAudio['offsets'],
): Promise<TranscriptSegment[]> {
    const isGoogleApi = settings.value.baseUrl.includes('generativelanguage.googleapis.com');
    let uri: string | null = null;
    let audioBase64: string | null = null;

    if (isGoogleApi) {
        status.value = "Uploading file...";
        uri = await invoke<string | null>("upload_file", {
            apiKey: settings.value.apiKey,
            baseUrl: settings.value.baseUrl,
            path: analysisAudioPath
        });

        if (uri) {
            status.value = "File uploaded successfully";
        }
    } else {
        status.value = "Encoding audio as base64...";
        audioBase64 = await invoke<string>("read_file_as_base64", { path: analysisAudioPath });
        status.value = "Audio encoded successfully";
    }

    const response = await invoke<string>("analyze_audio", {
        apiKey: settings.value.apiKey,
        baseUrl: settings.value.baseUrl,
        model: settings.value.model,
        enforceJsonSchema: settings.value.enforceJsonSchema,
        context: context.value,
        glossary: settings.value.glossary,
        speakerCount: speakerCount.value,
        removeFillerWords: removeFillerWords.value,
        audioUri: uri,
        audioBase64: audioBase64
    });

    const timestampAdjuster = adjustTimestamps && processedOffsets
        ? (timestamp: string) => adjustTimestamp(timestamp, processedOffsets)
        : undefined;

    return parseTranscriptResponse(response, timestampAdjuster);
}

function adjustWordWithOffsets(word: TranscriptWord, offsets: ProcessedAudio['offsets']): TranscriptWord {
    return {
        ...word,
        start: adjustTimestamp(word.start, offsets),
        end: adjustTimestamp(word.end, offsets),
    };
}

function adjustSegmentsWithOffsets(
    transcriptSegments: TranscriptSegment[],
    offsets: ProcessedAudio['offsets'],
): TranscriptSegment[] {
    return transcriptSegments.map((segment) => ({
        ...segment,
        start: adjustTimestamp(segment.start, offsets),
        end: adjustTimestamp(segment.end, offsets),
        words: segment.words?.map((word) => adjustWordWithOffsets(word, offsets)),
    }));
}

async function processFile() {
    if (!inputPath.value) {
        status.value = "Please provide a media file.";
        return;
    }

    if (!hasMediaFile.value) {
        status.value = "Selected media file could not be found. Choose a valid file to continue.";
        return;
    }

    if (!hasBackendConfiguration.value) {
        status.value = settings.value.transcriptionBackend === 'parakeet'
            ? "Parakeet models will auto-download or use your custom paths. Please retry if FFmpeg is not initialized."
            : settings.value.transcriptionBackend === 'llm'
            ? "Please provide an API key."
            : "Please provide an API key for hybrid cleanup.";
        return;
    }

    isProcessing.value = true;
    progressPercentage.value = null;
    progressEtaSeconds.value = null;
    status.value = "Preparing audio...";
    segments.value = [];

    try {
        const failStage = (stage: string, error: unknown) => {
            const details = error instanceof Error ? error.message : String(error);
            const message = `${stage} failed.`;
            showError(message, details);
            status.value = `${message} ${details}`;
            throw new Error(message);
        };

        let audioInfo: AudioInfo;
        try {
            audioInfo = await invoke<AudioInfo>("prepare_audio_for_ai", { inputPath: inputPath.value });
        } catch (error) {
            failStage("Audio preparation", error);
            return;
        }
        status.value = `Audio prepared: ${audioInfo.path} (${(audioInfo.size / 1024 / 1024).toFixed(2)} MB)`;

        let processedAudio: ProcessedAudio;
        if (trimSilence.value) {
            status.value = "Removing silence...";
            try {
                processedAudio = await invoke<ProcessedAudio>("remove_silence", { path: audioInfo.path });
            } catch (error) {
                failStage("Silence removal", error);
                return;
            }
            console.log(`Found ${processedAudio.silence_intervals.length} silence intervals.`);
        } else {
            processedAudio = {
                path: audioInfo.path,
                silence_intervals: [],
                offsets: [{ min_time: 0.0, offset: 0.0 }]
            };
        }
        
        // Use processed audio for upload/analysis
        const analysisAudioPath = processedAudio.path;

        const estimatedTime = estimateTime('analysis', audioInfo.duration);
        status.value = isLlmOnlyBackend.value
            ? `Analyzing with AI... (Est. ${estimatedTime.toFixed(0)}s)`
            : settings.value.transcriptionBackend === 'hybrid'
                ? `Running hybrid transcription... (Est. ${estimatedTime.toFixed(0)}s)`
                : settings.value.transcriptionBackend === 'hybrid-merge'
                    ? `Running merged hybrid transcription... (Est. ${estimatedTime.toFixed(0)}s)`
                : `Transcribing with Parakeet... (Est. ${estimatedTime.toFixed(0)}s)`;
        const startTime = Date.now();
        let hybridCleanupUsedFallback = false;

        startSimulatedProgress(estimatedTime);
        let nextSegments: TranscriptSegment[] = [];
        try {
            if (isLlmOnlyBackend.value) {
                try {
                    nextSegments = await analyzeWithLlmTranscript(
                        analysisAudioPath,
                        true,
                        processedAudio.offsets,
                    );
                } catch (error) {
                    failStage("AI analysis request", error);
                    return;
                }
            } else {
                let parakeetSegments: TranscriptSegment[];
                try {
                    parakeetSegments = await invoke<TranscriptSegment[]>("transcribe_with_parakeet", {
                        audioPath: analysisAudioPath,
                        parakeetModelPath: settings.value.parakeetModelPath,
                        sortformerModelPath: settings.value.sortformerModelPath,
                    });
                } catch (error) {
                    failStage("Parakeet transcription", error);
                    return;
                }

                if (settings.value.transcriptionBackend === 'hybrid') {
                    status.value = "Cleaning transcript with AI...";
                    const originalSegments = parakeetSegments;
                    try {
                        nextSegments = await invoke<TranscriptSegment[]>("cleanup_parakeet_transcript", {
                            apiKey: settings.value.apiKey,
                            baseUrl: settings.value.baseUrl,
                            model: settings.value.model,
                            transcript: parakeetSegments,
                            context: context.value,
                            glossary: settings.value.glossary,
                            removeFillerWords: removeFillerWords.value,
                        });
                    } catch (error) {
                        console.warn("Hybrid cleanup failed, using Parakeet transcript", error);
                        nextSegments = originalSegments;
                        hybridCleanupUsedFallback = true;
                    }
                } else if (settings.value.transcriptionBackend === 'hybrid-merge') {
                    status.value = "Querying remote transcript for merge...";
                    let referenceTranscript: TranscriptSegment[];
                    try {
                        referenceTranscript = await analyzeWithLlmTranscript(analysisAudioPath);
                    } catch (error) {
                        console.warn("Merged hybrid remote transcript failed, using Parakeet transcript", error);
                        nextSegments = parakeetSegments;
                        hybridCleanupUsedFallback = true;
                        referenceTranscript = [];
                    }

                    if (referenceTranscript.length > 0) {
                        status.value = "Merging Parakeet and remote transcripts...";
                        try {
                            nextSegments = await invoke<TranscriptSegment[]>("merge_transcript_hypotheses", {
                                primaryTranscript: parakeetSegments,
                                referenceTranscript,
                            });
                        } catch (error) {
                            console.warn("Merged hybrid reconciliation failed, using Parakeet transcript", error);
                            nextSegments = parakeetSegments;
                            hybridCleanupUsedFallback = true;
                        }
                    } else {
                        nextSegments = parakeetSegments;
                        hybridCleanupUsedFallback = true;
                    }
                } else {
                    nextSegments = parakeetSegments;
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

        segments.value = nextSegments;
        status.value = isLlmOnlyBackend.value
            ? `Analysis complete. Found ${segments.value.length} segments.`
            : settings.value.transcriptionBackend === 'hybrid'
                ? hybridCleanupUsedFallback
                    ? `Hybrid cleanup failed, using Parakeet transcript. Found ${segments.value.length} segments.`
                    : `Hybrid transcription complete. Found ${segments.value.length} segments.`
                : settings.value.transcriptionBackend === 'hybrid-merge'
                    ? hybridCleanupUsedFallback
                        ? `Hybrid merge failed, using Parakeet transcript. Found ${segments.value.length} segments.`
                        : `Hybrid merge complete. Found ${segments.value.length} segments.`
                : `Parakeet transcription complete. Found ${segments.value.length} segments.`;

        lastAnalyzedSettings.value = {
            context: context.value,
            glossary: settings.value.glossary,
            speakerCount: speakerCount.value,
            removeFillerWords: removeFillerWords.value,
            trimSilence: trimSilence.value,
            transcriptionBackend: settings.value.transcriptionBackend,
            parakeetModelPath: settings.value.parakeetModelPath,
            sortformerModelPath: settings.value.sortformerModelPath,
        };

        await saveTranscript();

        if (isLlmOnlyBackend.value && useAdvancedAlignment.value && segments.value.length > 0) {
            status.value = "Aligning transcript with local model...";
            try {
                const alignedSegments = await invoke<TranscriptSegment[]>("align_transcript", {
                    audioPath: audioInfo.path,
                    transcript: segments.value
                });
                segments.value = alignedSegments;
                status.value = `Alignment complete. Adjusted ${segments.value.length} segments.`;
                await saveTranscript();
            } catch (e) {
                console.error("Alignment failed", e);
                status.value = `Alignment failed: ${e}. Using original timestamps.`;
            }
        }

    } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        if (!showErrorOverlay.value) {
            showError(
                "Analysis failed before transcription completed.",
                message
            );
        }
        status.value = message;
    } finally {
        isProcessing.value = false;
        progressPercentage.value = null;
        progressEtaSeconds.value = null;
    }
}

async function cutVideo() {
    if (segments.value.length === 0) return;
    if (!hasMediaFile.value) {
        status.value = "Select a valid media file before exporting video.";
        return;
    }

    status.value = "Cutting media...";
    isProcessing.value = true;
    progressPercentage.value = null;
    progressEtaSeconds.value = null;

    try {
        const cutSegments = segments.value.map(s => ({ start: s.start, end: s.end }));
        const outputPath = inputPath.value.replace(/(\.[\ w\d]+)$/, "_cut$1");

        await invoke("cut_video", {
            inputPath: inputPath.value,
            segments: cutSegments,
            outputPath
        });

        status.value = `Media cut successfully to ${outputPath}`;
    } catch (e) {
        status.value = `Error cutting media: ${e}`;
    } finally {
        isProcessing.value = false;
        progressPercentage.value = null;
        progressEtaSeconds.value = null;
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
            { title: 'Merge Speakers?', kind: 'warning' }
        );
        
        if (!confirmed) {
            inputElement.value = oldName;
            return;
        }
    }

    // Update segments
    segments.value = segments.value.map(seg => {
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
    <div class="min-h-screen bg-gray-900 text-gray-200 p-8 font-sans selection:bg-blue-500/30">
        <div class="max-w-5xl mx-auto">
            <HomeSourcePanel
                :currentEngineLabel="currentEngineLabel"
                :currentModelDisplay="currentModelDisplay"
                :inputPath="inputPath"
                :hasMediaFile="hasMediaFile"
                :isProcessing="isProcessing"
                :hasBackendConfiguration="hasBackendConfiguration"
                :hasTranscript="hasTranscript"
                :settingsChanged="settingsChanged"
                :transcriptionBackend="settings.transcriptionBackend"
                :context="context"
                :glossary="settings.glossary"
                :speakerCount="speakerCount"
                :removeFillerWords="removeFillerWords"
                :trimSilence="trimSilence"
                @update:inputPath="inputPath = $event"
                @update:transcriptionBackend="settings.transcriptionBackend = $event"
                @update:context="context = $event"
                @update:glossary="settings.glossary = $event"
                @update:speakerCount="speakerCount = $event"
                @update:removeFillerWords="removeFillerWords = $event"
                @update:trimSilence="trimSilence = $event"
                @invalid-selection="updateStatus"
                @save-session="sessionPersistence.handleSaveSession"
                @load-session="sessionPersistence.handleLoadSession"
                @open-settings="goToSettings"
                @process="processFile"
            />

            <!-- Editor Section -->
            <transition name="fade">
                <TranscriptWorkspacePanel
                    v-if="segments.length > 0"
                    :inputPath="inputPath"
                    :hasMediaFile="hasMediaFile"
                    :displaySegments="displaySegments"
                    :originalSegments="segments"
                    :translations="translations"
                    :currentLanguage="currentLanguage"
                    :targetLanguage="targetLanguage"
                    :isTranslating="isTranslating"
                    :isLlmOnlyBackend="isLlmOnlyBackend"
                    :useAdvancedAlignment="useAdvancedAlignment"
                    :uniqueSpeakers="uniqueSpeakers"
                    :isProcessing="isProcessing"
                    @update:currentLanguage="currentLanguage = $event"
                    @update:targetLanguage="targetLanguage = $event"
                    @translate="translateTranscript"
                    @export-video="cutVideo"
                    @update:useAdvancedAlignment="useAdvancedAlignment = $event"
                    @rename-speaker="renameSpeaker($event.oldName, $event.newName, $event.inputElement)"
                    @update:segments="displaySegments = $event"
                />
            </transition>

            <!-- Viral Clips Generator -->
            <transition name="fade">
                <ViralClipsGenerator
                    v-if="segments.length > 0"
                    :segments="segments"
                    :inputPath="inputPath"
                    :hasMediaFile="hasMediaFile"
                    :state="viralClipsState"
                    class="mb-8"
                    @update:status="updateStatus"
                    @update:processing="updateProcessing"
                    @update:state="viralClipsState = $event"
                />
            </transition>

            <!-- Podcast Generator -->
            <transition name="fade">
                <PodcastGenerator
                    v-if="segments.length > 0"
                    :segments="segments"
                    :inputPath="inputPath"
                    :hasMediaFile="hasMediaFile"
                    :state="podcastWorkspaceState"
                    class="mb-20"
                    @update:status="updateStatus"
                    @update:processing="updateProcessing"
                    @update:state="podcastWorkspaceState = $event"
                />
            </transition>

            <!-- Clip Generator -->
            <transition name="fade">
                <div v-if="segments.length > 0" class="mb-20">
                    <ClipGenerator
                        v-model:count="clipCount"
                        v-model:minDuration="clipMinDuration"
                        v-model:maxDuration="clipMaxDuration"
                        v-model:topic="clipTopic"
                        v-model:splicing="allowSplicing"
                        :isProcessing="isProcessing"
                        @generate="clipGeneration.generateClips"
                    />

                    <ClipList
                        :clips="clips"
                        :lastExportPath="lastExportPath"
                        :isProcessing="isProcessing"
                        :hasMediaFile="hasMediaFile"
                        :includeSubtitles="includeSubtitles"
                        :fastMode="fastMode"
                        :trimBoundarySilence="clipTrimBoundarySilence"
                        :selectedClipIndices="selectedClipIndices"
                        @export="clipGeneration.exportClips"
                        @openFolder="clipGeneration.openExportFolder"
                        @update:includeSubtitles="includeSubtitles = $event"
                        @update:fastMode="fastMode = $event"
                        @update:trimBoundarySilence="clipTrimBoundarySilence = $event"
                        @update:selectedClipIndices="selectedClipIndices = $event"
                    />
                </div>
            </transition>
        </div>
    </div>
    <!-- Status Bar (Outside main container to ensure fixed positioning works) -->
    <div class="fixed bottom-0 left-0 right-0 p-4 bg-black/50 backdrop-blur-md border-t border-white/10 flex items-center justify-between z-50">
        <div class="max-w-5xl mx-auto w-full flex flex-col gap-2">
            <div v-if="progressPercentage !== null" class="w-full bg-gray-700 rounded-full h-1.5 overflow-hidden">
                <div class="bg-blue-500 h-full transition-all duration-300 ease-out" :style="{ width: `${progressPercentage}%` }"></div>
            </div>
            <div class="flex items-center gap-3">
                <div class="w-2 h-2 rounded-full"
                    :class="isProcessing ? 'bg-yellow-400 animate-pulse' : 'bg-emerald-400'"></div>
                <span class="text-sm font-mono text-gray-400 truncate">{{ status }}</span>
                <span v-if="progressEtaSeconds !== null && progressPercentage !== null && progressPercentage < 100" class="text-xs font-mono text-gray-500 whitespace-nowrap">
                    ETA {{ Math.floor(Math.ceil(progressEtaSeconds) / 60) }}:{{ (Math.ceil(progressEtaSeconds) % 60).toString().padStart(2, '0') }}
                </span>
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
    />
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
    transition: opacity 0.5s ease, transform 0.5s ease;
}

.fade-enter-from,
.fade-leave-to {
    opacity: 0;
    transform: translateY(20px);
}
</style>
