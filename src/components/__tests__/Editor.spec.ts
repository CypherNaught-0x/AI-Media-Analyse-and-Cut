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

  it('does not control playback when a segment body is clicked', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        videoAvailable: true,
        audioAvailable: true,
      },
    });

    await wrapper.find('.segment').trigger('click');

    expect(wrapper.emitted('jump-to')).toBeFalsy();
    expect(wrapper.emitted('preview')).toBeFalsy();
    expect(wrapper.emitted('preview-video')).toBeFalsy();
  });

  it('emits jump-to from the "start from here" button using the segment start', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        videoAvailable: true,
      },
    });

    await wrapper.get('[data-testid="segment-play-from-1"]').trigger('click');

    expect(wrapper.emitted('jump-to')).toBeTruthy();
    expect(wrapper.emitted('jump-to')![0]).toEqual([10]); // 00:10 is 10 seconds
  });

  it('emits a video preview request with the segment bounds', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        videoAvailable: true,
      },
    });

    await wrapper.get('[data-testid="segment-play-video-0"]').trigger('click');

    expect(wrapper.emitted('preview-video')).toBeTruthy();
    expect(wrapper.emitted('preview-video')![0]).toEqual([{ start: '00:00', end: '00:10', index: 0 }]);
    expect(wrapper.emitted('jump-to')).toBeFalsy();
  });

  it('hides the video playback buttons when no media file is available', () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        videoAvailable: false,
      },
    });

    expect(wrapper.find('[data-testid="segment-play-video-0"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="segment-play-from-0"]').exists()).toBe(false);
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

  it('keeps the segment header and playback buttons visible while editing', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        audioAvailable: true,
        videoAvailable: true,
      },
    });

    const editButton = wrapper.findAll('button').find(b => b.text() === 'Edit');
    await editButton!.trigger('click');

    // The edit form is shown...
    expect(wrapper.find('textarea').exists()).toBe(true);
    // ...but the header info and per-segment playback buttons remain.
    expect(wrapper.text()).toContain('Speaker 1');
    expect(wrapper.find('[data-testid="segment-preview-0"]').exists()).toBe(true);
    expect(wrapper.find('[data-testid="segment-play-video-0"]').exists()).toBe(true);
    expect(wrapper.find('[data-testid="segment-play-from-0"]').exists()).toBe(true);
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

  it('filters to segments that require review below the selected threshold', () => {
    const reviewSegments: TranscriptSegment[] = [
      { start: '00:00', end: '00:05', speaker: 'Speaker 1', text: 'Aligned', mergeStatus: 'matched', similarityScore: 0.96 },
      { start: '00:05', end: '00:10', speaker: 'Speaker 2', text: 'Needs review', mergeStatus: 'matched', similarityScore: 0.62 },
      { start: '00:10', end: '00:15', speaker: 'Speaker 3', text: 'Missing in Google', mergeStatus: 'missing_google' },
    ];

    const wrapper = mount(Editor, {
      props: {
        segments: reviewSegments,
        showOnlyReviewSegments: true,
        reviewThreshold: 0.8,
      },
    });

    const segments = wrapper.findAll('.segment');
    expect(segments).toHaveLength(2);
    expect(wrapper.text()).toContain('Needs review');
    expect(wrapper.text()).toContain('Missing in Google');
    expect(segments[0].text()).toContain('Needs review');
    expect(segments[1].text()).toContain('Missing in Google');
  });

  it('treats blacklist hits as review items and renders a warning badge', () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        showOnlyReviewSegments: true,
        blacklistMatchesBySegment: {
          1: [
            {
              languageCode: 'de',
              matchedText: 'Arsch',
              normalizedWord: 'arsch',
              segmentIndex: 1,
              speaker: 'Speaker 2',
              start: '00:10',
              end: '00:11',
              segmentText: 'How are you?',
            },
          ],
        },
      },
    });

    const segments = wrapper.findAll('.segment');
    expect(segments).toHaveLength(1);
    expect(segments[0].text()).toContain('blacklist match');
    expect(segments[0].text()).toContain('Matched words: Arsch');
  });

  it('does not show higher-similarity conflict segments below a stricter threshold', () => {
    const reviewSegments: TranscriptSegment[] = [
      {
        start: '00:00',
        end: '00:05',
        speaker: 'Speaker 1',
        text: 'Conflict but acceptable',
        mergeStatus: 'conflict',
        similarityScore: 0.72,
      },
      {
        start: '00:05',
        end: '00:10',
        speaker: 'Speaker 2',
        text: 'Actually low confidence',
        mergeStatus: 'conflict',
        similarityScore: 0.41,
      },
      {
        start: '00:10',
        end: '00:15',
        speaker: 'Speaker 3',
        text: 'Missing in Google',
        mergeStatus: 'missing_google',
      },
    ];

    const wrapper = mount(Editor, {
      props: {
        segments: reviewSegments,
        showOnlyReviewSegments: true,
        reviewThreshold: 0.5,
      },
    });

    const segments = wrapper.findAll('.segment');
    expect(segments).toHaveLength(2);
    expect(wrapper.text()).toContain('Actually low confidence');
    expect(wrapper.text()).toContain('Missing in Google');
    expect(wrapper.text()).not.toContain('Conflict but acceptable');
  });

  it('edits the correct original segment while filtered', async () => {
    const reviewSegments: TranscriptSegment[] = [
      { start: '00:00', end: '00:05', speaker: 'Speaker 1', text: 'Aligned', mergeStatus: 'matched', similarityScore: 0.96 },
      { start: '00:05', end: '00:10', speaker: 'Speaker 2', text: 'Needs review', mergeStatus: 'matched', similarityScore: 0.62 },
      { start: '00:10', end: '00:15', speaker: 'Speaker 3', text: 'Also aligned', mergeStatus: 'matched', similarityScore: 0.92 },
    ];

    const wrapper = mount(Editor, {
      props: {
        segments: reviewSegments,
        showOnlyReviewSegments: true,
        reviewThreshold: 0.8,
      },
    });

    const editButton = wrapper.findAll('button').find((button) => button.text() === 'Edit');
    await editButton!.trigger('click');

    const textarea = wrapper.find('textarea');
    await textarea.setValue('Updated reviewed segment');

    const saveButton = wrapper.findAll('button').find((button) => button.text() === 'Save Changes');
    await saveButton!.trigger('click');

    const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updatedSegments[0].text).toBe('Aligned');
    expect(updatedSegments[1].text).toBe('Updated reviewed segment');
    expect(updatedSegments[2].text).toBe('Also aligned');
  });

  it('selects transcript alternatives against the original segment while filtered', async () => {
    const reviewSegments: TranscriptSegment[] = [
      { start: '00:00', end: '00:05', speaker: 'Speaker 1', text: 'Aligned', mergeStatus: 'matched', similarityScore: 0.96 },
      {
        start: '00:05',
        end: '00:10',
        speaker: 'Speaker 2',
        text: 'Parakeet text',
        mergeStatus: 'conflict',
        similarityScore: 0.51,
        alternatives: [
          { source: 'google', text: 'Google text', speaker: 'Named Speaker' },
          { source: 'parakeet', text: 'Parakeet text', speaker: 'Speaker 2' },
        ],
      },
    ];

    const wrapper = mount(Editor, {
      props: {
        segments: reviewSegments,
        showOnlyReviewSegments: true,
        reviewThreshold: 0.8,
      },
    });

    const useButton = wrapper.findAll('button').find((button) => button.text() === 'Use');
    await useButton!.trigger('click');

    const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updatedSegments[1].text).toBe('Google text');
    expect(updatedSegments[1].speaker).toBe('Named Speaker');
    expect(updatedSegments[1].activeSource).toBe('google');
  });

  it('replaces all matches in visible transcript segments', async () => {
    const segments: TranscriptSegment[] = [
      { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'world news world' },
      { start: '00:10', end: '00:20', speaker: 'Speaker 2', text: 'around the world' },
    ];

    const wrapper = mount(Editor, {
      props: {
        segments,
      },
    });

    await wrapper.get('[data-testid="editor-search-input"]').setValue('world');
    await wrapper.get('[data-testid="editor-replace-input"]').setValue('planet');
    await wrapper.get('[data-testid="editor-replace-all"]').trigger('click');

    const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updatedSegments[0].text).toBe('planet news planet');
    expect(updatedSegments[1].text).toBe('around the planet');
  });

  it('supports optional whole-word matching for replace all', async () => {
    const segments: TranscriptSegment[] = [
      { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'test contest testing' },
      { start: '00:10', end: '00:20', speaker: 'Speaker 2', text: 'another test case' },
    ];

    const wrapper = mount(Editor, {
      props: {
        segments,
      },
    });

    await wrapper.get('[data-testid="editor-search-input"]').setValue('test');
    await wrapper.get('[data-testid="editor-replace-input"]').setValue('exam');
    await wrapper.get('[data-testid="editor-whole-word-toggle"]').setValue(true);
    await wrapper.get('[data-testid="editor-replace-all"]').trigger('click');

    const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updatedSegments[0].text).toBe('exam contest testing');
    expect(updatedSegments[1].text).toBe('another exam case');
  });

  it('replaces the correct original segment while filtered', async () => {
    const reviewSegments: TranscriptSegment[] = [
      { start: '00:00', end: '00:05', speaker: 'Speaker 1', text: 'Aligned', mergeStatus: 'matched', similarityScore: 0.96 },
      { start: '00:05', end: '00:10', speaker: 'Speaker 2', text: 'Needs review review', mergeStatus: 'matched', similarityScore: 0.62 },
      { start: '00:10', end: '00:15', speaker: 'Speaker 3', text: 'Also aligned', mergeStatus: 'matched', similarityScore: 0.92 },
    ];

    const wrapper = mount(Editor, {
      props: {
        segments: reviewSegments,
        showOnlyReviewSegments: true,
        reviewThreshold: 0.8,
      },
    });

    await wrapper.get('[data-testid="editor-search-input"]').setValue('review');
    await wrapper.get('[data-testid="editor-replace-input"]').setValue('checked');
    await wrapper.get('[data-testid="editor-replace-all"]').trigger('click');

    const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updatedSegments[0].text).toBe('Aligned');
    expect(updatedSegments[1].text).toBe('Needs checked checked');
    expect(updatedSegments[2].text).toBe('Also aligned');
  });

  it('emits a preview request with the segment timestamps when audio is available', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        audioAvailable: true,
      },
    });

    const previewButton = wrapper.get('[data-testid="segment-preview-1"]');
    await previewButton.trigger('click');

    expect(wrapper.emitted('preview')).toBeTruthy();
    expect(wrapper.emitted('preview')![0]).toEqual([{ start: '00:10', end: '00:20', index: 1 }]);
    // Previewing must not double as a jump-to seek on the segment body.
    expect(wrapper.emitted('jump-to')).toBeFalsy();
  });

  it('hides the preview button when no extracted audio is available', () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        audioAvailable: false,
      },
    });

    expect(wrapper.find('[data-testid="segment-preview-0"]').exists()).toBe(false);
  });

  it('splits a segment at the chosen point, partitioning word-timed text', async () => {
    const wordSegments: TranscriptSegment[] = [
      {
        start: '00:00',
        end: '00:10',
        speaker: 'Speaker 1',
        text: 'hello world',
        words: [
          { start: '00:00', end: '00:01', text: 'hello' },
          { start: '00:05', end: '00:06', text: 'world' },
        ],
      },
    ];

    const wrapper = mount(Editor, { props: { segments: wordSegments } });

    await wrapper.get('[data-testid="segment-split-0"]').trigger('click');
    await wrapper.get('[data-testid="split-time-input"]').setValue(3);
    await wrapper.get('[data-testid="confirm-split"]').trigger('click');

    const updated = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updated).toHaveLength(2);

    expect(updated[0].start).toBe('00:00');
    expect(updated[0].end).toBe('00:03.000');
    expect(updated[0].text).toBe('hello');
    expect(updated[0].words).toEqual([{ start: '00:00', end: '00:01', text: 'hello' }]);

    expect(updated[1].start).toBe('00:03.000');
    expect(updated[1].end).toBe('00:10');
    expect(updated[1].text).toBe('world');
    expect(updated[1].speaker).toBe('Speaker 1');
  });

  it('assigns a new speaker to the second half when splitting', async () => {
    const wrapper = mount(Editor, { props: { segments: mockSegments } });

    await wrapper.get('[data-testid="segment-split-0"]').trigger('click');
    await wrapper.get('[data-testid="split-time-input"]').setValue(5);
    await wrapper.get('[data-testid="split-second-speaker"]').setValue('Speaker 2');
    await wrapper.get('[data-testid="confirm-split"]').trigger('click');

    const updated = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
    expect(updated).toHaveLength(3);
    expect(updated[0].speaker).toBe('Speaker 1');
    expect(updated[1].start).toBe('00:05.000');
    expect(updated[1].speaker).toBe('Speaker 2');
    // The untouched segment is preserved after the split pair.
    expect(updated[2].text).toBe('How are you?');
  });

  it('disables the split confirmation when the point is at a boundary', async () => {
    const wrapper = mount(Editor, { props: { segments: mockSegments } });

    await wrapper.get('[data-testid="segment-split-0"]').trigger('click');
    await wrapper.get('[data-testid="split-time-input"]').setValue(0); // equal to start

    expect(wrapper.get('[data-testid="confirm-split"]').attributes('disabled')).toBeDefined();
  });

  it('captures the playback position as the split point', async () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        getPlayhead: () => 4.2,
      },
    });

    await wrapper.get('[data-testid="segment-split-0"]').trigger('click');
    await wrapper.get('[data-testid="split-use-playhead"]').trigger('click');

    expect(wrapper.get('[data-testid="split-time-display"]').text()).toBe('00:04.200');
  });

  it('hides segments for speakers marked invisible', () => {
    const wrapper = mount(Editor, {
      props: {
        segments: mockSegments,
        speakerVisibility: {
          'Speaker 1': false,
          'Speaker 2': true,
        },
      },
    });

    const segments = wrapper.findAll('.segment');
    expect(segments).toHaveLength(1);
    expect(wrapper.text()).not.toContain('Hello world');
    expect(wrapper.text()).toContain('How are you?');
  });
});
