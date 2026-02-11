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
      },
    });
    
    expect(wrapper.find('.bg-blue-500').exists()).toBe(true);
    expect(wrapper.find('.bg-blue-500').attributes('style')).toContain('width: 50%');
  });
});
