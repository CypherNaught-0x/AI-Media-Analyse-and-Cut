import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import { ref } from 'vue';
import AnalysisSettings from '../AnalysisSettings.vue';
import type { LocalEngine } from '../../types';

const supported = ref(false);

vi.mock('../../composables/useAppleSpeech', () => ({
    useAppleSpeech: () => ({ supported }),
}));

function mountPanel(localEngine: LocalEngine = 'parakeet') {
    return mount(AnalysisSettings, {
        props: {
            transcriptionBackend: 'local',
            localEngine,
            context: '',
            glossary: '',
            speakerCount: null,
            removeFillerWords: false,
            trimSilence: true,
        },
    });
}

const appleSpeechButton = (wrapper: ReturnType<typeof mountPanel>) =>
    wrapper.findAll('button').find((button) => button.text().includes('Apple Speech'));

describe('AnalysisSettings.vue — Apple Speech engine', () => {
    beforeEach(() => {
        supported.value = false;
    });

    it('is not offered where the platform has no Apple Speech', () => {
        expect(appleSpeechButton(mountPanel())).toBeUndefined();
    });

    it('is offered on macOS builds that include the helper', async () => {
        supported.value = true;
        const wrapper = mountPanel();

        const button = appleSpeechButton(wrapper);
        expect(button).toBeDefined();
        await button!.trigger('click');
        expect(wrapper.emitted('update:localEngine')![0]).toEqual(['apple-speech']);
    });

    it('stays visible when it is the stored choice, so it can be changed', () => {
        expect(appleSpeechButton(mountPanel('apple-speech'))).toBeDefined();
    });
});
