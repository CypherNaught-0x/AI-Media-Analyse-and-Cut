import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, flushPromises } from '@vue/test-utils';
import Home from '../Home.vue';
import { createRouter, createWebHistory } from 'vue-router';
import type { EditSessionV1 } from '../../types';
import { ref } from 'vue';

// Mock Tauri APIs
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string) => path),
  invoke: vi.fn((cmd, args) => {
    if (cmd === 'init_ffmpeg') return Promise.resolve('FFmpeg initialized');
    if (cmd === 'path_exists') return Promise.resolve(args.path !== '/missing/source.mp4');
    if (cmd === 'read_text_file') {
      if (args.path === '/tmp/session.json') {
        return Promise.resolve(JSON.stringify(buildSession('/loaded/from-file.mp4')));
      }
      return Promise.reject(new Error('missing file'));
    }
    if (cmd === 'write_text_file') return Promise.resolve(null);
    return Promise.resolve(null);
  }),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
  save: vi.fn(),
  ask: vi.fn(),
}));

// Mock useSettings
vi.mock('../../composables/useSettings', () => ({
  useSettings: () => ({
    settings: ref({
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
    }),
  }),
}));

// Mock Editor component to avoid testing it again
vi.mock('../../components/Editor.vue', () => ({
  default: {
    template: '<div class="mock-editor"></div>',
    props: ['segments', 'getPlayhead'],
  },
}));

// Mock components
vi.mock('../../components/FileSelector.vue', () => ({
  default: { template: '<div class="mock-file-selector"></div>' }
}));
vi.mock('../../components/AnalysisSettings.vue', () => ({
  default: { template: '<div class="mock-analysis-settings"></div>' }
}));
vi.mock('../../components/ClipGenerator.vue', () => ({
  default: { template: '<div class="mock-clip-generator"></div>' }
}));
vi.mock('../../components/ClipList.vue', () => ({
  default: { template: '<div class="mock-clip-list"></div>' }
}));
vi.mock('../../components/ViralClipsGenerator.vue', () => ({
  default: { template: '<div class="mock-viral-clips-generator"></div>' }
}));
vi.mock('../../components/PodcastGenerator.vue', () => ({
  default: { template: '<div class="mock-podcast-generator"></div>' }
}));
vi.mock('../../components/SubtitleExport.vue', () => ({
  default: { template: '<div class="mock-subtitle-export"></div>' }
}));
vi.mock('../../components/ErrorOverlay.vue', () => ({
  default: { template: '<div class="mock-error-overlay"></div>' }
}));
vi.mock('../../components/StatusBar.vue', () => ({
  default: { template: '<div class="mock-status-bar"></div>' }
}));

const router = createRouter({
  history: createWebHistory(),
  routes: [{ path: '/', component: Home }, { path: '/settings', component: { template: '<div>Settings</div>' } }],
});

function buildSession(inputPath = '/tmp/source.mp4'): EditSessionV1 {
  return {
    version: 1,
    savedAt: '2026-04-07T10:00:00.000Z',
    transcriptWorkspace: {
      inputPath,
      segments: [{ start: '00:00', end: '00:02', speaker: 'Speaker 1', text: 'Saved text' }],
      translations: { Spanish: [{ start: '00:00', end: '00:02', speaker: 'Speaker 1', text: 'Texto guardado' }] },
      currentLanguage: 'Original',
      targetLanguage: 'Spanish',
      context: 'saved context',
      speakerCount: 2,
      removeFillerWords: true,
      trimSilence: false,
      useAdvancedAlignment: false,
      speakerOrder: ['Speaker 1'],
      lastAnalyzedSettings: {
        context: 'saved context',
        glossary: 'AI',
        speakerCount: 2,
        removeFillerWords: true,
        trimSilence: false,
        transcriptionBackend: 'llm',
        parakeetModelPath: '',
        sortformerModelPath: '',
      },
      settingsSnapshot: {
        glossary: 'AI',
        transcriptionBackend: 'llm',
        parakeetModelPath: '',
        sortformerModelPath: '',
      },
    },
    clipWorkspace: {
      count: 3,
      minDuration: 10,
      maxDuration: 120,
      topic: 'topic',
      allowSplicing: true,
      clips: [{ title: 'Clip 1', reason: 'Reason', segments: [{ start: '00:00', end: '00:10' }] }],
      lastExportPath: '/tmp/export',
      includeSubtitles: true,
      fastMode: true,
      trimBoundarySilence: false,
      selectedClipIndices: [0],
    },
    viralClipsWorkspace: {
      count: 3,
      minDuration: 10,
      maxDuration: 120,
      topic: 'viral',
      allowSplicing: false,
      clips: [],
      lastExportPath: '',
      trimBoundarySilence: false,
    },
    podcastWorkspace: {
      minDurationMinutes: 10,
      maxDurationMinutes: 15,
      startPadding: 0.5,
      endPadding: 0.5,
      introPath: '',
      outroPath: '',
      podcastScript: null,
      lastExportPath: '',
    },
  };
}

