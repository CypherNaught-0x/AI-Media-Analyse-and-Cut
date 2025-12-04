import { describe, it, expect } from 'vitest';
import { mount } from '@vue/test-utils';
import App from '../App.vue';
import { createRouter, createWebHistory } from 'vue-router';

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
