import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, flushPromises } from '@vue/test-utils';
import Home from '../Home.vue';
import { createRouter, createWebHistory } from 'vue-router';

// Mock Tauri APIs
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn((cmd) => {
    if (cmd === 'init_ffmpeg') return Promise.resolve('FFmpeg initialized');
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
    settings: {
      value: {
        apiKey: 'test-api-key',
        baseUrl: 'https://test.url',
        model: 'test-model',
      },
    },
  }),
}));

// Mock Editor component to avoid testing it again
vi.mock('../../components/Editor.vue', () => ({
  default: {
    template: '<div class="mock-editor"></div>',
    props: ['segments'],
  },
}));

const router = createRouter({
  history: createWebHistory(),
  routes: [{ path: '/', component: Home }, { path: '/settings', component: { template: '<div>Settings</div>' } }],
});

describe('Home.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders correctly', async () => {
    const wrapper = mount(Home, {
      global: {
        plugins: [router],
      },
    });
    
    await flushPromises();
    expect(wrapper.text()).toContain('Media AI Cutter');
    expect(wrapper.text()).toContain('Intelligent Video Segmentation & Processing');
  });

  it('initializes ffmpeg on mount', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    mount(Home, {
      global: {
        plugins: [router],
      },
    });
    
    expect(invoke).toHaveBeenCalledWith('init_ffmpeg');
  });
});
