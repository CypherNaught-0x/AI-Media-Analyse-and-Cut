import { describe, expect, it, vi, beforeEach } from 'vitest';
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils';
import { invoke } from '@tauri-apps/api/core';
import PodcastGenerator from '../PodcastGenerator.vue';
import { createDefaultPodcastWorkspaceState } from '../../utils/editSession';
import type { PodcastScript, PodcastWorkspaceState } from '../../types';

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
                baseUrl: '',
                model: 'model',
            },
        },
    }),
}));

const segments = [{ start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'Hello' }];

const script: PodcastScript = {
    title: 'Episode',
    summary: 'About things',
    totalDuration: 30,
    segments: [
        { start: '00:00', end: '00:10', text: 'First', speaker: 'Speaker 1', type: 'content' },
        { start: '00:10', end: '00:20', text: 'Second', speaker: 'Speaker 2', type: 'content' },
        { start: '00:20', end: '00:30', text: 'Third', speaker: 'Speaker 1', type: 'content' },
    ],
};

function mountGenerator(
    props: {
        context?: string;
        busy?: boolean;
        state?: Partial<PodcastWorkspaceState>;
        segments?: typeof segments;
        speakerOrder?: string[];
    } = {},
) {
    const wrapper: VueWrapper = mount(PodcastGenerator, {
        props: {
            segments: props.segments ?? segments,
            inputPath: '/tmp/source.mp4',
            hasMediaFile: true,
            state: { ...createDefaultPodcastWorkspaceState(), ...props.state },
            cancelGeneration: 0,
            context: props.context ?? '',
            busy: props.busy ?? false,
            speakerOrder: props.speakerOrder,
            // Act as the parent: feed every emitted state back in.
            'onUpdate:state': (next: PodcastWorkspaceState) => wrapper.setProps({ state: next }),
        },
        global: { stubs: { teleport: true } },
    });
    return wrapper;
}

function generateButton(wrapper: ReturnType<typeof mountGenerator>) {
    return wrapper.get('[data-testid="podcast-generate"]');
}

function lastState(wrapper: ReturnType<typeof mountGenerator>) {
    const events = wrapper.emitted('update:state') as PodcastWorkspaceState[][];
    return events[events.length - 1][0];
}

