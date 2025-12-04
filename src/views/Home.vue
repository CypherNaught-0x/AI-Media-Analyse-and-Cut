<script setup lang="ts">
import { ref, onMounted, computed, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from '@tauri-apps/api/event';
import { open, save, ask } from '@tauri-apps/plugin-dialog';
import { useRouter } from 'vue-router';
import Editor from "../components/Editor.vue";
import type { TranscriptSegment, AudioInfo, Clip, SilenceInterval } from "../types";
import { useSettings } from "../composables/useSettings";

import LightningIcon from '../assets/icons/lightning.svg?component';
import VideoFileIcon from '../assets/icons/video-file.svg?component';
import SpinnerIcon from '../assets/icons/spinner.svg?component';
import DownloadIcon from '../assets/icons/download.svg?component';
import UserIcon from '../assets/icons/user.svg?component';
import FolderOpenIcon from '../assets/icons/folder-open.svg?component';

const router = useRouter();
const { settings } = useSettings();

const status = ref("Initializing...");
const isProcessing = ref(false);
const inputPath = ref("");
const segments = ref<TranscriptSegment[]>([]);
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

const contextTextarea = ref<HTMLTextAreaElement | null>(null);

function startResize(e: MouseEvent) {
    const textarea = contextTextarea.value;
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

async function selectFile() {
    try {
        const selected = await open({
            multiple: false,
            filters: [{
                name: 'Video',
                extensions: ['mp4', 'mkv', 'mov', 'avi']
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

        // 1b. Detect Silence
        status.value = "Detecting silence...";
        const silenceIntervals = await invoke<SilenceInterval[]>("detect_silence", { path: audioInfo.path });
        console.log(`Found ${silenceIntervals.length} silence intervals.`);

        const isGoogleApi = settings.value.baseUrl.includes('generativelanguage.googleapis.com');
        let uri: string | null = null;
        let audioBase64: string | null = null;

        if (isGoogleApi) {
            // 2. Upload for Google API (only for large files)
            if (audioInfo.size > 20 * 1024 * 1024) {
                status.value = "Uploading file...";
                uri = await invoke<string | null>("upload_file", {
                    apiKey: settings.value.apiKey,
                    baseUrl: settings.value.baseUrl,
                    path: audioInfo.path
                });

                if (uri) {
                    status.value = "File uploaded successfully";
                }
            }
        } else {
            // For non-Google APIs, read the file as base64
            status.value = "Encoding audio as base64...";
            audioBase64 = await invoke<string>("read_file_as_base64", { path: audioInfo.path });
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
            audioUri: uri,
            audioBase64: audioBase64
        });

        // 4. Parse Response
        const jsonMatch = response.match(/\[[\s\S]*\]/);
        if (jsonMatch) {
            try {
                const parsed = JSON.parse(jsonMatch[0]);
                if (!Array.isArray(parsed)) throw new Error("Response is not an array");
                segments.value = parsed;
                status.value = `Analysis complete. Found ${segments.value.length} segments.`;

                // Filter silent segments
                if (silenceIntervals.length > 0) {
                    status.value = "Filtering silent segments...";
                    const originalCount = segments.value.length;
                    segments.value = filterSilentSegments(segments.value, silenceIntervals);
                    const removedCount = originalCount - segments.value.length;
                    if (removedCount > 0) {
                        console.log(`Removed ${removedCount} silent segments.`);
                        status.value = `Analysis complete. Found ${segments.value.length} segments (${removedCount} removed).`;
                    }
                }

                await saveTranscript();

                // 5. Advanced Alignment (Optional)
                if (useAdvancedAlignment.value && segments.value.length > 0) {
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

    status.value = "Cutting video...";
    isProcessing.value = true;

    try {
        const cutSegments = segments.value.map(s => ({ start: s.start, end: s.end }));
        const outputPath = inputPath.value.replace(/(\.[\ w\d]+)$/, "_cut$1");

        await invoke("cut_video", {
            inputPath: inputPath.value,
            segments: cutSegments,
            outputPath
        });

        status.value = `Video cut successfully to ${outputPath}`;
    } catch (e) {
        status.value = `Error cutting video: ${e}`;
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

async function exportSubtitles(format: 'srt' | 'vtt' | 'txt', manualSave: boolean = false) {
    if (segments.value.length === 0) return;
    
    // Helper to ensure timestamps are HH:MM:SS,mmm (SRT) or HH:MM:SS.mmm (VTT)
    const formatTime = (time: string, separator: string) => {
        let [base, ms] = time.split(/[.,]/);
        if (!ms) ms = "000";
        ms = ms.padEnd(3, '0').slice(0, 3);

        const parts = base.split(':');
        let h = "00";
        let m = "00";
        let s = "00";

        if (parts.length >= 3) {
            h = parts[parts.length - 3].padStart(2, '0');
            m = parts[parts.length - 2].padStart(2, '0');
            s = parts[parts.length - 1].padStart(2, '0');
        } else if (parts.length === 2) {
            m = parts[0].padStart(2, '0');
            s = parts[1].padStart(2, '0');
        } else {
            s = parts[0].padStart(2, '0');
        }

        return `${h}:${m}:${s}${separator}${ms}`;
    };
    
    try {
        let content = "";
        // Robustly remove extension
        const baseName = inputPath.value.replace(/\.[^/\\.]+$/, "");
        let outputPath = `${baseName}.${format}`;
        
        if (format === 'srt') {
            content = segments.value.map((s, i) => {
                const start = formatTime(s.start, ',');
                const end = formatTime(s.end, ',');
                return `${i + 1}\n${start} --> ${end}\n${s.speaker}: ${s.text}\n`;
            }).join('\n');
        } else if (format === 'vtt') {
            content = "WEBVTT\n\n" + segments.value.map((s) => {
                const start = formatTime(s.start, '.');
                const end = formatTime(s.end, '.');
                return `${start} --> ${end}\n<v ${s.speaker}>${s.text}`;
            }).join('\n\n');
        } else {
            content = segments.value.map(s => `[${s.start} - ${s.end}] ${s.speaker}: ${s.text}`).join('\n');
        }

        if (manualSave) {
            const saved = await save({
                defaultPath: outputPath,
                filters: [{
                    name: format.toUpperCase(),
                    extensions: [format]
                }]
            });
            if (!saved) return;
            outputPath = saved;
        }
        
        await invoke("write_text_file", { path: outputPath, content });
        status.value = `Exported ${format.toUpperCase()} to ${outputPath}`;
    } catch (e) {
        status.value = `Error exporting subtitles: ${e}`;
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

function filterSilentSegments(segments: TranscriptSegment[], silence: SilenceInterval[]): TranscriptSegment[] {
    const parseTime = (t: string) => {
        const parts = t.split(':').map(Number);
        if (parts.length === 2) return parts[0] * 60 + parts[1];
        if (parts.length === 3) return parts[0] * 3600 + parts[1] * 60 + parts[2];
        return 0;
    };

    return segments.filter(seg => {
        const start = parseTime(seg.start);
        const end = parseTime(seg.end);
        
        for (const s of silence) {
            // If segment is fully contained within silence (with small tolerance)
            if (start >= s.start && end <= s.end) {
                console.log(`Removing silent segment: [${seg.start}-${seg.end}] "${seg.text}" (Silence: ${s.start}-${s.end})`);
                return false;
            }
        }
        return true;
    });
}

function jumpTo(time: number) {
    console.log("Jump to", time);
}

function goToSettings() {
    router.push('/settings');
}
</script>

<template>
    <div class="min-h-screen bg-gray-900 text-gray-200 p-8 font-sans selection:bg-blue-500/30">
        <div class="max-w-5xl mx-auto">
            <div class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl mb-8">

                <!-- LLM Configuration Display -->
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
                        Configure
                    </button>
                </div>

                <!-- File Selection Section -->
                <div class="mb-8">
                    <label class="block text-sm font-medium text-gray-400 mb-3 uppercase tracking-wider">Source Media</label>
                    <div class="flex gap-3">
                        <div class="flex-1 relative group">
                            <input v-model="inputPath" type="text"
                                class="w-full p-4 pl-12 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 focus:bg-black/30 outline-none transition-all text-gray-300 placeholder-gray-600 font-mono text-sm"
                                placeholder="Select a video file..." readonly />
                            <div class="absolute left-4 top-4 text-gray-500">
                                <VideoFileIcon class="h-5 w-5" />
                            </div>
                        </div>
                        <button @click="selectFile"
                            class="px-8 py-4 bg-blue-600 hover:bg-blue-500 text-white font-semibold rounded-2xl shadow-lg shadow-blue-900/20 transition-all transform active:scale-95">
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
                                placeholder="Describe the video content to help the AI..."></textarea>
                            <div @mousedown.prevent="startResize"
                                class="absolute bottom-0 left-0 right-0 h-6 cursor-ns-resize flex items-center justify-center hover:bg-white/5 rounded-b-2xl transition-colors group">
                                <div class="w-12 h-1 bg-white/10 rounded-full group-hover:bg-white/20 transition-colors"></div>
                            </div>
                        </div>
                    </div>
                    
                    <div>
                        <label class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">Glossary</label>
                        <textarea v-model="settings.glossary" rows="2"
                            class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600 resize-none"
                            placeholder="Specific terms, names, acronyms..."></textarea>
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

                <!-- Action Buttons -->
                <div class="flex gap-4 mb-6">
                    <button @click="processFile" :disabled="isProcessing || !hasApiKey || hasTranscript"
                        class="flex-1 bg-blue-600 hover:bg-blue-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg shadow-blue-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all transform hover:-translate-y-0.5 active:translate-y-0 flex items-center justify-center gap-2">
                        <SpinnerIcon v-if="isProcessing" class="animate-spin h-5 w-5 text-white" />
                        {{ isProcessing ? 'Processing...' : (hasTranscript ? 'Transcript Loaded' : 'Analyze Video') }}
                    </button>

                    <button @click="cutVideo" :disabled="segments.length === 0 || isProcessing"
                        class="flex-1 bg-emerald-600 hover:bg-emerald-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg shadow-emerald-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all transform hover:-translate-y-0.5 active:translate-y-0">
                        Cut Video
                    </button>
                </div>
            </div>

            <!-- Editor Section -->
            <transition name="fade">
                <div v-if="segments.length > 0"
                    class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl mb-8">
                    <div class="flex justify-between items-center mb-6">
                        <div class="flex items-center gap-4">
                            <h2 class="text-2xl font-bold text-white">Transcript</h2>
                            <span class="px-3 py-1 rounded-full bg-white/10 text-gray-300 text-xs font-bold border border-white/10">
                                {{ segments.length }} Segments
                            </span>
                        </div>
                        <div class="flex gap-2">
                            <div class="flex rounded-lg bg-white/5 border border-white/10 overflow-hidden">
                                <button @click="exportSubtitles('srt')" class="px-3 py-1.5 hover:bg-white/10 text-xs text-gray-300 transition-colors border-r border-white/10">SRT</button>
                                <button @click="exportSubtitles('srt', true)" class="px-2 py-1.5 hover:bg-white/10 text-gray-300 transition-colors" title="Save SRT as...">
                                    <DownloadIcon class="h-3 w-3" />
                                </button>
                            </div>
                            <div class="flex rounded-lg bg-white/5 border border-white/10 overflow-hidden">
                                <button @click="exportSubtitles('vtt')" class="px-3 py-1.5 hover:bg-white/10 text-xs text-gray-300 transition-colors border-r border-white/10">VTT</button>
                                <button @click="exportSubtitles('vtt', true)" class="px-2 py-1.5 hover:bg-white/10 text-gray-300 transition-colors" title="Save VTT as...">
                                    <DownloadIcon class="h-3 w-3" />
                                </button>
                            </div>
                            <div class="flex rounded-lg bg-white/5 border border-white/10 overflow-hidden">
                                <button @click="exportSubtitles('txt')" class="px-3 py-1.5 hover:bg-white/10 text-xs text-gray-300 transition-colors border-r border-white/10">TXT</button>
                                <button @click="exportSubtitles('txt', true)" class="px-2 py-1.5 hover:bg-white/10 text-gray-300 transition-colors" title="Save TXT as...">
                                    <DownloadIcon class="h-3 w-3" />
                                </button>
                            </div>
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

                    <Editor :segments="segments" @jump-to="jumpTo" @update:segments="segments = $event" />
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
