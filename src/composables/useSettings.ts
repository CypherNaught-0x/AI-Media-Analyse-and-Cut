import { commands, type ExportQuality } from '../bindings';
import type { CrisperLanguage, CrisperMode, LocalEngine, TranscriptionBackend } from '../types';
import { migrateTranscriptionBackend } from '../types';
import { ref, watch } from 'vue';

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
}

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
    transcriptionBackend: 'llm',
    localEngine: 'parakeet',
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

/** Whether an API key is saved in the OS credential store. */
const apiKeyStored = ref(false);

/** Re-read whether an API key is stored (after saving or removing one). */
export async function refreshApiKeyStatus(): Promise<void> {
    try {
        apiKeyStored.value = await commands.hasApiKey();
    } catch (error) {
        console.error('Failed to check for a stored API key:', error);
        apiKeyStored.value = false;
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
        updateSettings,
        resetSettings,
        modelFetchState,
        updateModelFetchState,
    };
};
