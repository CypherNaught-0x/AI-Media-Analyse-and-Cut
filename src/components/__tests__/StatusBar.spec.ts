import { describe, it, expect } from 'vitest';
import { mount } from '@vue/test-utils';
import StatusBar from '../StatusBar.vue';

describe('StatusBar.vue', () => {
    it('renders status', () => {
        const wrapper = mount(StatusBar, {
            props: {
                status: 'Ready',
                isProcessing: false,
                progressPercentage: null,
            },
        });

        expect(wrapper.text()).toContain('Ready');
    });

    it('shows progress bar when percentage is provided', () => {
        const wrapper = mount(StatusBar, {
            props: {
                status: 'Processing',
                isProcessing: true,
                progressPercentage: 50,
                progressEtaSeconds: 95,
            },
        });

        const fill = wrapper.get('[data-testid="status-progress-fill"]');
        expect(fill.classes()).toContain('bg-accent');
        expect(fill.attributes('style')).toContain('width: 50%');
        expect(wrapper.text()).toContain('50%');
        expect(wrapper.text()).toContain('ETA 1:35');
    });

    it('shows a cancel button while processing', async () => {
        const wrapper = mount(StatusBar, {
            props: {
                status: 'Processing',
                isProcessing: true,
                progressPercentage: 10,
                isCancelling: false,
            },
        });

        const button = wrapper.get('[data-testid="status-cancel"]');
        expect(button.text()).toBe('Cancel');

        await button.trigger('click');
        expect(wrapper.emitted('cancel')).toHaveLength(1);
    });

    it('does not offer cancel when idle', () => {
        const wrapper = mount(StatusBar, {
            props: { status: 'Ready', isProcessing: false, progressPercentage: null },
        });
        expect(wrapper.find('[data-testid="status-cancel"]').exists()).toBe(false);
        expect(wrapper.find('[role="progressbar"]').exists()).toBe(false);
    });

    it('shows an indeterminate track while processing without a percentage', () => {
        const wrapper = mount(StatusBar, {
            props: { status: 'Preparing audio...', isProcessing: true, progressPercentage: null },
        });
        expect(wrapper.find('[data-testid="status-progress-indeterminate"]').exists()).toBe(true);
        expect(wrapper.find('[data-testid="status-progress-fill"]').exists()).toBe(false);
    });

    it('lists recent distinct messages when the status is clicked', async () => {
        const wrapper = mount(StatusBar, {
            attachTo: document.body,
            props: { status: 'First', isProcessing: false, progressPercentage: null },
        });
        await wrapper.setProps({ status: 'Second' });
        await wrapper.setProps({ status: 'Second' });
        await wrapper.setProps({ status: 'Third' });

        const toggle = wrapper.get('[data-testid="status-text"]');
        expect(toggle.attributes('aria-expanded')).toBe('false');
        await toggle.trigger('click');
        expect(toggle.attributes('aria-expanded')).toBe('true');

        const items = wrapper.findAll('#status-history li').map((li) => li.text());
        expect(items).toHaveLength(3);
        expect(items[0]).toContain('Third');
        expect(items[0]).toContain('just now');
        expect(items[2]).toContain('First');

        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
        await wrapper.vm.$nextTick();
        expect(wrapper.find('#status-history').exists()).toBe(false);
        wrapper.unmount();
    });

    it('keeps at most 20 history entries', async () => {
        // Letters, not numbers: messages that differ only in numbers collapse.
        const label = (i: number) =>
            `Message ${String.fromCharCode(97 + (i % 26))}${String.fromCharCode(97 + Math.floor(i / 26))}`;
        const wrapper = mount(StatusBar, {
            props: { status: label(0), isProcessing: false, progressPercentage: null },
        });
        for (let i = 1; i < 30; i++) await wrapper.setProps({ status: label(i) });
        await wrapper.get('[data-testid="status-text"]').trigger('click');

        const items = wrapper.findAll('#status-history li');
        expect(items).toHaveLength(20);
        expect(items[0].text()).toContain(label(29));
    });

    it('collapses progress ticks of the same message into one entry', async () => {
        const wrapper = mount(StatusBar, {
            props: { status: 'Loading model', isProcessing: true, progressPercentage: null },
        });
        await wrapper.setProps({ status: 'Step 1/5' });
        await wrapper.setProps({ status: 'Step 2/5' });
        await wrapper.setProps({ status: 'Step 3/5' });
        await wrapper.setProps({ status: 'Transcribing… 12%' });
        await wrapper.setProps({ status: 'Transcribing… 13.5%' });
        await wrapper.get('[data-testid="status-text"]').trigger('click');

        const items = wrapper.findAll('#status-history li').map((li) => li.text());
        expect(items).toHaveLength(3);
        expect(items[0]).toContain('Transcribing… 13.5%');
        expect(items[1]).toContain('Step 3/5');
        expect(items[1]).not.toContain('Step 2/5');
        expect(items[2]).toContain('Loading model');
    });
});