describe('Home.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    const storage = new Map<string, string>();
    Object.defineProperty(globalThis, 'localStorage', {
      value: {
        getItem: vi.fn((key: string) => storage.get(key) ?? null),
        setItem: vi.fn((key: string, value: string) => {
          storage.set(key, value);
        }),
        removeItem: vi.fn((key: string) => {
          storage.delete(key);
        }),
        clear: vi.fn(() => {
          storage.clear();
        }),
      },
      configurable: true,
    });
  });

  it('initializes ffmpeg on mount', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    mount(Home, {
      global: {
        plugins: [router],
      },
    });

    await flushPromises();
    
    expect(invoke).toHaveBeenCalledWith('init_ffmpeg');
  });

  it('disables transcript-dependent tabs until a transcript exists', async () => {
    const wrapper = mount(Home, {
      global: {
        plugins: [router],
      },
    });

    await flushPromises();

    expect(wrapper.get('[data-testid="workspace-tab-source"]').attributes('aria-selected')).toBe('true');
    expect(wrapper.get('[data-testid="workspace-tab-transcript"]').attributes('disabled')).toBeDefined();
    expect(wrapper.get('[data-testid="workspace-tab-clips"]').attributes('disabled')).toBeDefined();
    expect(wrapper.get('[data-testid="workspace-tab-podcast"]').attributes('disabled')).toBeDefined();
  });

  it('activates the transcript tab once a transcript becomes available', async () => {
    localStorage.setItem('home-edit-session-v1', JSON.stringify(buildSession()));

    const wrapper = mount(Home, {
      global: {
        plugins: [router],
      },
    });

    await flushPromises();

    expect(wrapper.get('[data-testid="workspace-tab-transcript"]').attributes('disabled')).toBeUndefined();
    expect(wrapper.get('[data-testid="workspace-tab-transcript"]').attributes('aria-selected')).toBe('true');
  });

  it('restores an autosaved session on mount', async () => {
    localStorage.setItem('home-edit-session-v1', JSON.stringify(buildSession()));

    const wrapper = mount(Home, {
      global: {
        plugins: [router],
      },
    });

    await flushPromises();

    expect(wrapper.text()).toContain('Transcript');
    expect(wrapper.text()).toContain('1 Segments');
    expect(wrapper.text()).not.toContain('Media file missing');
    expect(wrapper.find('.mock-viral-clips-generator').exists()).toBe(true);
    expect(wrapper.find('.mock-podcast-generator').exists()).toBe(true);
    expect(wrapper.find('.mock-clip-generator').exists()).toBe(false);
    expect(wrapper.find('.mock-clip-list').exists()).toBe(false);
  });

  it('loads a session file from disk', async () => {
    const { open } = await import('@tauri-apps/plugin-dialog');
    vi.mocked(open).mockResolvedValue('/tmp/session.json');

    const wrapper = mount(Home, {
      global: {
        plugins: [router],
      },
    });

    await flushPromises();
    const loadButton = wrapper.findAll('button').find((button) => button.text() === 'Load Session');
    expect(loadButton).toBeDefined();
    await loadButton!.trigger('click');
    await flushPromises();

    expect(wrapper.text()).toContain('Transcript');
    expect(wrapper.text()).toContain('1 Segments');
  });

  it('shows a missing-media warning for restored sessions with an invalid source path', async () => {
    localStorage.setItem('home-edit-session-v1', JSON.stringify(buildSession('/missing/source.mp4')));

    const wrapper = mount(Home, {
      global: {
        plugins: [router],
      },
    });

    await flushPromises();

    expect(wrapper.text()).toContain('Media file missing');
  });
});
