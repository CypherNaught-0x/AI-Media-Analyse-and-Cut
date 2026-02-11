import { describe, it, expect } from 'vitest';
import { mount } from '@vue/test-utils';
import AnalysisSettings from '../AnalysisSettings.vue';

describe('AnalysisSettings.vue', () => {
  it('renders correctly', () => {
    const wrapper = mount(AnalysisSettings, {
      props: {
        context: 'test context',
        glossary: 'test glossary',
        speakerCount: 2,
        removeFillerWords: false,
      },
    });
    
    expect(wrapper.find('textarea').element.value).toBe('test context');
    expect(wrapper.findAll('textarea')[1].element.value).toBe('test glossary');
    expect(wrapper.find('input[type="number"]').element.value).toBe('2');
  });

  it('emits updates', async () => {
    const wrapper = mount(AnalysisSettings, {
      props: {
        context: '',
        glossary: '',
        speakerCount: null,
        removeFillerWords: false,
      },
    });

    await wrapper.find('textarea').setValue('new context');
    expect(wrapper.emitted('update:context')![0]).toEqual(['new context']);

    await wrapper.findAll('textarea')[1].setValue('new glossary');
    expect(wrapper.emitted('update:glossary')![0]).toEqual(['new glossary']);

    await wrapper.find('input[type="number"]').setValue(3);
    expect(wrapper.emitted('update:speakerCount')![0]).toEqual([3]);
    
    await wrapper.find('.cursor-pointer').trigger('click');
    expect(wrapper.emitted('update:removeFillerWords')![0]).toEqual([true]);
  });
});
