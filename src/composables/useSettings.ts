import { ref, watch } from 'vue';

export interface LLMSettings {
  baseUrl: string;
  apiKey: string;
  model: string;
  enforceJsonSchema: boolean;
  glossary: string;
  preClipPadding: number;
  postClipPadding: number;
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
  glossary: '',
  preClipPadding: 0.0,
  postClipPadding: 0.0,
};

// Load from localStorage
const loadSettings = (): LLMSettings => {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) {
      return { ...defaultSettings, ...JSON.parse(stored) };
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
