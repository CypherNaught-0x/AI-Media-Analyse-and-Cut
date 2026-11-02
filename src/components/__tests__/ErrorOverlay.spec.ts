import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { ref } from 'vue';
import ErrorOverlay from '../ErrorOverlay.vue';

const mocks = vi.hoisted(() => ({
    save: vi.fn(),
    zipLogs: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({ save: mocks.save }));
vi.mock('../../bindings', () => ({ commands: { zipLogs: mocks.zipLogs } }));
vi.mock('../../composables/useSettings', () => ({
    useSettings: () => ({ settings: ref({ model: 'test-model', baseUrl: 'https://test.url' }) }),
}));

function mountOverlay(props: Record<string, unknown> = {}) {
    return mount(ErrorOverlay, {
        props: { show: true, message: 'Analysis failed.', rawResponse: '', ...props },
        global: { stubs: { teleport: true } },
    });
}

describe('ErrorOverlay.vue', () => {
    const writeText = vi.fn(() => Promise.resolve());

    beforeEach(() => {
        vi.clearAllMocks();
        Object.defineProperty(navigator, 'clipboard', {
            value: { writeText },
            configurable: true,
        });
    });

    it('is a dialog titled for generic failures without an AI response', () => {
        const wrapper = mountOverlay();
        const dialog = wrapper.get('[role="dialog"]');
        expect(dialog.attributes('aria-modal')).toBe('true');
        expect(dialog.text()).toContain('Something went wrong');
        expect(dialog.text()).toContain('Analysis failed.');
        expect(dialog.text()).not.toContain('Raw AI response');
    });

    it('shows the raw response and parse error for AI failures', async () => {
        const wrapper = mountOverlay({
            rawResponse: '{"bad": json',
            parseError: 'Unexpected token',
        });
        expect(wrapper.text()).toContain('AI response error');
        expect(wrapper.text()).toContain('Unexpected token');
        expect(wrapper.get('pre').text()).toBe('{"bad": json');

        await wrapper.get('[data-testid="copy-raw-response"]').trigger('click');
        await flushPromises();
        expect(writeText).toHaveBeenCalledWith('{"bad": json');
        expect(wrapper.get('[data-testid="copy-raw-response"]').text()).toBe('Copied');
    });

    it('dismisses from the footer, the close button and Escape', async () => {
        const wrapper = mountOverlay();
        await wrapper
            .findAll('button')
            .find((b) => b.text() === 'Dismiss')!
            .trigger('click');
        await wrapper.get('button[aria-label="Close"]').trigger('click');
        await wrapper.get('[role="dialog"]').trigger('keydown', { key: 'Escape' });
        expect(wrapper.emitted('dismiss')).toHaveLength(3);
    });

    it('copies all details and reports it', async () => {
        const wrapper = mountOverlay({ rawResponse: 'raw', parseError: 'oops' });
        await wrapper
            .findAll('button')
            .find((b) => b.text() === 'Copy all')!
            .trigger('click');
        await flushPromises();

        const copied = (writeText.mock.calls[0] as unknown as [string])[0];
        expect(copied).toContain('Error: Analysis failed.');
        expect(copied).toContain('Parse Error: oops');
        expect(copied).toContain('Model: test-model');
        expect(wrapper.emitted('update:status')?.[0]).toEqual([
            'Error details copied to clipboard.',
        ]);
    });

    it('exports logs to the chosen zip file', async () => {
        mocks.save.mockResolvedValue('/tmp/logs.zip');
        const wrapper = mountOverlay();
        await wrapper
            .findAll('button')
            .find((b) => b.text() === 'Export logs')!
            .trigger('click');
        await flushPromises();

        expect(mocks.zipLogs).toHaveBeenCalledWith('/tmp/logs.zip');
        expect(wrapper.emitted('update:status')?.[0]).toEqual(['Logs exported successfully.']);
    });

    it('renders nothing when hidden', () => {
        const wrapper = mountOverlay({ show: false });
        expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    });
});
