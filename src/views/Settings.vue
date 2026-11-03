<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, reactive, watch } from 'vue';
import { useRouter } from 'vue-router';
import {
    AudioLines,
    ChevronRight,
    Cloud,
    Cpu,
    Download,
    ExternalLink,
    Eye,
    EyeOff,
    FileArchive,
    Film,
    FolderOpen,
    Info,
    KeyRound,
    LifeBuoy,
    List,
    Loader2,
    PenLine,
    RefreshCw,
    Scale,
    Sparkles,
    Timer,
    TriangleAlert,
    Wand2,
} from '@lucide/vue';
import PageHeader from '../components/ui/PageHeader.vue';
import SectionCard from '../components/ui/SectionCard.vue';
import UiDialog from '../components/ui/UiDialog.vue';
import OptionCardGroup, { type OptionCard } from '../components/settings/OptionCardGroup.vue';
import SettingsSwitchRow from '../components/settings/SettingsSwitchRow.vue';
import DisclosureCard from '../components/settings/DisclosureCard.vue';
import { useWorkspaceNav } from '../composables/useWorkspaceNav';
import { refreshApiKeyStatus, useSettings } from '../composables/useSettings';
import { appleSpeechLocale, useAppleSpeech } from '../composables/useAppleSpeech';
import { effectiveNemotronVariant, useNemotron } from '../composables/useNemotron';
import { useSetupWizard } from '../composables/useSetupWizard';
import {
    ASR_LANGUAGES,
    DEFAULT_GEMINI_MODEL,
    GEMINI_BASE_URL,
    defaultCloudAsrModel,
    defaultOpenRouterModel,
    formatBytes,
    openRouterBaseUrl,
    providerForBaseUrl,
    type LlmProvider,
} from '../utils/providers';
import { open, save, ask, message } from '@tauri-apps/plugin-dialog';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { listen } from '@tauri-apps/api/event';
import Toast from '../components/Toast.vue';
import { openUrl } from '@tauri-apps/plugin-opener';
import { runtimeDownload, runtimeLicenses } from '../utils/runtimeLicenses';
import { CRISPER_LANGUAGES, CRISPER_MODELS } from '../types';
import type { NemotronVariant, TranscriptionModel } from '../bindings';
import type {
    CrisperEnvironmentStatus,
    CrisperLanguage,
    CrisperMode,
    LocalEngine,
    TranscriptionBackend,
} from '../types';
import { commands } from '../bindings';
import { errorMessage } from '../utils/appError';

const PIPELINE_OPTIONS: OptionCard<TranscriptionBackend>[] = [
    {
        value: 'llm',
        title: 'LLM Only',
        description: 'Uses the API settings below for transcription and speaker labeling.',
    },
    {
        value: 'local',
        title: 'Speech Engine Only',
        description: 'Runs just the speech engine. On-device engines need no API key.',
    },
    {
        value: 'hybrid',
        title: 'Hybrid Cleanup',
        description: 'Engine timings, then a remote LLM pass to clean up wording.',
    },
    {
        value: 'hybrid-merge',
        title: 'Hybrid Merge',
        description: 'Merges the engine and a remote LLM transcript onto the engine timings.',
    },
];

const ENGINE_OPTIONS: OptionCard<LocalEngine>[] = [
    {
        value: 'nemotron',
        title: 'Nemotron 3.5',
        badge: 'Recommended',
        description:
            "NVIDIA's speech-LLM on this machine: most accurate, ~40 languages, speakers, glossary.",
    },
    {
        value: 'parakeet',
        title: 'Parakeet',
        description: 'Parakeet TDT + Sortformer diarization with word timestamps.',
    },
    {
        value: 'crisper',
        title: 'CrisperWhisper',
        badge: 'EN / DE',
        description: 'Verbatim transcription with precise word timings. English and German only.',
    },
    {
        value: 'apple-speech',
        title: 'Apple Speech',
        badge: 'macOS 26+',
        description: "macOS's built-in on-device model. Very fast, ~45 languages; drops fillers.",
    },
    {
        value: 'cloud',
        title: 'Cloud',
        badge: 'OpenRouter',
        description: 'Speech-to-text API via OpenRouter (Nemotron, Voxtral, …). Optional EU-only.',
    },
];

const PROVIDER_OPTIONS: OptionCard<LlmProvider>[] = [
    {
        value: 'openrouter',
        title: 'OpenRouter',
        description: 'Hundreds of models with one key, shared with cloud speech-to-text.',
        testId: 'provider-openrouter',
    },
    {
        value: 'gemini',
        title: 'Google Gemini',
        description: 'Gemini API key from Google AI Studio.',
        testId: 'provider-gemini',
    },
    {
        value: 'custom',
        title: 'Custom',
        description: 'Any OpenAI-compatible endpoint (LiteLLM, vLLM, Ollama, …).',
        testId: 'provider-custom',
    },
];

const router = useRouter();
const {
    settings,
    apiKeyStored,
    openRouterKeyStored,
    updateSettings,
    modelFetchState,
    updateModelFetchState,
} = useSettings();
const setupWizard = useSetupWizard();
const nemotron = useNemotron();
const {
    status: appleSpeechStatus,
    checking: isCheckingAppleSpeech,
    supported: appleSpeechSupported,
    refresh: refreshAppleSpeechStatus,
    setStatus: setAppleSpeechStatus,
} = useAppleSpeech();

const localBaseUrl = ref(settings.value.baseUrl);
const localProvider = ref<LlmProvider>(providerForBaseUrl(settings.value.baseUrl));
const localOpenRouterEuOnly = ref(settings.value.openRouterEuOnly ?? false);
// New keys typed here; stored keys are never loaded into the UI.
const localApiKey = ref('');
const localOpenRouterKey = ref('');
const providerUsesOpenRouter = computed(() => localProvider.value === 'openrouter');
/** The key field of the AI provider section edits the provider's own key. */
const providerKey = computed({
    get: () => (providerUsesOpenRouter.value ? localOpenRouterKey.value : localApiKey.value),
    set: (value: string) => {
        if (providerUsesOpenRouter.value) localOpenRouterKey.value = value;
        else localApiKey.value = value;
    },
});
const providerKeyStored = computed(() =>
    providerUsesOpenRouter.value ? openRouterKeyStored.value : apiKeyStored.value,
);
const canUseApiKey = computed(() => providerKeyStored.value || providerKey.value.trim().length > 0);

// Presets own the base URL; only Custom edits it.
watch([localProvider, localOpenRouterEuOnly], ([provider, euOnly], [previous]) => {
    if (provider === 'gemini') {
        localBaseUrl.value = GEMINI_BASE_URL;
        if (previous !== 'gemini') localModel.value = DEFAULT_GEMINI_MODEL;
    } else if (provider === 'openrouter') {
        localBaseUrl.value = openRouterBaseUrl(euOnly);
        if (previous !== 'openrouter' || !localModel.value.includes('/')) {
            localModel.value = defaultOpenRouterModel(euOnly);
        }
    } else if (previous !== 'custom') {
        localBaseUrl.value = '';
    }
    if (provider !== previous || provider === 'openrouter') {
        availableModels.value = [];
        if (canUseApiKey.value) void fetchModels(true);
    }
});
const localModel = ref(settings.value.model);
const localEnforceJsonSchema = ref(settings.value.enforceJsonSchema ?? true);
const localMaxAnalysisChunkMinutes = ref(settings.value.maxAnalysisChunkMinutes ?? 30);
const availableModels = ref<string[]>(modelFetchState.value.availableModels);
const localPreClipPadding = ref(settings.value.preClipPadding || 0);
const localPostClipPadding = ref(settings.value.postClipPadding || 0);
const localExportQuality = ref(settings.value.exportQuality ?? 'balanced');
const EXPORT_QUALITY_OPTIONS: OptionCard<'high' | 'balanced' | 'draft'>[] = [
    {
        value: 'high',
        title: 'High',
        description: 'Visually lossless; larger files, slower.',
        testId: 'export-quality-high',
    },
    {
        value: 'balanced',
        title: 'Balanced',
        description: 'Good quality at a sensible size.',
        testId: 'export-quality-balanced',
    },
    {
        value: 'draft',
        title: 'Draft',
        description: 'Fast and small, for previews.',
        testId: 'export-quality-draft',
    },
];
const CRISPER_MODE_OPTIONS: OptionCard<CrisperMode>[] = [
    {
        value: 'verbatim',
        title: 'Verbatim',
        description:
            'Exactly what was said, including fillers, repetitions and stutters. Best for cutting: you can see and remove every “um”.',
    },
    {
        value: 'intended',
        title: 'Intended',
        description:
            'The clean version the speaker meant, with numbers and dates formatted for reading. Best for subtitles.',
    },
];
const localTranscriptionBackend = ref<TranscriptionBackend>(
    settings.value.transcriptionBackend ?? 'llm',
);
const localLocalEngine = ref<LocalEngine>(settings.value.localEngine ?? 'nemotron');
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
const localNemotronVariant = ref<NemotronVariant | ''>(settings.value.nemotronVariant ?? '');
const localNemotronDiarize = ref(settings.value.nemotronDiarize ?? true);
const localAsrLanguage = ref(settings.value.asrLanguage ?? '');
const localCloudAsrModel = ref(settings.value.cloudAsrModel || defaultCloudAsrModel(false));
const localCloudAsrDiarize = ref(settings.value.cloudAsrDiarize ?? true);

