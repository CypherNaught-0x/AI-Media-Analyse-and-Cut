import { describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import TranscriptWorkspacePanel from '../TranscriptWorkspacePanel.vue';
import type { TranscriptSegment } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string) => path),
}));

vi.mock('../Editor.vue', () => ({
  default: {
    template: '<button class="mock-editor" @click="$emit(\'update:segments\', segments)"></button>',
    props: ['segments'],
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
});
