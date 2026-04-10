import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import { ref, nextTick } from 'vue';
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
    expect(wrapper.find('.mt-2').exists()).toBe(false);

    // Click validate button
    const validateButton = wrapper.find('button');
    // Find the Validate button specifically
    const buttons = wrapper.findAll('button');
    const validateBtn = buttons.find(b => b.text() === 'Validate');
    expect(validateBtn).toBeDefined();
    await validateBtn?.trigger('click');

    // Panel should be visible
    await nextTick();
    expect(wrapper.find('.mt-2').exists()).toBe(true);
    expect(wrapper.text()).toContain('Validation Results');
  });

  it('should display validation errors when present', async () => {
    const wrapper = mount(SubtitleExport, {
      props: {
        segments: mockSegments,
        inputPath: '/test/video.mp4',
      },
    });

    const buttons = wrapper.findAll('button');
    const validateBtn = buttons.find(b => b.text() === 'Validate');
    await validateBtn?.trigger('click');
    await nextTick();

    // Should show validation info
    expect(wrapper.text()).toContain('Validation Results');
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
    const validateBtn = buttons.find(b => b.text() === 'Validate');
    await validateBtn?.trigger('click');
    await nextTick();

    expect(wrapper.text()).toContain('Validation Results');
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
    const validateBtn = buttons.find(b => b.text() === 'Validate');
    await validateBtn?.trigger('click');
    await nextTick();

    // Button should have warning classes
    expect(validateBtn?.classes()).toContain('bg-yellow-500/20');
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
    const validateBtn = buttons.find(b => b.text() === 'Validate');
    await validateBtn?.trigger('click');
    await nextTick();

    // Button should have error classes
    expect(validateBtn?.classes()).toContain('bg-red-500/20');
  });

  it('should call processSubtitlesForDisplay when exporting', async () => {
    const processSpy = vi.spyOn(subtitleValidation, 'processSubtitlesForDisplay').mockReturnValue({
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
    const srtButton = wrapper.findAll('button').find(b => b.text() === 'SRT');
    await srtButton?.trigger('click');

    expect(processSpy).toHaveBeenCalled();
    expect(processSpy).toHaveBeenCalledWith(
      mockSegments,
      expect.objectContaining({
        maxCharsPerLine: 42,
        maxLines: 2,
      })
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
    const processSpy = vi.spyOn(subtitleValidation, 'processSubtitlesForDisplay').mockReturnValue({
      segments: legacySegments,
      errors: [],
    });

    const wrapper = mount(SubtitleExport, {
      props: {
        segments: legacySegments,
        inputPath: '/test/video.mp4',
      },
    });

    const srtButton = wrapper.findAll('button').find(b => b.text() === 'SRT');
    await srtButton?.trigger('click');

    const writeCall = vi.mocked(invoke).mock.calls.find(([command]) => command === 'write_text_file');
    expect(writeCall).toBeDefined();

    const payload = writeCall?.[1] as { content: string };
    expect(payload.content).toContain('00:59:57,920 --> 01:00:03,520');
    expect(payload.content).toContain('01:00:58,800 --> 01:01:06,720');
    expect(payload.content).not.toContain('00:60:');
    expect(payload.content).not.toContain('00:61:');

    processSpy.mockRestore();
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
    const validateBtn = buttons.find(b => b.text() === 'Validate');
    await validateBtn?.trigger('click');
    await nextTick();

    expect(wrapper.find('.mt-2').exists()).toBe(true);

    // Find and click close button
    const closeBtn = wrapper.find('button.text-gray-500');
    await closeBtn.trigger('click');
    await nextTick();

    // Panel should be hidden
    expect(wrapper.find('.mt-2').exists()).toBe(false);
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

    expect(wrapper.find('.mt-2').exists()).toBe(false);
    expect(wrapper.vm.showValidationPanel).toBe(false);
  });
});
