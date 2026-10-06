<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue';
import { useRouter } from 'vue-router';
import { refreshApiKeyStatus, useSettings } from '../composables/useSettings';
import { appleSpeechLocale, useAppleSpeech } from '../composables/useAppleSpeech';
import { open, save, ask, message } from '@tauri-apps/plugin-dialog';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { listen } from '@tauri-apps/api/event';
import Toast from '../components/Toast.vue';
import { openUrl } from '@tauri-apps/plugin-opener';
import { runtimeDownload, runtimeLicenses } from '../utils/runtimeLicenses';
import { CRISPER_LANGUAGES, CRISPER_MODELS } from '../types';
import type {
    CrisperEnvironmentStatus,
    CrisperLanguage,
    CrisperMode,
    LocalEngine,
    TranscriptionBackend,
} from '../types';
import { commands } from '../bindings';
import { errorMessage } from '../utils/appError';

const PIPELINE_OPTIONS: { value: TranscriptionBackend; title: string; description: string }[] = [
    {
        value: 'llm',
        title: 'LLM Only',
        description: 'Uses the API settings below for transcription and speaker labeling.',
    },
    {
        value: 'local',
        title: 'Local Only',
        description: 'Runs the local engine on this machine. No API key required.',
    },
    {
        value: 'hybrid',
        title: 'Hybrid Cleanup',
        description: 'Local timings, then a remote LLM pass to clean up wording.',
    },
    {
        value: 'hybrid-merge',
        title: 'Hybrid Merge',
        description: 'Merges a local and a remote transcript onto the local timings.',
    },
];

const ENGINE_OPTIONS: { value: LocalEngine; title: string; badge?: string; description: string }[] =
    [
        {
            value: 'parakeet',
            title: 'Parakeet',
            description: 'Parakeet TDT + Sortformer diarization with word timestamps.',
        },
        {
            value: 'crisper',
            title: 'CrisperWhisper',
            badge: 'EN / DE',
            description:
                'Verbatim transcription with precise word timings. English and German only.',
        },
        {
            value: 'apple-speech',
            title: 'Apple Speech',
            badge: 'macOS 26+',
            description:
                "macOS's built-in on-device model. Very fast, ~45 languages; drops fillers.",
        },
    ];

const router = useRouter();
const { settings, apiKeyStored, updateSettings, modelFetchState, updateModelFetchState } =
    useSettings();
const {
    status: appleSpeechStatus,
    checking: isCheckingAppleSpeech,
    supported: appleSpeechSupported,
    refresh: refreshAppleSpeechStatus,
    setStatus: setAppleSpeechStatus,
} = useAppleSpeech();

const localBaseUrl = ref(settings.value.baseUrl);
// A new key typed here; the stored key itself is never loaded into the UI.
const localApiKey = ref('');
const canUseApiKey = computed(() => apiKeyStored.value || localApiKey.value.trim().length > 0);
const localModel = ref(settings.value.model);
const localEnforceJsonSchema = ref(settings.value.enforceJsonSchema ?? true);
const localMaxAnalysisChunkMinutes = ref(settings.value.maxAnalysisChunkMinutes ?? 30);
const availableModels = ref<string[]>(modelFetchState.value.availableModels);
const localPreClipPadding = ref(settings.value.preClipPadding || 0);
const localPostClipPadding = ref(settings.value.postClipPadding || 0);
const localExportQuality = ref(settings.value.exportQuality ?? 'balanced');
const EXPORT_QUALITY_OPTIONS = [
    { value: 'high', label: 'High', description: 'Visually lossless; larger files, slower.' },
    { value: 'balanced', label: 'Balanced', description: 'Good quality at a sensible size.' },
    { value: 'draft', label: 'Draft', description: 'Fast and small, for previews.' },
] as const;
const localTranscriptionBackend = ref<TranscriptionBackend>(
    settings.value.transcriptionBackend ?? 'llm',
);
const localLocalEngine = ref<LocalEngine>(settings.value.localEngine ?? 'parakeet');
const localParakeetModelPath = ref(settings.value.parakeetModelPath ?? '');
const localSortformerModelPath = ref(settings.value.sortformerModelPath ?? '');
const localCrisperModel = ref(settings.value.crisperModel ?? 'large');
const localCrisperLanguage = ref<CrisperLanguage>(settings.value.crisperLanguage ?? 'en');
const localCrisperMode = ref<CrisperMode>(settings.value.crisperMode ?? 'verbatim');
const localCrisperBackend = ref(settings.value.crisperBackend ?? 'auto');
const localCrisperDevice = ref(settings.value.crisperDevice ?? 'auto');
const localCrisperComputeType = ref(settings.value.crisperComputeType ?? 'auto');
const localCrisperRemoveVocalEvents = ref(settings.value.crisperRemoveVocalEvents ?? false);
const localCrisperDiarize = ref(settings.value.crisperDiarize ?? true);
const localCrisperPythonPath = ref(settings.value.crisperPythonPath ?? '');
const crisperStatus = ref<CrisperEnvironmentStatus | null>(null);
const isCheckingCrisper = ref(false);
const isInstallingCrisper = ref(false);
const crisperProgress = ref('');
const localAppleSpeechLocale = ref(settings.value.appleSpeechLocale ?? '');
const localAppleSpeechDiarize = ref(settings.value.appleSpeechDiarize ?? true);
const isInstallingAppleSpeech = ref(false);
const appleSpeechProgress = ref('');
// Apple Speech is only offered where it can exist (macOS builds with the
// helper); keep the current choice visible even if it can't run here.
const engineOptions = computed(() =>
    ENGINE_OPTIONS.filter(
        (engine) =>
            engine.value !== 'apple-speech' ||
            appleSpeechSupported.value ||
            localLocalEngine.value === 'apple-speech',
    ),
);
/** The locale a run would use, after resolving "system language". */
const effectiveAppleSpeechLocale = computed(() => appleSpeechLocale(localAppleSpeechLocale.value));
const appleSpeechLocaleInstalled = computed(() => {
    const installed = appleSpeechStatus.value?.installedLocales ?? [];
    const wanted = effectiveAppleSpeechLocale.value.toLowerCase();
    return installed.some(
        (locale) =>
            locale.toLowerCase() === wanted || locale.toLowerCase().startsWith(`${wanted}-`),
    );
});
const appleSpeechLocaleOptions = computed(() => {
    const status = appleSpeechStatus.value;
    if (!status) return [];
    const installed = new Set(status.installedLocales);
    const names = new Intl.DisplayNames([navigator.language || 'en'], { type: 'language' });
    return status.supportedLocales
        .map((locale) => {
            let name = locale;
            try {
                name = names.of(locale) ?? locale;
            } catch {
                // Unknown to Intl; show the identifier.
            }
            return {
                value: locale,
                label: `${name} (${locale})${installed.has(locale) ? ' \u2713' : ''}`,
            };
        })
        .sort((a, b) => a.label.localeCompare(b.label));
});
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

