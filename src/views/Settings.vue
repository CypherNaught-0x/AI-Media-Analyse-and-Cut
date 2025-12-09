<script setup lang="ts">
import { ref, computed, onMounted } from 'vue';
import { useRouter } from 'vue-router';
import { useSettings } from '../composables/useSettings';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';

const router = useRouter();
const { settings, updateSettings } = useSettings();

const localBaseUrl = ref(settings.value.baseUrl);
const localApiKey = ref(settings.value.apiKey);
const localModel = ref(settings.value.model);
const localPreClipPadding = ref(settings.value.preClipPadding || 0);
const localPostClipPadding = ref(settings.value.postClipPadding || 0);
const availableModels = ref<string[]>([]);
const isFetchingModels = ref(false);
const fetchError = ref('');
const showManualInput = ref(false);

const appVersion = ref('');
const updateStatus = ref('');
const isCheckingUpdate = ref(false);
const updateAvailable = ref(false);
const newVersion = ref('');

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
                let downloaded = 0;
                let contentLength = 0;
                
                await update.downloadAndInstall((event) => {
                    switch (event.event) {
                        case 'Started':
                            contentLength = event.data.contentLength || 0;
                            console.log(`started downloading ${contentLength} bytes`);
                            break;
                        case 'Progress':
                            downloaded += event.data.chunkLength;
                            console.log(`downloaded ${downloaded} from ${contentLength}`);
                            break;
                        case 'Finished':
                            console.log('download finished');
                            break;
                    }
                });
                
                updateStatus.value = 'Update installed. Restarting...';
                await relaunch();
            } else {
                updateStatus.value = 'Update cancelled.';
            }
        } else {
            updateStatus.value = 'You are on the latest version.';
        }
    } catch (e) {
        console.error('Failed to check for updates:', e);
        updateStatus.value = `Error checking for updates: ${e}`;
    } finally {
        isCheckingUpdate.value = false;
    }
}

const hasChanges = computed(() => {
    return (
        localBaseUrl.value !== settings.value.baseUrl ||
        localApiKey.value !== settings.value.apiKey ||
        localModel.value !== settings.value.model ||
        localPreClipPadding.value !== (settings.value.preClipPadding || 0) ||
        localPostClipPadding.value !== (settings.value.postClipPadding || 0)
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

async function fetchModels() {
    if (!localApiKey.value) {
        fetchError.value = 'Please enter an API key first';
        return;
    }

    isFetchingModels.value = true;
    fetchError.value = '';
    showManualInput.value = false;

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
            throw new Error(`Failed to fetch models: ${response.statusText}`);
        }

        const data = await response.json();

        if (data.models && Array.isArray(data.models)) {
            availableModels.value = data.models
                .map((m: any) => m.name?.replace('models/', '') || m.name)
                .filter(Boolean);

            if (availableModels.value.length === 0) {
                throw new Error('No models found');
            }
        } else if (data.data && Array.isArray(data.data)) {
            availableModels.value = data.data
                .map((m: any) => m.id)
                .filter(Boolean);

            if (availableModels.value.length === 0) {
                throw new Error('No models found');
            }
        } else {
            throw new Error('Invalid response format');
        }
    } catch (e) {
        fetchError.value = `Error: ${e}`;
        showManualInput.value = true;
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
    } finally {
        isFetchingModels.value = false;
    }
}

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

function saveSettings() {
    const normalizedUrl = normalizeBaseUrl(localBaseUrl.value);

    updateSettings({
        baseUrl: normalizedUrl,
        apiKey: localApiKey.value,
        model: localModel.value,
        preClipPadding: localPreClipPadding.value,
        postClipPadding: localPostClipPadding.value,
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
                    LLM Settings
                </h1>
                <p class="text-gray-400">Configure your AI model and API credentials</p>
            </header>

            <div class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl">

                <!-- Base URL -->
                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">
                        Base URL
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
                        API Key
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
                        Model
                    </label>
                    <div class="flex gap-3 mb-2">
                        <select v-if="!showManualInput" v-model="localModel"
                            class="flex-1 p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300">
                            <option v-if="availableModels.length === 0" :value="localModel">{{ localModel }}</option>
                            <option v-for="model in availableModels" :key="model" :value="model">{{ model }}</option>
                        </select>
                        <input v-else v-model="localModel" type="text"
                            class="flex-1 p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                            placeholder="Enter model name manually" />
                        <button @click="fetchModels" :disabled="isFetchingModels || !localApiKey"
                            class="px-6 py-3 bg-blue-600 hover:bg-blue-500 text-white font-semibold rounded-2xl shadow-lg shadow-blue-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all active:scale-95">
                            {{ isFetchingModels ? 'Fetching...' : 'Fetch Models' }}
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
</template>
