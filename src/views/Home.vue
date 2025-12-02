<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from '@tauri-apps/plugin-dialog';
import { useRouter } from 'vue-router';
import Editor from "../components/Editor.vue";
import type { TranscriptSegment, AudioInfo } from "../types";
import { useSettings } from "../composables/useSettings";

const router = useRouter();
const { settings } = useSettings();

const status = ref("Initializing...");
const isProcessing = ref(false);
const inputPath = ref("");
const segments = ref<TranscriptSegment[]>([]);

const hasApiKey = computed(() => settings.value.apiKey.length > 0);
const currentModelDisplay = computed(() => {
    if (!hasApiKey.value) return "No API Key configured";
    return `${settings.value.model}`;
});

onMounted(async () => {
    try {
        const res = await invoke<string>("init_ffmpeg");
        status.value = res;
    } catch (e) {
        status.value = `Error initializing FFmpeg: ${e}`;
    }
});

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
        const context = "General video content";
        const glossary = "";
        const response = await invoke<string>("analyze_audio", {
            apiKey: settings.value.apiKey,
            baseUrl: settings.value.baseUrl,
            model: settings.value.model,
            context,
            glossary,
            audioUri: uri,
            audioBase64: audioBase64
        });

        // 4. Parse Response
        const jsonMatch = response.match(/\[[\s\S]*\]/);
        if (jsonMatch) {
            segments.value = JSON.parse(jsonMatch[0]);
            status.value = `Analysis complete. Found ${segments.value.length} segments.`;
        } else {
            status.value = "Failed to parse segments from AI response.";
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

function jumpTo(time: number) {
    console.log("Jump to", time);
}

function goToSettings() {
    router.push('/settings');
}
</script>

<template>
    <div class="min-h-screen bg-gradient-to-br from-gray-900 via-gray-800 to-gray-900 text-gray-200 p-8 font-sans">
        <div class="max-w-4xl mx-auto">
            <header class="mb-10 text-center">
                <h1
                    class="text-5xl font-extrabold text-transparent bg-clip-text bg-gradient-to-r from-blue-400 to-purple-600 mb-2 drop-shadow-lg">
                    Media AI Cutter
                </h1>
                <p class="text-gray-400 text-lg">Intelligent Video Segmentation powered by AI</p>
            </header>

            <div
                class="backdrop-blur-xl bg-white/5 border border-white/10 p-8 rounded-2xl shadow-2xl mb-8 transition-all hover:shadow-blue-500/10 hover:border-white/20">

                <!-- LLM Configuration Display -->
                <div class="mb-6 group">
                    <label
                        class="block text-sm font-semibold text-gray-300 mb-2 group-hover:text-blue-400 transition-colors">Current
                        LLM</label>
                    <div class="flex gap-3 items-center">
                        <div class="flex-1 p-3 pl-4 rounded-xl bg-gray-900/50 border border-gray-700 text-gray-300">
                            {{ currentModelDisplay }}
                        </div>
                        <button @click="goToSettings"
                            class="px-6 py-3 bg-gradient-to-r from-purple-600 to-purple-700 hover:from-purple-500 hover:to-purple-600 text-white font-semibold rounded-xl shadow-lg hover:shadow-purple-500/40 transition-all duration-300 active:scale-95">
                            Settings
                        </button>
                    </div>
                </div>

                <!-- File Selection Section -->
                <div class="mb-8 group">
                    <label
                        class="block text-sm font-semibold text-gray-300 mb-2 group-hover:text-blue-400 transition-colors">Video
                        File</label>
                    <div class="flex gap-3">
                        <input v-model="inputPath" type="text"
                            class="flex-1 p-3 pl-4 rounded-xl bg-gray-900/50 border border-gray-700 focus:border-blue-500 focus:ring-2 focus:ring-blue-500/20 outline-none transition-all duration-300 placeholder-gray-600"
                            placeholder="Select a video file..." readonly />
                        <button @click="selectFile"
                            class="px-6 py-3 bg-gray-700 hover:bg-gray-600 text-white font-semibold rounded-xl border border-gray-600 hover:border-gray-500 transition-all duration-300 shadow-lg hover:shadow-xl active:scale-95">
                            Browse
                        </button>
                    </div>
                </div>

                <!-- Action Buttons -->
                <div class="flex gap-4 mb-6">
                    <button @click="processFile" :disabled="isProcessing || !hasApiKey"
                        class="flex-1 bg-gradient-to-r from-blue-600 to-blue-700 hover:from-blue-500 hover:to-blue-600 text-white font-bold py-3 px-6 rounded-xl shadow-lg shadow-blue-900/30 hover:shadow-blue-500/40 disabled:opacity-50 disabled:cursor-not-allowed transition-all duration-300 transform hover:-translate-y-0.5 active:translate-y-0">
                        {{ isProcessing ? 'Processing...' : 'Analyze Video' }}
                    </button>

                    <button @click="cutVideo" :disabled="segments.length === 0 || isProcessing"
                        class="flex-1 bg-gradient-to-r from-emerald-600 to-emerald-700 hover:from-emerald-500 hover:to-emerald-600 text-white font-bold py-3 px-6 rounded-xl shadow-lg shadow-emerald-900/30 hover:shadow-emerald-500/40 disabled:opacity-50 disabled:cursor-not-allowed transition-all duration-300 transform hover:-translate-y-0.5 active:translate-y-0">
                        Cut Video
                    </button>
                </div>

                <!-- Status Bar -->
                <div class="p-4 rounded-xl bg-gray-900/50 border border-gray-700/50">
                    <div class="flex items-center gap-3">
                        <div class="w-2 h-2 rounded-full"
                            :class="isProcessing ? 'bg-yellow-400 animate-pulse' : 'bg-green-400'"></div>
                        <span class="text-sm font-mono text-gray-300">{{ status }}</span>
                    </div>
                </div>
            </div>

            <!-- Editor Section -->
            <transition name="fade">
                <div v-if="segments.length > 0"
                    class="backdrop-blur-xl bg-white/5 border border-white/10 p-8 rounded-2xl shadow-2xl">
                    <div class="flex justify-between items-center mb-6">
                        <h2
                            class="text-2xl font-bold text-transparent bg-clip-text bg-gradient-to-r from-blue-400 to-purple-400">
                            Transcript & Segments
                        </h2>
                        <span
                            class="px-3 py-1 rounded-full bg-blue-500/20 text-blue-300 text-xs font-bold border border-blue-500/30">
                            {{ segments.length }} Segments
                        </span>
                    </div>
                    <Editor :segments="segments" @jump-to="jumpTo" />
                </div>
            </transition>
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
