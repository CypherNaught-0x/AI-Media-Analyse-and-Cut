import { describe, it, expect, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import Editor from '../Editor.vue';
import type { TranscriptSegment } from '../../types';
import { ask } from '@tauri-apps/plugin-dialog';

// Mock Tauri dialog plugin
vi.mock('@tauri-apps/plugin-dialog', () => ({
  ask: vi.fn(() => Promise.resolve(true)),
}));

describe('Editor.vue', () => {
  const mockSegments: TranscriptSegment[] = [
    { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'Hello world' },
    { start: '00:10', end: '00:20', speaker: 'Speaker 2', text: 'How are you?' },
  ];

  it('renders segments correctly', () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
      },
    });

    const segments = wrapper.findAll('.segment');
    expect(segments).toHaveLength(2);
    expect(wrapper.text()).toContain('Hello world');
    expect(wrapper.text()).toContain('How are you?');
  });

  it('emits jump-to event when clicking on timestamp', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
      },
    });

    await wrapper.find('.segment .cursor-pointer').trigger('click');
    expect(wrapper.emitted('jump-to')).toBeTruthy();
    expect(wrapper.emitted('jump-to')![0]).toEqual([0]); // 00:00 is 0 seconds
  });

  it('enters edit mode when edit button is clicked', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
      },
    });

    // Find the edit button (it's hidden by default, but we can still trigger it or check existence)
    // The button has text "Edit"
    const editButton = wrapper.findAll('button').find(b => b.text() === 'Edit');
    expect(editButton).toBeDefined();
    
    await editButton!.trigger('click');
    
    // Check if inputs appear
    expect(wrapper.find('input[placeholder="MM:SS"]').exists()).toBe(true);
    expect(wrapper.find('textarea').exists()).toBe(true);
  });

  it('saves edits correctly', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
      },
    });

    const editButton = wrapper.findAll('button').find(b => b.text() === 'Edit');
    await editButton!.trigger('click');

    const textarea = wrapper.find('textarea');
    await textarea.setValue('Hello updated world');

    const saveButton = wrapper.findAll('button').find(b => b.text() === 'Save Changes');
    await saveButton!.trigger('click');

    expect(wrapper.emitted('update:segments')).toBeTruthy();
    const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updatedSegments[0].text).toBe('Hello updated world');
  });

  it('deletes a segment', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
      },
    });

    const deleteButton = wrapper.findAll('button').find(b => b.text() === 'Del');
    await deleteButton!.trigger('click');

    expect(ask).toHaveBeenCalled();
    expect(wrapper.emitted('update:segments')).toBeTruthy();
    const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updatedSegments).toHaveLength(1);
    expect(updatedSegments[0].text).toBe('How are you?');
  });

  it('merges segments', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
      },
    });

    const mergeButton = wrapper.findAll('button').find(b => b.text() === 'Merge ↓');
    await mergeButton!.trigger('click');

    expect(wrapper.emitted('update:segments')).toBeTruthy();
    const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updatedSegments).toHaveLength(1);
    expect(updatedSegments[0].text).toBe('Hello world How are you?');
  });
});
