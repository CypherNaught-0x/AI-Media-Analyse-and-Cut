<script setup lang="ts">
import { ref, onMounted, computed, watch } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from '@tauri-apps/api/event';
import { open, ask } from '@tauri-apps/plugin-dialog';
import { useRouter } from 'vue-router';
import Editor from "../components/Editor.vue";
import SubtitleExport from "../components/SubtitleExport.vue";
import type { TranscriptSegment, AudioInfo, Clip, ProcessedAudio, SegmentOffset } from "../types";
import { useSettings } from "../composables/useSettings";

import LightningIcon from '../assets/icons/lightning.svg?component';
import VideoFileIcon from '../assets/icons/video-file.svg?component';
import SpinnerIcon from '../assets/icons/spinner.svg?component';
import UserIcon from '../assets/icons/user.svg?component';
import FolderOpenIcon from '../assets/icons/folder-open.svg?component';
import TranslateIcon from '../assets/icons/translate.svg?component';
import CheckIcon from '../assets/icons/check.svg?component';
import ChevronDownIcon from '../assets/icons/chevron-down.svg?component';

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
const inputPath = ref("");
const segments = ref<TranscriptSegment[]>([]);
const translations = ref<Record<string, TranscriptSegment[]>>({});
const currentLanguage = ref("Original");
const targetLanguage = ref("");
const isTranslating = ref(false);
const showLanguageDropdown = ref(false);
const removeFillerWords = ref(false);
const videoRef = ref<HTMLVideoElement | null>(null);

function parseTime(timeStr: string): number {
    const parts = timeStr.split(':');
    if (parts.length === 3) {
        return parseFloat(parts[0]) * 3600 + parseFloat(parts[1]) * 60 + parseFloat(parts[2]);
    } else if (parts.length === 2) {
        return parseFloat(parts[0]) * 60 + parseFloat(parts[1]);
    }
    return parseFloat(timeStr);
}

function formatTime(seconds: number): string {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = (seconds % 60).toFixed(3);
    if (h > 0) {
        return `${h.toString().padStart(2, '0')}:${m.toString().padStart(2, '0')}:${s.padStart(6, '0')}`;
    }
    return `${m.toString().padStart(2, '0')}:${s.padStart(6, '0')}`;
}

function adjustTimestamp(timeStr: string, offsets: SegmentOffset[]): string {
    const t = parseTime(timeStr);
    let offset = 0;
    for (const seg of offsets) {
        if (t >= seg.min_time) {
            offset = seg.offset;
        } else {
            break;
        }
    }
    return formatTime(t + offset);
}

const clips = ref<Clip[]>([]);
const clipCount = ref(3);
const clipMinDuration = ref(10);
const clipMaxDuration = ref(120);
const clipTopic = ref("");
const allowSplicing = ref(false);
const speakerCount = ref<number | null>(null);
const context = ref("");
const lastExportPath = ref("");
const useAdvancedAlignment = ref(false);

