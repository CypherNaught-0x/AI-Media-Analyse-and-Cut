import { commands, type ExportQuality, type NemotronVariant } from '../bindings';
import type { CrisperLanguage, CrisperMode, LocalEngine, TranscriptionBackend } from '../types';
import { migrateTranscriptionBackend } from '../types';
import { NEMOTRON_CLOUD_MODEL, isOpenRouterUrl } from '../utils/providers';
import { computed, ref, watch } from 'vue';

export interface LLMSettings {
    baseUrl: string;
    // The API key is not a setting: it lives in the OS credential store (see
    // apiKeyStored / migrateLegacyApiKey below).
    model: string;
    enforceJsonSchema: boolean;
    maxAnalysisChunkMinutes: number;
    glossary: string;
    preClipPadding: number;
    postClipPadding: number;
    /** Speed/quality trade-off for video cuts and clip exports. */
    exportQuality: ExportQuality;
    /** Pipeline: `llm`, `local`, `hybrid`, or `hybrid-merge`. */
    transcriptionBackend: TranscriptionBackend;
    /** Which local model the non-`llm` pipelines run. */
    localEngine: LocalEngine;
    parakeetModelPath: string;
    sortformerModelPath: string;
    /** Size shorthand (`large`/`medium`/`turbo`/`small`), HF id, or local path. */
    crisperModel: string;
    /** CrisperWhisper 2.0 is published for English and German only. */
    crisperLanguage: CrisperLanguage;
    crisperMode: CrisperMode;
    /** `auto` | `ct2` | `transformers` — `ct2` is Linux x86_64 + NVIDIA only. */
    crisperBackend: string;
    /** `auto` | `cpu` | `cuda` */
    crisperDevice: string;
    /** `auto` | `float32` | `float16` | `int8_float16` */
    crisperComputeType: string;
    /** Strip `[laughter]`, `[breath]`, `[cough]`, ... from the transcript. */
    crisperRemoveVocalEvents: boolean;
    /** Attribute speakers with Sortformer; CrisperWhisper does not diarize. */
    crisperDiarize: boolean;
    /** Interpreter override; empty uses the app-managed environment. */
    crisperPythonPath: string;
    /** BCP 47 locale for Apple Speech; empty follows the system language. */
    appleSpeechLocale: string;
    /** Attribute speakers with Sortformer; Apple Speech does not diarize. */
    appleSpeechDiarize: boolean;
    /** Nemotron decoder build; empty uses the one recommended for this machine. */
    nemotronVariant: NemotronVariant | '';
    /** Label speakers (Nemotron's speaker branch, Sortformer as fallback). */
    nemotronDiarize: boolean;
    /** ISO 639-1 code for Nemotron and cloud speech-to-text; empty detects it. */
    asrLanguage: string;
    /** OpenRouter speech-to-text model for the `cloud` engine. */
    cloudAsrModel: string;
    /** Attribute speakers with Sortformer for the `cloud` engine. */
    cloudAsrDiarize: boolean;
    /**
     * Send OpenRouter requests (speech-to-text and, with the OpenRouter
     * provider, the LLM) through eu.openrouter.ai: processed in Europe only.
     */
    openRouterEuOnly: boolean;
    /** The setup wizard version last completed or skipped; 0 = never. */
    setupWizardVersion: number;
}

export { SETUP_WIZARD_VERSION } from '../utils/setupWizardVersion';

export interface ModelFetchState {
    availableModels: string[];
    supportsModelFetch: boolean | null; // null = unknown, true = supported, false = not supported
}

const STORAGE_KEY = 'llm-settings';
const MODEL_FETCH_STATE_KEY = 'model-fetch-state';

const defaultSettings: LLMSettings = {
    baseUrl: 'https://generativelanguage.googleapis.com',
    model: 'gemini-2.5-flash',
    enforceJsonSchema: true,
    maxAnalysisChunkMinutes: 30,
    glossary: '',
    preClipPadding: 0.0,
    postClipPadding: 0.0,
    exportQuality: 'balanced',
    transcriptionBackend: 'local',
    localEngine: 'nemotron',
    parakeetModelPath: '',
    sortformerModelPath: '',
    crisperModel: 'large',
    crisperLanguage: 'en',
    crisperMode: 'verbatim',
    crisperBackend: 'auto',
    crisperDevice: 'auto',
    crisperComputeType: 'auto',
    crisperRemoveVocalEvents: false,
    crisperDiarize: true,
    crisperPythonPath: '',
    appleSpeechLocale: '',
    appleSpeechDiarize: true,
    nemotronVariant: '',
    nemotronDiarize: true,
    asrLanguage: '',
    cloudAsrModel: NEMOTRON_CLOUD_MODEL,
    cloudAsrDiarize: true,
    openRouterEuOnly: false,
    setupWizardVersion: 0,
};

