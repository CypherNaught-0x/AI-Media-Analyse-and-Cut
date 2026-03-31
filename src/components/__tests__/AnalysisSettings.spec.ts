import { describe, it, expect } from 'vitest';
import { mount } from '@vue/test-utils';
import AnalysisSettings from '../AnalysisSettings.vue';

describe('AnalysisSettings.vue', () => {
  it('renders correctly', () => {
    const wrapper = mount(AnalysisSettings, {
      props: {
        transcriptionBackend: 'llm',
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
    const wrapper = mount(AnalysisSettings, {
      props: {
        transcriptionBackend: 'llm',
        context: '',
        glossary: '',
        speakerCount: null,
        removeFillerWords: false,
        trimSilence: true,
      },
    });

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

  it('emits backend updates', async () => {
    const wrapper = mount(AnalysisSettings, {
      props: {
        transcriptionBackend: 'llm',
        context: '',
        glossary: '',
        speakerCount: null,
        removeFillerWords: false,
        trimSilence: true,
      },
    });

    const parakeetButton = wrapper.findAll('button').find((button) => button.text().includes('Parakeet'));
    await parakeetButton!.trigger('click');

    expect(wrapper.emitted('update:transcriptionBackend')![0]).toEqual(['parakeet']);
  });

  it('keeps speaker count and filler-word cleanup enabled for hybrid merge', async () => {
    const wrapper = mount(AnalysisSettings, {
      props: {
        transcriptionBackend: 'hybrid-merge',
        context: '',
        glossary: '',
        speakerCount: 2,
        removeFillerWords: false,
        trimSilence: true,
      },
    });

    const speakerInput = wrapper.find('input[type="number"]');
    expect(speakerInput.attributes('disabled')).toBeUndefined();

    await speakerInput.setValue(4);
    expect(wrapper.emitted('update:speakerCount')![0]).toEqual([4]);

    await wrapper.findAll('.cursor-pointer')[0].trigger('click');
    expect(wrapper.emitted('update:removeFillerWords')![0]).toEqual([true]);
  });
});
