import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createMemoryHistory, createRouter } from 'vue-router';
import AppRail from '../AppRail.vue';
import { resetWorkspaceNav, useWorkspaceNav } from '../../../composables/useWorkspaceNav';

function makeRouter() {
    return createRouter({
        history: createMemoryHistory(),
        routes: [
            { path: '/', component: { template: '<div>home</div>' } },
            { path: '/settings', component: { template: '<div>settings</div>' } },
        ],
    });
}

async function mountRail(path = '/') {
    const router = makeRouter();
    await router.push(path);
    await router.isReady();
    const wrapper = mount(AppRail, { global: { plugins: [router] } });
    return { wrapper, router };
}

describe('AppRail.vue', () => {
    let storage: Map<string, string>;

    beforeEach(() => {
        resetWorkspaceNav();
        storage = new Map();
        Object.defineProperty(globalThis, 'localStorage', {
            value: {
                getItem: vi.fn((key: string) => storage.get(key) ?? null),
                setItem: vi.fn((key: string, value: string) => void storage.set(key, value)),
                removeItem: vi.fn((key: string) => void storage.delete(key)),
                clear: vi.fn(() => storage.clear()),
            },
            configurable: true,
        });
    });

    it('disables the transcript-dependent steps until a transcript exists', async () => {
        const { wrapper } = await mountRail();

        expect(
            wrapper.get('[data-testid="rail-step-source"]').attributes('disabled'),
        ).toBeUndefined();
        for (const id of ['transcript', 'clips', 'podcast']) {
            const step = wrapper.get(`[data-testid="rail-step-${id}"]`);
            expect(step.attributes('disabled')).toBeDefined();
            expect(step.attributes('title')).toBe('Analyze a media file first');
        }
        expect(wrapper.get('[data-testid="rail-step-source"]').attributes('aria-current')).toBe(
            'page',
        );

        useWorkspaceNav().state.hasTranscript = true;
        await flushPromises();
        for (const id of ['transcript', 'clips', 'podcast']) {
            expect(
                wrapper.get(`[data-testid="rail-step-${id}"]`).attributes('disabled'),
            ).toBeUndefined();
        }
    });

    it('sets the active step when an enabled step is clicked', async () => {
        const { state } = useWorkspaceNav();
        state.hasTranscript = true;
        const { wrapper } = await mountRail();

        await wrapper.get('[data-testid="rail-step-clips"]').trigger('click');
        await flushPromises();

        expect(state.activeStep).toBe('clips');
        expect(wrapper.get('[data-testid="rail-step-clips"]').attributes('aria-current')).toBe(
            'page',
        );
        expect(
            wrapper.get('[data-testid="rail-step-source"]').attributes('aria-current'),
        ).toBeUndefined();
    });

    it('ignores clicks on a disabled step', async () => {
        const { state } = useWorkspaceNav();
        const { wrapper } = await mountRail();

        await wrapper.get('[data-testid="rail-step-podcast"]').trigger('click');
        expect(state.activeStep).toBe('source');
    });

    it('navigates back to the workspace when a step is clicked from Settings', async () => {
        const { state } = useWorkspaceNav();
        state.hasTranscript = true;
        const { wrapper, router } = await mountRail('/settings');

        expect(wrapper.get('[data-testid="rail-settings"]').attributes('aria-current')).toBe(
            'page',
        );
        expect(
            wrapper.get('[data-testid="rail-step-source"]').attributes('aria-current'),
        ).toBeUndefined();

        await wrapper.get('[data-testid="rail-step-transcript"]').trigger('click');
        await flushPromises();

        expect(router.currentRoute.value.path).toBe('/');
        expect(state.activeStep).toBe('transcript');
    });

    it('remembers the collapsed state', async () => {
        const { wrapper } = await mountRail();
        const toggle = wrapper.get('button[aria-label="Collapse sidebar"]');

        await toggle.trigger('click');
        await flushPromises();

        expect(localStorage.setItem).toHaveBeenCalledWith('rail-collapsed', '1');
        expect(wrapper.find('button[aria-label="Expand sidebar"]').exists()).toBe(true);
        expect(wrapper.get('aside').classes()).toContain('w-[60px]');

        // A fresh rail starts collapsed from storage, and expanding persists too.
        const { wrapper: again } = await mountRail();
        expect(again.get('aside').classes()).toContain('w-[60px]');
        await again.get('button[aria-label="Expand sidebar"]').trigger('click');
        await flushPromises();
        expect(storage.get('rail-collapsed')).toBe('0');
    });
});
