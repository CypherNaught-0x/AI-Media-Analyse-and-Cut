import { ref, watch } from 'vue';

export interface LLMSettings {
  baseUrl: string;
  apiKey: string;
  model: string;
  glossary: string;
}

const STORAGE_KEY = 'llm-settings';

const defaultSettings: LLMSettings = {
  baseUrl: 'https://generativelanguage.googleapis.com',
  apiKey: '',
  model: 'gemini-2.0-flash',
  glossary: '',
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

export const useSettings = () => {
  const updateSettings = (newSettings: Partial<LLMSettings>) => {
    settings.value = { ...settings.value, ...newSettings };
  };

  const resetSettings = () => {
    settings.value = { ...defaultSettings };
  };

  return {
    settings,
    updateSettings,
    resetSettings,
  };
};
