import { describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import TranscriptWorkspacePanel from '../TranscriptWorkspacePanel.vue';
import type { TranscriptSegment } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string) => path),
}));

vi.mock('../Editor.vue', () => ({
  default: {
    template: `
      <div
        class="mock-editor"
        :data-show-only-review-segments="String(showOnlyReviewSegments)"
        :data-review-threshold="String(reviewThreshold)"
      ></div>
    `,
    props: ['segments', 'showOnlyReviewSegments', 'reviewThreshold'],
  },
}));

vi.mock('../SubtitleExport.vue', () => ({
  default: {
    template: '<div class="mock-subtitle-export"></div>',
  },
}));

describe('TranscriptWorkspacePanel', () => {
  const segments: TranscriptSegment[] = [
    { start: '00:00', end: '00:05', speaker: 'Host', text: 'Hello world' },
  ];

  it('emits translation target updates and translate requests', async () => {
    const wrapper = mount(TranscriptWorkspacePanel, {
      props: {
        inputPath: '/tmp/audio.mp3',
        hasMediaFile: true,
        displaySegments: segments,
        originalSegments: segments,
        translations: {},
        currentLanguage: 'Original',
        targetLanguage: '',
        isTranslating: false,
        isLlmOnlyBackend: true,
        useAdvancedAlignment: false,
        uniqueSpeakers: ['Host'],
        isProcessing: false,
      },
    });

    const buttons = wrapper.findAll('button');
    await buttons[0].trigger('click');
    const germanOption = wrapper.findAll('button').find((button) => button.text().includes('German'));
    expect(germanOption).toBeDefined();
    await germanOption!.trigger('click');

    expect(wrapper.emitted('update:targetLanguage')?.[0]).toEqual(['German']);
    await wrapper.setProps({ targetLanguage: 'German' });

    const translateButton = wrapper.findAll('button').find((button) => button.attributes('title') === 'Translate');
    await translateButton!.trigger('click');
    expect(wrapper.emitted('translate')).toBeTruthy();
  });

  it('passes review filter state through to the editor', async () => {
    const reviewSegments: TranscriptSegment[] = [
      { start: '00:00', end: '00:05', speaker: 'Host', text: 'Aligned', mergeStatus: 'matched', similarityScore: 0.95 },
      { start: '00:05', end: '00:10', speaker: 'Guest', text: 'Needs review', mergeStatus: 'conflict', similarityScore: 0.42 },
    ];

    const wrapper = mount(TranscriptWorkspacePanel, {
      props: {
        inputPath: '/tmp/audio.mp3',
        hasMediaFile: true,
        displaySegments: reviewSegments,
        originalSegments: reviewSegments,
        translations: {},
        currentLanguage: 'Original',
        targetLanguage: '',
        isTranslating: false,
        isLlmOnlyBackend: false,
        useAdvancedAlignment: false,
        uniqueSpeakers: ['Host', 'Guest'],
        isProcessing: false,
      },
    });

    const toggle = wrapper.get('[data-testid="review-filter-toggle"]');
    await toggle.setValue(true);

    const threshold = wrapper.get('[data-testid="review-filter-threshold"]');
    await threshold.setValue('70');

    const editor = wrapper.get('.mock-editor');
    expect(editor.attributes('data-show-only-review-segments')).toBe('true');
    expect(editor.attributes('data-review-threshold')).toBe('0.7');
  });
});
