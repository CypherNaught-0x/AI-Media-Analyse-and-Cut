import { describe, it, expect } from 'vitest';
import { mount } from '@vue/test-utils';
import ClipGenerator from '../ClipGenerator.vue';

describe('ClipGenerator.vue', () => {
  it('renders correctly', () => {
    const wrapper = mount(ClipGenerator, {
      props: {
        count: 3,
        minDuration: 10,
        maxDuration: 60,
        topic: '',
        splicing: false,
        isProcessing: false,
      },
    });
    
    expect(wrapper.text()).toContain('Generate Clips');
  });

  it('emits generate event', async () => {
    const wrapper = mount(ClipGenerator, {
      props: {
        count: 3,
        minDuration: 10,
        maxDuration: 60,
        topic: '',
        splicing: false,
        isProcessing: false,
      },
    });

    await wrapper.find('button.bg-gradient-to-r').trigger('click');
    expect(wrapper.emitted('generate')).toBeTruthy();
  });

  it('disables button when processing', async () => {
    const wrapper = mount(ClipGenerator, {
      props: {
        count: 3,
        minDuration: 10,
        maxDuration: 60,
        topic: '',
        splicing: false,
        isProcessing: true,
      },
    });

    expect(wrapper.find('button.bg-gradient-to-r').attributes('disabled')).toBeDefined();
    expect(wrapper.text()).toContain('Processing...');
  });
});
