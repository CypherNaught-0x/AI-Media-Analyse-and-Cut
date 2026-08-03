import type { CrisperLanguage, CrisperMode, LocalEngine, TranscriptionBackend } from '../types';
import { migrateTranscriptionBackend } from '../types';
import { ref, watch } from 'vue';

export interface LLMSettings {
  baseUrl: string;
  apiKey: string;
  model: string;
  enforceJsonSchema: boolean;
  maxAnalysisChunkMinutes: number;
  glossary: string;
  preClipPadding: number;
  postClipPadding: number;
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
}

export interface ModelFetchState {
  availableModels: string[];
  supportsModelFetch: boolean | null; // null = unknown, true = supported, false = not supported
}

const STORAGE_KEY = 'llm-settings';
const MODEL_FETCH_STATE_KEY = 'model-fetch-state';

const defaultSettings: LLMSettings = {
  baseUrl: 'https://generativelanguage.googleapis.com',
  apiKey: '',
  model: 'gemini-2.5-flash',
  enforceJsonSchema: true,
  maxAnalysisChunkMinutes: 30,
  glossary: '',
  preClipPadding: 0.0,
  postClipPadding: 0.0,
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
};

// Load from localStorage
const loadSettings = (): LLMSettings => {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) {
      const parsed = JSON.parse(stored) as Partial<LLMSettings>;
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
      localStorage.setItem(STORAGE_KEY, JSON.stringify(newSettings));
    } catch (e) {
      console.error('Failed to save settings:', e);
    }
  },
  { deep: true }
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
  { deep: true }
);

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
    updateSettings,
    resetSettings,
    modelFetchState,
    updateModelFetchState,
  };
};
