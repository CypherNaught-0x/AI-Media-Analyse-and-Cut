import { describe, it, expect, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import App from '../App.vue';
import { createRouter, createWebHistory } from 'vue-router';

// Mock Tauri plugins
vi.mock('@tauri-apps/plugin-updater', () => ({
  check: vi.fn(() => Promise.resolve({ available: false })),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  ask: vi.fn(() => Promise.resolve(false)),
}));

vi.mock('@tauri-apps/plugin-process', () => ({
  relaunch: vi.fn(),
}));

const router = createRouter({
  history: createWebHistory(),
  routes: [{ path: '/', component: { template: '<div>Home</div>' } }],
});

describe('App.vue', () => {
  it('renders router view', async () => {
    const wrapper = mount(App, {
      global: {
        plugins: [router],
      },
    });

    await router.isReady();
    expect(wrapper.html()).toContain('Home');
  });
});
