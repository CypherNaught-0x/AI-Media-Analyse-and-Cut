<script setup lang="ts">
import { ref, computed, onMounted } from 'vue';
import { useRouter } from 'vue-router';
import { useSettings } from '../composables/useSettings';
import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import Toast from '../components/Toast.vue';

const router = useRouter();
const { settings, updateSettings, modelFetchState, updateModelFetchState } = useSettings();

const localBaseUrl = ref(settings.value.baseUrl);
const localApiKey = ref(settings.value.apiKey);
const localModel = ref(settings.value.model);
const localEnforceJsonSchema = ref(settings.value.enforceJsonSchema ?? true);
const localMaxAnalysisChunkMinutes = ref(settings.value.maxAnalysisChunkMinutes ?? 30);
const availableModels = ref<string[]>(modelFetchState.value.availableModels);
const localPreClipPadding = ref(settings.value.preClipPadding || 0);
const localPostClipPadding = ref(settings.value.postClipPadding || 0);
const localTranscriptionBackend = ref(settings.value.transcriptionBackend ?? 'llm');
const localParakeetModelPath = ref(settings.value.parakeetModelPath ?? '');
const localSortformerModelPath = ref(settings.value.sortformerModelPath ?? '');
const isFetchingModels = ref(false);
const fetchError = ref('');
const showManualInput = ref(false);

// Check if the current model is not in the available models list
const modelNotInList = computed(() => {
    if (availableModels.value.length === 0) return false;
    if (modelFetchState.value.supportsModelFetch === false) return false;
    return !availableModels.value.includes(localModel.value);
});

const appVersion = ref('');
const updateStatus = ref('');
const isCheckingUpdate = ref(false);
const updateAvailable = ref(false);
const newVersion = ref('');
const toastVisible = ref(false);
const toastMessage = ref('');
const toastTone = ref<'info' | 'success' | 'error'>('info');
const toastProgress = ref<number | null>(null);
const toastActionLabel = ref('');
const downloadedBytes = ref(0);
const totalBytes = ref(0);
let pendingRelaunch = false;

function showToast(message: string, tone: 'info' | 'success' | 'error' = 'info') {
    toastMessage.value = message;
    toastTone.value = tone;
    toastVisible.value = true;
}

function hideToast() {
    toastVisible.value = false;
    toastActionLabel.value = '';
    toastProgress.value = null;
}

async function handleToastAction() {
    if (pendingRelaunch) {
        await relaunch();
    }
}

onMounted(async () => {
    try {
        appVersion.value = await getVersion();
    } catch (e) {
        console.error('Failed to get app version:', e);
        appVersion.value = 'Unknown';
    }
});

async function checkForUpdates() {
    isCheckingUpdate.value = true;
    updateStatus.value = 'Checking for updates...';
    updateAvailable.value = false;
    
    try {
        const update = await check();
        if (update) {
            updateAvailable.value = true;
            newVersion.value = update.version;
            updateStatus.value = `Update available: v${update.version}`;
            
            const confirmed = await confirm(`Update to v${update.version} is available.\n\nRelease notes:\n${update.body}\n\nDo you want to download and install it now?`);
            
            if (confirmed) {
                updateStatus.value = 'Downloading and installing update...';
                pendingRelaunch = false;
                toastActionLabel.value = '';
                showToast('Downloading update...', 'info');
                toastProgress.value = 0;
                downloadedBytes.value = 0;
                totalBytes.value = 0;
                
                try {
                    await update.downloadAndInstall((event) => {
                        switch (event.event) {
                            case 'Started':
                                totalBytes.value = event.data.contentLength || 0;
                                toastMessage.value = 'Downloading update...';
                                toastProgress.value = 0;
                                downloadedBytes.value = 0;
                                break;
                            case 'Progress': {
                                const chunkLength = event.data.chunkLength || 0;
                                downloadedBytes.value += chunkLength;
                                if (totalBytes.value > 0) {
                                    const nextValue = Math.min(100, (downloadedBytes.value / totalBytes.value) * 100);
                                    toastProgress.value = Math.max(toastProgress.value || 0, nextValue);
                                } else {
                                    toastProgress.value = null;
                                }
                                break;
                            }
                            case 'Finished':
                                toastMessage.value = 'Installing update...';
                                toastProgress.value = null;
                                break;
                        }
                    });
                } catch (e) {
                    console.error('Failed to download and install update:', e);
                    updateStatus.value = `Update failed: ${e}`;
                    showToast('Update failed to download or install.', 'error');
                    return;
                }

                updateStatus.value = 'Update installed. Restart to apply changes.';
                pendingRelaunch = true;
                toastActionLabel.value = 'Restart now';
                showToast('Update installed. Restart to apply changes.', 'success');
            } else {
                updateStatus.value = 'Update cancelled.';
            }
        } else {
            updateStatus.value = 'You are on the latest version.';
        }
    } catch (e) {
        console.error('Failed to check for updates:', e);
        updateStatus.value = `Error checking for updates: ${e}`;
        showToast('Update check failed. Try again later.', 'error');
    } finally {
        isCheckingUpdate.value = false;
    }
}

