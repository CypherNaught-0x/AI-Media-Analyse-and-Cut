import { describe, it, expect, vi, beforeEach } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { nextTick } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import SubtitleExport from '../SubtitleExport.vue';
import * as subtitleValidation from '../../utils/subtitleValidation';
import type { TranscriptSegment } from '../../types';

// Mock Tauri API
vi.mock('@tauri-apps/api/core', () => ({
    invoke: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
    save: vi.fn(),
}));

describe('SubtitleExport', () => {
    const mockSegments: TranscriptSegment[] = [
        { start: '0:00', end: '0:05', text: 'Hello world', speaker: 'Speaker 1' },
        { start: '0:05', end: '0:10', text: 'A'.repeat(200), speaker: 'Speaker 2' },
    ];

    beforeEach(() => {
        vi.clearAllMocks();
        vi.mocked(invoke).mockResolvedValue(undefined);
    });

    it('should render export buttons', () => {
        const wrapper = mount(SubtitleExport, {
            props: {
                segments: mockSegments,
                inputPath: '/test/video.mp4',
            },
        });

        expect(wrapper.find('button').exists()).toBe(true);
        expect(wrapper.text()).toContain('SRT');
        expect(wrapper.text()).toContain('VTT');
        expect(wrapper.text()).toContain('TXT');
        expect(wrapper.text()).toContain('Validate');
    });

    it('should show validation panel when validate button is clicked', async () => {
        const wrapper = mount(SubtitleExport, {
            props: {
                segments: mockSegments,
                inputPath: '/test/video.mp4',
            },
        });

        // Validation panel should be hidden initially
        expect(wrapper.find('[data-testid="subtitle-validation-panel"]').exists()).toBe(false);

        // Click validate button
        // Find the Validate button specifically
        const buttons = wrapper.findAll('button');
        const validateBtn = buttons.find((b) => b.text() === 'Validate');
        expect(validateBtn).toBeDefined();
        await validateBtn?.trigger('click');

        // Panel should be visible
        await nextTick();
        expect(wrapper.find('[data-testid="subtitle-validation-panel"]').exists()).toBe(true);
        expect(wrapper.text()).toContain('Validation results');
    });

    it('should display validation errors when present', async () => {
        const wrapper = mount(SubtitleExport, {
            props: {
                segments: mockSegments,
                inputPath: '/test/video.mp4',
            },
        });

        const buttons = wrapper.findAll('button');
        const validateBtn = buttons.find((b) => b.text() === 'Validate');
        await validateBtn?.trigger('click');
        await nextTick();

        // Should show validation info
        expect(wrapper.text()).toContain('Validation results');
    });

    it('should show success message when no validation errors', async () => {
        const validSegments: TranscriptSegment[] = [
            { start: '0:00', end: '0:05', text: 'Valid text', speaker: 'Speaker 1' },
        ];

        const wrapper = mount(SubtitleExport, {
            props: {
                segments: validSegments,
                inputPath: '/test/video.mp4',
            },
        });

        const buttons = wrapper.findAll('button');
        const validateBtn = buttons.find((b) => b.text() === 'Validate');
        await validateBtn?.trigger('click');
        await nextTick();

        expect(wrapper.text()).toContain('Validation results');
        expect(wrapper.text()).toContain('All subtitles meet comfortable display requirements');
    });

    it('should show warning style on validate button when warnings exist', async () => {
        const segmentsWithWarnings: TranscriptSegment[] = [
            { start: '0:00', end: '0:00', text: 'Quick', speaker: 'Speaker 1' }, // Short duration
        ];

        const wrapper = mount(SubtitleExport, {
            props: {
                segments: segmentsWithWarnings,
                inputPath: '/test/video.mp4',
            },
        });

        const buttons = wrapper.findAll('button');
        const validateBtn = buttons.find((b) => b.text() === 'Validate');
        await validateBtn?.trigger('click');
        await nextTick();

        // Button reflects the warning severity
        expect(validateBtn?.attributes('data-severity')).toBe('warning');
        expect(validateBtn?.classes()).toContain('text-warning!');
    });

    it('should show error style on validate button when errors exist', async () => {
        const segmentsWithErrors: TranscriptSegment[] = [
            { start: '0:00', end: '0:05', text: '', speaker: 'Speaker 1' }, // Empty text
        ];

        const wrapper = mount(SubtitleExport, {
            props: {
                segments: segmentsWithErrors,
                inputPath: '/test/video.mp4',
            },
        });

        const buttons = wrapper.findAll('button');
        const validateBtn = buttons.find((b) => b.text() === 'Validate');
        await validateBtn?.trigger('click');
        await nextTick();

        // Button reflects the error severity
        expect(validateBtn?.attributes('data-severity')).toBe('error');
        expect(validateBtn?.classes()).toContain('text-danger!');
    });

    it('should call processSubtitlesForDisplay when exporting', async () => {
        const processSpy = vi
            .spyOn(subtitleValidation, 'processSubtitlesForDisplay')
            .mockReturnValue({
                segments: mockSegments,
                errors: [],
            });

        const wrapper = mount(SubtitleExport, {
            props: {
                segments: mockSegments,
                inputPath: '/test/video.mp4',
            },
        });

        // Click SRT export button
        const srtButton = wrapper.findAll('button').find((b) => b.text() === 'SRT');
        await srtButton?.trigger('click');

        expect(processSpy).toHaveBeenCalled();
        expect(processSpy).toHaveBeenCalledWith(
            mockSegments,
            expect.objectContaining({
                maxCharsPerLine: 42,
                maxLines: 2,
            }),
        );

        processSpy.mockRestore();
    });

    it('should normalize post-hour timestamps when exporting legacy subtitle data', async () => {
        const legacySegments: TranscriptSegment[] = [
            {
                start: '59:57.920',
                end: '60:03.520',
                text: 'Legacy first line',
                speaker: 'Speaker 1',
            },
            {
                start: '60:58.800',
                end: '61:06.720',
                text: 'Legacy second line',
                speaker: 'Speaker 1',
            },
        ];
        const processSpy = vi
            .spyOn(subtitleValidation, 'processSubtitlesForDisplay')
            .mockReturnValue({
                segments: legacySegments,
                errors: [],
            });

        const wrapper = mount(SubtitleExport, {
            props: {
                segments: legacySegments,
                inputPath: '/test/video.mp4',
            },
        });

        const srtButton = wrapper.findAll('button').find((b) => b.text() === 'SRT');
        await srtButton?.trigger('click');

        const writeCall = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'write_text_file');
        expect(writeCall).toBeDefined();

        const payload = writeCall?.[1] as { content: string };
        expect(payload.content).toContain('00:59:57,920 --> 01:00:03,520');
        expect(payload.content).toContain('01:00:58,800 --> 01:01:06,720');
        expect(payload.content).not.toContain('00:60:');
        expect(payload.content).not.toContain('00:61:');

        processSpy.mockRestore();
    });

    it('should export cut-timeline subtitles that start at zero, next to the cut video', async () => {
        // Regression: with a silent intro the source timeline starts at 03:17.449,
        // which runs that far late against the `_cut` export.
        const introSegments: TranscriptSegment[] = [
            { start: '03:17.449', end: '03:21.289', text: 'First line', speaker: 'Speaker 1' },
            { start: '03:21.289', end: '03:29.049', text: 'Second line', speaker: 'Speaker 1' },
        ];

        const wrapper = mount(SubtitleExport, {
            props: {
                segments: introSegments,
                cutSegments: introSegments,
                inputPath: '/test/video.mp4',
            },
        });

        await wrapper.get('[data-testid="subtitle-timeline-cut"]').trigger('click');
        const srtButton = wrapper.findAll('button').find((b) => b.text() === 'SRT');
        await srtButton?.trigger('click');

        const writeCall = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'write_text_file');
        const payload = writeCall?.[1] as { path: string; content: string };

        expect(payload.path).toBe('/test/video_cut.srt');
        expect(payload.content).toContain('00:00:00,000 --> 00:00:03,840');
        expect(payload.content).toContain('00:00:03,840 --> 00:00:11,600');
        expect(payload.content).not.toContain('00:03:17,449');
    });

    it('should keep source-timeline timestamps and file name by default', async () => {
        const introSegments: TranscriptSegment[] = [
            { start: '03:17.449', end: '03:21.289', text: 'First line', speaker: 'Speaker 1' },
        ];

        const wrapper = mount(SubtitleExport, {
            props: {
                segments: introSegments,
                cutSegments: introSegments,
                inputPath: '/test/video.mp4',
            },
        });

        const srtButton = wrapper.findAll('button').find((b) => b.text() === 'SRT');
        await srtButton?.trigger('click');

        const writeCall = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'write_text_file');
        const payload = writeCall?.[1] as { path: string; content: string };

        expect(payload.path).toBe('/test/video.srt');
        expect(payload.content).toContain('00:03:17,449 --> 00:03:21,289');
    });

    it('should close validation panel when close button is clicked', async () => {
        const wrapper = mount(SubtitleExport, {
            props: {
                segments: mockSegments,
                inputPath: '/test/video.mp4',
            },
        });

        // Open panel
        const buttons = wrapper.findAll('button');
        const validateBtn = buttons.find((b) => b.text() === 'Validate');
        await validateBtn?.trigger('click');
        await nextTick();

        expect(wrapper.find('[data-testid="subtitle-validation-panel"]').exists()).toBe(true);

        // Find and click close button
        const closeBtn = wrapper.get('[data-testid="subtitle-validation-close"]');
        await closeBtn.trigger('click');
        await nextTick();

        // Panel should be hidden
        expect(wrapper.find('[data-testid="subtitle-validation-panel"]').exists()).toBe(false);
    });

    it('should pass language prop correctly', () => {
        const wrapper = mount(SubtitleExport, {
            props: {
                segments: mockSegments,
                inputPath: '/test/video.mp4',
                language: 'Spanish',
            },
        });

        expect(wrapper.vm.language).toBe('Spanish');
    });

    it('should not show validation panel initially', () => {
        const wrapper = mount(SubtitleExport, {
            props: {
                segments: mockSegments,
                inputPath: '/test/video.mp4',
            },
        });

        expect(wrapper.find('[data-testid="subtitle-validation-panel"]').exists()).toBe(false);
    });
    it('shows export errors in the danger tone and clears them on the next action', async () => {
        vi.mocked(invoke).mockRejectedValueOnce(new Error('disk full'));
        const wrapper = mount(SubtitleExport, {
            props: { segments: mockSegments, inputPath: '/test/video.mp4' },
        });

        const srtButton = wrapper.findAll('button').find((b) => b.text() === 'SRT');
        await srtButton!.trigger('click');
        await flushPromises();

        const status = wrapper.get('[data-testid="subtitle-export-status"]');
        expect(status.text()).toContain('Error: disk full');
        expect(status.classes()).toContain('text-danger');
        // The always-mounted live region announces the same message.
        expect(wrapper.get('[role="status"]').text()).toBe('Error: disk full');

        const validateBtn = wrapper.findAll('button').find((b) => b.text() === 'Validate');
        await validateBtn!.trigger('click');
        expect(wrapper.find('[data-testid="subtitle-export-status"]').exists()).toBe(false);
        expect(wrapper.get('[role="status"]').text()).toBe('');
    });

    it('shows a success status after exporting', async () => {
        const wrapper = mount(SubtitleExport, {
            props: { segments: mockSegments, inputPath: '/test/video.mp4' },
        });

        const vttButton = wrapper.findAll('button').find((b) => b.text() === 'VTT');
        await vttButton!.trigger('click');
        await flushPromises();

        const status = wrapper.get('[data-testid="subtitle-export-status"]');
        expect(status.text()).toBe('Exported VTT');
        expect(status.classes()).toContain('text-success');
    });

    it('labels the save-as buttons for assistive tech', () => {
        const wrapper = mount(SubtitleExport, {
            props: { segments: mockSegments, inputPath: '/test/video.mp4' },
        });

        expect(wrapper.find('button[aria-label="Save SRT as…"]').exists()).toBe(true);
        expect(wrapper.find('button[aria-label="Save VTT as…"]').exists()).toBe(true);
        expect(wrapper.find('button[aria-label="Save TXT as…"]').exists()).toBe(true);
    });

    it('closes the validation panel with Escape', async () => {
        const wrapper = mount(SubtitleExport, {
            props: { segments: mockSegments, inputPath: '/test/video.mp4' },
            attachTo: document.body,
        });

        const validateBtn = wrapper.findAll('button').find((b) => b.text() === 'Validate');
        await validateBtn!.trigger('click');
        expect(wrapper.find('[data-testid="subtitle-validation-panel"]').exists()).toBe(true);

        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
        await nextTick();
        expect(wrapper.find('[data-testid="subtitle-validation-panel"]').exists()).toBe(false);
        wrapper.unmount();
    });
});
