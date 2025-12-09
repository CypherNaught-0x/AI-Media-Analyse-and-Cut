import { describe, it, expect } from 'vitest';
import { mount } from '@vue/test-utils';
import ClipList from '../ClipList.vue';

describe('ClipList.vue', () => {
  const clips = [
    {
      title: 'Test Clip',
      segments: [{ start: '00:00:00', end: '00:00:10' }],
      reason: 'Test reason',
    },
  ];

  it('renders clips', () => {
    const wrapper = mount(ClipList, {
      props: {
        clips,
        lastExportPath: '',
        isProcessing: false,
      },
    });
    
    expect(wrapper.text()).toContain('Test Clip');
    expect(wrapper.text()).toContain('Test reason');
  });

  it('emits export event', async () => {
    const wrapper = mount(ClipList, {
      props: {
        clips,
        lastExportPath: '',
        isProcessing: false,
      },
    });

    await wrapper.find('button').trigger('click');
    expect(wrapper.emitted('export')).toBeTruthy();
  });
});
