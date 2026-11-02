import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import HomeSourcePanel from '../HomeSourcePanel.vue';
import { resetWorkspaceNav, useWorkspaceNav } from '../../composables/useWorkspaceNav';

vi.mock('../FileSelector.vue', () => ({
    default: {
        props: ['modelValue', 'missing'],
        template: '<div class="mock-file-selector" :data-missing="String(missing)" />',
    },
}));
vi.mock('../AnalysisSettings.vue', () => ({
    default: { template: '<div class="mock-analysis-settings" />' },
}));

const baseProps = {
    currentEngineLabel: 'Current model',
    currentModelDisplay: 'gemini-2.5-flash',
    inputPath: '/videos/talk.mp4',
    hasMediaFile: true,
    isProcessing: false,
    hasBackendConfiguration: true,
    hasTranscript: false,
    settingsChanged: false,
    transcriptionBackend: 'llm' as const,
    localEngine: 'parakeet' as const,
    context: '',
    glossary: '',
    speakerCount: null,
    removeFillerWords: false,
    trimSilence: true,
};

function mountPanel(props: Partial<typeof baseProps> = {}) {
    return mount(HomeSourcePanel, { props: { ...baseProps, ...props } });
}

describe('HomeSourcePanel.vue', () => {
    beforeEach(() => resetWorkspaceNav());

    it('shows the page header and the current pipeline', async () => {
        const wrapper = mountPanel();
        expect(wrapper.get('h1').text()).toBe('Source');
        expect(wrapper.get('[data-testid="pipeline-card"]').text()).toContain('gemini-2.5-flash');

        await wrapper
            .findAll('button')
            .find((b) => b.text() === 'Change defaults')!
            .trigger('click');
        expect(wrapper.emitted('open-settings')).toHaveLength(1);
    });

    it('analyzes when media and configuration are ready', async () => {
        const wrapper = mountPanel();
        const analyze = wrapper.get('[data-testid="analyze-button"]');
        expect(analyze.text()).toBe('Analyze media');
        expect(analyze.attributes('disabled')).toBeUndefined();
        await analyze.trigger('click');
        expect(wrapper.emitted('process')).toHaveLength(1);
    });

    it.each([
        [{ inputPath: '', hasMediaFile: false }, 'Choose a media file to analyze.'],
        [{ hasMediaFile: false }, 'The media file is missing. Locate it to analyze.'],
        [
            { hasBackendConfiguration: false },
            'This pipeline needs an API key. Add one in Settings.',
        ],
    ])('explains why analyze is disabled (%o)', (props, reason) => {
        const wrapper = mountPanel(props);
        const analyze = wrapper.get('[data-testid="analyze-button"]');
        expect(analyze.attributes('disabled')).toBeDefined();
        expect(analyze.attributes('aria-describedby')).toBe('analyze-note');
        expect(wrapper.get('#analyze-note').text()).toBe(reason);
    });

    it('passes the missing-media state to the file selector', () => {
        const wrapper = mountPanel({ hasMediaFile: false });
        expect(wrapper.get('.mock-file-selector').attributes('data-missing')).toBe('true');
        expect(mountPanel().get('.mock-file-selector').attributes('data-missing')).toBe('false');
    });

    it('offers to open the transcript instead of a dead button when nothing changed', async () => {
        useWorkspaceNav().state.hasTranscript = true;
        const wrapper = mountPanel({ hasTranscript: true, settingsChanged: false });

        expect(wrapper.find('[data-testid="analyze-button"]').exists()).toBe(false);
        expect(wrapper.get('[data-testid="open-transcript"]').attributes('aria-describedby')).toBe(
            'analyze-note',
        );
        await wrapper.get('[data-testid="open-transcript"]').trigger('click');
        expect(useWorkspaceNav().state.activeStep).toBe('transcript');
        expect(wrapper.emitted('process')).toBeUndefined();
    });

    it('re-analyzes once an option changed', async () => {
        const wrapper = mountPanel({ hasTranscript: true, settingsChanged: true });
        const analyze = wrapper.get('[data-testid="analyze-button"]');
        expect(analyze.text()).toBe('Re-analyze media');
        expect(wrapper.get('#analyze-note').text()).toContain('replaces the current transcript');
        await analyze.trigger('click');
        expect(wrapper.emitted('process')).toHaveLength(1);
    });

    it('shows a neutral busy state while any task is running', () => {
        const wrapper = mountPanel({ isProcessing: true, hasTranscript: true });
        const analyze = wrapper.get('[data-testid="analyze-button"]');
        expect(analyze.text()).toBe('Working…');
        expect(analyze.attributes('disabled')).toBeDefined();
        expect(wrapper.get('#analyze-note').text()).toBe(
            'Another task is running. Follow it in the activity bar below.',
        );
    });

    it('relays session actions', async () => {
        const wrapper = mountPanel();
        await wrapper
            .findAll('button')
            .find((b) => b.text() === 'Save session')!
            .trigger('click');
        await wrapper
            .findAll('button')
            .find((b) => b.text() === 'Load session')!
            .trigger('click');
        expect(wrapper.emitted('save-session')).toHaveLength(1);
        expect(wrapper.emitted('load-session')).toHaveLength(1);
    });
});
