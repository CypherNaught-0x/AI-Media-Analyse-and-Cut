import { describe, it, expect } from 'vitest';
import { mount } from '@vue/test-utils';
import AnalysisSettings from '../AnalysisSettings.vue';
import type { LocalEngine, TranscriptionBackend } from '../../types';
import { runtimeDownload } from '../../utils/runtimeLicenses';

type Overrides = {
    transcriptionBackend?: TranscriptionBackend;
    localEngine?: LocalEngine;
    removeFillerWords?: boolean;
    speakerCount?: number | null;
};

function mountPanel(overrides: Overrides = {}) {
    return mount(AnalysisSettings, {
        props: {
            transcriptionBackend: 'llm',
            localEngine: 'parakeet',
            context: '',
            glossary: '',
            speakerCount: null,
            removeFillerWords: false,
            trimSilence: true,
            ...overrides,
        },
    });
}

const buttonWithText = (wrapper: ReturnType<typeof mountPanel>, text: string) =>
    wrapper.findAll('button').find((button) => button.text().includes(text));

describe('AnalysisSettings.vue', () => {
    it('renders correctly', () => {
        const wrapper = mount(AnalysisSettings, {
            props: {
                transcriptionBackend: 'llm',
                localEngine: 'parakeet',
                context: 'test context',
                glossary: 'test glossary',
                speakerCount: 2,
                removeFillerWords: false,
                trimSilence: true,
            },
        });

        expect(wrapper.find('textarea').element.value).toBe('test context');
        expect(wrapper.findAll('textarea')[1].element.value).toBe('test glossary');
        expect((wrapper.find('input[type="number"]').element as HTMLInputElement).value).toBe('2');
    });

    it('emits updates', async () => {
        const wrapper = mountPanel();

        await wrapper.find('textarea').setValue('new context');
        expect(wrapper.emitted('update:context')![0]).toEqual(['new context']);

        await wrapper.findAll('textarea')[1].setValue('new glossary');
        expect(wrapper.emitted('update:glossary')![0]).toEqual(['new glossary']);

        await wrapper.find('input[type="number"]').setValue(3);
        expect(wrapper.emitted('update:speakerCount')![0]).toEqual([3]);

        await wrapper.get('[data-testid="remove-filler-words"]').trigger('click');
        expect(wrapper.emitted('update:removeFillerWords')![0]).toEqual([true]);

        await wrapper.get('[data-testid="trim-silence"]').trigger('click');
        expect(wrapper.emitted('update:trimSilence')![0]).toEqual([false]);
    });

    it('emits pipeline updates independently of the engine', async () => {
        const wrapper = mountPanel();

        await buttonWithText(wrapper, 'Speech Engine Only')!.trigger('click');
        expect(wrapper.emitted('update:transcriptionBackend')![0]).toEqual(['local']);

        await buttonWithText(wrapper, 'Hybrid Merge')!.trigger('click');
        expect(wrapper.emitted('update:transcriptionBackend')![1]).toEqual(['hybrid-merge']);

        // Choosing a pipeline must not change the engine.
        expect(wrapper.emitted('update:localEngine')).toBeUndefined();
    });

    it('hides the engine row for the LLM-only pipeline', () => {
        const llm = mountPanel({ transcriptionBackend: 'llm' });
        expect(llm.find('[data-testid="engine-picker"]').exists()).toBe(false);
        expect(llm.text()).not.toContain('Speech engine');
        const local = mountPanel({ transcriptionBackend: 'local' });
        expect(local.find('[data-testid="engine-picker"]').exists()).toBe(true);
        expect(local.text()).toContain('Speech engine');
    });

    it('gives the pipeline and engine cards radio semantics', async () => {
        const wrapper = mountPanel({ transcriptionBackend: 'hybrid', localEngine: 'crisper' });

        const groups = wrapper.findAll('[role="radiogroup"]');
        expect(groups).toHaveLength(2);
        const checked = wrapper
            .findAll('[role="radio"][aria-checked="true"]')
            .map((radio) => radio.text());
        expect(checked).toHaveLength(2);
        expect(checked[0]).toContain('Hybrid Cleanup');
        expect(checked[1]).toContain('CrisperWhisper');

        // Arrow keys move the selection like a native radio group.
        await groups[0].trigger('keydown', { key: 'ArrowRight' });
        expect(wrapper.emitted('update:transcriptionBackend')![0]).toEqual(['hybrid-merge']);

        // Home and End jump to the first and last option.
        await groups[0].trigger('keydown', { key: 'Home' });
        expect(wrapper.emitted('update:transcriptionBackend')![1]).toEqual(['llm']);
        await groups[1].trigger('keydown', { key: 'End' });
        expect(wrapper.emitted('update:localEngine')![0]).toEqual(['cloud']);
        await groups[1].trigger('keydown', { key: 'Home' });
        expect(wrapper.emitted('update:localEngine')![1]).toEqual(['nemotron']);
    });

    it('labels every input and explains that these settings are per project', () => {
        const wrapper = mountPanel({ transcriptionBackend: 'hybrid' });

        expect(wrapper.text()).toContain('this project only');
        for (const selector of ['textarea', 'input[type="number"]']) {
            for (const field of wrapper.findAll(selector)) {
                const id = field.attributes('id');
                expect(id).toBeTruthy();
                expect(wrapper.find(`label[for="${id}"]`).exists()).toBe(true);
            }
        }
        expect(wrapper.get('input[type="number"]').attributes('placeholder')).toBe('Auto');
    });

    it.each<TranscriptionBackend>(['local', 'hybrid', 'hybrid-merge'])(
        'offers both engines for the %s pipeline',
        async (backend) => {
            const wrapper = mountPanel({ transcriptionBackend: backend });

            const crisper = buttonWithText(wrapper, 'CrisperWhisper');
            expect(crisper).toBeTruthy();
            expect(crisper!.text()).toContain('EN / DE');
            expect(buttonWithText(wrapper, 'Parakeet')).toBeTruthy();

            // The engine is selectable for hybrids too, not just the local pipeline.
            await crisper!.trigger('click');
            expect(wrapper.emitted('update:localEngine')![0]).toEqual(['crisper']);
        },
    );

    it('warns about the non-commercial license only while CrisperWhisper is the engine', () => {
        const selected = mountPanel({
            transcriptionBackend: 'local',
            localEngine: 'crisper',
        }).text();
        expect(selected).toContain('Non-commercial use only');
        // The notice comes from the runtime licence register.
        expect(selected).toContain(runtimeDownload('crisperwhisper-weights').notice);

        // The warning follows the engine, and applies to the hybrids as well.
        expect(
            mountPanel({ transcriptionBackend: 'hybrid', localEngine: 'crisper' }).text(),
        ).toContain('Non-commercial use only');
        expect(
            mountPanel({ transcriptionBackend: 'local', localEngine: 'parakeet' }).text(),
        ).not.toContain('Non-commercial use only');
        expect(mountPanel({ transcriptionBackend: 'llm' }).text()).not.toContain(
            'Non-commercial use only',
        );
    });

    it('keeps LLM-only inputs enabled for the hybrid pipelines', async () => {
        const wrapper = mountPanel({ transcriptionBackend: 'hybrid-merge', speakerCount: 2 });

        const speakerInput = wrapper.find('input[type="number"]');
        expect(speakerInput.attributes('disabled')).toBeUndefined();

        await speakerInput.setValue(4);
        expect(wrapper.emitted('update:speakerCount')![0]).toEqual([4]);

        await wrapper.get('[data-testid="remove-filler-words"]').trigger('click');
        expect(wrapper.emitted('update:removeFillerWords')![0]).toEqual([true]);
    });

    it('hides LLM-only inputs for the local pipeline and says why', () => {
        const wrapper = mountPanel({ transcriptionBackend: 'local', localEngine: 'crisper' });

        expect(wrapper.find('[data-testid="llm-assist"]').exists()).toBe(false);
        expect(wrapper.find('input[type="number"]').exists()).toBe(false);
        expect(wrapper.findAll('textarea')).toHaveLength(0);
        expect(wrapper.get('[data-testid="llm-assist-hint"]').text()).toContain(
            'LLM and hybrid pipelines',
        );

        // Nothing to explain when the inputs are in use.
        const hybrid = mountPanel({ transcriptionBackend: 'hybrid' });
        expect(hybrid.find('[data-testid="llm-assist-hint"]').exists()).toBe(false);
        expect(hybrid.findAll('textarea')).toHaveLength(2);
    });

    it('brings the LLM-only inputs back with their values when the pipeline uses an LLM again', async () => {
        const wrapper = mountPanel({ transcriptionBackend: 'local' });
        await wrapper.setProps({ transcriptionBackend: 'hybrid', context: 'kept' } as never);
        expect(wrapper.get('#analysis-context').element).toHaveProperty('value', 'kept');
    });

    it('offers filler removal whenever the engine or an LLM stage can do it', async () => {
        // CrisperWhisper removes fillers itself.
        const crisper = mountPanel({ transcriptionBackend: 'local', localEngine: 'crisper' });
        await crisper.get('[data-testid="remove-filler-words"]').trigger('click');
        expect(crisper.emitted('update:removeFillerWords')![0]).toEqual([true]);

        // So does an LLM cleanup pass, even with Parakeet as the engine.
        const hybrid = mountPanel({ transcriptionBackend: 'hybrid', localEngine: 'parakeet' });
        await hybrid.get('[data-testid="remove-filler-words"]').trigger('click');
        expect(hybrid.emitted('update:removeFillerWords')![0]).toEqual([true]);
    });

    it('disables filler removal for Parakeet alone, which has no such pass', async () => {
        const wrapper = mountPanel({ transcriptionBackend: 'local', localEngine: 'parakeet' });

        const fillerSwitch = wrapper.get('[data-testid="remove-filler-words"]');
        expect(fillerSwitch.attributes('role')).toBe('switch');
        expect(fillerSwitch.attributes('disabled')).toBeDefined();

        await fillerSwitch.trigger('click');
        // Clicking its label does nothing either.
        await wrapper.get(`label[for="${fillerSwitch.attributes('id')}"]`).trigger('click');
        expect(wrapper.emitted('update:removeFillerWords')).toBeUndefined();

        // The row says why instead of silently dimming.
        expect(wrapper.get('[data-testid="remove-filler-words-reason"]').text()).toContain(
            'Parakeet keeps every word',
        );
    });

    it('explains that Apple Speech already drops fillers', () => {
        const wrapper = mountPanel({ transcriptionBackend: 'local', localEngine: 'apple-speech' });
        expect(wrapper.get('[data-testid="remove-filler-words-reason"]').text()).toContain(
            'Apple Speech',
        );
    });

    it('toggles a switch from its label', async () => {
        const wrapper = mountPanel();
        const trim = wrapper.get('[data-testid="trim-silence"]');
        await wrapper.get(`label[for="${trim.attributes('id')}"]`).trigger('click');
        expect(wrapper.emitted('update:trimSilence')![0]).toEqual([false]);
    });
});