const hasApiKey = computed(() => settings.value.apiKey.length > 0);
const currentModelDisplay = computed(() => {
    if (!hasApiKey.value) return "No API Key configured";
    return `${settings.value.model}`;
});
const hasTranscript = computed(() => segments.value.length > 0);
const uniqueSpeakers = computed(() => {
    const s = new Set(segments.value.map(seg => seg.speaker));
    return Array.from(s).sort();
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

const contextTextarea = ref<HTMLTextAreaElement | null>(null);
const glossaryTextarea = ref<HTMLTextAreaElement | null>(null);

function startResize(e: MouseEvent, textarea: HTMLTextAreaElement | null) {
    if (!textarea) return;

    const startY = e.clientY;
    const startHeight = textarea.offsetHeight;

    function onMouseMove(e: MouseEvent) {
        const newHeight = startHeight + (e.clientY - startY);
        if (newHeight > 60) { // Minimum height
            textarea!.style.height = `${newHeight}px`;
        }
    }

    function onMouseUp() {
        document.removeEventListener('mousemove', onMouseMove);
        document.removeEventListener('mouseup', onMouseUp);
    }

    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
}

onMounted(async () => {
    try {
        const res = await invoke<string>("init_ffmpeg");
        status.value = res;
        
        await listen<string>('progress', (event) => {
            status.value = `Processing... ${event.payload}`;
        });
    } catch (e) {
        status.value = `Error initializing FFmpeg: ${e}`;
    }
});

watch(inputPath, () => {
    segments.value = [];
    translations.value = {};
    currentLanguage.value = "Original";
    clips.value = [];
    loadTranscript();
});

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
            status.value = "Loaded existing transcript and context.";
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
            context: context.value
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
            const parsed = JSON.parse(jsonMatch[0]);
            if (Array.isArray(parsed)) {
                translations.value[lang] = parsed;
                currentLanguage.value = lang;
                status.value = `Translation to ${lang} complete.`;
            } else {
                throw new Error("Response is not an array");
            }
        } else {
            throw new Error("Failed to find JSON in response");
        }
    } catch (e) {
        console.error("Translation failed:", e);
        status.value = `Translation failed: ${e}`;
    } finally {
        isTranslating.value = false;
    }
}

async function selectFile() {
    try {
        const selected = await open({
            multiple: false,
            filters: [{
                name: 'Media',
                extensions: ['mp4', 'mkv', 'mov', 'avi', 'webm', 'flv', 'wmv', 'm4v', 'mp3', 'wav', 'aac', 'flac', 'ogg', 'm4a', 'wma']
            }]
        });

        if (selected && typeof selected === 'string') {
            inputPath.value = selected;
        }
    } catch (e) {
        console.error("Failed to open dialog:", e);
    }
}