let unlistenCrisperProgress: (() => void) | null = null;

onMounted(async () => {
    try {
        appVersion.value = await getVersion();
    } catch (e) {
        console.error('Failed to get app version:', e);
        appVersion.value = 'Unknown';
    }

    // The Rust side reports venv creation, pip output and model loading through
    // the shared "progress" event.
    try {
        unlistenCrisperProgress = await listen<{ message: string }>('progress', (event) => {
            if (isCheckingCrisper.value || isInstallingCrisper.value) {
                crisperProgress.value = event.payload?.message ?? '';
            }
            if (isInstallingAppleSpeech.value) {
                appleSpeechProgress.value = event.payload?.message ?? '';
            }
        });
    } catch (e) {
        console.error('Failed to listen for CrisperWhisper progress:', e);
    }

    void refreshCrisperStatus();
});

onUnmounted(() => {
    unlistenCrisperProgress?.();
    unlistenCrisperProgress = null;
});

async function refreshCrisperStatus() {
    if (isInstallingCrisper.value) return;
    isCheckingCrisper.value = true;
    crisperProgress.value = '';
    try {
        crisperStatus.value = await commands.crisperEnvironmentStatus(
            localCrisperPythonPath.value.trim(),
        );
    } catch (e) {
        console.error('Failed to probe the CrisperWhisper environment:', e);
        crisperStatus.value = null;
        showToast(`Could not check the CrisperWhisper environment: ${errorMessage(e)}`, 'error');
    } finally {
        isCheckingCrisper.value = false;
        crisperProgress.value = '';
    }
}

/** The licence notice the runtime licence register requires for the weights. */
const crisperWeightsNotice = runtimeDownload('crisperwhisper-weights').notice;

async function setUpCrisperEnvironment() {
    const confirmed = await ask(
        'This downloads PyTorch and the CrisperWhisper package into a private ' +
            'environment inside the app data directory (several GB, a few minutes).\n\n' +
            `${crisperWeightsNotice}\n\n` +
            'Continue?',
        { title: 'Set up CrisperWhisper', kind: 'info' },
    );
    if (!confirmed) return;

    isInstallingCrisper.value = true;
    crisperProgress.value = 'Starting...';
    try {
        crisperStatus.value = await commands.installCrisperEnvironment(
            localCrisperPythonPath.value.trim(),
            localCrisperBackend.value === 'ct2' ? 'ct2' : 'transformers',
        );
        if (crisperStatus.value?.ready) {
            showToast('CrisperWhisper is ready to use.', 'success');
        } else {
            showToast(crisperStatus.value?.message ?? 'Setup did not complete.', 'error');
        }
    } catch (e) {
        console.error('Failed to set up the CrisperWhisper environment:', e);
        showToast(`CrisperWhisper setup failed: ${errorMessage(e)}`, 'error');
    } finally {
        isInstallingCrisper.value = false;
        crisperProgress.value = '';
    }
}

async function selectCrisperPython() {
    const selected = await open({
        directory: false,
        multiple: false,
        title: 'Select a Python 3.10+ interpreter',
    });
    if (typeof selected === 'string') {
        localCrisperPythonPath.value = selected;
        void refreshCrisperStatus();
    }
}

async function downloadAppleSpeechLocale() {
    isInstallingAppleSpeech.value = true;
    appleSpeechProgress.value = 'Starting...';
    try {
        setAppleSpeechStatus(
            await commands.installAppleSpeechLocale(effectiveAppleSpeechLocale.value),
        );
        showToast('The Apple Speech language is ready to use.', 'success');
    } catch (e) {
        console.error('Failed to download the Apple Speech language:', e);
        showToast(`Apple Speech download failed: ${errorMessage(e)}`, 'error');
    } finally {
        isInstallingAppleSpeech.value = false;
        appleSpeechProgress.value = '';
    }
}

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

            const confirmed = await ask(
                `Update to v${update.version} is available.\n\nRelease notes:\n${update.body}\n\nDo you want to download and install it now?`,
                {
                    title: 'Update Available',
                    kind: 'info',
                },
            );

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
                                    const nextValue = Math.min(
                                        100,
                                        (downloadedBytes.value / totalBytes.value) * 100,
                                    );
                                    toastProgress.value = Math.max(
                                        toastProgress.value || 0,
                                        nextValue,
                                    );
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
                    updateStatus.value = `Update failed: ${errorMessage(e)}`;
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
        updateStatus.value = `Error checking for updates: ${errorMessage(e)}`;
        showToast('Update check failed. Try again later.', 'error');
    } finally {
        isCheckingUpdate.value = false;
    }
}

