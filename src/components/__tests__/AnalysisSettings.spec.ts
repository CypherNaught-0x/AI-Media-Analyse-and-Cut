import { describe, it, expect } from 'vitest';
import { mount } from '@vue/test-utils';
import AnalysisSettings from '../AnalysisSettings.vue';
import type { LocalEngine, TranscriptionBackend } from '../../types';

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
    expect(wrapper.find('input[type="number"]').element.value).toBe('2');
  });

  it('emits updates', async () => {
    const wrapper = mountPanel();

    await wrapper.find('textarea').setValue('new context');
    expect(wrapper.emitted('update:context')![0]).toEqual(['new context']);

    await wrapper.findAll('textarea')[1].setValue('new glossary');
    expect(wrapper.emitted('update:glossary')![0]).toEqual(['new glossary']);

    await wrapper.find('input[type="number"]').setValue(3);
    expect(wrapper.emitted('update:speakerCount')![0]).toEqual([3]);

    await wrapper.findAll('.cursor-pointer')[0].trigger('click');
    expect(wrapper.emitted('update:removeFillerWords')![0]).toEqual([true]);

    await wrapper.findAll('.cursor-pointer')[1].trigger('click');
    expect(wrapper.emitted('update:trimSilence')![0]).toEqual([false]);
  });

  it('emits pipeline updates independently of the engine', async () => {
    const wrapper = mountPanel();

    await buttonWithText(wrapper, 'Local Only')!.trigger('click');
    expect(wrapper.emitted('update:transcriptionBackend')![0]).toEqual(['local']);

    await buttonWithText(wrapper, 'Hybrid Merge')!.trigger('click');
    expect(wrapper.emitted('update:transcriptionBackend')![1]).toEqual(['hybrid-merge']);

    // Choosing a pipeline must not change the engine.
    expect(wrapper.emitted('update:localEngine')).toBeUndefined();
  });

  it('hides the engine row for the LLM-only pipeline', () => {
    expect(mountPanel({ transcriptionBackend: 'llm' }).text()).not.toContain('Local Engine');
    expect(mountPanel({ transcriptionBackend: 'local' }).text()).toContain('Local Engine');
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
    const selected = mountPanel({ transcriptionBackend: 'local', localEngine: 'crisper' }).text();
    expect(selected).toContain('Non-commercial use only');
    expect(selected).toContain('Non-Commercial Research License');
    expect(selected).toContain('commercial use requires a license');

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

    await wrapper.findAll('.cursor-pointer')[0].trigger('click');
    expect(wrapper.emitted('update:removeFillerWords')![0]).toEqual([true]);
  });

  it('disables LLM-only inputs for the local pipeline', () => {
    const wrapper = mountPanel({ transcriptionBackend: 'local', localEngine: 'crisper' });

    expect(wrapper.find('input[type="number"]').attributes('disabled')).toBeDefined();
    expect(wrapper.findAll('textarea')[0].attributes('disabled')).toBeDefined();
  });

  it('offers filler removal whenever the engine or an LLM stage can do it', async () => {
    // CrisperWhisper removes fillers itself.
    const crisper = mountPanel({ transcriptionBackend: 'local', localEngine: 'crisper' });
    await crisper.findAll('.cursor-pointer')[0].trigger('click');
    expect(crisper.emitted('update:removeFillerWords')![0]).toEqual([true]);

    // So does an LLM cleanup pass, even with Parakeet as the engine.
    const hybrid = mountPanel({ transcriptionBackend: 'hybrid', localEngine: 'parakeet' });
    await hybrid.findAll('.cursor-pointer')[0].trigger('click');
    expect(hybrid.emitted('update:removeFillerWords')![0]).toEqual([true]);
  });

  it('disables filler removal for Parakeet alone, which has no such pass', async () => {
    const wrapper = mountPanel({ transcriptionBackend: 'local', localEngine: 'parakeet' });

    const fillerCard = wrapper
      .findAll('div')
      .find((node) => node.text().trim() === 'Remove Filler Words');
    expect(fillerCard).toBeTruthy();
    expect(fillerCard!.classes()).toContain('cursor-not-allowed');

    await fillerCard!.trigger('click');
    expect(wrapper.emitted('update:removeFillerWords')).toBeUndefined();
  });
});
