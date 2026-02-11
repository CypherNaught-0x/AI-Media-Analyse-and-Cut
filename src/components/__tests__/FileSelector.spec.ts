import { describe, it, expect, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import FileSelector from '../FileSelector.vue';

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(() => Promise.resolve('test-file.mp4')),
}));

describe('FileSelector.vue', () => {
  it('renders correctly', () => {
    const wrapper = mount(FileSelector, {
      props: {
        modelValue: '',
      },
    });
    expect(wrapper.find('input').exists()).toBe(true);
    expect(wrapper.find('button').text()).toBe('Browse');
  });

  it('emits update:modelValue when file is selected', async () => {
    const wrapper = mount(FileSelector, {
      props: {
        modelValue: '',
      },
    });
    
    await wrapper.find('button').trigger('click');
    
    // Wait for async open call
    await new Promise(resolve => setTimeout(resolve, 0));
    
    expect(wrapper.emitted('update:modelValue')).toBeTruthy();
    expect(wrapper.emitted('update:modelValue')![0]).toEqual(['test-file.mp4']);
  });
});