function texts(state: PodcastWorkspaceState) {
    return state.podcastScript!.segments.map((s) => s.text);
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

        const call = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'generate_podcast');
        expect(call?.[1]).toMatchObject({ context: 'A Rust tutorial' });
    });

    it('sends no context when none was given', async () => {
        const wrapper = mountGenerator();
        await generateButton(wrapper).trigger('click');
        await flushPromises();

        const call = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'generate_podcast');
        expect(call?.[1]).toMatchObject({ context: null });
    });

    it('does not start while another job is running', async () => {
        const wrapper = mountGenerator({ busy: true });
        expect(generateButton(wrapper).attributes('disabled')).toBeDefined();
        await generateButton(wrapper).trigger('click');
        await flushPromises();

        expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'begin_run')).toBe(
            false,
        );
    });

    it('labels the generate action and explains when it is unavailable', () => {
        const wrapper = mountGenerator({ busy: true });
        expect(generateButton(wrapper).element.tagName).toBe('BUTTON');
        expect(generateButton(wrapper).text()).toContain('Generate script');
        expect(wrapper.get('[data-testid="podcast-generate-hint"]').text()).toContain(
            'Another job is running',
        );
    });

    it('shows an empty state before a script exists', () => {
        const wrapper = mountGenerator();
        expect(wrapper.text()).toContain('No script yet');
        expect(wrapper.find('[data-testid="podcast-export"]').exists()).toBe(false);
    });

    it('flags a minimum longer than the maximum and blocks generation', async () => {
        const wrapper = mountGenerator({
            state: { minDurationMinutes: 20, maxDurationMinutes: 10 },
        });
        expect(wrapper.get('[data-testid="podcast-range-error"]').text()).toContain(
            'Minimum is longer than the maximum',
        );
        expect(wrapper.get('#podcast-min').attributes('aria-invalid')).toBe('true');
        expect(generateButton(wrapper).attributes('disabled')).toBeDefined();
        await generateButton(wrapper).trigger('click');
        await flushPromises();
        expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'begin_run')).toBe(
            false,
        );
    });

    it('moves a focused row with Alt+Arrow keys', async () => {
        const wrapper = mountGenerator({ state: { podcastScript: script } });
        await wrapper.get('[data-testid="podcast-segment-0"]').trigger('keydown', {
            key: 'ArrowDown',
            altKey: true,
        });
        expect(texts(lastState(wrapper))).toEqual(['Second', 'First', 'Third']);

        await wrapper.get('[data-testid="podcast-segment-2"]').trigger('keydown', {
            key: 'ArrowUp',
            altKey: true,
        });
        expect(texts(lastState(wrapper))).toEqual(['Second', 'Third', 'First']);

        // Without Alt, or past the ends, nothing moves.
        const count = wrapper.emitted('update:state')!.length;
        await wrapper.get('[data-testid="podcast-segment-0"]').trigger('keydown', {
            key: 'ArrowUp',
            altKey: true,
        });
        await wrapper.get('[data-testid="podcast-segment-1"]').trigger('keydown', {
            key: 'ArrowDown',
        });
        expect(wrapper.emitted('update:state')!.length).toBe(count);
    });

    it('only starts a drag from the handle', () => {
        const wrapper = mountGenerator({ state: { podcastScript: script } });
        const row = wrapper.get('[data-testid="podcast-segment-0"]');
        expect(row.attributes('draggable')).toBeUndefined();
        expect(row.get('[data-testid="podcast-drag-handle"]').attributes('draggable')).toBe('true');
    });

    it('keeps an invalid timestamp in the editor with an error', async () => {
        const wrapper = mountGenerator({ state: { podcastScript: script } });
        await wrapper.get('[data-testid="podcast-time-start-1"]').trigger('click');
        const input = wrapper.get('[data-testid="podcast-time-input"]');
        await input.setValue('abc');
        await input.trigger('keydown', { key: 'Enter' });

        expect(wrapper.emitted('update:state')).toBeUndefined();
        expect(wrapper.get('[data-testid="podcast-time-error"]').text()).toContain('mm:ss');
        expect(input.attributes('aria-invalid')).toBe('true');

        await input.setValue('00:12');
        await input.trigger('keydown', { key: 'Enter' });
        expect(lastState(wrapper).podcastScript!.segments[1].start).toBe('00:12');
        expect(wrapper.find('[data-testid="podcast-time-input"]').exists()).toBe(false);
    });

    it('cancels a timestamp edit with Escape', async () => {
        const wrapper = mountGenerator({ state: { podcastScript: script } });
        await wrapper.get('[data-testid="podcast-time-end-0"]').trigger('click');
        const input = wrapper.get('[data-testid="podcast-time-input"]');
        await input.setValue('nonsense');
        await input.trigger('keydown', { key: 'Escape' });
        expect(wrapper.find('[data-testid="podcast-time-input"]').exists()).toBe(false);
        expect(wrapper.emitted('update:state')).toBeUndefined();
    });

    it('removes segments and inserts voiceovers', async () => {
        const wrapper = mountGenerator({ state: { podcastScript: script } });
        await wrapper.get('[data-testid="podcast-add-voiceover-0"]').trigger('click');
        const segmentsAfter = lastState(wrapper).podcastScript!.segments;
        expect(segmentsAfter.map((s) => s.type)).toEqual([
            'content',
            'voiceover',
            'content',
            'content',
        ]);
        expect(wrapper.text()).toContain('Not exported');

        await wrapper.get('[data-testid="podcast-note-1"]').setValue('Meanwhile');
        expect(lastState(wrapper).podcastScript!.segments[1].transitionNote).toBe('Meanwhile');

        await wrapper.get('[data-testid="podcast-remove-1"]').trigger('click');
        expect(texts(lastState(wrapper))).toEqual(['First', 'Second', 'Third']);
    });

    it('adds several transcript segments from a searchable dialog', async () => {
        const transcript = [
            { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'First' },
            { start: '01:00', end: '01:10', speaker: 'Speaker 2', text: 'Rust is fast' },
            { start: '02:00', end: '02:10', speaker: 'Speaker 1', text: 'Rust is safe' },
        ];
        const wrapper = mountGenerator({
            segments: transcript,
            state: { podcastScript: { ...script, segments: [script.segments[0]] } },
        });
        await wrapper.get('[data-testid="podcast-add-segment"]').trigger('click');
        expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
        // The segment already in the script can't be added again.
        expect(wrapper.get('[data-testid="podcast-add-row-0"]').text()).toContain('Included');

        await wrapper.get('[data-testid="podcast-add-search"]').setValue('rust');
        expect(wrapper.findAll('[data-testid^="podcast-add-row-button-"]')).toHaveLength(2);

        await wrapper.get('[data-testid="podcast-add-row-button-0"]').trigger('click');
        await wrapper.get('[data-testid="podcast-add-row-button-1"]').trigger('click');
        expect(wrapper.find('[role="dialog"]').exists()).toBe(true);
        expect(wrapper.get('[data-testid="podcast-added-count"]').text()).toBe('2 segments added');
        expect(texts(lastState(wrapper))).toEqual(['First', 'Rust is fast', 'Rust is safe']);
        expect(lastState(wrapper).podcastScript!.segments[2].includeReason).toBe('Manually added');

        await wrapper.get('[data-testid="podcast-add-done"]').trigger('click');
        expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    });

    it('closes the add dialog with Escape', async () => {
        const wrapper = mountGenerator({ state: { podcastScript: script } });
        await wrapper.get('[data-testid="podcast-add-segment"]').trigger('click');
        await wrapper.get('[role="dialog"]').trigger('keydown', { key: 'Escape' });
        expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    });

    it('shows a busy state only on the export that is running', async () => {
        let finish: () => void = () => {};
        vi.mocked(invoke).mockImplementation((command) => {
            if (command === 'begin_run') return Promise.resolve(1);
            if (command === 'export_podcast_clips')
                return new Promise<undefined>((resolve) => (finish = () => resolve(undefined)));
            return Promise.resolve(undefined);
        });
        const wrapper = mountGenerator({ state: { podcastScript: script } });
        await wrapper.get('[data-testid="podcast-export-clips"]').trigger('click');
        await flushPromises();

        const clips = wrapper.get('[data-testid="podcast-export-clips"]');
        const podcast = wrapper.get('[data-testid="podcast-export"]');
        expect(clips.attributes('aria-busy')).toBe('true');
        expect(podcast.attributes('aria-busy')).toBeUndefined();
        expect(podcast.attributes('disabled')).toBeDefined();

        finish();
        await flushPromises();
        expect(lastState(wrapper).lastExportPath).toBe('/tmp/source_podcast_clips');
        expect(wrapper.find('[data-testid="podcast-open-folder"]').attributes('aria-label')).toBe(
            'Open export folder',
        );
    });

    it('colours the duration by the target range', () => {
        const wrapper = mountGenerator({
            state: { podcastScript: script, minDurationMinutes: 1, maxDurationMinutes: 2 },
        });
        const chip = wrapper.get('[data-testid="podcast-duration"]');
        expect(chip.attributes('data-status')).toBe('short');
        expect(chip.classes()).toContain('chip-warning');
        expect(wrapper.text()).toContain('Shorter than the 1–2 min target');
    });

    it('does not compare the duration against an invalid target range', () => {
        const wrapper = mountGenerator({
            state: { podcastScript: script, minDurationMinutes: 20, maxDurationMinutes: 15 },
        });
        const chip = wrapper.get('[data-testid="podcast-duration"]');
        expect(chip.attributes('data-status')).toBe('unknown');
        expect(chip.classes()).toContain('chip');
        expect(chip.classes()).not.toContain('chip-warning');
        expect(wrapper.text()).toContain('Fix the target length to compare');
        expect(wrapper.text()).not.toContain('Shorter than');
    });

    it('disables generation when the transcript has no segments, with the reason', async () => {
        const wrapper = mountGenerator({ segments: [] });
        expect(generateButton(wrapper).attributes('disabled')).toBeDefined();
        expect(wrapper.get('[data-testid="podcast-generate-hint"]').text()).toContain(
            'The transcript has no segments',
        );
        await generateButton(wrapper).trigger('click');
        await flushPromises();
        expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'begin_run')).toBe(
            false,
        );
    });

    it('explains disabled exports while its own script is generating', async () => {
        let finish: (value: string) => void = () => {};
        vi.mocked(invoke).mockImplementation((command) => {
            if (command === 'begin_run') return Promise.resolve(1);
            if (command === 'generate_podcast')
                return new Promise<string>((resolve) => (finish = resolve));
            return Promise.resolve(undefined);
        });
        const wrapper = mountGenerator({ state: { podcastScript: script } });
        await generateButton(wrapper).trigger('click');
        await flushPromises();
        // Home reports its own processing flag back as `busy`.
        await wrapper.setProps({ busy: true });

        const exportButton = wrapper.get('[data-testid="podcast-export"]');
        expect(exportButton.attributes('disabled')).toBeDefined();
        expect(exportButton.attributes('title')).toBe('Wait for the script to finish generating.');
        finish('{}');
        await flushPromises();
    });

    it('colours speakers by the given speaker order', () => {
        const colourOf = (wrapper: ReturnType<typeof mountGenerator>, index: number) =>
            wrapper
                .get(`[data-testid="podcast-segment-${index}"] [data-testid="podcast-speaker"]`)
                .attributes('style');

        // Without an order: order of first appearance in the transcript.
        const byAppearance = mountGenerator({
            segments: [
                { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'a' },
                { start: '00:10', end: '00:20', speaker: 'Speaker 2', text: 'b' },
            ],
            state: { podcastScript: script },
        });
        expect(colourOf(byAppearance, 0)).toContain('--color-speaker-1');
        expect(colourOf(byAppearance, 1)).toContain('--color-speaker-2');

        // With the user's order, Speaker 2 comes first.
        const byOrder = mountGenerator({
            speakerOrder: ['Speaker 2', 'Speaker 1'],
            state: { podcastScript: script },
        });
        expect(colourOf(byOrder, 0)).toContain('--color-speaker-2');
        expect(colourOf(byOrder, 1)).toContain('--color-speaker-1');
    });
});
