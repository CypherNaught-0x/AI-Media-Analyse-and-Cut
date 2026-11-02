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
        :data-playhead="String(playhead)"
        :data-speaker-order="JSON.stringify(speakerOrder ?? [])"
      ><slot name="status" /><slot name="toolbar" /></div>
    `,
        props: [
            'segments',
            'speakerVisibility',
            'showOnlyReviewSegments',
            'reviewThreshold',
            'blacklistMatchesBySegment',
            'audioAvailable',
            'previewIndex',
            'videoAvailable',
            'videoPreviewIndex',
            'getPlayhead',
            'playhead',
            'speakerOrder',
        ],
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
                uniqueSpeakers: ['Host'],
                isProcessing: false,
            },
        });

        // The language controls live in the editor's sticky toolbar slot.
        await wrapper.get('[data-testid="target-language-trigger"]').trigger('click');
        const germanOption = wrapper
            .findAll('button')
            .find((button) => button.text().includes('German'));
        expect(germanOption).toBeDefined();
        await germanOption!.trigger('click');

        expect(wrapper.emitted('update:targetLanguage')?.[0]).toEqual(['German']);
        await wrapper.setProps({ targetLanguage: 'German' });

        const translateButton = wrapper
            .findAll('button')
            .find((button) => button.attributes('title') === 'Translate');
        await translateButton!.trigger('click');
        expect(wrapper.emitted('translate')).toBeTruthy();
    });

    it('passes review filter state through to the editor', async () => {
        const reviewSegments: TranscriptSegment[] = [
            {
                start: '00:00',
                end: '00:05',
                speaker: 'Host',
                text: 'Aligned',
                mergeStatus: 'matched',
                similarityScore: 0.95,
            },
            {
                start: '00:05',
                end: '00:10',
                speaker: 'Guest',
                text: 'Needs review',
                mergeStatus: 'conflict',
                similarityScore: 0.42,
            },
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
                uniqueSpeakers: ['Host', 'Guest'],
                isProcessing: false,
            },
        });

        const toggle = wrapper.get('[data-testid="review-filter-toggle"]');
        expect(toggle.attributes('role')).toBe('switch');
        await toggle.trigger('click');

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
                uniqueSpeakers: ['Host'],
                isProcessing: false,
            },
        });

        expect(wrapper.get('[data-testid="blacklist-warnings"]').text()).toContain(
            'Blacklist warnings',
        );
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
                uniqueSpeakers: ['Host', 'Guest'],
                isProcessing: false,
            },
        });

        const hostToggle = wrapper.get('[data-testid="speaker-visibility-toggle-Host"]');
        expect(hostToggle.attributes('aria-pressed')).toBe('true');

        await hostToggle.trigger('click');

        const editor = wrapper.get('.mock-editor');
        expect(hostToggle.attributes('aria-pressed')).toBe('false');
        expect(editor.attributes('data-speaker-visibility')).toBe(
            JSON.stringify({ Host: false, Guest: true }),
        );
        expect(wrapper.text()).toContain('1 of 2 segments');

        await hostToggle.trigger('click');

        expect(editor.attributes('data-speaker-visibility')).toBe(
            JSON.stringify({ Host: true, Guest: true }),
        );
        expect(wrapper.text()).toContain('2 segments');
    });

    it('plays the seekable preview audio (not the source) and enables segment previews', () => {
        const wrapper = mount(TranscriptWorkspacePanel, {
            props: {
                inputPath: '/tmp/audio.mp3',
                hasMediaFile: true,
                // The preview scrubber plays the transcoded, seekable AAC/m4a preview —
                // not the source file (whose audio codec the webview may not decode) and
                // not the Opus/Ogg analysis audio (which WKWebView can't reliably seek).
                extractedAudioPath: '/tmp/audio_preview.m4a',
                displaySegments: segments,
                originalSegments: segments,
                translations: {},
                currentLanguage: 'Original',
                targetLanguage: '',
                isTranslating: false,
                uniqueSpeakers: ['Host'],
                isProcessing: false,
            },
        });

        const audio = wrapper.get('[data-testid="extracted-audio"]');
        expect(audio.attributes('src')).toBe('/tmp/audio_preview.m4a');
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
                uniqueSpeakers: ['Host'],
                isProcessing: false,
            },
        });

        expect(wrapper.get('.mock-editor').attributes('data-video-available')).toBe('false');
    });

    it('hides the audio player and disables audio previews without a preview stream', () => {
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
                uniqueSpeakers: ['Host', 'Guest', 'Narrator'],
                isProcessing: false,
            },
        });

        await wrapper
            .get('[data-testid="speaker-visibility-toggle-Guest"]')
            .trigger('click', { shiftKey: true });

        const editor = wrapper.get('.mock-editor');
        expect(editor.attributes('data-speaker-visibility')).toBe(
            JSON.stringify({ Host: false, Guest: true, Narrator: false }),
        );
        expect(wrapper.text()).toContain('1 of 3 segments');
    });
    const baseProps = {
        inputPath: '/tmp/audio.mp3',
        hasMediaFile: true,
        displaySegments: segments,
        originalSegments: segments,
        translations: {},
        currentLanguage: 'Original',
        targetLanguage: '',
        isTranslating: false,
        uniqueSpeakers: ['Host'],
        isProcessing: false,
    };

    it('renders a page header with the segment count and the export actions', async () => {
        const wrapper = mount(TranscriptWorkspacePanel, { props: baseProps });

        expect(wrapper.get('h1').text()).toBe('Transcript');
        expect(wrapper.get('[data-testid="segment-count"]').text()).toBe('1 segment');
        expect(wrapper.find('.mock-subtitle-export').exists()).toBe(true);

        const exportButton = wrapper.get('[data-testid="export-video"]');
        expect(exportButton.attributes('disabled')).toBeUndefined();
        await exportButton.trigger('click');
        expect(wrapper.emitted('export-video')).toBeTruthy();
        expect(wrapper.find('[data-testid="export-disabled-reason"]').exists()).toBe(false);
    });

    it('explains why exporting is unavailable when the media file is missing', () => {
        const wrapper = mount(TranscriptWorkspacePanel, {
            props: { ...baseProps, hasMediaFile: false },
        });

        const exportButton = wrapper.get('[data-testid="export-video"]');
        expect(exportButton.attributes('disabled')).toBeDefined();
        expect(exportButton.attributes('title')).toContain('source media file is missing');
        expect(wrapper.get('[data-testid="export-disabled-reason"]').text()).toContain(
            'source media file is missing',
        );
        expect(wrapper.find('[data-testid="missing-media-notice"]').exists()).toBe(true);
        expect(wrapper.find('video').exists()).toBe(false);
    });

    it('explains why translating is unavailable', async () => {
        const wrapper = mount(TranscriptWorkspacePanel, { props: baseProps });
        const translate = wrapper.get('[data-testid="translate-button"]');

        expect(translate.attributes('disabled')).toBeDefined();
        expect(translate.attributes('title')).toBe('Pick a language to translate to first');

        await wrapper.setProps({ targetLanguage: 'German', translations: { German: segments } });
        expect(translate.attributes('title')).toContain('Already translated to German');
    });

    it('switches to an existing translation when it is picked as the target', async () => {
        const wrapper = mount(TranscriptWorkspacePanel, {
            props: { ...baseProps, translations: { German: segments } },
        });

        await wrapper.get('[data-testid="target-language-trigger"]').trigger('click');
        const german = wrapper.findAll('[role="option"]').find((o) => o.text().includes('German'))!;
        expect(german.text()).toContain('translated');
        await german.trigger('click');

        expect(wrapper.emitted('update:targetLanguage')?.[0]).toEqual(['German']);
        expect(wrapper.emitted('update:currentLanguage')?.[0]).toEqual(['German']);
        expect(wrapper.find('[role="listbox"]').exists()).toBe(false);
    });

    it('closes the language menu with Escape', async () => {
        const wrapper = mount(TranscriptWorkspacePanel, {
            props: baseProps,
            attachTo: document.body,
        });

        await wrapper.get('[data-testid="target-language-trigger"]').trigger('click');
        const listbox = wrapper.get('[role="listbox"]');
        await listbox.trigger('keydown', { key: 'Escape' });

        expect(wrapper.find('[role="listbox"]').exists()).toBe(false);
        expect(document.activeElement).toBe(
            wrapper.get('[data-testid="target-language-trigger"]').element,
        );
        wrapper.unmount();
    });

    it('emits the viewing language from the view select', async () => {
        const wrapper = mount(TranscriptWorkspacePanel, {
            props: { ...baseProps, translations: { German: segments } },
        });

        await wrapper.get('[data-testid="view-language-select"]').setValue('German');
        expect(wrapper.emitted('update:currentLanguage')?.[0]).toEqual(['German']);
    });

    it('shows every speaker again from "Show all" after hiding some', async () => {
        const multiSpeakerSegments: TranscriptSegment[] = [
            { start: '00:00', end: '00:05', speaker: 'Host', text: 'Hello world' },
            { start: '00:05', end: '00:10', speaker: 'Guest', text: 'Hi there' },
        ];
        const wrapper = mount(TranscriptWorkspacePanel, {
            props: {
                ...baseProps,
                displaySegments: multiSpeakerSegments,
                originalSegments: multiSpeakerSegments,
                uniqueSpeakers: ['Host', 'Guest'],
            },
        });

        expect(wrapper.find('[data-testid="speaker-show-all"]').exists()).toBe(false);
        const hostToggle = wrapper.get('[data-testid="speaker-visibility-toggle-Host"]');
        await hostToggle.trigger('click');
        expect(hostToggle.attributes('aria-pressed')).toBe('false');

        await wrapper.get('[data-testid="speaker-show-all"]').trigger('click');
        expect(wrapper.get('.mock-editor').attributes('data-speaker-visibility')).toBe(
            JSON.stringify({ Host: true, Guest: true }),
        );
        expect(hostToggle.attributes('aria-pressed')).toBe('true');
    });

    it('passes the speaker order and the media playhead to the editor', async () => {
        const wrapper = mount(TranscriptWorkspacePanel, {
            props: { ...baseProps, extractedAudioPath: '/tmp/audio_preview.m4a' },
        });
        const editor = wrapper.get('.mock-editor');
        expect(editor.attributes('data-speaker-order')).toBe(JSON.stringify(['Host']));
        expect(editor.attributes('data-playhead')).toBe('null');

        const audio = wrapper.get('[data-testid="extracted-audio"]');
        (audio.element as HTMLAudioElement).currentTime = 3;
        await audio.trigger('timeupdate');
        expect(editor.attributes('data-playhead')).toBe('3');
    });
});
