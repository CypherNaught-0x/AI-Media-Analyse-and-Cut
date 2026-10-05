import { describe, expect, it, vi, beforeEach } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { invoke } from '@tauri-apps/api/core';
import ViralClipsGenerator from '../ViralClipsGenerator.vue';
import type { ClipCandidate, TranscriptSegment, ViralClipsWorkspaceState } from '../../types';
import { createDefaultViralClipsWorkspaceState } from '../../utils/editSession';
import { fromCandidates } from '../../utils/shortClips';

vi.mock('@tauri-apps/api/core', () => ({
    invoke: vi.fn(),
    convertFileSrc: (path: string) => `asset://${path}`,
}));

vi.mock('../../composables/useSettings', () => ({
    useSettings: () => ({
        settings: {
            value: {
                baseUrl: 'https://llm.example',
                model: 'model-x',
                preClipPadding: 0,
                postClipPadding: 0,
                exportQuality: 'balanced',
            },
        },
    }),
}));

const segments: TranscriptSegment[] = [
    {
        start: '00:10',
        end: '00:13',
        speaker: 'Host',
        text: 'one two three',
        words: [
            { start: '00:10.000', end: '00:10.800', text: 'one' },
            { start: '00:11.000', end: '00:11.900', text: 'two' },
            { start: '00:12.100', end: '00:13.000', text: 'three' },
        ],
    },
];

const candidate: ClipCandidate = {
    title: 'The reveal',
    hookLine: 'So we deleted production.',
    reason: 'Surprising',
    ranges: [{ start: 10, end: 13, role: 'body', firstSegment: 0, lastSegment: 0 }],
    ratings: { hook: 9, standalone: 8, emotion: 7, info: 6, loopContinuity: 0 },
    signals: { speakerChanges: 1, laughs: 0, wordsPerSecond: 2.5 },
    score: 84.4,
    duration: 3,
    looped: false,
};

function mountWith(state: Partial<ViralClipsWorkspaceState> = {}, busy = false) {
    return mount(ViralClipsGenerator, {
        props: {
            segments,
            inputPath: '/tmp/source.mp4',
            hasMediaFile: true,
            state: { ...createDefaultViralClipsWorkspaceState(), ...state },
            cancelGeneration: 0,
            busy,
        },
    });
}

function lastState(wrapper: ReturnType<typeof mountWith>): ViralClipsWorkspaceState {
    const updates = wrapper.emitted('update:state')!;
    return updates[updates.length - 1][0] as ViralClipsWorkspaceState;
}

describe('ViralClipsGenerator', () => {
    beforeEach(() => {
        vi.clearAllMocks();
        vi.mocked(invoke).mockImplementation((command) => {
            if (command === 'begin_run') return Promise.resolve(123);
            if (command === 'select_clips') return Promise.resolve([candidate]);
            return Promise.resolve(null);
        });
    });

    it('asks the backend for clips with the chosen options and keeps them selected', async () => {
        const wrapper = mountWith({ looped: true, topic: '  AI  ', count: 2 });
        await wrapper.get('[data-testid="clips-generate"]').trigger('click');
        await flushPromises();

        const call = vi.mocked(invoke).mock.calls.find(([command]) => command === 'select_clips');
        expect(call?.[1]).toMatchObject({
            runId: 123,
            llm: { baseUrl: 'https://llm.example', model: 'model-x' },
            transcript: segments,
            request: {
                count: 2,
                minSeconds: 20,
                maxSeconds: 60,
                topic: 'AI',
                allowSplicing: false,
                looped: true,
            },
        });
        const [clip] = lastState(wrapper).clips;
        expect(clip).toMatchObject({ title: 'The reveal', score: 84, selected: true });
    });

    it('shows each clip with its score, hook line and duration', () => {
        const wrapper = mountWith({ clips: fromCandidates([candidate]) });
        const cards = wrapper.get('[data-testid="clip-cards"]');
        expect(cards.text()).toContain('The reveal');
        expect(cards.text()).toContain('84');
        expect(cards.text()).toContain('So we deleted production.');
        expect(cards.text()).toContain('00:03');
    });

    it('trims a clip edge by one word', async () => {
        const clips = fromCandidates([candidate]);
        const wrapper = mountWith({ clips });
        await wrapper.get(`[data-testid="clip-start-later-${clips[0].id}"]`).trigger('click');
        expect(lastState(wrapper).clips[0].ranges[0].start).toBe(11);
    });

    it('exports only the selected clips, as timestamp ranges', async () => {
        const [keep, skip] = fromCandidates([candidate, { ...candidate, title: 'Skipped' }]);
        const wrapper = mountWith({ clips: [keep, { ...skip, selected: false }] });
        expect(wrapper.get('[data-testid="clips-export-selected"]').text()).toContain('1 selected');

        await wrapper.get('[data-testid="clips-export-selected"]').trigger('click');
        await flushPromises();

        const call = vi.mocked(invoke).mock.calls.find(([command]) => command === 'export_clips');
        expect(call?.[1]).toMatchObject({
            inputPath: '/tmp/source.mp4',
            outputDir: '/tmp/source_clips',
            fastMode: false,
            quality: 'balanced',
            segments: [
                {
                    label: 'The reveal',
                    segments: [{ start: '00:10.000', end: '00:13.000' }],
                },
            ],
        });
        expect((call?.[1] as { segments: unknown[] }).segments).toHaveLength(1);
    });

    it('does not start while another job is running', async () => {
        const wrapper = mountWith({}, true);
        expect(wrapper.get('[data-testid="clips-generate"]').attributes('disabled')).toBeDefined();
    });
});
