import { describe, expect, it, vi, beforeEach } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { invoke } from '@tauri-apps/api/core';
import ViralClipsGenerator from '../ViralClipsGenerator.vue';
import type { ViralClipsWorkspaceState } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('../../composables/useSettings', () => ({
  useSettings: () => ({
    settings: {
      value: {
        apiKey: '',
        baseUrl: '',
        model: '',
      },
    },
  }),
}));

describe('ViralClipsGenerator', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === 'begin_run') return Promise.resolve(123);
      return Promise.resolve(undefined);
    });
  });

  it('normalizes numeric clip times before invoking export_clips', async () => {
    const state: ViralClipsWorkspaceState = {
      count: 1,
      minDuration: 10,
      maxDuration: 60,
      topic: '',
      allowSplicing: false,
      clips: [
        {
          title: 'Numeric clip',
          reason: 'AI returned seconds',
          segments: [{ start: 41.744, end: 59.2 }],
        },
      ] as any,
      lastExportPath: '',
      trimBoundarySilence: false,
    };

    const wrapper = mount(ViralClipsGenerator, {
      props: {
        segments: [],
        inputPath: '/tmp/source.mp4',
        hasMediaFile: true,
        state,
        cancelGeneration: 0,
      },
    });

    const exportButton = wrapper.findAll('button').find((button) => button.text() === 'Export All Clips');
    expect(exportButton).toBeDefined();
    await exportButton?.trigger('click');
    await flushPromises();

    const exportCall = vi.mocked(invoke).mock.calls.find(([command]) => command === 'export_clips');
    expect(exportCall).toBeDefined();
    expect(exportCall?.[1]).toMatchObject({
      runId: 123,
      inputPath: '/tmp/source.mp4',
      outputDir: '/tmp/source_clips',
      fastMode: true,
      segments: [
        {
          label: 'Numeric clip',
          reason: 'AI returned seconds',
          segments: [{ start: '00:41.744', end: '00:59.200' }],
        },
      ],
    });
  });
});