const hasChanges = computed(() => {
    return (
        localBaseUrl.value !== settings.value.baseUrl ||
        localApiKey.value !== settings.value.apiKey ||
        localModel.value !== settings.value.model ||
        localEnforceJsonSchema.value !== (settings.value.enforceJsonSchema ?? true) ||
        localMaxAnalysisChunkMinutes.value !== (settings.value.maxAnalysisChunkMinutes ?? 30) ||
        localPreClipPadding.value !== (settings.value.preClipPadding || 0) ||
        localPostClipPadding.value !== (settings.value.postClipPadding || 0) ||
        localTranscriptionBackend.value !== (settings.value.transcriptionBackend ?? 'llm') ||
        localParakeetModelPath.value !== (settings.value.parakeetModelPath ?? '') ||
        localSortformerModelPath.value !== (settings.value.sortformerModelPath ?? '')
    );
});

const isGoogleApi = computed(() => {
    return localBaseUrl.value.includes('generativelanguage.googleapis.com');
});

const endpointInfo = computed(() => {
    return isGoogleApi.value
        ? 'Using Google API (query parameter auth)'
        : 'Using OpenAI-compatible API (Bearer token auth)';
});

const normalizeBaseUrl = (url: string): string => {
    let normalized = url.trim().replace(/\/+$/, '');
    if (!normalized.match(/^https?:\/\//)) {
        normalized = `https://${normalized}`;
    }
    return normalized;
};

async function fetchModels(silent = false) {
    if (!localApiKey.value) {
        if (!silent) {
            fetchError.value = 'Please enter an API key first';
        }
        return;
    }

    isFetchingModels.value = true;
    fetchError.value = '';
    if (!silent) {
        showManualInput.value = false;
    }

    // Preserve current model selection
    const currentModel = localModel.value;

    try {
        const normalizedUrl = normalizeBaseUrl(localBaseUrl.value);
        const apiPath = isGoogleApi.value ? '/v1beta/models' : '/v1/models';

        let modelsUrl: string;
        const headers: Record<string, string> = {
            'Content-Type': 'application/json',
        };

        if (isGoogleApi.value) {
            modelsUrl = `${normalizedUrl}${apiPath}?key=${localApiKey.value}`;
        } else {
            modelsUrl = `${normalizedUrl}${apiPath}`;
            headers['Authorization'] = `Bearer ${localApiKey.value}`;
        }

        const response = await fetch(modelsUrl, { headers });

        if (!response.ok) {
            // Check for 404 - endpoint doesn't support model listing
            if (response.status === 404) {
                updateModelFetchState({ supportsModelFetch: false, availableModels: [] });
                availableModels.value = [];
                if (!silent) {
                    fetchError.value = 'This endpoint does not support model listing';
                }
                return;
            }
            throw new Error(`Failed to fetch models: ${response.statusText}`);
        }

        const data = await response.json();
        let fetchedModels: string[] = [];

        if (data.models && Array.isArray(data.models)) {
            fetchedModels = data.models
                .map((m: any) => m.name?.replace('models/', '') || m.name)
                .filter(Boolean);
        } else if (data.data && Array.isArray(data.data)) {
            fetchedModels = data.data
                .map((m: any) => m.id)
                .filter(Boolean);
        } else {
            throw new Error('Invalid response format');
        }

        if (fetchedModels.length === 0) {
            throw new Error('No models found');
        }

        availableModels.value = fetchedModels;
        updateModelFetchState({ supportsModelFetch: true, availableModels: fetchedModels });

        // Restore model selection (it stays as-is, just ensuring dropdown has it)
        localModel.value = currentModel;
    } catch (e) {
        if (!silent) {
            fetchError.value = `Error: ${e}`;
            showManualInput.value = true;
        }
        // On error, set supportsModelFetch to false and provide fallback models
        updateModelFetchState({ supportsModelFetch: false });
        if (isGoogleApi.value) {
            availableModels.value = [
                'gemini-2.0-flash',
                'gemini-1.5-pro',
                'gemini-1.5-flash',
            ];
        } else {
            availableModels.value = [
                'gpt-4',
                'gpt-3.5-turbo',
            ];
        }
        // Restore model selection
        localModel.value = currentModel;
    } finally {
        isFetchingModels.value = false;
    }
}

// Auto-fetch models on mount if API key is present
onMounted(() => {
    if (localApiKey.value && modelFetchState.value.supportsModelFetch !== false) {
        fetchModels(true);
    }
});

async function exportLogs() {
    try {
        const path = await save({
            filters: [{
                name: 'Zip Files',
                extensions: ['zip']
            }],
            defaultPath: 'ai-media-cutter-logs.zip'
        });
        
        if (path) {
            await invoke('zip_logs', { targetPath: path });
            alert('Logs exported successfully!');
        }
    } catch (e) {
        alert(`Failed to export logs: ${e}`);
    }
}

async function selectParakeetModelDirectory() {
    const selected = await open({
        directory: true,
        multiple: false,
        title: 'Select Parakeet TDT model directory',
    });
    if (typeof selected === 'string') {
        localParakeetModelPath.value = selected;
    }
}

async function selectSortformerModelFile() {
    const selected = await open({
        directory: false,
        multiple: false,
        title: 'Select Sortformer model file',
        filters: [
            {
                name: 'ONNX Model',
                extensions: ['onnx'],
            },
        ],
    });
    if (typeof selected === 'string') {
        localSortformerModelPath.value = selected;
    }
}

function saveSettings() {
    const normalizedUrl = normalizeBaseUrl(localBaseUrl.value);

    updateSettings({
        baseUrl: normalizedUrl,
        apiKey: localApiKey.value,
        model: localModel.value,
        enforceJsonSchema: localEnforceJsonSchema.value,
        maxAnalysisChunkMinutes: localMaxAnalysisChunkMinutes.value,
        preClipPadding: localPreClipPadding.value,
        postClipPadding: localPostClipPadding.value,
        transcriptionBackend: localTranscriptionBackend.value,
        parakeetModelPath: localParakeetModelPath.value.trim(),
        sortformerModelPath: localSortformerModelPath.value.trim(),
    });
    router.push('/');
}

function cancel() {
    router.push('/');
}
</script>

<template>
    <div class="min-h-screen bg-gray-900 text-gray-200 p-8 font-sans">
        <div class="max-w-2xl mx-auto">
            <header class="mb-10">
                <h1 class="text-4xl font-bold text-white mb-2">
                    AI Settings
                </h1>
                <p class="text-gray-400">Configure remote LLMs and the local Parakeet transcription backend</p>
            </header>

            <div class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl">

                <!-- Base URL -->
                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">
                        Default Transcription Backend
                    </label>
                    <div class="grid grid-cols-1 sm:grid-cols-4 gap-3">
                        <button
                            type="button"
                            class="text-left p-4 rounded-2xl border transition-all"
                            :class="localTranscriptionBackend === 'llm'
                                ? 'bg-blue-600/15 border-blue-500/40 text-white'
                                : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'"
                            @click="localTranscriptionBackend = 'llm'"
                        >
                            <div class="text-sm font-semibold">LLM-Based</div>
                            <p class="text-xs text-gray-400 mt-1">Uses the API settings below for transcription and speaker labeling.</p>
                        </button>
                        <button
                            type="button"
                            class="text-left p-4 rounded-2xl border transition-all"
                            :class="localTranscriptionBackend === 'parakeet'
                                ? 'bg-blue-600/15 border-blue-500/40 text-white'
                                : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'"
                            @click="localTranscriptionBackend = 'parakeet'"
                        >
                            <div class="text-sm font-semibold">Parakeet</div>
                            <p class="text-xs text-gray-400 mt-1">Runs local Parakeet TDT + Sortformer with diarization and word timestamps.</p>
                        </button>
                        <button
                            type="button"
                            class="text-left p-4 rounded-2xl border transition-all"
                            :class="localTranscriptionBackend === 'hybrid'
                                ? 'bg-blue-600/15 border-blue-500/40 text-white'
                                : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'"
                            @click="localTranscriptionBackend = 'hybrid'"
                        >
                            <div class="text-sm font-semibold">Hybrid</div>
                            <p class="text-xs text-gray-400 mt-1">Uses Parakeet for timings and a remote LLM pass to clean and merge transcript lines.</p>
                        </button>
                        <button
                            type="button"
                            class="text-left p-4 rounded-2xl border transition-all"
                            :class="localTranscriptionBackend === 'hybrid-merge'
                                ? 'bg-blue-600/15 border-blue-500/40 text-white'
                                : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'"
                            @click="localTranscriptionBackend = 'hybrid-merge'"
                        >
                            <div class="text-sm font-semibold">Hybrid Merge</div>
                            <p class="text-xs text-gray-400 mt-1">Queries both Parakeet and the remote model, then merges their strengths onto Parakeet timings.</p>
                        </button>
                    </div>
                </div>

                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-4 uppercase tracking-wider">
                        Parakeet Settings
                    </label>
                    <div class="space-y-4">
                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2">Parakeet TDT Model Directory</label>
                            <div class="flex gap-3">
                                <input v-model="localParakeetModelPath" type="text"
                                    class="flex-1 p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                    placeholder="Leave blank to auto-download into app data" />
                                <button @click="selectParakeetModelDirectory"
                                    class="px-4 py-3 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-2xl transition-all border border-white/10">
                                    Browse
                                </button>
                            </div>
                            <p class="text-xs text-gray-500 mt-2">Leave blank to let the app download an int8 TDT model into Tauri app data. Custom directories should contain encoder, decoder, and `vocab.txt`.</p>
                        </div>
                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2">Sortformer Model File</label>
                            <div class="flex gap-3">
                                <input v-model="localSortformerModelPath" type="text"
                                    class="flex-1 p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                    placeholder="Leave blank to auto-download into app data" />
                                <button @click="selectSortformerModelFile"
                                    class="px-4 py-3 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-2xl transition-all border border-white/10">
                                    Browse
                                </button>
                            </div>
                            <p class="text-xs text-gray-500 mt-2">Leave blank to let the app download Sortformer v2 automatically, or provide your own `.onnx` file.</p>
                        </div>
                    </div>
                </div>

                <!-- Base URL -->
                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">
                        LLM Base URL
                    </label>
                    <input v-model="localBaseUrl" type="text"
                        class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                        placeholder="https://generativelanguage.googleapis.com" />
                    <p class="text-xs text-gray-500 mt-2">The base URL for the LLM API endpoint (trailing slashes will
                        be removed)</p>
                </div>

                <!-- API Key -->
                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">
                        LLM API Key
                    </label>
                    <input v-model="localApiKey" type="password"
                        class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                        placeholder="Enter your API key" />
                    <p class="text-xs text-gray-500 mt-2">Your API key will be stored locally in the browser</p>
                </div>

                <!-- Model Selection -->
                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">
                        LLM Model
                    </label>
                    <div class="flex gap-3 mb-2">
                        <div class="flex-1 relative">
                            <select v-if="!showManualInput" v-model="localModel"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300"
                                :class="{ 'pr-12': modelNotInList }">
                                <option v-if="availableModels.length === 0 || !availableModels.includes(localModel)" :value="localModel">{{ localModel }}</option>
                                <option v-for="model in availableModels" :key="model" :value="model">{{ model }}</option>
                            </select>
                            <input v-else v-model="localModel" type="text"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                placeholder="Enter model name manually" />
                            <!-- Warning indicator when model is not in the available list -->
                            <div v-if="modelNotInList && !showManualInput"
                                class="absolute right-4 top-1/2 -translate-y-1/2 group/tooltip">
                                <svg class="w-5 h-5 text-amber-400" fill="currentColor" viewBox="0 0 20 20">
                                    <path fill-rule="evenodd" d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z" clip-rule="evenodd" />
                                </svg>
                                <div class="absolute bottom-full left-1/2 -translate-x-1/2 mb-2 px-3 py-1.5 bg-gray-800 text-xs text-amber-300 rounded-lg whitespace-nowrap opacity-0 group-hover/tooltip:opacity-100 transition-opacity pointer-events-none border border-amber-500/30">
                                    Model not found in available models list
                                </div>
                            </div>
                        </div>
                        <button @click="fetchModels()" :disabled="isFetchingModels || !localApiKey"
                            class="px-6 py-3 bg-blue-600 hover:bg-blue-500 text-white font-semibold rounded-2xl shadow-lg shadow-blue-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all active:scale-95 flex items-center gap-2">
                            <svg v-if="!isFetchingModels" class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                            </svg>
                            {{ isFetchingModels ? 'Fetching...' : 'Refresh Models' }}
                        </button>
                    </div>
                    <div class="flex items-center gap-2 mb-1">
                        <button @click="showManualInput = !showManualInput"
                            class="text-xs text-blue-400 hover:text-blue-300 transition-colors underline">
                            {{ showManualInput ? 'Use dropdown' : 'Enter manually' }}
                        </button>
                    </div>
                    <p v-if="fetchError" class="text-xs text-red-400 mt-1">{{ fetchError }}</p>
                    <p v-else class="text-xs text-gray-500 mt-1">{{ endpointInfo }}</p>
                </div>

                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">
                        Response Validation
                    </label>
                    <div
                        class="flex items-start gap-4 rounded-2xl bg-black/20 border border-white/10 p-4 cursor-pointer hover:bg-black/30 transition-colors"
                        @click="localEnforceJsonSchema = !localEnforceJsonSchema">
                        <div class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors"
                            :class="localEnforceJsonSchema ? 'bg-blue-600' : 'bg-gray-700'">
                            <span class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                                :class="localEnforceJsonSchema ? 'translate-x-6' : 'translate-x-1'" />
                        </div>
                        <div class="flex-1">
                            <p class="text-sm font-medium text-gray-200">Enforce Structured JSON for transcript analysis</p>
                            <p class="text-xs text-gray-500 mt-1">
                                Sends a strict JSON schema with AI analysis requests on OpenAI-compatible APIs.
                                Improves reliability, but some providers may respond slower or behave differently.
                            </p>
                        </div>
                    </div>
                </div>

                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">
                        Long Audio Handling
                    </label>
                    <label class="block text-xs font-medium text-gray-500 mb-2">Max analysis chunk length (minutes)</label>
                    <input v-model.number="localMaxAnalysisChunkMinutes" type="number" step="1" min="0"
                        class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                        placeholder="30" />
                    <p class="text-xs text-gray-500 mt-2">
                        Audio longer than this is split into chunks before LLM transcription, so each request stays
                        under the provider's request timeout (avoids <span class="text-gray-400">504 Gateway Timeout</span>
                        on long videos). Splits prefer a ~1s silence near the boundary and fall back to Parakeet word
                        boundaries. Set to 0 to disable chunking. Only applies to LLM-based transcription.
                    </p>
                </div>

                <!-- Clip Settings -->
                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label class="block text-sm font-medium text-gray-400 mb-4 uppercase tracking-wider">
                        Clip Settings
                    </label>
                    <div class="grid grid-cols-2 gap-4">
                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2">Pre-Clip Padding (seconds)</label>
                            <input v-model.number="localPreClipPadding" type="number" step="0.1" min="0"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                placeholder="0.0" />
                            <p class="text-xs text-gray-500 mt-2">Added before the start of each clip</p>
                        </div>
                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2">Post-Clip Padding (seconds)</label>
                            <input v-model.number="localPostClipPadding" type="number" step="0.1" min="0"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                placeholder="0.0" />
                            <p class="text-xs text-gray-500 mt-2">Added after the end of each clip</p>
                        </div>
                    </div>
                </div>

                <!-- Application Info -->
                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">
                        Application Info
                    </label>
                    <div class="flex items-center justify-between bg-black/20 p-4 rounded-2xl border border-white/10">
                        <div>
                            <p class="text-gray-300 font-medium">Version: <span class="text-white">{{ appVersion }}</span></p>
                            <p v-if="updateStatus" class="text-xs mt-1" :class="updateAvailable ? 'text-green-400' : 'text-gray-400'">
                                {{ updateStatus }}
                            </p>
                        </div>
                        <button @click="checkForUpdates" :disabled="isCheckingUpdate"
                            class="px-4 py-2 bg-blue-600 hover:bg-blue-500 text-white text-sm font-semibold rounded-xl shadow-lg shadow-blue-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all active:scale-95">
                            {{ isCheckingUpdate ? 'Checking...' : 'Check for Updates' }}
                        </button>
                    </div>
                </div>

                <!-- Troubleshooting -->
                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">
                        Troubleshooting
                    </label>
                    <div class="flex gap-3">
                        <button @click="exportLogs"
                            class="px-6 py-3 bg-gray-700 hover:bg-gray-600 text-white font-semibold rounded-2xl border border-gray-600 hover:border-gray-500 transition-all shadow-lg hover:shadow-xl active:scale-95">
                            Export Logs
                        </button>
                    </div>
                    <p class="text-xs text-gray-500 mt-2">Export application logs for debugging purposes.</p>
                </div>

                <!-- Action Buttons -->
                <div class="flex gap-4 mt-8">
                    <button @click="saveSettings" :disabled="!hasChanges"
                        class="flex-1 bg-emerald-600 hover:bg-emerald-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg shadow-emerald-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all transform hover:-translate-y-0.5 active:translate-y-0">
                        Save Settings
                    </button>
                    <button @click="cancel"
                        class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-bold py-4 px-6 rounded-2xl border border-gray-600 hover:border-gray-500 transition-all shadow-lg hover:shadow-xl active:scale-95">
                        Cancel
                    </button>
                </div>
            </div>
        </div>
    </div>
    <Toast
        :show="toastVisible"
        :message="toastMessage"
        :tone="toastTone"
        :progress="toastProgress"
        :action-label="toastActionLabel"
        @dismiss="hideToast"
        @action="handleToastAction"
    />
</template>
