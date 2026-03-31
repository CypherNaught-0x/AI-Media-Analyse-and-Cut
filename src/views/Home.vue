<script setup lang="ts">
import { ref, onMounted, computed, watch } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from '@tauri-apps/api/event';
import { ask } from '@tauri-apps/plugin-dialog';
import { useRouter } from 'vue-router';
import Editor from "../components/Editor.vue";
import SubtitleExport from "../components/SubtitleExport.vue";
import ViralClipsGenerator from "../components/ViralClipsGenerator.vue";
import PodcastGenerator from "../components/PodcastGenerator.vue";
import ErrorOverlay from "../components/ErrorOverlay.vue";
import type { TranscriptSegment, AudioInfo, ProcessedAudio, SilenceInterval, ClipExportPayload, Clip, TranscriptionBackend } from "../types";
import FileSelector from "../components/FileSelector.vue";
import AnalysisSettings from "../components/AnalysisSettings.vue";
import ClipGenerator from "../components/ClipGenerator.vue";
import ClipList from "../components/ClipList.vue";
import StatusBar from "../components/StatusBar.vue";
import { useSettings } from "../composables/useSettings";
import { parseTime, adjustTimestamp, formatTime } from "../composables/useTimeFormat";
import { generateSubtitleContent } from "../utils/subtitle";
import { trimClipBoundarySilence } from "../utils/clipSilence";
import { parseTranscriptResponse } from "../utils/transcriptParsing";

import LightningIcon from '../assets/icons/lightning.svg?component';
import SpinnerIcon from '../assets/icons/spinner.svg?component';
import UserIcon from '../assets/icons/user.svg?component';
import TranslateIcon from '../assets/icons/translate.svg?component';
import CheckIcon from '../assets/icons/check.svg?component';
import ChevronDownIcon from '../assets/icons/chevron-down.svg?component';
import type { TranscriptWord } from '../types';

const router = useRouter();
const { settings } = useSettings();

const SUPPORTED_LANGUAGES = [
    { code: 'en', name: 'English', country: 'us' },
    { code: 'es', name: 'Spanish', country: 'es' },
    { code: 'fr', name: 'French', country: 'fr' },
    { code: 'de', name: 'German', country: 'de' },
    { code: 'it', name: 'Italian', country: 'it' },
    { code: 'pt', name: 'Portuguese', country: 'pt' },
    { code: 'nl', name: 'Dutch', country: 'nl' },
    { code: 'ru', name: 'Russian', country: 'ru' },
    { code: 'ja', name: 'Japanese', country: 'jp' },
    { code: 'zh', name: 'Chinese', country: 'cn' },
    { code: 'ko', name: 'Korean', country: 'kr' },
    { code: 'hi', name: 'Hindi', country: 'in' },
    { code: 'ar', name: 'Arabic', country: 'sa' },
    { code: 'tr', name: 'Turkish', country: 'tr' },
    { code: 'pl', name: 'Polish', country: 'pl' },
];

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
const executionHistory = ref<{type: string, inputSize: number, duration: number, timestamp: number}[]>([]);
const inputPath = ref("");
const segments = ref<TranscriptSegment[]>([]);
const translations = ref<Record<string, TranscriptSegment[]>>({});
const currentLanguage = ref("Original");
const targetLanguage = ref("");
const isTranslating = ref(false);
const showLanguageDropdown = ref(false);
const removeFillerWords = ref(false);
const trimSilence = ref(true);
const videoRef = ref<HTMLVideoElement | null>(null);

const speakerCount = ref<number | null>(null);
const context = ref("");
const useAdvancedAlignment = ref(false);
const clipCount = ref(3);
const clipMinDuration = ref(10);
const clipMaxDuration = ref(120);
const clipTopic = ref("");
const allowSplicing = ref(false);
const clips = ref<Clip[]>([]);
const lastExportPath = ref("");
const clipExportSilenceCache = ref<{ path: string; intervals: SilenceInterval[] } | null>(null);
const speakerOrder = ref<string[]>([]);

const lastAnalyzedSettings = ref({
    context: '',
    glossary: '',
    speakerCount: null as number | null,
    removeFillerWords: false,
    trimSilence: true,
    transcriptionBackend: 'llm' as TranscriptionBackend,
    parakeetModelPath: '',
    sortformerModelPath: '',
});

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