// ---------------------------------------------------------------- Nemotron
/** The build a run would use (the chosen one, else the recommended one). */
const nemotronActiveVariant = computed(() =>
    effectiveNemotronVariant(nemotron.status.value, localNemotronVariant.value),
);
const nemotronActiveInfo = computed(() =>
    nemotron.status.value?.variants.find((entry) => entry.variant === nemotronActiveVariant.value),
);
const nemotronReady = computed(() => nemotron.isInstalled(nemotronActiveVariant.value));
const nemotronDownloadBytes = computed(
    () =>
        (nemotronActiveInfo.value?.installed ? 0 : (nemotronActiveInfo.value?.downloadBytes ?? 0)) +
        (nemotron.status.value?.runtimeDownloadBytes ?? 0),
);
const nemotronChip = computed<Chip>(() => {
    if (nemotron.installing.value) {
        const percent = nemotron.percent.value;
        return {
            label: percent == null ? 'Downloading' : `Downloading ${percent.toFixed(0)}%`,
            tone: 'chip-accent',
            busy: true,
        };
    }
    if (nemotron.checking.value && !nemotron.status.value)
        return { label: 'Checking', tone: 'chip', busy: true };
    if (nemotronReady.value) return { label: 'Ready', tone: 'chip-success' };
    if (nemotron.status.value?.recommendation.rating === 'unsupported')
        return { label: 'Not on this machine', tone: 'chip-warning' };
    return { label: 'Not downloaded', tone: 'chip-warning' };
});
const nemotronVariantOptions = computed(() =>
    (nemotron.status.value?.variants ?? []).map((entry) => ({
        value: entry.variant,
        label: `${entry.label} · ${formatBytes(entry.downloadBytes)}, needs ~${Math.ceil(entry.requiredMemoryGb)} GB${entry.installed ? ' \u2713' : ''}`,
    })),
);
async function downloadNemotron() {
    const variant = nemotronActiveVariant.value;
    if (!variant) return;
    const finished = await nemotron.install(variant);
    if (finished) showToast('Nemotron is ready to use.', 'success');
    else if (nemotron.installError.value)
        showToast(`Nemotron download stopped: ${nemotron.installError.value}`, 'error');
}
async function removeNemotron(variant: NemotronVariant) {
    const confirmed = await ask(`Delete the downloaded Nemotron ${variant} model?`, {
        title: 'Delete model',
        kind: 'warning',
    });
    if (!confirmed) return;
    try {
        await nemotron.remove(variant);
    } catch (error) {
        showToast(`Could not delete the model: ${errorMessage(error)}`, 'error');
    }
}

// ---------------------------------------------------------------- Cloud ASR
const cloudModels = ref<TranscriptionModel[]>([]);
const loadingCloudModels = ref(false);
const cloudModelsError = ref('');
async function loadCloudModels() {
    loadingCloudModels.value = true;
    cloudModelsError.value = '';
    try {
        cloudModels.value =
            (await commands.listTranscriptionModels(
                openRouterBaseUrl(localOpenRouterEuOnly.value),
            )) ?? [];
    } catch (error) {
        cloudModels.value = [];
        cloudModelsError.value = errorMessage(error);
    } finally {
        loadingCloudModels.value = false;
    }
}
watch(localOpenRouterEuOnly, (euOnly, previous) => {
    if (localCloudAsrModel.value === defaultCloudAsrModel(previous)) {
        localCloudAsrModel.value = defaultCloudAsrModel(euOnly);
    }
    void loadCloudModels();
});
const cloudModelOptions = computed(() => {
    const entries = cloudModels.value.map((model) => ({
        value: model.id,
        label:
            model.usdPerMinute != null
                ? `${model.name} · $${(model.usdPerMinute * 60).toFixed(2)}/h`
                : model.name,
    }));
    if (!cloudModels.value.some((model) => model.id === localCloudAsrModel.value)) {
        entries.unshift({ value: localCloudAsrModel.value, label: localCloudAsrModel.value });
    }
    return entries;
});
const cloudModelUnavailable = computed(
    () =>
        cloudModels.value.length > 0 &&
        !cloudModels.value.some((model) => model.id === localCloudAsrModel.value),
);
const cloudChip = computed<Chip>(() =>
    openRouterKeyStored.value || localOpenRouterKey.value.trim()
        ? {
              label: localOpenRouterEuOnly.value ? 'EU only' : 'Key saved',
              tone: 'chip-success',
          }
        : { label: 'Needs an OpenRouter key', tone: 'chip-warning' },
);
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
/**
 * The model list is the built-in fallback, not what the endpoint offers. The
 * list itself persists across visits, so a fallback from an earlier fetch is
 * flagged as soon as the page opens.
 */
const usingFallbackModels = ref(
    modelFetchState.value.supportsModelFetch === false &&
        modelFetchState.value.availableModels.length > 0,
);
const showApiKey = ref(false);
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
    void loadCloudModels();
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
        localLocalEngine.value !== (settings.value.localEngine ?? 'nemotron') ||
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
        localAppleSpeechDiarize.value !== (settings.value.appleSpeechDiarize ?? true) ||
        localOpenRouterKey.value.trim() !== '' ||
        localOpenRouterEuOnly.value !== (settings.value.openRouterEuOnly ?? false) ||
        localNemotronVariant.value !== (settings.value.nemotronVariant ?? '') ||
        localNemotronDiarize.value !== (settings.value.nemotronDiarize ?? true) ||
        localAsrLanguage.value !== (settings.value.asrLanguage ?? '') ||
        localCloudAsrModel.value !==
            (settings.value.cloudAsrModel || defaultCloudAsrModel(false)) ||
        localCloudAsrDiarize.value !== (settings.value.cloudAsrDiarize ?? true)
    );
});

const isGoogleApi = computed(() => {
    return localBaseUrl.value.includes('generativelanguage.googleapis.com');
});