async function processFile() {
    if (!inputPath.value || !hasApiKey.value) {
        status.value = "Please provide file path and API key.";
        return;
    }

    isProcessing.value = true;
    status.value = "Preparing audio...";
    segments.value = [];

    try {
        // 1. Prepare Audio
        const audioInfo = await invoke<AudioInfo>("prepare_audio_for_ai", { inputPath: inputPath.value });
        status.value = `Audio prepared: ${audioInfo.path} (${(audioInfo.size / 1024 / 1024).toFixed(2)} MB)`;

        // 1b. Remove Silence
        status.value = "Removing silence...";
        const processedAudio = await invoke<ProcessedAudio>("remove_silence", { path: audioInfo.path });
        console.log(`Found ${processedAudio.silence_intervals.length} silence intervals.`);
        
        // Use processed audio for upload/analysis
        const analysisAudioPath = processedAudio.path;

        const isGoogleApi = settings.value.baseUrl.includes('generativelanguage.googleapis.com');
        let uri: string | null = null;
        let audioBase64: string | null = null;

        if (isGoogleApi) {
            // 2. Upload for Google API (only for large files)
            
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
            // For non-Google APIs, read the file as base64
            status.value = "Encoding audio as base64...";
            audioBase64 = await invoke<string>("read_file_as_base64", { path: analysisAudioPath });
            status.value = "Audio encoded successfully";
        }

        // 3. Analyze
        status.value = "Analyzing with AI...";
        const response = await invoke<string>("analyze_audio", {
            apiKey: settings.value.apiKey,
            baseUrl: settings.value.baseUrl,
            model: settings.value.model,
            context: context.value,
            glossary: settings.value.glossary,
            speakerCount: speakerCount.value,
            removeFillerWords: removeFillerWords.value,
            audioUri: uri,
            audioBase64: audioBase64
        });

        // 4. Parse Response
        const jsonMatch = response.match(/\[[\s\S]*\]/);
        if (jsonMatch) {
            try {
                const parsed = JSON.parse(jsonMatch[0]);
                if (!Array.isArray(parsed)) throw new Error("Response is not an array");
                
                // Adjust timestamps back to original timeline
                const adjustedSegments = parsed.map((seg: any) => ({
                    ...seg,
                    start: adjustTimestamp(seg.start, processedAudio.offsets),
                    end: adjustTimestamp(seg.end, processedAudio.offsets)
                }));
                
                segments.value = adjustedSegments;
                status.value = `Analysis complete. Found ${segments.value.length} segments.`;

                await saveTranscript();

                // 5. Advanced Alignment (Optional)
                if (useAdvancedAlignment.value && segments.value.length > 0) {
                    status.value = "Aligning transcript with local model...";
                    try {
                        // Use the processed audio for alignment since the transcript matches it?
                        // No, the transcript now has ORIGINAL timestamps.
                        // But align_transcript expects audio and transcript to match.
                        // If we pass original audio and original timestamps, it should work.
                        // But alignment might be confused by silence if the transcript doesn't have it?
                        // Actually, if we use original audio, alignment is fine.
                        
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
                console.error("JSON Parse Error", e);
                status.value = "Failed to parse segments from AI response. Check console for details.";
            }
        } else {
            status.value = "Failed to find JSON in AI response.";
            console.error(response);
        }

    } catch (e) {
        status.value = `Error: ${e}`;
    } finally {
        isProcessing.value = false;
    }
}

async function cutVideo() {
    if (segments.value.length === 0) return;

    status.value = "Cutting media...";
    isProcessing.value = true;

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
    }
}

async function generateClips() {
    if (segments.value.length === 0) return;
    
    status.value = "Generating clips...";
    isProcessing.value = true;
    
    try {
        const transcript = segments.value
            .map(s => `[${s.start}-${s.end}] ${s.speaker}: ${s.text}`)
            .join("\n");
            
        const response = await invoke<string>("generate_clips", {
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
    }
}

async function exportClips() {
    if (clips.value.length === 0) return;
    
    status.value = "Exporting clips...";
    isProcessing.value = true;
    
    try {
        // Robust extension replacement
        const outputDir = inputPath.value.replace(/\.[^/\\.]+$/, "") + "_clips";
        const clipSegments = clips.value.map(c => ({ 
            segments: c.segments,
            label: c.title,
            reason: c.reason
        }));
        
        console.log({outputDir});
        
        status.value = `Exporting to ${outputDir}...`;
        await invoke("export_clips", {
            inputPath: inputPath.value,
            segments: clipSegments,
            outputDir
        });
        
        lastExportPath.value = outputDir;
        status.value = `Clips exported to ${outputDir}`;
    } catch (e) {
        status.value = `Error exporting clips: ${e}`;
    } finally {
        isProcessing.value = false;
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
                            <label class="block text-xs font-medium text-gray-400 uppercase tracking-wider">Current Model</label>
                            <div class="text-white font-medium">{{ currentModelDisplay }}</div>
                        </div>
                    </div>
                    <button @click="goToSettings"
                        class="px-6 py-2 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-xl transition-all border border-white/10">
                        Settings
                    </button>
                </div>

                <!-- File Selection Section -->
                <div class="mb-8">
                    <label class="block text-sm font-medium text-gray-400 mb-3 uppercase tracking-wider">Source Media</label>
                    <div class="flex gap-3">
                        <div class="flex-1 relative group">
                            <input v-model="inputPath" type="text"
                                class="w-full p-4 pl-12 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 focus:bg-black/30 outline-none transition-all text-gray-300 placeholder-gray-600 font-mono text-sm"
                                placeholder="Select a media file..." readonly />
                            <div class="absolute left-4 top-4 text-gray-500">
                                <VideoFileIcon class="h-5 w-5" />
                            </div>
                        </div>
                        <button @click="selectFile"
                            class="px-8 bg-blue-600 hover:bg-blue-500 text-white font-semibold rounded-2xl shadow-lg shadow-blue-900/20 transition-all transform active:scale-95">
                            Browse
                        </button>
                    </div>
                </div>

                <!-- Analysis Settings -->
                <div class="grid grid-cols-1 md:grid-cols-2 gap-6 mb-8">
                    <div class="md:col-span-2">
                        <label class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">Context</label>
                        <div class="relative">
                            <textarea ref="contextTextarea" v-model="context" rows="2"
                                class="w-full p-4 pb-8 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-colors text-gray-300 placeholder-gray-600 resize-none"
                                placeholder="Describe the video content to help the AI... Especially for translation"></textarea>
                            <div @mousedown.prevent="startResize($event, contextTextarea)"
                                class="absolute bottom-0 left-0 right-0 h-6 cursor-ns-resize flex items-center justify-center hover:bg-white/5 rounded-b-2xl transition-colors group">
                                <div class="w-12 h-1 bg-white/10 rounded-full group-hover:bg-white/20 transition-colors"></div>
                            </div>
                        </div>
                    </div>
                    
                    <div>
                        <label class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">Glossary</label>
                        <div class="relative">
                            <textarea ref="glossaryTextarea" v-model="settings.glossary" rows="2"
                                class="w-full p-4 pb-8 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600 resize-none"
                                placeholder="Specific terms, names, acronyms..."></textarea>
                            <div @mousedown.prevent="startResize($event, glossaryTextarea)"
                                class="absolute bottom-0 left-0 right-0 h-6 cursor-ns-resize flex items-center justify-center hover:bg-white/5 rounded-b-2xl transition-colors group">
                                <div class="w-12 h-1 bg-white/10 rounded-full group-hover:bg-white/20 transition-colors"></div>
                            </div>
                        </div>
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">Speakers</label>
                        <div class="relative">
                            <input v-model.number="speakerCount" type="number" min="1"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                placeholder="Auto-detect" />
                            <div class="absolute right-4 top-4 text-gray-600 text-xs pointer-events-none select-none">Optional</div>
                        </div>
                    </div>
                </div>

                <!-- Advanced Options -->
                <div class="mb-8 flex items-center gap-4">
                    <div class="flex items-center gap-3 p-4 bg-black/20 rounded-xl border border-white/5 cursor-pointer hover:bg-black/30 transition-colors" @click="removeFillerWords = !removeFillerWords">
                        <div class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none"
                            :class="removeFillerWords ? 'bg-blue-600' : 'bg-gray-700'">
                            <span class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                                :class="removeFillerWords ? 'translate-x-6' : 'translate-x-1'" />
                        </div>
                        <span class="text-sm font-medium text-gray-300">Remove Filler Words</span>
                    </div>
                </div>

                <!-- Action Buttons -->
                <div class="flex gap-4 mb-6">
                    <button @click="processFile" :disabled="isProcessing || !hasApiKey || hasTranscript"
                        class="flex-1 bg-blue-600 hover:bg-blue-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg shadow-blue-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all transform hover:-translate-y-0.5 active:translate-y-0 flex items-center justify-center gap-2">
                        <SpinnerIcon v-if="isProcessing" class="animate-spin h-5 w-5 text-white" />
                        {{ isProcessing ? 'Processing...' : (hasTranscript ? 'Transcript Loaded' : 'Analyze Media') }}
                    </button>

                    <button @click="cutVideo" :disabled="segments.length === 0 || isProcessing"
                        class="flex-1 bg-emerald-600 hover:bg-emerald-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg shadow-emerald-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all transform hover:-translate-y-0.5 active:translate-y-0"
                        title="Export the video with the current cuts applied">
                        Export Video
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

                            <SubtitleExport :segments="displaySegments" :inputPath="inputPath" :language="currentLanguage" />
                        </div>
                    </div>
                    
                    <!-- Advanced Alignment Toggle (Placeholder for now) -->
                    <div class="mb-4 p-4 bg-black/20 rounded-xl border border-white/5 flex items-center justify-between">
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

            <!-- Short Clips Section -->
            <transition name="fade">
                <div v-if="segments.length > 0"
                    class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl">
                    <div class="flex justify-between items-center mb-6">
                        <h2 class="text-2xl font-bold text-white">
                            Viral Clips Generator
                        </h2>
                    </div>

                    <div class="grid grid-cols-1 md:grid-cols-3 gap-6 mb-6">
                        <div class="group">
                            <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Count</label>
                            <input v-model.number="clipCount" type="number" min="1" max="10"
                                class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white text-center" />
                        </div>
                        <div class="group">
                            <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Min Sec</label>
                            <input v-model.number="clipMinDuration" type="number" min="5"
                                class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white text-center" />
                        </div>
                        <div class="group">
                            <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Max Sec</label>
                            <input v-model.number="clipMaxDuration" type="number" min="10"
                                class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white text-center" />
                        </div>
                    </div>

                    <div class="mb-6">
                        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Topic (Optional)</label>
                        <input v-model="clipTopic" type="text"
                            class="w-full p-4 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white placeholder-gray-600"
                            placeholder="e.g. 'Funny moments', 'Technical explanation', 'Rants'..." />
                    </div>

                    <div class="mb-8 flex items-center justify-between p-4 bg-black/20 rounded-xl border border-white/5">
                        <div>
                            <h3 class="text-sm font-semibold text-gray-300">Smart Splicing</h3>
                            <p class="text-xs text-gray-500">Allow AI to combine non-contiguous segments into one clip</p>
                        </div>
                        <button 
                            @click="allowSplicing = !allowSplicing"
                            class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-pink-500 focus:ring-offset-2 focus:ring-offset-gray-900"
                            :class="allowSplicing ? 'bg-pink-600' : 'bg-gray-700'"
                        >
                            <span class="sr-only">Enable smart splicing</span>
                            <span
                                class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                                :class="allowSplicing ? 'translate-x-6' : 'translate-x-1'"
                            />
                        </button>
                    </div>

                    <button @click="generateClips" :disabled="isProcessing"
                        class="w-full mb-8 bg-gradient-to-r from-pink-600 to-purple-600 hover:from-pink-500 hover:to-purple-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg transition-all transform hover:-translate-y-0.5 active:translate-y-0">
                        {{ isProcessing ? 'Processing...' : 'Generate Clips' }}
                    </button>

                    <div v-if="clips.length > 0" class="space-y-4">
                        <div v-for="(clip, index) in clips" :key="index"
                            class="p-6 bg-black/20 rounded-2xl border border-white/5 hover:border-pink-500/30 transition-colors">
                            <div class="flex justify-between items-start mb-3">
                                <h3 class="font-bold text-lg text-pink-400">{{ clip.title }}</h3>
                                <div class="flex flex-col items-end gap-1">
                                    <span v-for="(seg, i) in clip.segments" :key="i" class="px-2 py-1 rounded bg-white/5 text-xs text-gray-400 font-mono">
                                        {{ seg.start }} - {{ seg.end }}
                                    </span>
                                </div>
                            </div>
                            <p class="text-gray-300 text-sm leading-relaxed">{{ clip.reason }}</p>
                        </div>

                        <div class="flex gap-4 mt-6">
                            <button @click="exportClips" :disabled="isProcessing"
                                class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-bold py-4 px-6 rounded-2xl border border-gray-600 hover:border-gray-500 transition-all">
                                Export All Clips
                            </button>
                            <button v-if="lastExportPath" @click="openExportFolder"
                                class="px-6 bg-gray-800 hover:bg-gray-700 text-white font-bold rounded-2xl border border-gray-700 transition-all" title="Open Folder">
                                <FolderOpenIcon class="h-6 w-6" />
                            </button>
                        </div>
                    </div>
                </div>
            </transition>
        </div>
    </div>
    <!-- Status Bar (Outside main container to ensure fixed positioning works) -->
    <div class="fixed bottom-0 left-0 right-0 p-4 bg-black/50 backdrop-blur-md border-t border-white/10 flex items-center justify-between z-50">
        <div class="max-w-5xl mx-auto w-full flex items-center gap-3">
            <div class="w-2 h-2 rounded-full"
                :class="isProcessing ? 'bg-yellow-400 animate-pulse' : 'bg-emerald-400'"></div>
            <span class="text-sm font-mono text-gray-400 truncate">{{ status }}</span>
        </div>
    </div>
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