const hasChanges = computed(() => {
    return (
        localBaseUrl.value !== settings.value.baseUrl ||
        localApiKey.value.trim() !== '' ||
        localModel.value !== settings.value.model ||
        localEnforceJsonSchema.value !== (settings.value.enforceJsonSchema ?? true) ||
        localMaxAnalysisChunkMinutes.value !== (settings.value.maxAnalysisChunkMinutes ?? 30) ||
        localPreClipPadding.value !== (settings.value.preClipPadding || 0) ||
        localPostClipPadding.value !== (settings.value.postClipPadding || 0) ||
        localExportQuality.value !== (settings.value.exportQuality ?? 'balanced') ||
        localTranscriptionBackend.value !== (settings.value.transcriptionBackend ?? 'llm') ||
        localLocalEngine.value !== (settings.value.localEngine ?? 'parakeet') ||
        localParakeetModelPath.value !== (settings.value.parakeetModelPath ?? '') ||
        localSortformerModelPath.value !== (settings.value.sortformerModelPath ?? '') ||
        localCrisperModel.value !== (settings.value.crisperModel ?? 'large') ||
        localCrisperLanguage.value !== (settings.value.crisperLanguage ?? 'en') ||
        localCrisperMode.value !== (settings.value.crisperMode ?? 'verbatim') ||
        localCrisperBackend.value !== (settings.value.crisperBackend ?? 'auto') ||
        localCrisperDevice.value !== (settings.value.crisperDevice ?? 'auto') ||
        localCrisperComputeType.value !== (settings.value.crisperComputeType ?? 'auto') ||
        localCrisperRemoveVocalEvents.value !==
            (settings.value.crisperRemoveVocalEvents ?? false) ||
        localCrisperDiarize.value !== (settings.value.crisperDiarize ?? true) ||
        localCrisperPythonPath.value !== (settings.value.crisperPythonPath ?? '') ||
        localAppleSpeechLocale.value !== (settings.value.appleSpeechLocale ?? '') ||
        localAppleSpeechDiarize.value !== (settings.value.appleSpeechDiarize ?? true)
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
    if (!canUseApiKey.value) {
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
        // The backend makes the request, with the stored key unless a new one
        // was typed here and not saved yet.
        const list = await commands.listModels(
            normalizeBaseUrl(localBaseUrl.value),
            localApiKey.value.trim() || null,
        );
        if (!list.supported) {
            updateModelFetchState({ supportsModelFetch: false, availableModels: [] });
            availableModels.value = [];
            if (!silent) {
                fetchError.value = 'This endpoint does not support model listing';
            }
            return;
        }
        const fetchedModels = list.models;

        if (fetchedModels.length === 0) {
            throw new Error('No models found');
        }

        availableModels.value = fetchedModels;
        updateModelFetchState({ supportsModelFetch: true, availableModels: fetchedModels });

        // Restore model selection (it stays as-is, just ensuring dropdown has it)
        localModel.value = currentModel;
    } catch (e) {
        if (!silent) {
            fetchError.value = `Error: ${errorMessage(e)}`;
            showManualInput.value = true;
        }
        // On error, set supportsModelFetch to false and provide fallback models
        updateModelFetchState({ supportsModelFetch: false });
        if (isGoogleApi.value) {
            availableModels.value = ['gemini-2.0-flash', 'gemini-1.5-pro', 'gemini-1.5-flash'];
        } else {
            availableModels.value = ['gpt-4', 'gpt-3.5-turbo'];
        }
        // Restore model selection
        localModel.value = currentModel;
    } finally {
        isFetchingModels.value = false;
    }
}

// Auto-fetch models on mount if API key is present
onMounted(() => {
    if (canUseApiKey.value && modelFetchState.value.supportsModelFetch !== false) {
        fetchModels(true);
    }
});

async function removeApiKey() {
    const confirmed = await ask('Remove the saved API key from the system keychain?', {
        title: 'Remove API key',
        kind: 'warning',
    });
    if (!confirmed) return;
    try {
        await commands.clearApiKey();
    } catch (error) {
        showToast(`Could not remove the API key: ${errorMessage(error)}`, 'error');
    }
    await refreshApiKeyStatus();
}

async function exportLogs() {
    try {
        const path = await save({
            filters: [
                {
                    name: 'Zip Files',
                    extensions: ['zip'],
                },
            ],
            defaultPath: 'ai-media-cutter-logs.zip',
        });

        if (path) {
            await commands.zipLogs(path);
            await message('Logs exported successfully!', { title: 'Export Logs' });
        }
    } catch (e) {
        await message(`Failed to export logs: ${errorMessage(e)}`, {
            title: 'Export Logs',
            kind: 'error',
        });
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

async function saveSettings() {
    const normalizedUrl = normalizeBaseUrl(localBaseUrl.value);
    const newApiKey = localApiKey.value.trim();
    if (newApiKey) {
        try {
            await commands.setApiKey(newApiKey);
        } catch (error) {
            showToast(`Could not save the API key: ${errorMessage(error)}`, 'error');
            return;
        }
        localApiKey.value = '';
        await refreshApiKeyStatus();
    }

    updateSettings({
        baseUrl: normalizedUrl,
        model: localModel.value,
        enforceJsonSchema: localEnforceJsonSchema.value,
        maxAnalysisChunkMinutes: localMaxAnalysisChunkMinutes.value,
        preClipPadding: localPreClipPadding.value,
        postClipPadding: localPostClipPadding.value,
        exportQuality: localExportQuality.value,
        transcriptionBackend: localTranscriptionBackend.value,
        localEngine: localLocalEngine.value,
        parakeetModelPath: localParakeetModelPath.value.trim(),
        sortformerModelPath: localSortformerModelPath.value.trim(),
        crisperModel: localCrisperModel.value.trim() || 'large',
        crisperLanguage: localCrisperLanguage.value,
        crisperMode: localCrisperMode.value,
        crisperBackend: localCrisperBackend.value,
        crisperDevice: localCrisperDevice.value,
        crisperComputeType: localCrisperComputeType.value,
        crisperRemoveVocalEvents: localCrisperRemoveVocalEvents.value,
        crisperDiarize: localCrisperDiarize.value,
        crisperPythonPath: localCrisperPythonPath.value.trim(),
        appleSpeechLocale: localAppleSpeechLocale.value,
        appleSpeechDiarize: localAppleSpeechDiarize.value,
    });
    router.push('/');
}

function cancel() {
    router.push('/');
}
</script>

<template>
    <div class="min-h-screen bg-gray-900 text-gray-200 p-8 font-sans">
        <div class="mx-auto w-full max-w-4xl">
            <header class="mb-10">
                <h1 class="text-4xl font-bold text-white mb-2">AI Settings</h1>
                <p class="text-gray-400">
                    Configure the transcription pipeline, local engines, and remote LLM access
                </p>
            </header>

            <div
                class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl"
            >
                <div class="mb-6">
                    <label
                        class="mb-3 block text-sm font-medium uppercase tracking-wider text-gray-400"
                    >
                        Default Pipeline
                    </label>
                    <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        <button
                            v-for="pipeline in PIPELINE_OPTIONS"
                            :key="pipeline.value"
                            type="button"
                            class="flex h-full flex-col rounded-2xl border p-4 text-left transition-all focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
                            :class="
                                localTranscriptionBackend === pipeline.value
                                    ? 'bg-blue-600/15 border-blue-500/40 text-white'
                                    : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'
                            "
                            @click="localTranscriptionBackend = pipeline.value"
                        >
                            <span class="text-sm font-semibold">{{ pipeline.title }}</span>
                            <span class="mt-1 text-xs leading-relaxed text-gray-400">{{
                                pipeline.description
                            }}</span>
                        </button>
                    </div>
                </div>

                <div class="mb-6">
                    <label
                        class="mb-3 block text-sm font-medium uppercase tracking-wider text-gray-400"
                    >
                        Default Local Engine
                    </label>
                    <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        <button
                            v-for="engine in engineOptions"
                            :key="engine.value"
                            type="button"
                            class="flex h-full flex-col rounded-2xl border p-4 text-left transition-all focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
                            :class="
                                localLocalEngine === engine.value
                                    ? 'bg-blue-600/15 border-blue-500/40 text-white'
                                    : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'
                            "
                            @click="localLocalEngine = engine.value"
                        >
                            <span class="flex items-center gap-2">
                                <span class="text-sm font-semibold">{{ engine.title }}</span>
                                <span
                                    v-if="engine.badge"
                                    class="rounded bg-amber-500/20 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-amber-300"
                                    >{{ engine.badge }}</span
                                >
                            </span>
                            <span class="mt-1 text-xs leading-relaxed text-gray-400">{{
                                engine.description
                            }}</span>
                        </button>
                    </div>
                    <p class="mt-2 text-xs text-gray-500">
                        Used by every pipeline except <strong>LLM Only</strong> &mdash; including
                        both hybrids, which layer the AI pass on top of whichever engine you pick.
                    </p>
                </div>

                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-4 uppercase tracking-wider"
                    >
                        Parakeet Settings
                    </label>
                    <div class="space-y-4">
                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2"
                                >Parakeet TDT Model Directory</label
                            >
                            <div class="flex gap-3">
                                <input
                                    v-model="localParakeetModelPath"
                                    type="text"
                                    class="flex-1 p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                    placeholder="Leave blank to auto-download into app data"
                                />
                                <button
                                    @click="selectParakeetModelDirectory"
                                    class="px-4 py-3 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-2xl transition-all border border-white/10"
                                >
                                    Browse
                                </button>
                            </div>
                            <p class="text-xs text-gray-500 mt-2">
                                Leave blank to let the app download an int8 TDT model into Tauri app
                                data. Custom directories should contain encoder, decoder, and
                                `vocab.txt`.
                            </p>
                        </div>
                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2"
                                >Sortformer Model File</label
                            >
                            <div class="flex gap-3">
                                <input
                                    v-model="localSortformerModelPath"
                                    type="text"
                                    class="flex-1 p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                    placeholder="Leave blank to auto-download into app data"
                                />
                                <button
                                    @click="selectSortformerModelFile"
                                    class="px-4 py-3 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-2xl transition-all border border-white/10"
                                >
                                    Browse
                                </button>
                            </div>
                            <p class="text-xs text-gray-500 mt-2">
                                Leave blank to let the app download Sortformer v2 automatically, or
                                provide your own `.onnx` file.
                            </p>
                        </div>
                    </div>
                </div>

                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-4 uppercase tracking-wider"
                    >
                        CrisperWhisper Settings
                    </label>

                    <div class="mb-4 p-4 rounded-2xl bg-amber-500/10 border border-amber-500/30">
                        <p class="text-xs text-amber-200 font-semibold mb-1">
                            English and German only
                        </p>
                        <p class="text-xs text-amber-100/80">
                            The CrisperWhisper 2.0 model card is published for English
                            (<code>en</code>) and German (<code>de</code>). Other languages are not
                            supported by this backend &mdash; use the Parakeet engine or the LLM
                            Only pipeline for those.
                        </p>
                        <p class="text-xs text-amber-100/80 mt-2">
                            {{ crisperWeightsNotice }} The app installs the model on first use.
                        </p>
                    </div>

                    <!-- Environment status -->
                    <div class="mb-4 p-4 rounded-2xl bg-black/20 border border-white/10">
                        <div class="flex items-start justify-between gap-3 mb-2">
                            <div>
                                <p
                                    class="text-xs font-medium text-gray-400 uppercase tracking-wider"
                                >
                                    Runtime
                                </p>
                                <p
                                    v-if="isInstallingCrisper || isCheckingCrisper"
                                    class="text-sm text-gray-300 mt-1"
                                >
                                    {{
                                        crisperProgress ||
                                        (isInstallingCrisper ? 'Installing...' : 'Checking...')
                                    }}
                                </p>
                                <p
                                    v-else-if="crisperStatus?.ready"
                                    class="text-sm text-green-400 mt-1"
                                >
                                    Ready &mdash; CrisperWhisper
                                    {{ crisperStatus.crisperwhisperVersion }}, Python
                                    {{ crisperStatus.python }},
                                    {{ crisperStatus.backends.join(' + ') }}
                                    <span v-if="crisperStatus.cuda">(CUDA)</span>
                                    <span v-else-if="crisperStatus.mps">(Apple GPU available)</span>
                                    <span v-else>(CPU)</span>
                                </p>
                                <p v-else class="text-sm text-amber-300 mt-1">
                                    {{ crisperStatus?.message ?? 'Not set up yet.' }}
                                </p>
                            </div>
                            <div class="flex gap-2 shrink-0">
                                <button
                                    type="button"
                                    :disabled="isCheckingCrisper || isInstallingCrisper"
                                    @click="refreshCrisperStatus"
                                    class="px-3 py-2 bg-white/10 hover:bg-white/20 disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-medium rounded-xl transition-all border border-white/10"
                                >
                                    Re-check
                                </button>
                                <button
                                    v-if="!crisperStatus?.ready"
                                    type="button"
                                    :disabled="isInstallingCrisper"
                                    @click="setUpCrisperEnvironment"
                                    class="px-3 py-2 bg-blue-600 hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-medium rounded-xl transition-all"
                                >
                                    {{ isInstallingCrisper ? 'Installing...' : 'Set up' }}
                                </button>
                            </div>
                        </div>
                        <p class="text-xs text-gray-500">
                            CrisperWhisper ships only PyTorch and CTranslate2 weights, so it runs
                            through a private Python environment the app manages at
                            <code class="break-all">{{
                                crisperStatus?.environmentDir || 'the app data directory'
                            }}</code
                            >. Setup needs Python {{ crisperStatus?.minimumPython || '3.10' }} or
                            newer on your system.
                        </p>
                    </div>

                    <div class="space-y-4">
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-xs font-medium text-gray-500 mb-2"
                                    >Model Size</label
                                >
                                <select
                                    v-model="localCrisperModel"
                                    class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300"
                                >
                                    <option
                                        v-for="size in CRISPER_MODELS"
                                        :key="size"
                                        :value="size"
                                    >
                                        {{ size }}
                                    </option>
                                </select>
                                <p class="text-xs text-gray-500 mt-2">
                                    <code>large</code> is the most accurate, <code>turbo</code> the
                                    fastest, <code>medium</code> the best tradeoff.
                                </p>
                            </div>
                            <div>
                                <label class="block text-xs font-medium text-gray-500 mb-2"
                                    >Language</label
                                >
                                <select
                                    v-model="localCrisperLanguage"
                                    class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300"
                                >
                                    <option
                                        v-for="language in CRISPER_LANGUAGES"
                                        :key="language.value"
                                        :value="language.value"
                                    >
                                        {{ language.label }}
                                    </option>
                                </select>
                                <p class="text-xs text-gray-500 mt-2">
                                    Only English and German are supported.
                                </p>
                            </div>
                        </div>

                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2"
                                >Transcription Mode</label
                            >
                            <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                                <button
                                    type="button"
                                    class="text-left p-4 rounded-2xl border transition-all"
                                    :class="
                                        localCrisperMode === 'verbatim'
                                            ? 'bg-blue-600/15 border-blue-500/40 text-white'
                                            : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'
                                    "
                                    @click="localCrisperMode = 'verbatim'"
                                >
                                    <div class="text-sm font-semibold">Verbatim</div>
                                    <p class="text-xs text-gray-400 mt-1">
                                        Exactly what was said, including fillers, repetitions and
                                        stutters. Best for cutting &mdash; you can see and remove
                                        every &ldquo;um&rdquo;.
                                    </p>
                                </button>
                                <button
                                    type="button"
                                    class="text-left p-4 rounded-2xl border transition-all"
                                    :class="
                                        localCrisperMode === 'intended'
                                            ? 'bg-blue-600/15 border-blue-500/40 text-white'
                                            : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'
                                    "
                                    @click="localCrisperMode = 'intended'"
                                >
                                    <div class="text-sm font-semibold">Intended</div>
                                    <p class="text-xs text-gray-400 mt-1">
                                        The clean version the speaker meant, with numbers and dates
                                        formatted for reading. Best for subtitles.
                                    </p>
                                </button>
                            </div>
                            <p class="text-xs text-gray-500 mt-2">
                                Filler removal is toggled per run with
                                <strong>Remove Filler Words</strong> on the analysis panel. In
                                verbatim mode it cuts the filler out of the exported video too.
                            </p>
                        </div>

                        <div class="flex flex-col sm:flex-row gap-3">
                            <div
                                class="flex items-center gap-3 p-4 flex-1 bg-black/20 rounded-2xl border border-white/10 cursor-pointer hover:bg-black/30 transition-colors"
                                @click="
                                    localCrisperRemoveVocalEvents = !localCrisperRemoveVocalEvents
                                "
                            >
                                <div
                                    class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors shrink-0"
                                    :class="
                                        localCrisperRemoveVocalEvents
                                            ? 'bg-blue-600'
                                            : 'bg-gray-700'
                                    "
                                >
                                    <span
                                        class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                                        :class="
                                            localCrisperRemoveVocalEvents
                                                ? 'translate-x-6'
                                                : 'translate-x-1'
                                        "
                                    />
                                </div>
                                <div>
                                    <span class="text-sm font-medium text-gray-300"
                                        >Remove Vocal Events</span
                                    >
                                    <p class="text-xs text-gray-500">
                                        Cuts [laughter], [breath], [cough], [sigh]&hellip;
                                    </p>
                                </div>
                            </div>
                            <div
                                class="flex items-center gap-3 p-4 flex-1 bg-black/20 rounded-2xl border border-white/10 cursor-pointer hover:bg-black/30 transition-colors"
                                @click="localCrisperDiarize = !localCrisperDiarize"
                            >
                                <div
                                    class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors shrink-0"
                                    :class="localCrisperDiarize ? 'bg-blue-600' : 'bg-gray-700'"
                                >
                                    <span
                                        class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                                        :class="
                                            localCrisperDiarize ? 'translate-x-6' : 'translate-x-1'
                                        "
                                    />
                                </div>
                                <div>
                                    <span class="text-sm font-medium text-gray-300"
                                        >Identify Speakers</span
                                    >
                                    <p class="text-xs text-gray-500">
                                        Adds Sortformer diarization; slower.
                                    </p>
                                </div>
                            </div>
                        </div>

                        <details class="rounded-2xl bg-black/20 border border-white/10">
                            <summary
                                class="p-4 text-xs font-medium text-gray-400 uppercase tracking-wider cursor-pointer select-none"
                            >
                                Advanced runtime options
                            </summary>
                            <div class="px-4 pb-4 space-y-4">
                                <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
                                    <div>
                                        <label class="block text-xs font-medium text-gray-500 mb-2"
                                            >Inference Backend</label
                                        >
                                        <select
                                            v-model="localCrisperBackend"
                                            class="w-full p-3 rounded-xl bg-black/30 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 text-sm"
                                        >
                                            <option value="auto">Auto</option>
                                            <option value="transformers">PyTorch (portable)</option>
                                            <option value="ct2">
                                                CTranslate2 (Linux + NVIDIA)
                                            </option>
                                        </select>
                                    </div>
                                    <div>
                                        <label class="block text-xs font-medium text-gray-500 mb-2"
                                            >Device</label
                                        >
                                        <select
                                            v-model="localCrisperDevice"
                                            class="w-full p-3 rounded-xl bg-black/30 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 text-sm"
                                        >
                                            <option value="auto">Auto</option>
                                            <option value="cpu">CPU</option>
                                            <option value="cuda">CUDA</option>
                                            <option value="mps">Apple GPU (MPS)</option>
                                        </select>
                                    </div>
                                    <div>
                                        <label class="block text-xs font-medium text-gray-500 mb-2"
                                            >Precision</label
                                        >
                                        <select
                                            v-model="localCrisperComputeType"
                                            class="w-full p-3 rounded-xl bg-black/30 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 text-sm"
                                        >
                                            <option value="auto">Auto</option>
                                            <option value="float32">float32</option>
                                            <option value="float16">float16</option>
                                            <option value="int8_float16">
                                                int8_float16 (fastest)
                                            </option>
                                        </select>
                                    </div>
                                </div>
                                <p class="text-xs text-gray-500">
                                    CTranslate2 is roughly 4&ndash;5&times; faster but its wheels
                                    are Linux x86_64 only; everywhere else the portable PyTorch
                                    backend is used. Auto precision picks float32 on CPU and float16
                                    on a GPU. On Apple Silicon, Auto uses the GPU (MPS, float16):
                                    the app's own decoding loop runs about twice as fast there as on
                                    the CPU, with the same transcript.
                                    <strong>int8_float16</strong> is another ~1.4&times; faster on
                                    GPU and CPU; on 10 minutes of German the word error rate was
                                    unchanged, and up to 14 of 2,076 words differed, mostly fillers
                                    and punctuation.
                                </p>
                                <div>
                                    <label class="block text-xs font-medium text-gray-500 mb-2"
                                        >Python Interpreter</label
                                    >
                                    <div class="flex gap-3">
                                        <input
                                            v-model="localCrisperPythonPath"
                                            type="text"
                                            class="flex-1 p-3 rounded-xl bg-black/30 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600 text-sm"
                                            placeholder="Leave blank to use the app-managed environment"
                                        />
                                        <button
                                            @click="selectCrisperPython"
                                            class="px-4 py-2 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-xl transition-all border border-white/10"
                                        >
                                            Browse
                                        </button>
                                    </div>
                                    <p class="text-xs text-gray-500 mt-2">
                                        Point this at your own environment if you already have
                                        <code>crisperwhisper</code> installed.
                                    </p>
                                </div>
                            </div>
                        </details>
                    </div>
                </div>

                <div
                    v-if="appleSpeechSupported"
                    class="mb-6 group border-t border-white/10 pt-6 mt-6"
                >
                    <label
                        class="block text-sm font-medium text-gray-400 mb-4 uppercase tracking-wider"
                    >
                        Apple Speech Settings
                    </label>

                    <div class="mb-4 p-4 rounded-2xl bg-black/20 border border-white/10">
                        <div class="flex items-start justify-between gap-3 mb-2">
                            <div>
                                <p
                                    class="text-xs font-medium text-gray-400 uppercase tracking-wider"
                                >
                                    Runtime
                                </p>
                                <p
                                    v-if="isInstallingAppleSpeech"
                                    class="text-sm text-gray-300 mt-1"
                                >
                                    {{ appleSpeechProgress || 'Downloading...' }}
                                </p>
                                <p
                                    v-else-if="isCheckingAppleSpeech"
                                    class="text-sm text-gray-300 mt-1"
                                >
                                    Checking...
                                </p>
                                <p
                                    v-else-if="appleSpeechStatus?.available"
                                    class="text-sm mt-1"
                                    :class="
                                        appleSpeechLocaleInstalled
                                            ? 'text-green-400'
                                            : 'text-amber-300'
                                    "
                                >
                                    <template v-if="appleSpeechLocaleInstalled">
                                        Ready &mdash; {{ effectiveAppleSpeechLocale }} is installed.
                                    </template>
                                    <template v-else>
                                        The {{ effectiveAppleSpeechLocale }} model is not downloaded
                                        yet; it downloads on first use, or now.
                                    </template>
                                </p>
                                <p v-else class="text-sm text-amber-300 mt-1">
                                    {{ appleSpeechStatus?.message ?? 'Not available.' }}
                                </p>
                            </div>
                            <div class="flex gap-2 shrink-0">
                                <button
                                    type="button"
                                    :disabled="isCheckingAppleSpeech || isInstallingAppleSpeech"
                                    @click="refreshAppleSpeechStatus"
                                    class="px-3 py-2 bg-white/10 hover:bg-white/20 disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-medium rounded-xl transition-all border border-white/10"
                                >
                                    Re-check
                                </button>
                                <button
                                    v-if="
                                        appleSpeechStatus?.available && !appleSpeechLocaleInstalled
                                    "
                                    type="button"
                                    :disabled="isInstallingAppleSpeech"
                                    @click="downloadAppleSpeechLocale"
                                    class="px-3 py-2 bg-blue-600 hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-medium rounded-xl transition-all"
                                >
                                    {{ isInstallingAppleSpeech ? 'Downloading...' : 'Download' }}
                                </button>
                            </div>
                        </div>
                        <p class="text-xs text-gray-500">
                            Runs macOS's own on-device speech model (macOS 26 or newer). Nothing
                            leaves this Mac; macOS downloads one model per language and manages it.
                            It is not verbatim &mdash; fillers like &ldquo;um&rdquo; are dropped
                            &mdash; so use CrisperWhisper to cut those out.
                        </p>
                    </div>

                    <div class="space-y-4">
                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2"
                                >Language</label
                            >
                            <select
                                v-model="localAppleSpeechLocale"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300"
                            >
                                <option value="">
                                    System language ({{ appleSpeechLocale('') }})
                                </option>
                                <option
                                    v-for="locale in appleSpeechLocaleOptions"
                                    :key="locale.value"
                                    :value="locale.value"
                                >
                                    {{ locale.label }}
                                </option>
                            </select>
                            <p class="text-xs text-gray-500 mt-2">
                                &#10003; marks languages already downloaded.
                            </p>
                        </div>

                        <div
                            class="flex items-center gap-3 p-4 bg-black/20 rounded-2xl border border-white/10 cursor-pointer hover:bg-black/30 transition-colors"
                            @click="localAppleSpeechDiarize = !localAppleSpeechDiarize"
                        >
                            <div
                                class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors shrink-0"
                                :class="localAppleSpeechDiarize ? 'bg-blue-600' : 'bg-gray-700'"
                            >
                                <span
                                    class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                                    :class="
                                        localAppleSpeechDiarize ? 'translate-x-6' : 'translate-x-1'
                                    "
                                />
                            </div>
                            <div>
                                <span class="text-sm font-medium text-gray-300"
                                    >Identify Speakers</span
                                >
                                <p class="text-xs text-gray-500">
                                    Adds Sortformer diarization; Apple Speech does not tell speakers
                                    apart.
                                </p>
                            </div>
                        </div>
                    </div>
                </div>

                <!-- Base URL -->
                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >
                        LLM Base URL
                    </label>
                    <input
                        v-model="localBaseUrl"
                        type="text"
                        class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                        placeholder="https://generativelanguage.googleapis.com"
                    />
                    <p class="text-xs text-gray-500 mt-2">
                        The base URL for the LLM API endpoint (trailing slashes will be removed)
                    </p>
                </div>

                <!-- API Key -->
                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >
                        LLM API Key
                    </label>
                    <div class="flex gap-3">
                        <input
                            v-model="localApiKey"
                            type="password"
                            autocomplete="off"
                            data-testid="api-key-input"
                            class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                            :placeholder="
                                apiKeyStored
                                    ? 'Saved in the system keychain — enter a new key to replace it'
                                    : 'Enter your API key'
                            "
                        />
                        <button
                            v-if="apiKeyStored"
                            type="button"
                            data-testid="api-key-remove"
                            class="shrink-0 px-4 py-3 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-2xl transition-all border border-white/10"
                            @click="removeApiKey"
                        >
                            Remove
                        </button>
                    </div>
                    <p class="text-xs text-gray-500 mt-2">
                        Stored in your system keychain (macOS Keychain, Windows Credential Manager
                        or the Secret Service on Linux), never in the app's settings.
                    </p>
                </div>

                <!-- Model Selection -->
                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >
                        LLM Model
                    </label>
                    <div class="flex gap-3 mb-2">
                        <div class="flex-1 relative">
                            <select
                                v-if="!showManualInput"
                                v-model="localModel"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300"
                                :class="{ 'pr-12': modelNotInList }"
                            >
                                <option
                                    v-if="
                                        availableModels.length === 0 ||
                                        !availableModels.includes(localModel)
                                    "
                                    :value="localModel"
                                >
                                    {{ localModel }}
                                </option>
                                <option
                                    v-for="model in availableModels"
                                    :key="model"
                                    :value="model"
                                >
                                    {{ model }}
                                </option>
                            </select>
                            <input
                                v-else
                                v-model="localModel"
                                type="text"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                placeholder="Enter model name manually"
                            />
                            <!-- Warning indicator when model is not in the available list -->
                            <div
                                v-if="modelNotInList && !showManualInput"
                                class="absolute right-4 top-1/2 -translate-y-1/2 group/tooltip"
                            >
                                <svg
                                    class="w-5 h-5 text-amber-400"
                                    fill="currentColor"
                                    viewBox="0 0 20 20"
                                >
                                    <path
                                        fill-rule="evenodd"
                                        d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z"
                                        clip-rule="evenodd"
                                    />
                                </svg>
                                <div
                                    class="absolute bottom-full left-1/2 -translate-x-1/2 mb-2 px-3 py-1.5 bg-gray-800 text-xs text-amber-300 rounded-lg whitespace-nowrap opacity-0 group-hover/tooltip:opacity-100 transition-opacity pointer-events-none border border-amber-500/30"
                                >
                                    Model not found in available models list
                                </div>
                            </div>
                        </div>
                        <button
                            @click="fetchModels()"
                            :disabled="isFetchingModels || !canUseApiKey"
                            class="btn-primary px-6 py-3 flex items-center gap-2"
                        >
                            <svg
                                v-if="!isFetchingModels"
                                class="w-4 h-4"
                                fill="none"
                                stroke="currentColor"
                                viewBox="0 0 24 24"
                            >
                                <path
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                    stroke-width="2"
                                    d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
                                />
                            </svg>
                            {{ isFetchingModels ? 'Fetching...' : 'Refresh Models' }}
                        </button>
                    </div>
                    <div class="flex items-center gap-2 mb-1">
                        <button
                            @click="showManualInput = !showManualInput"
                            class="text-xs text-blue-400 hover:text-blue-300 transition-colors underline"
                        >
                            {{ showManualInput ? 'Use dropdown' : 'Enter manually' }}
                        </button>
                    </div>
                    <p v-if="fetchError" class="text-xs text-red-400 mt-1">{{ fetchError }}</p>
                    <p v-else class="text-xs text-gray-500 mt-1">{{ endpointInfo }}</p>
                </div>

                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >
                        Response Validation
                    </label>
                    <div
                        class="flex items-start gap-4 rounded-2xl bg-black/20 border border-white/10 p-4 cursor-pointer hover:bg-black/30 transition-colors"
                        @click="localEnforceJsonSchema = !localEnforceJsonSchema"
                    >
                        <div
                            class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors"
                            :class="localEnforceJsonSchema ? 'bg-blue-600' : 'bg-gray-700'"
                        >
                            <span
                                class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                                :class="localEnforceJsonSchema ? 'translate-x-6' : 'translate-x-1'"
                            />
                        </div>
                        <div class="flex-1">
                            <p class="text-sm font-medium text-gray-200">
                                Enforce Structured JSON for transcript analysis
                            </p>
                            <p class="text-xs text-gray-500 mt-1">
                                Sends a strict JSON schema with AI analysis requests on
                                OpenAI-compatible APIs. Improves reliability, but some providers may
                                respond slower or behave differently.
                            </p>
                        </div>
                    </div>
                </div>

                <div class="mb-6 group">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >
                        Long Audio Handling
                    </label>
                    <label class="block text-xs font-medium text-gray-500 mb-2"
                        >Max analysis chunk length (minutes)</label
                    >
                    <input
                        v-model.number="localMaxAnalysisChunkMinutes"
                        type="number"
                        step="1"
                        min="0"
                        class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                        placeholder="30"
                    />
                    <p class="text-xs text-gray-500 mt-2">
                        Audio longer than this is split into chunks before LLM transcription, so
                        each request stays under the provider's request timeout (avoids
                        <span class="text-gray-400">504 Gateway Timeout</span>
                        on long videos). Splits prefer a ~1s silence near the boundary and fall back
                        to Parakeet word boundaries. Set to 0 to disable chunking. Only applies to
                        LLM-based transcription.
                    </p>
                </div>

                <!-- Export -->
                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-4 uppercase tracking-wider"
                    >
                        Video Export Quality
                    </label>
                    <div class="grid grid-cols-1 gap-3 sm:grid-cols-3" role="radiogroup">
                        <button
                            v-for="option in EXPORT_QUALITY_OPTIONS"
                            :key="option.value"
                            type="button"
                            role="radio"
                            :aria-checked="localExportQuality === option.value"
                            :data-testid="`export-quality-${option.value}`"
                            class="flex flex-col rounded-xl border px-4 py-3 text-left transition-all focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
                            :class="
                                localExportQuality === option.value
                                    ? 'bg-blue-600/15 border-blue-500/40 text-white'
                                    : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'
                            "
                            @click="localExportQuality = option.value"
                        >
                            <span class="text-sm font-semibold">{{ option.label }}</span>
                            <span class="mt-1 text-xs text-gray-400">{{ option.description }}</span>
                        </button>
                    </div>
                    <p class="text-xs text-gray-500 mt-2">
                        Applies to Export Video and clip exports. A hardware encoder is used when
                        this machine has one.
                    </p>
                </div>

                <!-- Clip Settings -->
                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-4 uppercase tracking-wider"
                    >
                        Clip Settings
                    </label>
                    <div class="grid grid-cols-2 gap-4">
                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2"
                                >Pre-Clip Padding (seconds)</label
                            >
                            <input
                                v-model.number="localPreClipPadding"
                                type="number"
                                step="0.1"
                                min="0"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                placeholder="0.0"
                            />
                            <p class="text-xs text-gray-500 mt-2">
                                Added before the start of each clip
                            </p>
                        </div>
                        <div>
                            <label class="block text-xs font-medium text-gray-500 mb-2"
                                >Post-Clip Padding (seconds)</label
                            >
                            <input
                                v-model.number="localPostClipPadding"
                                type="number"
                                step="0.1"
                                min="0"
                                class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                                placeholder="0.0"
                            />
                            <p class="text-xs text-gray-500 mt-2">
                                Added after the end of each clip
                            </p>
                        </div>
                    </div>
                </div>

                <!-- Licences of what the app downloads or bundles at runtime
                     (src/assets/runtime-licenses.json). -->
                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >
                        Third-Party Models &amp; Tools
                    </label>
                    <ul
                        data-testid="runtime-licenses"
                        class="divide-y divide-white/5 rounded-2xl border border-white/10 bg-black/20"
                    >
                        <li
                            v-for="download in runtimeLicenses.downloads"
                            :key="download.id"
                            class="px-4 py-3"
                        >
                            <div class="flex items-baseline justify-between gap-3">
                                <span class="text-sm text-gray-200">{{ download.name }}</span>
                                <button
                                    type="button"
                                    class="shrink-0 text-xs text-blue-300 hover:text-blue-200 hover:underline"
                                    :title="download.licenseUrl"
                                    @click="openUrl(download.licenseUrl)"
                                >
                                    {{ download.license.replace('LicenseRef-', '') }}
                                </button>
                            </div>
                            <p class="text-xs text-gray-400">{{ download.usedBy }}</p>
                            <p v-if="download.notice" class="mt-1 text-xs text-gray-300">
                                {{ download.notice }}
                            </p>
                        </li>
                    </ul>
                </div>

                <!-- Application Info -->
                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >
                        Application Info
                    </label>
                    <div
                        class="flex items-center justify-between bg-black/20 p-4 rounded-2xl border border-white/10"
                    >
                        <div>
                            <p class="text-gray-300 font-medium">
                                Version: <span class="text-white">{{ appVersion }}</span>
                            </p>
                            <p
                                v-if="updateStatus"
                                class="text-xs mt-1"
                                :class="updateAvailable ? 'text-green-400' : 'text-gray-400'"
                            >
                                {{ updateStatus }}
                            </p>
                        </div>
                        <button
                            @click="checkForUpdates"
                            :disabled="isCheckingUpdate"
                            class="btn-primary px-4 py-2 text-sm"
                        >
                            {{ isCheckingUpdate ? 'Checking...' : 'Check for Updates' }}
                        </button>
                    </div>
                </div>

                <!-- Troubleshooting -->
                <div class="mb-6 group border-t border-white/10 pt-6 mt-6">
                    <label
                        class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >
                        Troubleshooting
                    </label>
                    <div class="flex gap-3">
                        <button
                            @click="exportLogs"
                            class="px-6 py-3 bg-gray-700 hover:bg-gray-600 text-white font-semibold rounded-2xl border border-gray-600 hover:border-gray-500 transition-all shadow-lg hover:shadow-xl active:scale-95"
                        >
                            Export Logs
                        </button>
                    </div>
                    <p class="text-xs text-gray-500 mt-2">
                        Export application logs for debugging purposes.
                    </p>
                </div>

                <!-- Action Buttons -->
                <div class="flex gap-4 mt-8">
                    <button
                        @click="saveSettings"
                        :disabled="!hasChanges"
                        class="flex-1 bg-emerald-600 hover:bg-emerald-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg shadow-emerald-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all transform hover:-translate-y-0.5 active:translate-y-0"
                    >
                        Save Settings
                    </button>
                    <button
                        @click="cancel"
                        class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-bold py-4 px-6 rounded-2xl border border-gray-600 hover:border-gray-500 transition-all shadow-lg hover:shadow-xl active:scale-95"
                    >
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
