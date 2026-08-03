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

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
  save: vi.fn(),
  ask: vi.fn(() => Promise.resolve(false)),
  message: vi.fn(() => Promise.resolve()),
}));

const invokeMock = vi.fn((command: string) => {
  if (command === 'crisper_environment_status') {
    return Promise.resolve({
      pythonPath: '/usr/bin/python3',
      python: '3.12.1',
      pythonSupported: true,
      minimumPython: '3.10',
      installed: true,
      crisperwhisperVersion: '2.0.1',
      backends: ['transformers'],
      torchVersion: '2.6.0',
      cuda: false,
      mps: true,
      environmentDir: '/tmp/env',
      managedEnvironmentExists: true,
      ready: true,
      message: null,
    });
  }
  return Promise.resolve(undefined);
});

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (command: string, args?: unknown) => invokeMock(command, args),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
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
  transcriptionBackend: 'llm',
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
      transcriptionBackend: 'llm',
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

    expect(wrapper.text()).toContain('AI Settings');
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

  it('probes the CrisperWhisper environment on mount and reports it as ready', async () => {
    const wrapper = mount(Settings, {
      global: {
        plugins: [router],
      },
    });
    await flushPromises();

    expect(invokeMock).toHaveBeenCalledWith('crisper_environment_status', {
      pythonPath: '',
    });
    expect(wrapper.text()).toContain('Ready');
    expect(wrapper.text()).toContain('2.0.1');
  });

  it('states the English/German restriction and the non-commercial license', () => {
    const wrapper = mount(Settings, {
      global: {
        plugins: [router],
      },
    });

    expect(wrapper.text()).toContain('English and German only');
    expect(wrapper.text()).toContain('non-commercial research use');
  });

  it('saves the CrisperWhisper options', async () => {
    const wrapper = mount(Settings, {
      global: {
        plugins: [router],
      },
    });
    await flushPromises();

    const intendedButton = wrapper
      .findAll('button')
      .find((button) => button.text().startsWith('Intended'));
    expect(intendedButton).toBeTruthy();
    await intendedButton!.trigger('click');

    const vocalEventsCard = wrapper
      .findAll('.cursor-pointer')
      .find((node) => node.text().includes('Remove Vocal Events'));
    expect(vocalEventsCard).toBeTruthy();
    await vocalEventsCard!.trigger('click');

    const saveButton = wrapper.findAll('button').find((b) => b.text() === 'Save Settings');
    await saveButton!.trigger('click');

    expect(updateSettingsMock).toHaveBeenCalledWith(
      expect.objectContaining({
        crisperMode: 'intended',
        crisperRemoveVocalEvents: true,
        crisperLanguage: 'en',
        crisperModel: 'large',
      }),
    );
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
