import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { createMemoryHistory, createRouter } from 'vue-router';
import { defineComponent } from 'vue';
import Toast from '../Toast.vue';

describe('Toast.vue', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it('renders the message, progress and action', async () => {
        const wrapper = mount(Toast, {
            props: {
                show: true,
                message: 'Downloading update...',
                progress: 42.4,
                actionLabel: 'Restart now',
            },
        });

        expect(wrapper.text()).toContain('Downloading update...');
        expect(wrapper.get('[data-testid="toast-progress"]').attributes('style')).toContain(
            'width: 42.4%',
        );
        expect(wrapper.text()).toContain('42%');

        const action = wrapper.findAll('button').find((b) => b.text() === 'Restart now');
        await action!.trigger('click');
        expect(wrapper.emitted('action')).toHaveLength(1);

        await wrapper.get('button[aria-label="Dismiss"]').trigger('click');
        expect(wrapper.emitted('dismiss')).toHaveLength(1);
    });

    it('auto-dismisses plain success messages but keeps info messages', () => {
        const success = mount(Toast, { props: { show: true, message: 'Saved', tone: 'success' } });
        const info = mount(Toast, { props: { show: true, message: 'Heads up', tone: 'info' } });
        vi.advanceTimersByTime(4900);
        expect(success.emitted('dismiss')).toBeUndefined();
        vi.advanceTimersByTime(200);
        expect(success.emitted('dismiss')).toHaveLength(1);
        vi.advanceTimersByTime(20000);
        expect(info.emitted('dismiss')).toBeUndefined();
    });

    it('keeps an in-flight info toast without progress (update install)', () => {
        const wrapper = mount(Toast, {
            props: { show: true, message: 'Installing update...', tone: 'info', progress: null },
        });
        vi.advanceTimersByTime(5100);
        expect(wrapper.emitted('dismiss')).toBeUndefined();
        expect(wrapper.text()).toContain('Installing update...');
    });

    it('pauses auto-dismiss while hovered', async () => {
        const wrapper = mount(Toast, { props: { show: true, message: 'Saved', tone: 'success' } });
        await wrapper.get('[data-testid="toast"]').trigger('mouseenter');
        vi.advanceTimersByTime(10000);
        expect(wrapper.emitted('dismiss')).toBeUndefined();
        await wrapper.get('[data-testid="toast"]').trigger('mouseleave');
        vi.advanceTimersByTime(5100);
        expect(wrapper.emitted('dismiss')).toHaveLength(1);
    });

    it('keeps errors, progress and actionable toasts until dismissed', () => {
        const error = mount(Toast, { props: { show: true, message: 'Failed', tone: 'error' } });
        const progress = mount(Toast, { props: { show: true, message: 'Working', progress: 10 } });
        const action = mount(Toast, {
            props: {
                show: true,
                message: 'Installed',
                tone: 'success',
                actionLabel: 'Restart now',
            },
        });
        vi.advanceTimersByTime(20000);
        expect(error.emitted('dismiss')).toBeUndefined();
        expect(progress.emitted('dismiss')).toBeUndefined();
        expect(action.emitted('dismiss')).toBeUndefined();
    });

    it('renders nothing when hidden', () => {
        const wrapper = mount(Toast, { props: { show: false, message: 'Hidden' } });
        expect(wrapper.find('[data-testid="toast"]').exists()).toBe(false);
        vi.advanceTimersByTime(10000);
        expect(wrapper.emitted('dismiss')).toBeUndefined();
    });

    it('clears the activity bar on Home and sits in the corner elsewhere', async () => {
        const Blank = defineComponent({ template: '<div />' });
        const router = createRouter({
            history: createMemoryHistory(),
            routes: [
                { path: '/', component: Blank },
                { path: '/settings', component: Blank },
            ],
        });
        await router.push('/');
        const wrapper = mount(Toast, {
            props: { show: true, message: 'Saved', tone: 'success' },
            global: { plugins: [router] },
        });
        expect(wrapper.get('[data-testid="toast"]').classes()).toContain('bottom-14');

        await router.push('/settings');
        await wrapper.vm.$nextTick();
        expect(wrapper.get('[data-testid="toast"]').classes()).toContain('bottom-5');

        const bare = mount(Toast, { props: { show: true, message: 'Saved' } });
        expect(bare.get('[data-testid="toast"]').classes()).toContain('bottom-5');
    });
});
