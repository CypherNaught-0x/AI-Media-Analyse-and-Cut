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

  it('emits export event with payload', async () => {
    const wrapper = mount(ClipList, {
      props: {
        clips,
        lastExportPath: '',
        isProcessing: false,
      },
    });

    // Find the export button (it's the first one)
    const exportButton = wrapper.findAll('button')[0];
    await exportButton.trigger('click');
    
    expect(wrapper.emitted('export')).toBeTruthy();
    const eventArgs = wrapper.emitted('export')![0][0] as any;
    expect(eventArgs.clips).toHaveLength(1); // Default is all
    expect(eventArgs.includeSubtitles).toBe(true);
  });

  it('selects clips and exports only selected', async () => {
    const wrapper = mount(ClipList, {
      props: {
        clips: [...clips, { ...clips[0], title: 'Clip 2' }],
        lastExportPath: '',
        isProcessing: false,
      },
    });

    // Select second clip
    const clipItems = wrapper.findAll('.group'); // The clip container has 'group' class
    await clipItems[1].trigger('click');

    // Click export
    const exportButton = wrapper.findAll('button')[0];
    await exportButton.trigger('click');

    const eventArgs = wrapper.emitted('export')![0][0] as any;
    expect(eventArgs.clips).toHaveLength(1);
    expect(eventArgs.clips[0].title).toBe('Clip 2');
  });
});