onMounted(async () => {
    const history = localStorage.getItem('executionHistory');
    if (history) {
        try {
            executionHistory.value = JSON.parse(history);
        } catch (e) {
            console.error("Failed to parse execution history", e);
        }
    }

    try {
        const res = await invoke<string>("init_ffmpeg");
        status.value = res;
        
        await listen<any>('progress', (event) => {
            const payload = event.payload;
            if (typeof payload === 'number') {
                 status.value = `Processing... ${payload.toFixed(1)}s`;
            } else if (typeof payload === 'object') {
                 if (payload.percentage !== undefined) {
                     if (progressInterval) {
                         clearInterval(progressInterval);
                         progressInterval = null;
                     }
                     progressPercentage.value = payload.percentage;
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

watch(inputPath, () => {
    segments.value = [];
    speakerOrder.value = [];
    translations.value = {};
    currentLanguage.value = "Original";
    loadTranscript();
});

watch(segments, () => {
    syncSpeakerOrder();
}, { deep: true });

async function loadTranscript() {
    if (!inputPath.value) return;
    const transcriptPath = inputPath.value + ".transcript.json";
    try {
        const content = await invoke<string>("read_text_file", { path: transcriptPath });
        const parsed = JSON.parse(content);
        if (Array.isArray(parsed)) {
            segments.value = parsed;
            status.value = "Loaded existing transcript.";
        } else if (parsed && typeof parsed === 'object') {
            if (Array.isArray(parsed.segments)) {
                segments.value = parsed.segments;
            }
            if (typeof parsed.context === 'string') {
                context.value = parsed.context;
            }
            if (typeof parsed.glossary === 'string') {
                settings.value.glossary = parsed.glossary;
            }
            if (typeof parsed.speakerCount === 'number' || parsed.speakerCount === null) {
                speakerCount.value = parsed.speakerCount;
            }
            if (typeof parsed.removeFillerWords === 'boolean') {
                removeFillerWords.value = parsed.removeFillerWords;
            }
            if (typeof parsed.trimSilence === 'boolean') {
                trimSilence.value = parsed.trimSilence;
            }
            
            // Update last analyzed settings
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
            
            status.value = "Loaded existing transcript and settings.";
        }
    } catch (e) {
        // Ignore error if file doesn't exist
        console.log("No existing transcript found or error loading it.");
    }
}

async function saveTranscript() {
    if (!inputPath.value || segments.value.length === 0) return;
    const transcriptPath = inputPath.value + ".transcript.json";
    try {
        const data = {
            segments: segments.value,
            context: context.value,
            glossary: settings.value.glossary,
            speakerCount: speakerCount.value,
            removeFillerWords: removeFillerWords.value,
            trimSilence: trimSilence.value,
            transcriptionBackend: settings.value.transcriptionBackend,
            parakeetModelPath: settings.value.parakeetModelPath,
            sortformerModelPath: settings.value.sortformerModelPath,
        };
        await invoke("write_text_file", { 
            path: transcriptPath, 
            content: JSON.stringify(data, null, 2) 
        });
        console.log("Transcript saved.");
    } catch (e) {
        console.error("Failed to save transcript:", e);
    }
}

function selectLanguage(langName: string) {
    targetLanguage.value = langName;
    showLanguageDropdown.value = false;
    
    // If translation exists, switch to it
    if (translations.value[langName]) {
        currentLanguage.value = langName;
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
    const startTime = Date.now();
    
    progressInterval = window.setInterval(() => {
        const elapsed = (Date.now() - startTime) / 1000;
        const p = (elapsed / estimatedSeconds) * 100;
        // Cap at 99% so it doesn't look finished until it actually is
        progressPercentage.value = Math.min(p, 99);
    }, 100);
}

function stopSimulatedProgress() {
    if (progressInterval) {
        clearInterval(progressInterval);
        progressInterval = null;
    }
    progressPercentage.value = 100;
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
    }
}

async function cutVideo() {
    if (segments.value.length === 0) return;

    status.value = "Cutting media...";
    isProcessing.value = true;
    progressPercentage.value = null;

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
    }
}

async function generateClips() {
    if (segments.value.length === 0) return;
    
    status.value = "Generating clips...";
    isProcessing.value = true;
    progressPercentage.value = null;
    
    try {
        const transcript = segments.value
            .map(s => `[${s.start}-${s.end}] ${s.speaker}: ${s.text}`)
            .join("\n");
            
        const estimatedTime = estimateTime('generation', transcript.length);
        status.value = `Generating clips... (Est. ${estimatedTime.toFixed(0)}s)`;
        const startTime = Date.now();

        startSimulatedProgress(estimatedTime);
        let response: string;
        try {
            response = await invoke<string>("generate_clips", {
                apiKey: settings.value.apiKey,
                baseUrl: settings.value.baseUrl,
                model: settings.value.model,
                transcript,
                count: clipCount.value,
                minDuration: clipMinDuration.value,
                maxDuration: clipMaxDuration.value,
                topic: clipTopic.value || null,
                splicing: allowSplicing.value
            });
        } finally {
            stopSimulatedProgress();
        }

        const duration = (Date.now() - startTime) / 1000;
        logExecution('generation', transcript.length, duration);
        
        const jsonMatch = response.match(/\[[\s\S]*\]/);
        if (jsonMatch) {
            try {
                const parsed = JSON.parse(jsonMatch[0]);
                if (!Array.isArray(parsed)) throw new Error("Response is not an array");
                
                // Normalize clips to always have 'segments'
                clips.value = parsed.map((c: any) => {
                    if (c.segments) return c;
                    // Backward compatibility for AI response without segments
                    return {
                        ...c,
                        segments: [{ start: c.start, end: c.end }]
                    };
                });
                
                status.value = `Found ${clips.value.length} clips.`;
            } catch (e) {
                console.error("JSON Parse Error", e);
                status.value = "Failed to parse clips from AI response. Check console for details.";
            }
        } else {
            status.value = "Failed to find JSON in AI response.";
            console.error(response);
        }
    } catch (e) {
        status.value = `Error generating clips: ${e}`;
    } finally {
        isProcessing.value = false;
        progressPercentage.value = null;
    }
}

async function getClipExportSilenceIntervals(): Promise<SilenceInterval[]> {
    if (clipExportSilenceCache.value?.path === inputPath.value) {
        return clipExportSilenceCache.value.intervals;
    }

    status.value = "Detecting clip boundary silence...";
    const intervals = await invoke<SilenceInterval[]>("detect_silence", { path: inputPath.value });
    clipExportSilenceCache.value = { path: inputPath.value, intervals };
    return intervals;
}

async function exportClips(payload?: ClipExportPayload) {
    const clipsToExport = payload?.clips || clips.value;
    const includeSubtitles = payload?.includeSubtitles || false;
    const fastMode = payload?.fastMode || false;
    const trimBoundarySilence = payload?.trimBoundarySilence || false;

    if (clipsToExport.length === 0) return;
    
    status.value = "Exporting clips...";
    isProcessing.value = true;
    progressPercentage.value = null;
    
    try {
        // Robust extension replacement
        const outputDir = inputPath.value.replace(/\.[^/\\.]+$/, "") + "_clips";
        
        const prePadding = settings.value.preClipPadding || 0;
        const postPadding = settings.value.postClipPadding || 0;
        const maxDuration = videoRef.value?.duration || Infinity;

        let clipSegments = clipsToExport.map(c => ({ 
            segments: c.segments.map(s => {
                const start = Math.max(0, parseTime(s.start) - prePadding);
                const end = Math.min(maxDuration, parseTime(s.end) + postPadding);
                return {
                    start: formatTime(start),
                    end: formatTime(end)
                };
            }),
            label: c.title,
            reason: c.reason
        }));

        if (trimBoundarySilence) {
            try {
                const silenceIntervals = await getClipExportSilenceIntervals();
                clipSegments = clipSegments.map((clip) => ({
                    ...clip,
                    segments: trimClipBoundarySilence(clip.segments, silenceIntervals),
                }));
            } catch (e) {
                console.warn("Failed to detect silence for clip export", e);
                status.value = "Silence detection failed, exporting without boundary trimming...";
            }
        }
        
        console.log({outputDir});
        
        status.value = `Exporting to ${outputDir}...`;
        await invoke("export_clips", {
            inputPath: inputPath.value,
            segments: clipSegments,
            outputDir,
            fastMode
        });

        if (includeSubtitles) {
            status.value = "Generating subtitles...";
            for (let i = 0; i < clipSegments.length; i++) {
                const clip = clipSegments[i];
                
                // Reconstruct filename logic from Rust
                const suffix = clip.label
                    ? clip.label.replace(/[^a-zA-Z0-9-_]/g, "")
                    : "";
                const indexStr = (i + 1).toString().padStart(3, '0');
                const filename = suffix ? `clip_${indexStr}_${suffix}.srt` : `clip_${indexStr}.srt`;
                const outputPath = `${outputDir}\\${filename}`; // Assuming Windows based on context, but should use path separator

                // Generate transcript for this clip
                const clipTranscript: TranscriptSegment[] = [];
                let currentOffset = 0;

                for (const seg of clip.segments) {
                    const segStart = parseTime(seg.start);
                    const segEnd = parseTime(seg.end);
                    const duration = segEnd - segStart;

                    // Find overlapping segments in full transcript
                    const overlapping = segments.value.filter(t => {
                        const tStart = parseTime(t.start);
                        const tEnd = parseTime(t.end);
                        // Intersection > 0
                        return Math.max(tStart, segStart) < Math.min(tEnd, segEnd);
                    });

                    for (const t of overlapping) {
                        const tStart = parseTime(t.start);
                        const tEnd = parseTime(t.end);
                        
                        const effStart = Math.max(tStart, segStart);
                        const effEnd = Math.min(tEnd, segEnd);
                        
                        if (effEnd > effStart) {
                            const relStart = currentOffset + (effStart - segStart);
                            const relEnd = currentOffset + (effEnd - segStart);
                            
                            clipTranscript.push({
                                start: formatTime(relStart),
                                end: formatTime(relEnd),
                                text: t.text,
                                speaker: t.speaker
                            });
                        }
                    }
                    currentOffset += duration;
                }

                if (clipTranscript.length > 0) {
                    const srtContent = generateSubtitleContent(clipTranscript, 'srt');
                    await invoke("write_text_file", { path: outputPath, content: srtContent });
                }
            }
        }
        
        lastExportPath.value = outputDir;
        status.value = `Clips exported to ${outputDir}`;
    } catch (e) {
        status.value = `Error exporting clips: ${e}`;
    } finally {
        isProcessing.value = false;
        progressPercentage.value = null;
    }
}

async function openExportFolder() {
    if (lastExportPath.value) {
        await invoke("open_folder", { path: lastExportPath.value });
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

function jumpTo(time: number) {
    if (videoRef.value) {
        videoRef.value.currentTime = time;
        videoRef.value.play();
    }
}

function onTimeUpdate() {
    if (!videoRef.value || segments.value.length === 0) return;
    
    const currentTime = videoRef.value.currentTime;
    
    // Check if current time is inside any segment
    // We assume segments are sorted by start time
    let inside = false;
    let nextStart = -1;

    for (const seg of segments.value) {
        const start = parseTime(seg.start);
        const end = parseTime(seg.end);

        if (currentTime >= start && currentTime < end) {
            inside = true;
            break;
        }
        if (start > currentTime) {
            nextStart = start;
            break;
        }
    }
    
    if (!inside && nextStart !== -1) {
        // Jump to next segment
        videoRef.value.currentTime = nextStart;
    } else if (!inside && nextStart === -1) {
        const lastEnd = parseTime(segments.value[segments.value.length - 1].end);
        if (currentTime > lastEnd) {
            // End of video
            videoRef.value.pause();
        }
    }
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
            <div class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl mb-8">

                <!-- Settings Display -->
                <div class="mb-8 flex items-center justify-between bg-black/20 p-4 rounded-2xl border border-white/5">
                    <div class="flex items-center gap-4">
                        <div class="w-10 h-10 rounded-full bg-blue-500/20 flex items-center justify-center text-blue-400">
                            <LightningIcon class="h-6 w-6" />
                        </div>
                        <div>
                            <label class="block text-xs font-medium text-gray-400 uppercase tracking-wider">{{ currentEngineLabel }}</label>
                            <div class="text-white font-medium">{{ currentModelDisplay }}</div>
                        </div>
                    </div>
                    <button @click="goToSettings"
                        class="px-6 py-2 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-xl transition-all border border-white/10">
                        Settings
                    </button>
                </div>

                <!-- File Selection Section -->
                <FileSelector v-model="inputPath" @invalid-selection="updateStatus" />

                <!-- Analysis Settings -->
                <AnalysisSettings
                    v-model:transcriptionBackend="settings.transcriptionBackend"
                    v-model:context="context"
                    v-model:glossary="settings.glossary"
                    v-model:speakerCount="speakerCount"
                    v-model:removeFillerWords="removeFillerWords"
                    v-model:trimSilence="trimSilence"
                />

                <!-- Action Buttons -->
                <div class="flex gap-4 mb-6">
                    <button @click="processFile" :disabled="isProcessing || !hasBackendConfiguration || (hasTranscript && !settingsChanged)"
                        class="flex-1 bg-blue-600 hover:bg-blue-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg shadow-blue-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all transform hover:-translate-y-0.5 active:translate-y-0 flex items-center justify-center gap-2">
                        <SpinnerIcon v-if="isProcessing" class="animate-spin h-5 w-5 text-white" />
                        {{ isProcessing ? 'Processing...' : (hasTranscript && !settingsChanged ? 'Transcript Loaded' : (hasTranscript ? 'Re-analyze Media' : 'Analyze Media')) }}
                    </button>
                </div>
            </div>

            <!-- Editor Section -->
            <transition name="fade">
                <div v-if="segments.length > 0"
                    class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl mb-8">
                    
                    <!-- Video Preview -->
                    <div class="mb-8 bg-black rounded-xl overflow-hidden border border-white/10 shadow-2xl">
                        <video 
                            ref="videoRef"
                            :src="convertFileSrc(inputPath)"
                            class="w-full max-h-[500px] mx-auto"
                            controls
                            @timeupdate="onTimeUpdate"
                        ></video>
                    </div>

                    <div class="flex justify-between items-center mb-6">
                        <div class="flex items-center gap-4">
                            <h2 class="text-2xl font-bold text-white">Transcript</h2>
                            <span class="px-3 py-1 rounded-full bg-white/10 text-gray-300 text-xs font-bold border border-white/10">
                                {{ displaySegments.length }} Segments
                            </span>
                        </div>
                        <div class="flex items-center gap-3">
                            <!-- Language Selector -->
                            <div class="flex items-center gap-2 bg-black/20 rounded-lg p-1 border border-white/10">
                                <select v-model="currentLanguage" class="bg-transparent text-xs text-gray-300 outline-none border-none py-1 pl-2 pr-2 cursor-pointer [&>option]:bg-gray-900">
                                    <option value="Original">Original</option>
                                    <option v-for="(_, lang) in translations" :key="lang" :value="lang">{{ lang }}</option>
                                </select>
                            </div>

                            <!-- New Translation Dropdown -->
                            <div class="relative">
                                <div class="flex items-center gap-2">
                                    <button @click="showLanguageDropdown = !showLanguageDropdown" 
                                        class="flex items-center gap-2 w-32 bg-white/5 border border-white/10 rounded-lg px-2 py-1.5 text-xs text-gray-300 outline-none hover:bg-white/10 transition-colors">
                                        <span class="truncate flex-1 text-left">{{ targetLanguage || 'Select Language' }}</span>
                                        <ChevronDownIcon class="h-3 w-3 text-gray-500" />
                                    </button>
                                    
                                    <button @click="translateTranscript" :disabled="isTranslating || !targetLanguage || !!translations[targetLanguage]" 
                                        class="p-1.5 bg-blue-600/20 hover:bg-blue-600/40 text-blue-400 rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed border border-blue-500/20" title="Translate">
                                        <TranslateIcon class="h-4 w-4" :class="{ 'animate-pulse': isTranslating }" />
                                    </button>
                                </div>

                                <!-- Dropdown Menu -->
                                <div v-if="showLanguageDropdown" 
                                    class="absolute top-full left-0 mt-1 w-48 max-h-64 overflow-y-auto bg-gray-900 border border-white/10 rounded-lg shadow-xl z-50 py-1">
                                    <button v-for="lang in SUPPORTED_LANGUAGES" :key="lang.code"
                                        @click="selectLanguage(lang.name)"
                                        class="w-full px-3 py-2 text-left text-xs text-gray-300 hover:bg-white/10 flex items-center justify-between group">
                                        <span class="flex items-center gap-2">
                                            <span :class="`fi fi-${lang.country}`" class="rounded-sm"></span>
                                            <span>{{ lang.name }}</span>
                                        </span>
                                        <CheckIcon v-if="translations[lang.name]" class="h-3 w-3 text-emerald-400" />
                                    </button>
                                </div>
                                
                                <!-- Backdrop to close -->
                                <div v-if="showLanguageDropdown" @click="showLanguageDropdown = false" class="fixed inset-0 z-40 bg-transparent"></div>
                            </div>

                            <div class="w-px h-6 bg-white/10 mx-1"></div>

                            <button @click="cutVideo" :disabled="segments.length === 0 || isProcessing"
                                class="px-4 py-1.5 bg-emerald-600/20 hover:bg-emerald-600/40 text-emerald-400 text-xs font-bold rounded-lg border border-emerald-500/20 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                                title="Export the video with the current cuts applied">
                                Export Video
                            </button>

                            <SubtitleExport :segments="displaySegments" :inputPath="inputPath" :language="currentLanguage" />
                        </div>
                    </div>
                    
                    <!-- Advanced Alignment Toggle (Placeholder for now) -->
                    <div v-if="isLlmOnlyBackend" class="mb-4 p-4 bg-black/20 rounded-xl border border-white/5 flex items-center justify-between">
                        <div>
                            <h3 class="text-sm font-semibold text-gray-300">Advanced Alignment</h3>
                            <p class="text-xs text-gray-500">Align AI transcript with local timestamps (Coming Soon)</p>
                        </div>
                        <button 
                            @click="useAdvancedAlignment = !useAdvancedAlignment"
                            class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-gray-900"
                            :class="useAdvancedAlignment ? 'bg-blue-600' : 'bg-gray-700'"
                        >
                            <span class="sr-only">Enable advanced alignment</span>
                            <span
                                class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                                :class="useAdvancedAlignment ? 'translate-x-6' : 'translate-x-1'"
                            />
                        </button>
                    </div>

                    <!-- Speaker Management -->
                    <div v-if="uniqueSpeakers.length > 0" class="mb-6 p-4 bg-black/20 rounded-xl border border-white/5">
                        <h3 class="text-sm font-semibold text-gray-300 mb-3 uppercase tracking-wider">Speakers</h3>
                        <div class="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 gap-3">
                            <div v-for="speaker in uniqueSpeakers" :key="speaker" class="relative group">
                                <div class="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
                                    <UserIcon class="h-4 w-4 text-gray-500" />
                                </div>
                                <input 
                                    :value="speaker" 
                                    @change="renameSpeaker(speaker, ($event.target as HTMLInputElement).value, $event.target as HTMLInputElement)"
                                    class="w-full pl-9 pr-3 py-2 rounded-lg bg-white/5 border border-white/10 focus:border-blue-500/50 focus:bg-black/30 outline-none text-sm text-gray-300 transition-all"
                                />
                            </div>
                        </div>
                    </div>

                    <Editor :segments="displaySegments" @jump-to="jumpTo" @update:segments="displaySegments = $event" />
                </div>
            </transition>

            <!-- Viral Clips Generator -->
            <transition name="fade">
                <ViralClipsGenerator
                    v-if="segments.length > 0"
                    :segments="segments"
                    :inputPath="inputPath"
                    class="mb-8"
                    @update:status="updateStatus"
                    @update:processing="updateProcessing"
                />
            </transition>

            <!-- Podcast Generator -->
            <transition name="fade">
                <PodcastGenerator
                    v-if="segments.length > 0"
                    :segments="segments"
                    :inputPath="inputPath"
                    class="mb-20"
                    @update:status="updateStatus"
                    @update:processing="updateProcessing"
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
                        @generate="generateClips"
                    />

                    <ClipList
                        :clips="clips"
                        :lastExportPath="lastExportPath"
                        :isProcessing="isProcessing"
                        @export="exportClips"
                        @openFolder="openExportFolder"
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
