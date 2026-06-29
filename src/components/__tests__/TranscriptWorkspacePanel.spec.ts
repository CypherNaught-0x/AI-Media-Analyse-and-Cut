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
        :data-segment-count="String(segments.length)"
        :data-show-only-review-segments="String(showOnlyReviewSegments)"
        :data-review-threshold="String(reviewThreshold)"
        :data-blacklist-match-count="String(Object.keys(blacklistMatchesBySegment ?? {}).length)"
        :data-speaker-visibility="JSON.stringify(speakerVisibility ?? {})"
        :data-audio-available="String(audioAvailable)"
        :data-video-available="String(videoAvailable)"
      ></div>
    `,
    props: ['segments', 'speakerVisibility', 'showOnlyReviewSegments', 'reviewThreshold', 'blacklistMatchesBySegment', 'audioAvailable', 'previewIndex', 'videoAvailable', 'videoPreviewIndex'],
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

  it('shows blacklist warnings and passes segment matches to the editor', () => {
    const warningSegments: TranscriptSegment[] = [
      {
        start: '00:00',
        end: '00:04',
        speaker: 'Host',
        text: 'Du Arsch',
        words: [
          { start: '00:00', end: '00:01', text: 'Du' },
          { start: '00:01', end: '00:02', text: 'Arsch' },
        ],
      },
    ];

    const wrapper = mount(TranscriptWorkspacePanel, {
      props: {
        inputPath: '/tmp/audio.mp3',
        hasMediaFile: true,
        displaySegments: warningSegments,
        originalSegments: warningSegments,
        translations: { German: warningSegments },
        currentLanguage: 'German',
        targetLanguage: '',
        isTranslating: false,
        isLlmOnlyBackend: false,
        useAdvancedAlignment: false,
        uniqueSpeakers: ['Host'],
        isProcessing: false,
      },
    });

    expect(wrapper.get('[data-testid="blacklist-warnings"]').text()).toContain('Blacklist Warnings');
    expect(wrapper.text()).toContain('Arsch');
    expect(wrapper.get('.mock-editor').attributes('data-blacklist-match-count')).toBe('1');
  });

  it('toggles individual speaker visibility for transcript segments', async () => {
    const multiSpeakerSegments: TranscriptSegment[] = [
      { start: '00:00', end: '00:05', speaker: 'Host', text: 'Hello world' },
      { start: '00:05', end: '00:10', speaker: 'Guest', text: 'Hi there' },
    ];

    const wrapper = mount(TranscriptWorkspacePanel, {
      props: {
        inputPath: '/tmp/audio.mp3',
        hasMediaFile: true,
        displaySegments: multiSpeakerSegments,
        originalSegments: multiSpeakerSegments,
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

    const hostToggle = wrapper.get('[data-testid="speaker-visibility-toggle-Host"]');
    expect(hostToggle.classes()).toContain('z-10');

    await hostToggle.trigger('click');

    const editor = wrapper.get('.mock-editor');
    expect(editor.attributes('data-speaker-visibility')).toBe(JSON.stringify({ Host: false, Guest: true }));
    expect(wrapper.text()).toContain('1 of 2 Segments');

    await hostToggle.trigger('click');

    expect(editor.attributes('data-speaker-visibility')).toBe(JSON.stringify({ Host: true, Guest: true }));
    expect(wrapper.text()).toContain('2 Segments');
  });

  it('loads the extracted audio stream and enables segment previews when available', () => {
    const wrapper = mount(TranscriptWorkspacePanel, {
      props: {
        inputPath: '/tmp/audio.mp3',
        hasMediaFile: true,
        extractedAudioPath: '/tmp/audio.ogg',
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

    const audio = wrapper.get('[data-testid="extracted-audio"]');
    expect(audio.attributes('src')).toBe('/tmp/audio.ogg');
    expect(wrapper.get('.mock-editor').attributes('data-audio-available')).toBe('true');
    // Video playback buttons are gated on the source media being present.
    expect(wrapper.get('.mock-editor').attributes('data-video-available')).toBe('true');
  });

  it('disables segment video playback when the source media file is missing', () => {
    const wrapper = mount(TranscriptWorkspacePanel, {
      props: {
        inputPath: '/tmp/audio.mp3',
        hasMediaFile: false,
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

    expect(wrapper.get('.mock-editor').attributes('data-video-available')).toBe('false');
  });

  it('hides the extracted audio player and disables previews without an extracted stream', () => {
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

    expect(wrapper.find('[data-testid="extracted-audio"]').exists()).toBe(false);
    expect(wrapper.get('.mock-editor').attributes('data-audio-available')).toBe('false');
  });

  it('shift-click solos a speaker in the transcript', async () => {
    const multiSpeakerSegments: TranscriptSegment[] = [
      { start: '00:00', end: '00:05', speaker: 'Host', text: 'Hello world' },
      { start: '00:05', end: '00:10', speaker: 'Guest', text: 'Hi there' },
      { start: '00:10', end: '00:15', speaker: 'Narrator', text: 'Closing note' },
    ];

    const wrapper = mount(TranscriptWorkspacePanel, {
      props: {
        inputPath: '/tmp/audio.mp3',
        hasMediaFile: true,
        displaySegments: multiSpeakerSegments,
        originalSegments: multiSpeakerSegments,
        translations: {},
        currentLanguage: 'Original',
        targetLanguage: '',
        isTranslating: false,
        isLlmOnlyBackend: false,
        useAdvancedAlignment: false,
        uniqueSpeakers: ['Host', 'Guest', 'Narrator'],
        isProcessing: false,
      },
    });

    await wrapper.get('[data-testid="speaker-visibility-toggle-Guest"]').trigger('click', { shiftKey: true });

    const editor = wrapper.get('.mock-editor');
    expect(editor.attributes('data-speaker-visibility')).toBe(
      JSON.stringify({ Host: false, Guest: true, Narrator: false })
    );
    expect(wrapper.text()).toContain('1 of 3 Segments');
  });
});
