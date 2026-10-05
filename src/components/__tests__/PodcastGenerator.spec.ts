import { describe, expect, it, vi, beforeEach } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { invoke } from '@tauri-apps/api/core';
import PodcastGenerator from '../PodcastGenerator.vue';
import { createDefaultPodcastWorkspaceState } from '../../utils/editSession';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}));

vi.mock('../../composables/useSettings', () => ({
  useSettings: () => ({
    settings: {
      value: {
        apiKey: 'key',
        baseUrl: '',
        model: 'model',
      },
    },
  }),
}));

const segments = [{ start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'Hello' }];

function mountGenerator(props: { context?: string; busy?: boolean } = {}) {
  return mount(PodcastGenerator, {
    props: {
      segments,
      inputPath: '/tmp/source.mp4',
      hasMediaFile: true,
      state: createDefaultPodcastWorkspaceState(),
      cancelGeneration: 0,
      context: props.context ?? '',
      busy: props.busy ?? false,
    },
  });
}

function generateButton(wrapper: ReturnType<typeof mountGenerator>) {
  return wrapper.findAll('button').find((button) => button.text().includes('Generate Podcast Script'))!;
}

describe('PodcastGenerator', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === 'begin_run') return Promise.resolve(1);
      if (command === 'generate_podcast') return Promise.resolve('{}');
      return Promise.resolve(undefined);
    });
  });

  it('sends the analysis context with the podcast request', async () => {
    const wrapper = mountGenerator({ context: '  A Rust tutorial  ' });
    await generateButton(wrapper).trigger('click');
    await flushPromises();

    const call = vi.mocked(invoke).mock.calls.find(([command]) => command === 'generate_podcast');
    expect(call?.[1]).toMatchObject({ context: 'A Rust tutorial' });
  });

  it('sends no context when none was given', async () => {
    const wrapper = mountGenerator();
    await generateButton(wrapper).trigger('click');
    await flushPromises();

    const call = vi.mocked(invoke).mock.calls.find(([command]) => command === 'generate_podcast');
    expect(call?.[1]).toMatchObject({ context: null });
  });

  it('does not start while another job is running', async () => {
    const wrapper = mountGenerator({ busy: true });
    expect(generateButton(wrapper).attributes('disabled')).toBeDefined();
    await generateButton(wrapper).trigger('click');
    await flushPromises();

    expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'begin_run')).toBe(false);
  });
});
