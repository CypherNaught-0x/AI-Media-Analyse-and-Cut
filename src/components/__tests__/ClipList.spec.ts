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
        hasMediaFile: true,
        includeSubtitles: true,
        fastMode: true,
        trimBoundarySilence: false,
        selectedClipIndices: [],
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
        hasMediaFile: true,
        includeSubtitles: true,
        fastMode: true,
        trimBoundarySilence: false,
        selectedClipIndices: [],
      },
    });

    // Find the export button (it's the first one)
    const exportButton = wrapper.findAll('button')[0];
    await exportButton.trigger('click');
    
    expect(wrapper.emitted('export')).toBeTruthy();
    const eventArgs = wrapper.emitted('export')![0][0] as any;
    expect(eventArgs.clips).toHaveLength(1); // Default is all
    expect(eventArgs.includeSubtitles).toBe(true);
    expect(eventArgs.fastMode).toBe(true);
    expect(eventArgs.trimBoundarySilence).toBe(false);
  });

  it('selects clips and exports only selected', async () => {
    const wrapper = mount(ClipList, {
      props: {
        clips: [...clips, { ...clips[0], title: 'Clip 2' }],
        lastExportPath: '',
        isProcessing: false,
        hasMediaFile: true,
        includeSubtitles: true,
        fastMode: true,
        trimBoundarySilence: false,
        selectedClipIndices: [],
      },
    });

    // Select second clip
    const clipItems = wrapper.findAll('.group'); // The clip container has 'group' class
    await clipItems[1].trigger('click');
    const selectedEventArgs = wrapper.emitted('update:selectedClipIndices')![0][0] as number[];
    await wrapper.setProps({ selectedClipIndices: selectedEventArgs });

    // Click export
    const exportButton = wrapper.findAll('button')[0];
    await exportButton.trigger('click');

    const eventArgs = wrapper.emitted('export')![0][0] as any;
    expect(eventArgs.clips).toHaveLength(1);
    expect(eventArgs.clips[0].title).toBe('Clip 2');
  });

  it('includes silence trimming when the toggle is enabled', async () => {
    const wrapper = mount(ClipList, {
      props: {
        clips,
        lastExportPath: '',
        isProcessing: false,
        hasMediaFile: true,
        includeSubtitles: true,
        fastMode: true,
        trimBoundarySilence: false,
        selectedClipIndices: [],
      },
    });

    await wrapper.get('[data-testid=\"trim-boundary-silence\"]').setValue(true);
    const trimEventArgs = wrapper.emitted('update:trimBoundarySilence')![0][0] as boolean;
    await wrapper.setProps({ trimBoundarySilence: trimEventArgs });

    const exportButton = wrapper.findAll('button')[0];
    await exportButton.trigger('click');

    const eventArgs = wrapper.emitted('export')![0][0] as any;
    expect(eventArgs.trimBoundarySilence).toBe(true);
  });
});
