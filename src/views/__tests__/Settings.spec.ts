import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, flushPromises } from '@vue/test-utils';
import { ref } from 'vue';
import Settings from '../Settings.vue';
import { createRouter, createWebHistory } from 'vue-router';

// Mock Tauri APIs
vi.mock('@tauri-apps/api/app', () => ({
  getVersion: vi.fn(() => Promise.resolve('1.0.0')),
}));

vi.mock('@tauri-apps/plugin-updater', () => ({
  check: vi.fn(() => Promise.resolve({ available: false })),
}));

// Mock useSettings
const updateSettingsMock = vi.fn();
const updateModelFetchStateMock = vi.fn();
const settingsRef = ref({
  apiKey: 'test-api-key',
  baseUrl: 'https://test.url',
  model: 'test-model',
  enforceJsonSchema: true,
  glossary: '',
  preClipPadding: 0,
  postClipPadding: 0,
});
const modelFetchStateRef = ref({
  availableModels: [],
  supportsModelFetch: true,
});
vi.mock('../../composables/useSettings', () => ({
  useSettings: () => ({
    settings: settingsRef,
    updateSettings: updateSettingsMock,
    modelFetchState: modelFetchStateRef,
    updateModelFetchState: updateModelFetchStateMock,
  }),
}));

const router = createRouter({
  history: createWebHistory(),
  routes: [{ path: '/', component: { template: '<div>Home</div>' } }, { path: '/settings', component: Settings }],
});

describe('Settings.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    settingsRef.value = {
      apiKey: 'test-api-key',
      baseUrl: 'https://test.url',
      model: 'test-model',
      enforceJsonSchema: true,
      glossary: '',
      preClipPadding: 0,
      postClipPadding: 0,
    };
    modelFetchStateRef.value = {
      availableModels: [],
      supportsModelFetch: true,
    };
    globalThis.fetch = vi.fn();
  });

  it('renders correctly', () => {
    const wrapper = mount(Settings, {
      global: {
        plugins: [router],
      },
    });

    expect(wrapper.text()).toContain('LLM Settings');
    expect(wrapper.find('input[type="password"]').exists()).toBe(true);
  });

  it('fetches models correctly', async () => {
    const mockModels = { models: [{ name: 'models/gemini-pro' }] };
    (globalThis.fetch as any).mockResolvedValue({
      ok: true,
      json: () => Promise.resolve(mockModels),
    });

    const wrapper = mount(Settings, {
      global: {
        plugins: [router],
      },
    });

    // Trigger fetch models
    // We need to make sure the button is enabled (apiKey is present in mock)
    const fetchButton = wrapper.findAll('button').find(b => b.text() === 'Refresh Models');
    await fetchButton!.trigger('click');
    
    await flushPromises();

    expect(globalThis.fetch).toHaveBeenCalled();
    expect(updateModelFetchStateMock).toHaveBeenCalledWith({
      supportsModelFetch: true,
      availableModels: ['gemini-pro'],
    });
  });

  it('saves settings', async () => {
    const wrapper = mount(Settings, {
      global: {
        plugins: [router],
      },
    });

    const saveButton = wrapper.findAll('button').find(b => b.text() === 'Save Settings');
    // It might be disabled if no changes. Let's change something.
    const input = wrapper.find('input[type="password"]');
    await input.setValue('new-api-key');

    await saveButton!.trigger('click');

    expect(updateSettingsMock).toHaveBeenCalledWith(expect.objectContaining({
      apiKey: 'new-api-key',
    }));
  });

  it('saves the JSON enforcement toggle', async () => {
    const wrapper = mount(Settings, {
      global: {
        plugins: [router],
      },
    });

    const toggleCard = wrapper.findAll('.cursor-pointer').find(node =>
      node.text().includes('Enforce Structured JSON for transcript analysis'),
    );
    expect(toggleCard).toBeTruthy();
    await toggleCard!.trigger('click');

    const saveButton = wrapper.findAll('button').find(b => b.text() === 'Save Settings');
    await saveButton!.trigger('click');

    expect(updateSettingsMock).toHaveBeenCalledWith(expect.objectContaining({
      enforceJsonSchema: false,
    }));
  });
});