const endpointInfo = computed(() => {
    if (providerUsesOpenRouter.value)
        return localOpenRouterEuOnly.value
            ? 'OpenRouter EU (eu.openrouter.ai): only models served in Europe'
            : 'OpenRouter (openrouter.ai)';
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
            providerKey.value.trim() || null,
        );
        if (!list.supported) {
            updateModelFetchState({ supportsModelFetch: false, availableModels: [] });
            availableModels.value = [];
            usingFallbackModels.value = false;
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
        usingFallbackModels.value = false;
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
        usingFallbackModels.value = true;
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

async function removeApiKey(openRouter = providerUsesOpenRouter.value) {
    const confirmed = await ask(
        `Remove the saved ${openRouter ? 'OpenRouter ' : ''}API key from the system keychain?`,
        {
            title: 'Remove API key',
            kind: 'warning',
        },
    );
    if (!confirmed) return;
    try {
        if (openRouter) await commands.clearOpenrouterApiKey();
        else await commands.clearApiKey();
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
    const newOpenRouterKey = localOpenRouterKey.value.trim();
    if (newOpenRouterKey) {
        try {
            await commands.setOpenrouterApiKey(newOpenRouterKey);
        } catch (error) {
            showToast(`Could not save the OpenRouter key: ${errorMessage(error)}`, 'error');
            return;
        }
        localOpenRouterKey.value = '';
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
        openRouterEuOnly: localOpenRouterEuOnly.value,
        nemotronVariant: localNemotronVariant.value,
        nemotronDiarize: localNemotronDiarize.value,
        asrLanguage: localAsrLanguage.value,
        cloudAsrModel: localCloudAsrModel.value.trim() || defaultCloudAsrModel(false),
        cloudAsrDiarize: localCloudAsrDiarize.value,
    });
    allowLeave = true;
    router.push('/');
}

/** Back to the workspace; with unsaved edits the leave guard asks first. */
function cancel() {
    router.push('/');
}

// ---------------------------------------------------------------- Leaving
// Any navigation away (Cancel/Discard, the rail) asks before dropping edits.
const leaveDialogOpen = ref(false);
let allowLeave = false;
let resolveLeave: ((leave: boolean) => void) | null = null;
// The rail switches the workspace step before it navigates, so a navigation
// the user then cancels would leave Home on a step they never opened. Put the
// step back to the one Settings was opened from.
const { state: workspaceNav } = useWorkspaceNav();
const stepOnEntry = workspaceNav.activeStep;

const removeLeaveGuard = router.beforeEach(() => {
    if (allowLeave || !hasChanges.value) return true;
    resolveLeave?.(false);
    leaveDialogOpen.value = true;
    return new Promise<boolean>((resolve) => {
        resolveLeave = resolve;
    });
});
onUnmounted(removeLeaveGuard);

function answerLeave(leave: boolean) {
    leaveDialogOpen.value = false;
    if (leave) allowLeave = true;
    else workspaceNav.activeStep = stepOnEntry;
    const resolve = resolveLeave;
    resolveLeave = null;
    resolve?.(leave);
}

// ---------------------------------------------------------------- Sections
const SECTIONS = [
    { id: 'transcription', label: 'Transcription defaults', icon: AudioLines },
    { id: 'ai-provider', label: 'AI provider', icon: Sparkles },
    { id: 'engines', label: 'Speech engines', icon: Cpu },
    { id: 'long-audio', label: 'Long audio & validation', icon: Timer },
    { id: 'export', label: 'Export & clips', icon: Film },
    { id: 'updates', label: 'Updates & logs', icon: LifeBuoy },
    { id: 'about', label: 'About & licences', icon: Scale },
] as const;
type SectionId = (typeof SECTIONS)[number]['id'];

const activeSection = ref<SectionId>('transcription');
let sectionObserver: IntersectionObserver | null = null;
const visibleSections = new Set<string>();
// Ignore the scroll-spy while a nav click scrolls smoothly to its target.
let spyPausedUntil = 0;

function scrollToSection(id: SectionId) {
    activeSection.value = id;
    spyPausedUntil = Date.now() + 900;
    document
        .getElementById(`settings-${id}`)
        ?.scrollIntoView({ behavior: 'smooth', block: 'start' });
}

onMounted(() => {
    // jsdom (specs) has no IntersectionObserver.
    if (typeof IntersectionObserver === 'undefined') return;
    sectionObserver = new IntersectionObserver(
        (entries) => {
            for (const entry of entries) {
                const id = (entry.target as HTMLElement).dataset.section ?? '';
                if (entry.isIntersecting) visibleSections.add(id);
                else visibleSections.delete(id);
            }
            if (Date.now() < spyPausedUntil) return;
            const first = SECTIONS.find((section) => visibleSections.has(section.id));
            if (first) activeSection.value = first.id;
        },
        { root: document.getElementById('app-main'), rootMargin: '-15% 0px -55% 0px' },
    );
    for (const section of SECTIONS) {
        const element = document.getElementById(`settings-${section.id}`);
        if (element) sectionObserver.observe(element);
    }
});
onUnmounted(() => sectionObserver?.disconnect());

// ---------------------------------------------------------------- Status
type Chip = { label: string; tone: string; busy?: boolean };

const pipelineUsesLlm = computed(() => localTranscriptionBackend.value !== 'local');
const pipelineUsesEngine = computed(() => localTranscriptionBackend.value !== 'llm');

const parakeetChip = computed<Chip>(() =>
    localParakeetModelPath.value.trim() || localSortformerModelPath.value.trim()
        ? { label: 'Custom model', tone: 'chip' }
        : { label: 'Downloads on first use', tone: 'chip' },
);
const crisperChip = computed<Chip>(() => {
    if (isInstallingCrisper.value) return { label: 'Installing', tone: 'chip-accent', busy: true };
    if (isCheckingCrisper.value) return { label: 'Checking', tone: 'chip', busy: true };
    if (crisperStatus.value?.ready) return { label: 'Ready', tone: 'chip-success' };
    return { label: 'Not installed', tone: 'chip-warning' };
});
const appleSpeechChip = computed<Chip>(() => {
    if (isInstallingAppleSpeech.value)
        return { label: 'Downloading', tone: 'chip-accent', busy: true };
    if (isCheckingAppleSpeech.value) return { label: 'Checking', tone: 'chip', busy: true };
    if (appleSpeechStatus.value?.available)
        return appleSpeechLocaleInstalled.value
            ? { label: 'Ready', tone: 'chip-success' }
            : { label: 'Not downloaded', tone: 'chip-warning' };
    return { label: 'Unavailable', tone: 'chip-warning' };
});

/**
 * Engine cards start open only for the engine the default pipeline runs.
 * Picking another default opens its card too, but never closes one the user
 * opened by hand.
 */
function engineOpen(engine: LocalEngine) {
    return pipelineUsesEngine.value && localLocalEngine.value === engine;
}
const openEngines = reactive<Record<LocalEngine, boolean>>({
    nemotron: engineOpen('nemotron'),
    cloud: engineOpen('cloud'),
    parakeet: engineOpen('parakeet'),
    crisper: engineOpen('crisper'),
    'apple-speech': engineOpen('apple-speech'),
});
watch([localLocalEngine, pipelineUsesEngine], ([engine, usesEngine]) => {
    if (usesEngine) openEngines[engine] = true;
});
</script>

<template>
    <div class="@container mx-auto w-full max-w-[1400px] px-8 pt-8 pb-10">
        <PageHeader
            title="Settings"
            description="Defaults for new projects, AI provider and speech engines."
        />

        <div class="@3xl:grid @3xl:grid-cols-[14rem_minmax(0,1fr)] @3xl:gap-8">
            <!-- Section nav -->
            <nav aria-label="Settings sections" class="hidden @3xl:block">
                <ol class="sticky top-8 space-y-0.5">
                    <li v-for="section in SECTIONS" :key="section.id">
                        <a
                            :href="`#settings-${section.id}`"
                            :aria-current="activeSection === section.id ? 'location' : undefined"
                            class="relative flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-[13px] transition-colors duration-150"
                            :class="
                                activeSection === section.id
                                    ? 'bg-surface-hover text-fg'
                                    : 'text-fg-subtle hover:bg-surface-2 hover:text-fg'
                            "
                            @click.prevent="scrollToSection(section.id)"
                        >
                            <span
                                v-if="activeSection === section.id"
                                class="absolute top-1.5 bottom-1.5 left-0 w-[2px] rounded-full bg-accent"
                                aria-hidden="true"
                            />
                            <component
                                :is="section.icon"
                                class="h-3.5 w-3.5 shrink-0"
                                aria-hidden="true"
                            />
                            <span class="truncate">{{ section.label }}</span>
                        </a>
                    </li>
                </ol>
            </nav>

            <div class="min-w-0 max-w-[880px] space-y-10">
                <!-- Transcription defaults -->
                <section
                    id="settings-transcription"
                    data-section="transcription"
                    class="scroll-mt-8"
                    aria-labelledby="settings-transcription-title"
                >
                    <header class="mb-3 flex items-start justify-between gap-3">
                        <div>
                            <h2
                                id="settings-transcription-title"
                                class="text-[15px] font-semibold text-fg"
                            >
                                Transcription defaults
                            </h2>
                            <p class="hint mt-0.5">
                                New projects start with these. Each project can override them on its
                                Source page.
                            </p>
                        </div>
                        <button
                            type="button"
                            class="btn-secondary btn-sm shrink-0"
                            data-testid="run-setup-wizard"
                            @click="setupWizard.show()"
                        >
                            <Wand2 aria-hidden="true" />
                            Run setup wizard
                        </button>
                    </header>
                    <div class="panel divide-y divide-line">
                        <div class="p-4">
                            <p id="settings-pipeline-label" class="field-label">Default pipeline</p>
                            <OptionCardGroup
                                v-model="localTranscriptionBackend"
                                class="grid-cols-1 @xl:grid-cols-2"
                                aria-labelledby="settings-pipeline-label"
                                :options="PIPELINE_OPTIONS"
                                data-testid="default-pipeline"
                            />
                        </div>
                        <div class="p-4">
                            <p id="settings-engine-label" class="field-label">
                                Default speech engine
                            </p>
                            <OptionCardGroup
                                v-model="localLocalEngine"
                                class="grid-cols-1 @xl:grid-cols-2 @3xl:grid-cols-3"
                                aria-labelledby="settings-engine-label"
                                :options="engineOptions"
                                data-testid="default-engine"
                            />
                            <p class="hint mt-2">
                                Used by every pipeline except LLM Only, including both hybrids,
                                which layer the AI pass on top of whichever engine you pick.
                            </p>
                        </div>
                    </div>
                </section>

                <!-- AI provider -->
                <section
                    id="settings-ai-provider"
                    data-section="ai-provider"
                    class="scroll-mt-8"
                    aria-labelledby="settings-ai-provider-title"
                >
                    <header class="mb-3">
                        <h2
                            id="settings-ai-provider-title"
                            class="text-[15px] font-semibold text-fg"
                        >
                            AI provider
                        </h2>
                        <p class="hint mt-0.5">
                            The remote model for LLM transcription, hybrid cleanup, translation and
                            clip suggestions.
                        </p>
                    </header>
                    <p
                        v-if="!pipelineUsesLlm"
                        class="mb-3 flex items-center gap-2 rounded-md border border-line bg-surface-2 px-3 py-2 text-[12px] text-fg-muted"
                        data-testid="ai-provider-unused"
                    >
                        <Info class="h-3.5 w-3.5 shrink-0 text-info" aria-hidden="true" />
                        Only used by LLM and hybrid pipelines. Your default pipeline is Speech
                        Engine Only.
                    </p>
                    <div class="panel divide-y divide-line">
                        <div class="p-4">
                            <p id="settings-provider-label" class="field-label">Provider</p>
                            <OptionCardGroup
                                v-model="localProvider"
                                class="grid-cols-1 @xl:grid-cols-3"
                                aria-labelledby="settings-provider-label"
                                :options="PROVIDER_OPTIONS"
                                dense
                                data-testid="llm-provider"
                            />
                        </div>

                        <div v-if="providerUsesOpenRouter" class="p-4">
                            <SettingsSwitchRow
                                v-model="localOpenRouterEuOnly"
                                test-id="openrouter-eu-only"
                                title="Keep requests in the EU (eu.openrouter.ai)"
                                description="Requests are decrypted and processed in Europe only and fail rather than leave it. Applies to the AI model and cloud speech-to-text. Needs OpenRouter's EU in-region routing on your account; fewer models are available."
                            />
                        </div>

                        <div v-if="localProvider === 'custom'" class="p-4">
                            <label for="settings-base-url" class="field-label">Base URL</label>
                            <input
                                id="settings-base-url"
                                v-model="localBaseUrl"
                                type="text"
                                spellcheck="false"
                                class="input font-mono text-[12px]"
                                placeholder="https://generativelanguage.googleapis.com"
                                aria-describedby="settings-base-url-hint"
                            />
                            <p id="settings-base-url-hint" class="hint mt-1.5">
                                The API endpoint. Trailing slashes are removed and https:// is added
                                when missing.
                            </p>
                        </div>

                        <div class="p-4">
                            <div class="mb-1.5 flex items-center justify-between gap-3">
                                <label for="settings-api-key" class="field-label !mb-0">{{
                                    providerUsesOpenRouter ? 'OpenRouter API key' : 'API key'
                                }}</label>
                                <span
                                    v-if="providerKeyStored"
                                    class="chip-success"
                                    data-testid="api-key-stored"
                                >
                                    <KeyRound aria-hidden="true" />
                                    Saved in keychain
                                </span>
                                <span v-else class="chip-warning">No key saved</span>
                            </div>
                            <div class="flex gap-2">
                                <div class="relative min-w-0 flex-1">
                                    <input
                                        id="settings-api-key"
                                        v-model="providerKey"
                                        :type="showApiKey ? 'text' : 'password'"
                                        autocomplete="off"
                                        spellcheck="false"
                                        data-testid="api-key-input"
                                        class="input pr-9"
                                        aria-describedby="settings-api-key-hint"
                                        :placeholder="
                                            providerKeyStored
                                                ? 'Saved in the system keychain — enter a new key to replace it'
                                                : 'Enter your API key'
                                        "
                                    />
                                    <button
                                        type="button"
                                        class="btn-ghost btn-sm btn-icon absolute top-1/2 right-1 -translate-y-1/2"
                                        data-testid="api-key-visibility"
                                        :aria-label="showApiKey ? 'Hide API key' : 'Show API key'"
                                        :aria-pressed="showApiKey ? 'true' : 'false'"
                                        @click="showApiKey = !showApiKey"
                                    >
                                        <EyeOff v-if="showApiKey" aria-hidden="true" />
                                        <Eye v-else aria-hidden="true" />
                                    </button>
                                </div>
                                <button
                                    v-if="providerKeyStored"
                                    type="button"
                                    data-testid="api-key-remove"
                                    class="btn-danger"
                                    @click="removeApiKey()"
                                >
                                    Remove
                                </button>
                            </div>
                            <p id="settings-api-key-hint" class="hint mt-1.5">
                                Stored in your system keychain (macOS Keychain, Windows Credential
                                Manager or the Secret Service on Linux), never in the app's
                                settings. A new key is saved when you press Save.
                            </p>
                        </div>

                        <div class="p-4">
                            <label for="settings-model" class="field-label">Model</label>
                            <div class="flex gap-2">
                                <div class="relative min-w-0 flex-1">
                                    <select
                                        v-if="!showManualInput"
                                        id="settings-model"
                                        v-model="localModel"
                                        class="input"
                                        :aria-invalid="
                                            modelNotInList && !usingFallbackModels
                                                ? 'true'
                                                : undefined
                                        "
                                        aria-describedby="settings-model-hint"
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
                                        id="settings-model"
                                        v-model="localModel"
                                        type="text"
                                        spellcheck="false"
                                        class="input font-mono text-[12px]"
                                        placeholder="Enter model name manually"
                                        aria-describedby="settings-model-hint"
                                    />
                                </div>
                                <button
                                    type="button"
                                    class="btn-secondary"
                                    :disabled="isFetchingModels || !canUseApiKey"
                                    :title="!canUseApiKey ? 'Enter an API key first' : undefined"
                                    @click="fetchModels()"
                                >
                                    <RefreshCw
                                        :class="isFetchingModels ? 'animate-spin' : ''"
                                        aria-hidden="true"
                                    />
                                    {{ isFetchingModels ? 'Fetching…' : 'Refresh models' }}
                                </button>
                            </div>
                            <div class="mt-1.5 flex items-start justify-between gap-3">
                                <p
                                    v-if="fetchError"
                                    id="settings-model-hint"
                                    class="text-[12px] leading-normal text-danger"
                                    role="alert"
                                >
                                    {{ fetchError }}
                                </p>
                                <p v-else id="settings-model-hint" class="hint">
                                    {{ endpointInfo }}
                                    <template v-if="!canUseApiKey">
                                        · Enter an API key to load the model list.</template
                                    >
                                </p>
                                <button
                                    type="button"
                                    class="btn-ghost btn-sm -mr-1 shrink-0"
                                    data-testid="model-manual-toggle"
                                    @click="showManualInput = !showManualInput"
                                >
                                    <List v-if="showManualInput" aria-hidden="true" />
                                    <PenLine v-else aria-hidden="true" />
                                    {{ showManualInput ? 'Choose from list' : 'Enter manually' }}
                                </button>
                            </div>
                            <p
                                v-if="usingFallbackModels && !showManualInput"
                                class="mt-2 flex items-start gap-2 text-[12px] leading-normal text-warning"
                                data-testid="model-fallback-warning"
                            >
                                <TriangleAlert
                                    class="mt-0.5 h-3.5 w-3.5 shrink-0"
                                    aria-hidden="true"
                                />
                                Couldn't load this endpoint's models, so the list is a built-in
                                fallback and may be out of date. Enter the model manually if yours
                                is missing.
                            </p>
                            <p
                                v-else-if="modelNotInList && !showManualInput"
                                class="mt-2 flex items-start gap-2 text-[12px] leading-normal text-warning"
                                data-testid="model-not-in-list"
                            >
                                <TriangleAlert
                                    class="mt-0.5 h-3.5 w-3.5 shrink-0"
                                    aria-hidden="true"
                                />
                                “{{ localModel }}” isn't in this endpoint's model list. It may be
                                retired or misspelled.
                            </p>
                        </div>
                    </div>
                </section>

                <!-- Local engines -->
                <section
                    id="settings-engines"
                    data-section="engines"
                    class="scroll-mt-8"
                    aria-labelledby="settings-engines-title"
                >
                    <header class="mb-3">
                        <h2 id="settings-engines-title" class="text-[15px] font-semibold text-fg">
                            Speech engines
                        </h2>
                        <p class="hint mt-0.5">
                            Speech recognition on this machine or in the cloud. The default engine
                            is expanded; the others keep their settings for projects that pick them.
                        </p>
                    </header>
                    <p
                        v-if="!pipelineUsesEngine"
                        class="mb-3 flex items-center gap-2 rounded-md border border-line bg-surface-2 px-3 py-2 text-[12px] text-fg-muted"
                        data-testid="engines-unused"
                    >
                        <Info class="h-3.5 w-3.5 shrink-0 text-info" aria-hidden="true" />
                        Only used by Speech Engine Only and hybrid pipelines. Your default pipeline
                        is LLM Only.
                    </p>
                    <div class="space-y-3">
                        <!-- Nemotron -->
                        <DisclosureCard
                            title="Nemotron 3.5 Transcribe"
                            description="NVIDIA's 30B speech-LLM on this machine, via NeMo-Speech.cpp."
                            v-model:open="openEngines.nemotron"
                            data-testid="engine-card-nemotron"
                        >
                            <template #actions>
                                <span v-if="localLocalEngine === 'nemotron'" class="chip-accent"
                                    >Default</span
                                >
                                <span :class="nemotronChip.tone" data-testid="nemotron-chip">
                                    <Loader2
                                        v-if="nemotronChip.busy"
                                        class="animate-spin"
                                        aria-hidden="true"
                                    />
                                    {{ nemotronChip.label }}
                                </span>
                            </template>
                            <div class="space-y-4">
                                <div class="well px-3 py-2.5">
                                    <div class="flex items-start justify-between gap-3">
                                        <div class="min-w-0" aria-live="polite">
                                            <p class="eyebrow">This machine</p>
                                            <p
                                                v-if="nemotron.installing.value"
                                                class="mt-1 truncate text-[13px] text-fg-muted"
                                            >
                                                {{
                                                    nemotron.progress.value?.message ??
                                                    'Downloading…'
                                                }}
                                            </p>
                                            <p
                                                v-else-if="nemotron.status.value"
                                                class="mt-1 text-[13px]"
                                                :class="
                                                    nemotron.status.value.recommendation.rating ===
                                                    'unsupported'
                                                        ? 'text-warning'
                                                        : 'text-fg'
                                                "
                                                data-testid="nemotron-fit"
                                            >
                                                {{ nemotron.status.value.recommendation.message }}
                                            </p>
                                            <p v-else class="mt-1 text-[13px] text-fg-muted">
                                                Checking…
                                            </p>
                                        </div>
                                        <div class="flex shrink-0 gap-2">
                                            <button
                                                v-if="nemotron.installing.value"
                                                type="button"
                                                class="btn-secondary btn-sm"
                                                @click="nemotron.cancel"
                                            >
                                                Pause
                                            </button>
                                            <button
                                                v-else-if="
                                                    !nemotronReady &&
                                                    nemotronActiveVariant &&
                                                    nemotron.status.value?.platformSupported
                                                "
                                                type="button"
                                                class="btn-primary btn-sm"
                                                data-testid="nemotron-download"
                                                @click="downloadNemotron"
                                            >
                                                <Download aria-hidden="true" />
                                                Download {{ formatBytes(nemotronDownloadBytes) }}
                                            </button>
                                            <button
                                                v-else-if="nemotronReady && nemotronActiveVariant"
                                                type="button"
                                                class="btn-ghost btn-sm"
                                                @click="removeNemotron(nemotronActiveVariant)"
                                            >
                                                Delete
                                            </button>
                                        </div>
                                    </div>
                                    <div
                                        v-if="nemotron.installing.value"
                                        class="mt-2.5 h-1.5 overflow-hidden rounded-full bg-accent/20"
                                        role="progressbar"
                                        aria-label="Downloading Nemotron"
                                        :aria-valuenow="Math.round(nemotron.percent.value ?? 0)"
                                        aria-valuemin="0"
                                        aria-valuemax="100"
                                    >
                                        <div
                                            class="h-full bg-accent transition-[width] duration-300"
                                            :style="{ width: `${nemotron.percent.value ?? 0}%` }"
                                        />
                                    </div>
                                    <p class="hint mt-2">
                                        Nothing leaves this machine. Punctuated transcripts in ~40
                                        languages with word timings and speakers; the project
                                        glossary steers how names are spelled. About 2x real time on
                                        an M3 Pro, so slower than the other on-device engines.
                                        Downloads resume if interrupted.
                                    </p>
                                </div>

                                <div class="grid grid-cols-1 gap-4 @xl:grid-cols-2">
                                    <div>
                                        <label for="settings-nemotron-variant" class="field-label"
                                            >Model build</label
                                        >
                                        <select
                                            id="settings-nemotron-variant"
                                            v-model="localNemotronVariant"
                                            class="input"
                                        >
                                            <option value="">
                                                Recommended for this machine{{
                                                    nemotron.status.value?.recommendation.variant
                                                        ? ` (${nemotron.status.value.recommendation.variant})`
                                                        : ''
                                                }}
                                            </option>
                                            <option
                                                v-for="option in nemotronVariantOptions"
                                                :key="option.value"
                                                :value="option.value"
                                            >
                                                {{ option.label }}
                                            </option>
                                        </select>
                                        <p class="hint mt-1.5">
                                            All builds from Q2_0_L up are within 0.2 WER points of
                                            the original; larger ones are slower.
                                        </p>
                                    </div>
                                    <div>
                                        <label for="settings-nemotron-language" class="field-label"
                                            >Spoken language</label
                                        >
                                        <select
                                            id="settings-nemotron-language"
                                            v-model="localAsrLanguage"
                                            class="input"
                                        >
                                            <option
                                                v-for="entry in ASR_LANGUAGES"
                                                :key="entry.value"
                                                :value="entry.value"
                                            >
                                                {{ entry.label }}
                                            </option>
                                        </select>
                                        <p class="hint mt-1.5">Shared with cloud speech-to-text.</p>
                                    </div>
                                </div>

                                <SettingsSwitchRow
                                    v-model="localNemotronDiarize"
                                    test-id="nemotron-diarize"
                                    title="Identify speakers"
                                    description="Uses the model's own speaker branch (up to 8 speakers); Sortformer fills in for long, chunked recordings."
                                />
                            </div>
                        </DisclosureCard>

                        <!-- Parakeet -->
                        <DisclosureCard
                            title="Parakeet"
                            description="Parakeet TDT transcription with Sortformer speaker labels."
                            v-model:open="openEngines['parakeet']"
                            data-testid="engine-card-parakeet"
                        >
                            <template #actions>
                                <span v-if="localLocalEngine === 'parakeet'" class="chip-accent"
                                    >Default</span
                                >
                                <span :class="parakeetChip.tone">{{ parakeetChip.label }}</span>
                            </template>
                            <div class="space-y-4">
                                <div>
                                    <label for="settings-parakeet-path" class="field-label"
                                        >Parakeet TDT model directory</label
                                    >
                                    <div class="flex gap-2">
                                        <input
                                            id="settings-parakeet-path"
                                            v-model="localParakeetModelPath"
                                            type="text"
                                            spellcheck="false"
                                            class="input font-mono text-[12px]"
                                            placeholder="Leave blank to auto-download into app data"
                                            aria-describedby="settings-parakeet-path-hint"
                                        />
                                        <button
                                            type="button"
                                            class="btn-secondary"
                                            @click="selectParakeetModelDirectory"
                                        >
                                            <FolderOpen aria-hidden="true" />
                                            Browse
                                        </button>
                                    </div>
                                    <p id="settings-parakeet-path-hint" class="hint mt-1.5">
                                        Leave blank to let the app download an int8 TDT model into
                                        app data. Custom directories should contain encoder, decoder
                                        and <code>vocab.txt</code>.
                                    </p>
                                </div>
                                <div>
                                    <label for="settings-sortformer-path" class="field-label"
                                        >Sortformer model file</label
                                    >
                                    <div class="flex gap-2">
                                        <input
                                            id="settings-sortformer-path"
                                            v-model="localSortformerModelPath"
                                            type="text"
                                            spellcheck="false"
                                            class="input font-mono text-[12px]"
                                            placeholder="Leave blank to auto-download into app data"
                                            aria-describedby="settings-sortformer-path-hint"
                                        />
                                        <button
                                            type="button"
                                            class="btn-secondary"
                                            @click="selectSortformerModelFile"
                                        >
                                            <FolderOpen aria-hidden="true" />
                                            Browse
                                        </button>
                                    </div>
                                    <p id="settings-sortformer-path-hint" class="hint mt-1.5">
                                        Leave blank to download Sortformer v2 automatically, or
                                        provide your own <code>.onnx</code> file.
                                    </p>
                                </div>
                            </div>
                        </DisclosureCard>

                        <!-- CrisperWhisper -->
                        <DisclosureCard
                            title="CrisperWhisper"
                            description="Verbatim transcription with precise word timings. English and German."
                            v-model:open="openEngines['crisper']"
                            data-testid="engine-card-crisper"
                        >
                            <template #actions>
                                <span v-if="localLocalEngine === 'crisper'" class="chip-accent"
                                    >Default</span
                                >
                                <span :class="crisperChip.tone" data-testid="crisper-status-chip">
                                    <Loader2
                                        v-if="crisperChip.busy"
                                        class="animate-spin"
                                        aria-hidden="true"
                                    />
                                    {{ crisperChip.label }}
                                </span>
                            </template>
                            <div class="space-y-4">
                                <div
                                    class="flex gap-2.5 rounded-md border border-warning/25 bg-warning/[0.06] px-3 py-2.5"
                                    role="note"
                                >
                                    <TriangleAlert
                                        class="mt-0.5 h-3.5 w-3.5 shrink-0 text-warning"
                                        aria-hidden="true"
                                    />
                                    <div class="space-y-1">
                                        <p class="text-[12px] font-semibold text-warning">
                                            English and German only
                                        </p>
                                        <p class="hint">
                                            The CrisperWhisper 2.0 model card is published for
                                            English (<code>en</code>) and German (<code>de</code>).
                                            For other languages use Parakeet or the LLM Only
                                            pipeline.
                                        </p>
                                        <p class="hint">
                                            {{ crisperWeightsNotice }} The app installs the model on
                                            first use.
                                        </p>
                                    </div>
                                </div>

                                <!-- Environment status -->
                                <div class="well px-3 py-2.5" data-testid="crisper-runtime">
                                    <div class="flex items-start justify-between gap-3">
                                        <div class="min-w-0" aria-live="polite">
                                            <p class="eyebrow">Runtime</p>
                                            <p
                                                v-if="isInstallingCrisper || isCheckingCrisper"
                                                class="mt-1 truncate text-[13px] text-fg-muted"
                                            >
                                                {{
                                                    crisperProgress ||
                                                    (isInstallingCrisper
                                                        ? 'Installing...'
                                                        : 'Checking...')
                                                }}
                                            </p>
                                            <p
                                                v-else-if="crisperStatus?.ready"
                                                class="mt-1 text-[13px] text-success"
                                            >
                                                Ready &mdash; CrisperWhisper
                                                {{ crisperStatus.crisperwhisperVersion }}, Python
                                                {{ crisperStatus.python }},
                                                {{ crisperStatus.backends.join(' + ') }}
                                                <span v-if="crisperStatus.cuda">(CUDA)</span>
                                                <span v-else-if="crisperStatus.mps"
                                                    >(Apple GPU available)</span
                                                >
                                                <span v-else>(CPU)</span>
                                            </p>
                                            <p v-else class="mt-1 text-[13px] text-warning">
                                                {{ crisperStatus?.message ?? 'Not set up yet.' }}
                                            </p>
                                        </div>
                                        <div class="flex shrink-0 gap-2">
                                            <button
                                                type="button"
                                                class="btn-ghost btn-sm"
                                                :disabled="isCheckingCrisper || isInstallingCrisper"
                                                @click="refreshCrisperStatus"
                                            >
                                                <RefreshCw aria-hidden="true" />
                                                Re-check
                                            </button>
                                            <button
                                                v-if="!crisperStatus?.ready"
                                                type="button"
                                                class="btn-primary btn-sm"
                                                :disabled="isInstallingCrisper"
                                                @click="setUpCrisperEnvironment"
                                            >
                                                {{
                                                    isInstallingCrisper ? 'Installing...' : 'Set up'
                                                }}
                                            </button>
                                        </div>
                                    </div>
                                    <div
                                        v-if="isInstallingCrisper"
                                        class="mt-2.5 h-1 overflow-hidden rounded-full bg-accent/25"
                                        role="progressbar"
                                        aria-label="Installing CrisperWhisper"
                                        data-testid="crisper-progress"
                                    >
                                        <div class="shimmer h-full w-full bg-accent/60" />
                                    </div>
                                    <details class="group mt-2">
                                        <summary
                                            class="inline-flex cursor-pointer list-none items-center gap-1 text-[12px] text-fg-subtle hover:text-fg [&::-webkit-details-marker]:hidden"
                                        >
                                            <ChevronRight
                                                class="h-3 w-3 transition-transform duration-150 group-open:rotate-90"
                                                aria-hidden="true"
                                            />
                                            Details
                                        </summary>
                                        <p class="hint mt-1.5">
                                            CrisperWhisper ships only PyTorch and CTranslate2
                                            weights, so it runs through a private Python environment
                                            the app manages at
                                            <code class="break-all text-fg-muted">{{
                                                crisperStatus?.environmentDir ||
                                                'the app data directory'
                                            }}</code
                                            >. Setup needs Python
                                            {{ crisperStatus?.minimumPython || '3.10' }} or newer on
                                            your system, and uses the Python path and backend shown
                                            here even before you save.
                                        </p>
                                    </details>
                                </div>

                                <div class="grid grid-cols-1 gap-4 @xl:grid-cols-2">
                                    <div>
                                        <label for="settings-crisper-model" class="field-label"
                                            >Model size</label
                                        >
                                        <select
                                            id="settings-crisper-model"
                                            v-model="localCrisperModel"
                                            class="input"
                                            aria-describedby="settings-crisper-model-hint"
                                        >
                                            <option
                                                v-for="size in CRISPER_MODELS"
                                                :key="size"
                                                :value="size"
                                            >
                                                {{ size }}
                                            </option>
                                        </select>
                                        <p id="settings-crisper-model-hint" class="hint mt-1.5">
                                            <code>large</code> is the most accurate,
                                            <code>turbo</code> the fastest, <code>medium</code> the
                                            best tradeoff.
                                        </p>
                                    </div>
                                    <div>
                                        <label for="settings-crisper-language" class="field-label"
                                            >Language</label
                                        >
                                        <select
                                            id="settings-crisper-language"
                                            v-model="localCrisperLanguage"
                                            class="input"
                                        >
                                            <option
                                                v-for="language in CRISPER_LANGUAGES"
                                                :key="language.value"
                                                :value="language.value"
                                            >
                                                {{ language.label }}
                                            </option>
                                        </select>
                                        <p class="hint mt-1.5">
                                            Only English and German are supported.
                                        </p>
                                    </div>
                                </div>

                                <div>
                                    <p id="settings-crisper-mode-label" class="field-label">
                                        Transcription mode
                                    </p>
                                    <OptionCardGroup
                                        v-model="localCrisperMode"
                                        class="grid-cols-1 @xl:grid-cols-2"
                                        aria-labelledby="settings-crisper-mode-label"
                                        :options="CRISPER_MODE_OPTIONS"
                                    />
                                    <p class="hint mt-1.5">
                                        Filler removal is toggled per project with
                                        <strong class="font-medium text-fg-muted"
                                            >Remove filler words</strong
                                        >
                                        on the Source page. In verbatim mode it cuts the filler out
                                        of the exported video too.
                                    </p>
                                </div>

                                <div class="grid grid-cols-1 gap-2 @xl:grid-cols-2">
                                    <SettingsSwitchRow
                                        v-model="localCrisperRemoveVocalEvents"
                                        test-id="crisper-remove-vocal-events"
                                        title="Remove vocal events"
                                        description="Cuts [laughter], [breath], [cough], [sigh]…"
                                    />
                                    <SettingsSwitchRow
                                        v-model="localCrisperDiarize"
                                        test-id="crisper-diarize"
                                        title="Identify speakers"
                                        description="Adds Sortformer diarization; slower."
                                    />
                                </div>

                                <SectionCard
                                    title="Advanced runtime options"
                                    description="Backend, device, precision and a custom Python."
                                    collapsible
                                    :default-open="false"
                                    class="!bg-surface-2"
                                >
                                    <div class="space-y-4">
                                        <div class="grid grid-cols-1 gap-3 @xl:grid-cols-3">
                                            <div>
                                                <label
                                                    for="settings-crisper-backend"
                                                    class="field-label"
                                                    >Inference backend</label
                                                >
                                                <select
                                                    id="settings-crisper-backend"
                                                    v-model="localCrisperBackend"
                                                    class="input"
                                                >
                                                    <option value="auto">Auto</option>
                                                    <option value="transformers">
                                                        PyTorch (portable)
                                                    </option>
                                                    <option value="ct2">
                                                        CTranslate2 (Linux + NVIDIA)
                                                    </option>
                                                </select>
                                            </div>
                                            <div>
                                                <label
                                                    for="settings-crisper-device"
                                                    class="field-label"
                                                    >Device</label
                                                >
                                                <select
                                                    id="settings-crisper-device"
                                                    v-model="localCrisperDevice"
                                                    class="input"
                                                >
                                                    <option value="auto">Auto</option>
                                                    <option value="cpu">CPU</option>
                                                    <option value="cuda">CUDA</option>
                                                    <option value="mps">Apple GPU (MPS)</option>
                                                </select>
                                            </div>
                                            <div>
                                                <label
                                                    for="settings-crisper-precision"
                                                    class="field-label"
                                                    >Precision</label
                                                >
                                                <select
                                                    id="settings-crisper-precision"
                                                    v-model="localCrisperComputeType"
                                                    class="input"
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
                                        <details class="group">
                                            <summary
                                                class="inline-flex cursor-pointer list-none items-center gap-1 text-[12px] text-fg-subtle hover:text-fg [&::-webkit-details-marker]:hidden"
                                            >
                                                <ChevronRight
                                                    class="h-3 w-3 transition-transform duration-150 group-open:rotate-90"
                                                    aria-hidden="true"
                                                />
                                                Performance details
                                            </summary>
                                            <p class="hint mt-1.5">
                                                CTranslate2 is roughly 4&ndash;5&times; faster but
                                                its wheels are Linux x86_64 only; everywhere else
                                                the portable PyTorch backend is used. Auto precision
                                                picks float32 on CPU and float16 on a GPU. On Apple
                                                Silicon, Auto uses the GPU (MPS, float16): the app's
                                                own decoding loop runs about twice as fast there as
                                                on the CPU, with the same transcript.
                                                <strong class="font-medium text-fg-muted"
                                                    >int8_float16</strong
                                                >
                                                is another ~1.4&times; faster on GPU and CPU; on 10
                                                minutes of German the word error rate was unchanged,
                                                and up to 14 of 2,076 words differed, mostly fillers
                                                and punctuation.
                                            </p>
                                        </details>
                                        <div>
                                            <label for="settings-crisper-python" class="field-label"
                                                >Python interpreter</label
                                            >
                                            <div class="flex gap-2">
                                                <input
                                                    id="settings-crisper-python"
                                                    v-model="localCrisperPythonPath"
                                                    type="text"
                                                    spellcheck="false"
                                                    class="input font-mono text-[12px]"
                                                    placeholder="Leave blank to use the app-managed environment"
                                                    aria-describedby="settings-crisper-python-hint"
                                                />
                                                <button
                                                    type="button"
                                                    class="btn-secondary"
                                                    @click="selectCrisperPython"
                                                >
                                                    <FolderOpen aria-hidden="true" />
                                                    Browse
                                                </button>
                                            </div>
                                            <p
                                                id="settings-crisper-python-hint"
                                                class="hint mt-1.5"
                                            >
                                                Point this at your own environment if you already
                                                have <code>crisperwhisper</code> installed.
                                            </p>
                                        </div>
                                    </div>
                                </SectionCard>
                            </div>
                        </DisclosureCard>

                        <!-- Apple Speech -->
                        <DisclosureCard
                            v-if="appleSpeechSupported"
                            title="Apple Speech"
                            description="macOS's on-device speech model (macOS 26 or newer)."
                            v-model:open="openEngines['apple-speech']"
                            data-testid="engine-card-apple-speech"
                        >
                            <template #actions>
                                <span v-if="localLocalEngine === 'apple-speech'" class="chip-accent"
                                    >Default</span
                                >
                                <span :class="appleSpeechChip.tone">
                                    <Loader2
                                        v-if="appleSpeechChip.busy"
                                        class="animate-spin"
                                        aria-hidden="true"
                                    />
                                    {{ appleSpeechChip.label }}
                                </span>
                            </template>
                            <div class="space-y-4">
                                <div class="well px-3 py-2.5">
                                    <div class="flex items-start justify-between gap-3">
                                        <div class="min-w-0" aria-live="polite">
                                            <p class="eyebrow">Runtime</p>
                                            <p
                                                v-if="isInstallingAppleSpeech"
                                                class="mt-1 truncate text-[13px] text-fg-muted"
                                            >
                                                {{ appleSpeechProgress || 'Downloading...' }}
                                            </p>
                                            <p
                                                v-else-if="isCheckingAppleSpeech"
                                                class="mt-1 text-[13px] text-fg-muted"
                                            >
                                                Checking...
                                            </p>
                                            <p
                                                v-else-if="appleSpeechStatus?.available"
                                                class="mt-1 text-[13px]"
                                                :class="
                                                    appleSpeechLocaleInstalled
                                                        ? 'text-success'
                                                        : 'text-warning'
                                                "
                                            >
                                                <template v-if="appleSpeechLocaleInstalled">
                                                    Ready &mdash;
                                                    {{ effectiveAppleSpeechLocale }} is installed.
                                                </template>
                                                <template v-else>
                                                    The {{ effectiveAppleSpeechLocale }} model is
                                                    not downloaded yet; it downloads on first use,
                                                    or now.
                                                </template>
                                            </p>
                                            <p v-else class="mt-1 text-[13px] text-warning">
                                                {{ appleSpeechStatus?.message ?? 'Not available.' }}
                                            </p>
                                        </div>
                                        <div class="flex shrink-0 gap-2">
                                            <button
                                                type="button"
                                                class="btn-ghost btn-sm"
                                                :disabled="
                                                    isCheckingAppleSpeech || isInstallingAppleSpeech
                                                "
                                                @click="refreshAppleSpeechStatus"
                                            >
                                                <RefreshCw aria-hidden="true" />
                                                Re-check
                                            </button>
                                            <button
                                                v-if="
                                                    appleSpeechStatus?.available &&
                                                    !appleSpeechLocaleInstalled
                                                "
                                                type="button"
                                                class="btn-primary btn-sm"
                                                :disabled="isInstallingAppleSpeech"
                                                @click="downloadAppleSpeechLocale"
                                            >
                                                {{
                                                    isInstallingAppleSpeech
                                                        ? 'Downloading...'
                                                        : 'Download'
                                                }}
                                            </button>
                                        </div>
                                    </div>
                                    <div
                                        v-if="isInstallingAppleSpeech"
                                        class="mt-2.5 h-1 overflow-hidden rounded-full bg-accent/25"
                                        role="progressbar"
                                        aria-label="Downloading the Apple Speech language"
                                    >
                                        <div class="shimmer h-full w-full bg-accent/60" />
                                    </div>
                                    <p class="hint mt-2">
                                        Nothing leaves this Mac; macOS downloads one model per
                                        language and manages it. It is not verbatim (fillers like
                                        “um” are dropped), so use CrisperWhisper to cut those out.
                                    </p>
                                </div>

                                <div>
                                    <label for="settings-apple-locale" class="field-label"
                                        >Language</label
                                    >
                                    <select
                                        id="settings-apple-locale"
                                        v-model="localAppleSpeechLocale"
                                        class="input"
                                        aria-describedby="settings-apple-locale-hint"
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
                                    <p id="settings-apple-locale-hint" class="hint mt-1.5">
                                        &#10003; marks languages already downloaded.
                                    </p>
                                </div>

                                <SettingsSwitchRow
                                    v-model="localAppleSpeechDiarize"
                                    test-id="apple-speech-diarize"
                                    title="Identify speakers"
                                    description="Adds Sortformer diarization; Apple Speech does not tell speakers apart."
                                />
                            </div>
                        </DisclosureCard>

                        <!-- Cloud speech-to-text -->
                        <DisclosureCard
                            title="Cloud speech-to-text"
                            description="OpenRouter's /audio/transcriptions: Nemotron, Voxtral and more, one key."
                            v-model:open="openEngines.cloud"
                            data-testid="engine-card-cloud"
                        >
                            <template #actions>
                                <span v-if="localLocalEngine === 'cloud'" class="chip-accent"
                                    >Default</span
                                >
                                <span :class="cloudChip.tone">
                                    <Cloud aria-hidden="true" />
                                    {{ cloudChip.label }}
                                </span>
                            </template>
                            <div class="space-y-4">
                                <div v-if="!providerUsesOpenRouter">
                                    <div class="mb-1.5 flex items-center justify-between gap-3">
                                        <label for="settings-cloud-key" class="field-label !mb-0"
                                            >OpenRouter API key</label
                                        >
                                        <span v-if="openRouterKeyStored" class="chip-success">
                                            <KeyRound aria-hidden="true" />
                                            Saved in keychain
                                        </span>
                                    </div>
                                    <div class="flex gap-2">
                                        <input
                                            id="settings-cloud-key"
                                            v-model="localOpenRouterKey"
                                            type="password"
                                            autocomplete="off"
                                            spellcheck="false"
                                            class="input min-w-0 flex-1"
                                            data-testid="cloud-key-input"
                                            :placeholder="
                                                openRouterKeyStored
                                                    ? 'Saved — enter a new key to replace it'
                                                    : 'sk-or-…'
                                            "
                                        />
                                        <button
                                            v-if="openRouterKeyStored"
                                            type="button"
                                            class="btn-danger"
                                            @click="removeApiKey(true)"
                                        >
                                            Remove
                                        </button>
                                    </div>
                                    <p class="hint mt-1.5">
                                        Also used by the AI model when its provider is OpenRouter.
                                    </p>
                                </div>
                                <p v-else class="hint">
                                    Uses the OpenRouter key from the AI provider section.
                                </p>

                                <SettingsSwitchRow
                                    v-model="localOpenRouterEuOnly"
                                    test-id="cloud-eu-only"
                                    title="Keep requests in the EU (eu.openrouter.ai)"
                                    description="Processed in Europe only. Nemotron isn't served there; Mistral's Voxtral models are."
                                />

                                <div class="grid grid-cols-1 gap-4 @xl:grid-cols-2">
                                    <div>
                                        <label for="settings-cloud-model" class="field-label"
                                            >Model</label
                                        >
                                        <select
                                            id="settings-cloud-model"
                                            v-model="localCloudAsrModel"
                                            class="input"
                                            data-testid="cloud-model"
                                        >
                                            <option
                                                v-for="option in cloudModelOptions"
                                                :key="option.value"
                                                :value="option.value"
                                            >
                                                {{ option.label }}
                                            </option>
                                        </select>
                                        <p
                                            v-if="cloudModelUnavailable"
                                            class="mt-1.5 flex items-start gap-2 text-[12px] text-warning"
                                            data-testid="cloud-model-unavailable"
                                        >
                                            <TriangleAlert
                                                class="mt-0.5 h-3.5 w-3.5 shrink-0"
                                                aria-hidden="true"
                                            />
                                            Not served in this region.
                                        </p>
                                        <p
                                            v-else-if="cloudModelsError"
                                            class="mt-1.5 text-[12px] text-warning"
                                        >
                                            Couldn't load the model list: {{ cloudModelsError }}
                                        </p>
                                        <p v-else class="hint mt-1.5">
                                            <Loader2
                                                v-if="loadingCloudModels"
                                                class="mr-1 inline h-3 w-3 animate-spin"
                                                aria-hidden="true"
                                            />
                                            Prices are OpenRouter's, per hour of audio.
                                        </p>
                                    </div>
                                    <div>
                                        <label for="settings-cloud-language" class="field-label"
                                            >Spoken language</label
                                        >
                                        <select
                                            id="settings-cloud-language"
                                            v-model="localAsrLanguage"
                                            class="input"
                                        >
                                            <option
                                                v-for="entry in ASR_LANGUAGES"
                                                :key="entry.value"
                                                :value="entry.value"
                                            >
                                                {{ entry.label }}
                                            </option>
                                        </select>
                                        <p class="hint mt-1.5">Shared with Nemotron.</p>
                                    </div>
                                </div>

                                <SettingsSwitchRow
                                    v-model="localCloudAsrDiarize"
                                    test-id="cloud-diarize"
                                    title="Identify speakers"
                                    description="Runs Sortformer on this machine, so speakers stay consistent across the uploaded chunks."
                                />
                            </div>
                        </DisclosureCard>
                    </div>
                </section>

                <!-- Long audio & validation -->
                <section
                    id="settings-long-audio"
                    data-section="long-audio"
                    class="scroll-mt-8"
                    aria-labelledby="settings-long-audio-title"
                >
                    <header class="mb-3">
                        <h2
                            id="settings-long-audio-title"
                            class="text-[15px] font-semibold text-fg"
                        >
                            Long audio &amp; validation
                        </h2>
                        <p class="hint mt-0.5">How requests to the AI provider are shaped.</p>
                    </header>
                    <p
                        v-if="!pipelineUsesLlm"
                        class="mb-3 flex items-center gap-2 rounded-md border border-line bg-surface-2 px-3 py-2 text-[12px] text-fg-muted"
                    >
                        <Info class="h-3.5 w-3.5 shrink-0 text-info" aria-hidden="true" />
                        Only used by LLM and hybrid pipelines. Your default pipeline is Local Only.
                    </p>
                    <div class="panel divide-y divide-line">
                        <div class="p-4">
                            <label for="settings-chunk-minutes" class="field-label"
                                >Max analysis chunk length</label
                            >
                            <div class="relative w-40">
                                <input
                                    id="settings-chunk-minutes"
                                    v-model.number="localMaxAnalysisChunkMinutes"
                                    type="number"
                                    step="1"
                                    min="0"
                                    class="input no-spinner pr-11"
                                    placeholder="30"
                                    aria-describedby="settings-chunk-minutes-hint"
                                />
                                <span
                                    class="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-[12px] text-fg-subtle"
                                    aria-hidden="true"
                                    >min</span
                                >
                            </div>
                            <p id="settings-chunk-minutes-hint" class="hint mt-1.5">
                                Longer audio is split into chunks before LLM transcription. Set to 0
                                to disable chunking.
                            </p>
                            <details class="group mt-1.5">
                                <summary
                                    class="inline-flex cursor-pointer list-none items-center gap-1 text-[12px] text-fg-subtle hover:text-fg [&::-webkit-details-marker]:hidden"
                                >
                                    <ChevronRight
                                        class="h-3 w-3 transition-transform duration-150 group-open:rotate-90"
                                        aria-hidden="true"
                                    />
                                    Details
                                </summary>
                                <p class="hint mt-1.5">
                                    Chunking keeps each request under the provider's request timeout
                                    (avoids
                                    <span class="text-fg-muted">504 Gateway Timeout</span> on long
                                    videos). Splits prefer a ~1s silence near the boundary and fall
                                    back to Parakeet word boundaries. Only applies to LLM-based
                                    transcription.
                                </p>
                            </details>
                        </div>
                        <div class="p-4">
                            <SettingsSwitchRow
                                v-model="localEnforceJsonSchema"
                                test-id="enforce-json-schema"
                                title="Enforce structured JSON for transcript analysis"
                                description="Sends a strict JSON schema with AI analysis requests on OpenAI-compatible APIs. Improves reliability, but some providers may respond slower or behave differently."
                            />
                        </div>
                    </div>
                </section>

                <!-- Export & clips -->
                <section
                    id="settings-export"
                    data-section="export"
                    class="scroll-mt-8"
                    aria-labelledby="settings-export-title"
                >
                    <header class="mb-3">
                        <h2 id="settings-export-title" class="text-[15px] font-semibold text-fg">
                            Export &amp; clips
                        </h2>
                        <p class="hint mt-0.5">Video encoding and clip boundaries.</p>
                    </header>
                    <div class="panel divide-y divide-line">
                        <div class="p-4">
                            <p id="settings-export-quality-label" class="field-label">
                                Video export quality
                            </p>
                            <OptionCardGroup
                                v-model="localExportQuality"
                                class="grid-cols-1 @xl:grid-cols-3"
                                aria-labelledby="settings-export-quality-label"
                                :options="EXPORT_QUALITY_OPTIONS"
                            />
                            <p class="hint mt-1.5">
                                Applies to Export Video and clip exports. A hardware encoder is used
                                when this machine has one.
                            </p>
                        </div>
                        <div class="grid grid-cols-1 gap-4 p-4 @xl:grid-cols-2">
                            <div>
                                <label for="settings-pre-padding" class="field-label"
                                    >Pre-clip padding</label
                                >
                                <div class="relative w-40">
                                    <input
                                        id="settings-pre-padding"
                                        v-model.number="localPreClipPadding"
                                        type="number"
                                        step="0.1"
                                        min="0"
                                        class="input no-spinner pr-8"
                                        placeholder="0.0"
                                        aria-describedby="settings-pre-padding-hint"
                                    />
                                    <span
                                        class="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-[12px] text-fg-subtle"
                                        aria-hidden="true"
                                        >s</span
                                    >
                                </div>
                                <p id="settings-pre-padding-hint" class="hint mt-1.5">
                                    Seconds added before the start of each clip.
                                </p>
                            </div>
                            <div>
                                <label for="settings-post-padding" class="field-label"
                                    >Post-clip padding</label
                                >
                                <div class="relative w-40">
                                    <input
                                        id="settings-post-padding"
                                        v-model.number="localPostClipPadding"
                                        type="number"
                                        step="0.1"
                                        min="0"
                                        class="input no-spinner pr-8"
                                        placeholder="0.0"
                                        aria-describedby="settings-post-padding-hint"
                                    />
                                    <span
                                        class="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-[12px] text-fg-subtle"
                                        aria-hidden="true"
                                        >s</span
                                    >
                                </div>
                                <p id="settings-post-padding-hint" class="hint mt-1.5">
                                    Seconds added after the end of each clip.
                                </p>
                            </div>
                        </div>
                    </div>
                </section>

                <!-- Updates & troubleshooting -->
                <section
                    id="settings-updates"
                    data-section="updates"
                    class="scroll-mt-8"
                    aria-labelledby="settings-updates-title"
                >
                    <header class="mb-3">
                        <h2 id="settings-updates-title" class="text-[15px] font-semibold text-fg">
                            Updates &amp; logs
                        </h2>
                    </header>
                    <div class="panel divide-y divide-line">
                        <div class="flex items-center justify-between gap-4 p-4">
                            <div class="min-w-0">
                                <p class="text-[13px] font-medium text-fg">
                                    Version
                                    <span class="ml-1 font-mono text-fg-muted">{{
                                        appVersion
                                    }}</span>
                                </p>
                                <p
                                    v-if="updateStatus"
                                    class="mt-0.5 text-[12px]"
                                    :class="updateAvailable ? 'text-success' : 'text-fg-subtle'"
                                    aria-live="polite"
                                >
                                    {{ updateStatus }}
                                </p>
                                <p v-else class="hint mt-0.5">The app also checks on launch.</p>
                            </div>
                            <button
                                type="button"
                                class="btn-secondary shrink-0"
                                :disabled="isCheckingUpdate"
                                @click="checkForUpdates"
                            >
                                <RefreshCw
                                    :class="isCheckingUpdate ? 'animate-spin' : ''"
                                    aria-hidden="true"
                                />
                                {{ isCheckingUpdate ? 'Checking...' : 'Check for updates' }}
                            </button>
                        </div>
                        <div class="flex items-center justify-between gap-4 p-4">
                            <div class="min-w-0">
                                <p class="text-[13px] font-medium text-fg">Application logs</p>
                                <p class="hint mt-0.5">
                                    Export the logs as a zip file to attach to a bug report.
                                </p>
                            </div>
                            <button
                                type="button"
                                class="btn-secondary shrink-0"
                                @click="exportLogs"
                            >
                                <FileArchive aria-hidden="true" />
                                Export logs
                            </button>
                        </div>
                    </div>
                </section>

                <!-- About & licences: what the app downloads or bundles at runtime
                     (src/assets/runtime-licenses.json). -->
                <section
                    id="settings-about"
                    data-section="about"
                    class="scroll-mt-8"
                    aria-labelledby="settings-about-title"
                >
                    <header class="mb-3">
                        <h2 id="settings-about-title" class="text-[15px] font-semibold text-fg">
                            About &amp; licences
                        </h2>
                        <p class="hint mt-0.5">
                            Third-party models and tools the app downloads or bundles.
                        </p>
                    </header>
                    <ul data-testid="runtime-licenses" class="panel divide-y divide-line">
                        <li
                            v-for="download in runtimeLicenses.downloads"
                            :key="download.id"
                            class="px-4 py-3"
                        >
                            <div class="flex items-start justify-between gap-3">
                                <div class="min-w-0">
                                    <p class="text-[13px] font-medium text-fg">
                                        {{ download.name }}
                                    </p>
                                    <p class="hint">{{ download.usedBy }}</p>
                                </div>
                                <button
                                    type="button"
                                    class="btn-ghost btn-sm shrink-0"
                                    :title="download.licenseUrl"
                                    @click="openUrl(download.licenseUrl)"
                                >
                                    {{ download.license.replace('LicenseRef-', '') }}
                                    <ExternalLink aria-hidden="true" />
                                </button>
                            </div>
                            <details v-if="download.notice" class="group mt-1">
                                <summary
                                    class="inline-flex cursor-pointer list-none items-center gap-1 text-[12px] text-fg-subtle hover:text-fg [&::-webkit-details-marker]:hidden"
                                >
                                    <ChevronRight
                                        class="h-3 w-3 transition-transform duration-150 group-open:rotate-90"
                                        aria-hidden="true"
                                    />
                                    Licence notice
                                </summary>
                                <p class="mt-1 text-[12px] leading-normal text-fg-muted">
                                    {{ download.notice }}
                                </p>
                            </details>
                        </li>
                    </ul>
                </section>

                <!-- Action bar: a quiet row at the end of the page; with unsaved
                     changes it floats at the bottom of the viewport, over a
                     canvas strip so no content shows underneath it. -->
                <div
                    class="z-20"
                    :class="hasChanges ? 'sticky bottom-0 -mx-1 bg-canvas px-1 pt-2 pb-4' : ''"
                >
                    <div
                        class="flex items-center gap-3 rounded-lg border px-4 py-2.5"
                        :class="
                            hasChanges
                                ? 'animate-rise-in border-accent/40 bg-surface-2 shadow-[var(--shadow-pop)]'
                                : 'border-line bg-surface'
                        "
                        data-testid="settings-action-bar"
                    >
                        <span
                            v-if="hasChanges"
                            class="h-2 w-2 shrink-0 rounded-full bg-accent"
                            aria-hidden="true"
                        />
                        <p class="min-w-0 flex-1 text-[13px]" aria-live="polite">
                            <span v-if="hasChanges" class="font-medium text-fg"
                                >Unsaved changes</span
                            >
                            <span v-else class="text-fg-subtle">No unsaved changes</span>
                        </p>
                        <button
                            type="button"
                            class="btn-secondary"
                            data-testid="settings-cancel"
                            @click="cancel"
                        >
                            {{ hasChanges ? 'Discard' : 'Cancel' }}
                        </button>
                        <button
                            type="button"
                            class="btn-primary"
                            data-testid="settings-save"
                            :disabled="!hasChanges"
                            :title="!hasChanges ? 'Nothing to save yet' : undefined"
                            @click="saveSettings"
                        >
                            Save settings
                        </button>
                    </div>
                </div>
            </div>
        </div>

        <UiDialog
            :open="leaveDialogOpen"
            title="Discard unsaved changes?"
            description="Your edits to these settings haven't been saved and will be lost."
            size="sm"
            tone="danger"
            @close="answerLeave(false)"
        >
            <p class="hint">To keep them, choose Keep editing and press Save settings.</p>
            <template #footer>
                <button
                    type="button"
                    class="btn-secondary"
                    data-testid="leave-keep-editing"
                    data-autofocus
                    @click="answerLeave(false)"
                >
                    Keep editing
                </button>
                <button
                    type="button"
                    class="btn-danger"
                    data-testid="leave-discard"
                    @click="answerLeave(true)"
                >
                    Discard changes
                </button>
            </template>
        </UiDialog>

        <Toast
            :show="toastVisible"
            :message="toastMessage"
            :tone="toastTone"
            :progress="toastProgress"
            :action-label="toastActionLabel"
            @dismiss="hideToast"
            @action="handleToastAction"
        />
    </div>
</template>

<style scoped>
/* Unit suffixes sit where the spin buttons would. */
.no-spinner {
    appearance: textfield;
    -moz-appearance: textfield;
}
.no-spinner::-webkit-inner-spin-button,
.no-spinner::-webkit-outer-spin-button {
    -webkit-appearance: none;
    margin: 0;
}
</style>