// Load from localStorage
/** An API key found in localStorage that still has to move to the keychain. */
let legacyApiKey: string | null = null;

const loadSettings = (): LLMSettings => {
    try {
        const stored = localStorage.getItem(STORAGE_KEY);
        if (stored) {
            const { apiKey, ...parsed } = JSON.parse(stored) as Partial<LLMSettings> & {
                apiKey?: unknown;
            };
            // Versions before 0.14 kept the key here; it moves to the keychain.
            if (typeof apiKey === 'string' && apiKey.trim()) {
                legacyApiKey = apiKey.trim();
            }
            const merged = { ...defaultSettings, ...parsed };

            // Older versions stored the engine inside the pipeline value
            // ('parakeet' / 'crisper'); split it back out.
            const migrated = migrateTranscriptionBackend(parsed.transcriptionBackend);
            if (migrated) {
                merged.transcriptionBackend = migrated.backend;
                // An explicitly stored localEngine wins; it only exists post-split.
                merged.localEngine =
                    parsed.localEngine ?? migrated.localEngine ?? defaultSettings.localEngine;
            } else {
                merged.transcriptionBackend = defaultSettings.transcriptionBackend;
            }

            return merged;
        }
    } catch (e) {
        console.error('Failed to load settings:', e);
    }
    return defaultSettings;
};

// Reactive settings
const settings = ref<LLMSettings>(loadSettings());

// Model fetch state
const defaultModelFetchState: ModelFetchState = {
    availableModels: [],
    supportsModelFetch: null,
};

const loadModelFetchState = (): ModelFetchState => {
    try {
        const stored = localStorage.getItem(MODEL_FETCH_STATE_KEY);
        if (stored) {
            return { ...defaultModelFetchState, ...JSON.parse(stored) };
        }
    } catch (e) {
        console.error('Failed to load model fetch state:', e);
    }
    return defaultModelFetchState;
};

const modelFetchState = ref<ModelFetchState>(loadModelFetchState());

// Watch for changes and persist
watch(
    settings,
    (newSettings) => {
        try {
            // Keep a not-yet-migrated key until the keychain has it.
            const stored = legacyApiKey ? { ...newSettings, apiKey: legacyApiKey } : newSettings;
            localStorage.setItem(STORAGE_KEY, JSON.stringify(stored));
        } catch (e) {
            console.error('Failed to save settings:', e);
        }
    },
    { deep: true },
);

watch(
    modelFetchState,
    (newState) => {
        try {
            localStorage.setItem(MODEL_FETCH_STATE_KEY, JSON.stringify(newState));
        } catch (e) {
            console.error('Failed to save model fetch state:', e);
        }
    },
    { deep: true },
);

/** Whether the LLM provider's API key is saved in the OS credential store. */
const apiKeyStored = ref(false);
/** Whether an OpenRouter API key is saved (speech-to-text and OpenRouter LLMs). */
const openRouterKeyStored = ref(false);
/** Whether the key the configured LLM endpoint uses is stored. */
const llmKeyStored = computed(() =>
    isOpenRouterUrl(settings.value.baseUrl) ? openRouterKeyStored.value : apiKeyStored.value,
);

/** Re-read whether the API keys are stored (after saving or removing one). */
export async function refreshApiKeyStatus(): Promise<void> {
    try {
        apiKeyStored.value = await commands.hasApiKey();
    } catch (error) {
        console.error('Failed to check for a stored API key:', error);
        apiKeyStored.value = false;
    }
    try {
        openRouterKeyStored.value = await commands.hasOpenrouterApiKey();
    } catch (error) {
        console.error('Failed to check for a stored OpenRouter key:', error);
        openRouterKeyStored.value = false;
    }
}

/**
 * Move an API key that an earlier version kept in localStorage into the OS
 * credential store, then drop it from localStorage. If the store is
 * unavailable the key stays where it is and the move is retried next start.
 */
export async function migrateLegacyApiKey(): Promise<void> {
    if (legacyApiKey) {
        try {
            await commands.setApiKey(legacyApiKey);
            legacyApiKey = null;
            localStorage.setItem(STORAGE_KEY, JSON.stringify(settings.value));
        } catch (error) {
            console.error('Failed to move the API key to the system keychain:', error);
        }
    }
    await refreshApiKeyStatus();
}

export const useSettings = () => {
    const updateSettings = (newSettings: Partial<LLMSettings>) => {
        settings.value = { ...settings.value, ...newSettings };
    };

    const resetSettings = () => {
        settings.value = { ...defaultSettings };
    };

    const updateModelFetchState = (newState: Partial<ModelFetchState>) => {
        modelFetchState.value = { ...modelFetchState.value, ...newState };
    };

    return {
        settings,
        apiKeyStored,
        openRouterKeyStored,
        llmKeyStored,
        updateSettings,
        resetSettings,
        modelFetchState,
        updateModelFetchState,
    };
};
