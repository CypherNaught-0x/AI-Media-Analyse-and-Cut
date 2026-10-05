import { describe, it, expect, vi } from 'vitest';
import { mount, flushPromises } from '@vue/test-utils';
import { defineComponent, h, onMounted, onUnmounted } from 'vue';
import App from '../App.vue';
import { createMemoryHistory, createRouter, createWebHistory } from 'vue-router';

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

    it('keeps the Home view alive while Settings is open', async () => {
        const lifecycle: string[] = [];
        const Home = defineComponent({
            name: 'Home',
            setup() {
                onMounted(() => lifecycle.push('home:mounted'));
                onUnmounted(() => lifecycle.push('home:unmounted'));
                return () => h('div', 'home view');
            },
        });
        const Settings = defineComponent({
            name: 'Settings',
            render: () => h('div', 'settings view'),
        });
        const keepAliveRouter = createRouter({
            history: createMemoryHistory(),
            routes: [
                { path: '/', component: Home },
                { path: '/settings', component: Settings },
            ],
        });

        keepAliveRouter.push('/');
        await keepAliveRouter.isReady();
        const wrapper = mount(App, { global: { plugins: [keepAliveRouter] } });
        await flushPromises();

        await keepAliveRouter.push('/settings');
        await flushPromises();
        expect(wrapper.text()).toContain('settings view');

        await keepAliveRouter.push('/');
        await flushPromises();
        expect(wrapper.text()).toContain('home view');
        expect(lifecycle).toEqual(['home:mounted']);
    });
});
