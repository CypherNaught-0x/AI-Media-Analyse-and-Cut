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
        vi.mocked(invoke).mockImplementation((command, args) => {
            if (command === 'begin_run') return Promise.resolve(123);
            if (command === 'select_clips') return Promise.resolve([candidate]);
            // Tightening cuts the last word off each clip.
            if (command === 'tighten_clips')
                return Promise.resolve(
                    (args as { segments: { segments: unknown[] }[] }).segments.map((clip) => ({
                        ...clip,
                        segments: [{ start: '00:10.000', end: '00:12.000' }],
                    })),
                );
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
        const wrapper = mountWith({
            clips: [keep, { ...skip, selected: false }],
            intensity: 'off',
        });
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
        expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).not.toContain(
            'tighten_clips',
        );
    });

    it('tightens clips before a normal export', async () => {
        const [clip] = fromCandidates([candidate]);
        const wrapper = mountWith({ clips: [clip], intensity: 'hyper' });

        await wrapper.get('[data-testid="clips-export-selected"]').trigger('click');
        await flushPromises();

        const tighten = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'tighten_clips');
        expect(tighten?.[1]).toMatchObject({
            intensity: 'hyper',
            looped: [false],
            segments: [{ segments: [{ start: '00:10.000', end: '00:13.000' }] }],
            words: [{ text: 'one' }, { text: 'two' }, { text: 'three' }],
        });
        const exported = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'export_clips');
        expect(exported?.[1]).toMatchObject({
            segments: [{ segments: [{ start: '00:10.000', end: '00:12.000' }] }],
        });
    });

    it('switches the tightening intensity', async () => {
        const wrapper = mountWith();
        await wrapper.get('[data-testid="clips-intensity-chill"]').trigger('click');
        expect(lastState(wrapper).intensity).toBe('chill');
    });

    it("exports vertical clips with the speakers' turns when 9:16 is on", async () => {
        const [clip] = fromCandidates([candidate]);
        const wrapper = mountWith({ clips: [clip], vertical: true });

        await wrapper.get('[data-testid="clips-export-selected"]').trigger('click');
        await flushPromises();

        const commandsCalled = vi.mocked(invoke).mock.calls.map(([command]) => command);
        expect(commandsCalled).toContain('export_vertical_clips');
        expect(commandsCalled).not.toContain('export_clips');
        const call = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'export_vertical_clips');
        expect(call?.[1]).toMatchObject({
            outputDir: '/tmp/source_clips',
            quality: 'balanced',
            request: {
                inputPath: '/tmp/source.mp4',
                segments: [{ segments: [{ start: '00:10.000', end: '00:13.000' }] }],
                turns: [{ start: 10, end: 13, speaker: 'Host' }],
                words: [
                    { start: 10, end: 10.8, text: 'one' },
                    { start: 11, end: 11.9, text: 'two' },
                    { start: 12.1, end: 13, text: 'three' },
                ],
                intensity: 'punchy',
                captions: true,
                looped: [false],
            },
        });
        // The backend tightens vertical clips itself.
        expect(commandsCalled).not.toContain('tighten_clips');
    });

    it('searches for faces on its own in 9:16 mode, once per set of clips', async () => {
        vi.useFakeTimers();
        try {
            const [clip] = fromCandidates([candidate]);
            vi.mocked(invoke).mockImplementation((command) => {
                if (command === 'begin_run') return Promise.resolve(123);
                if (command === 'detect_vertical_faces') return Promise.resolve([]);
                return Promise.resolve(null);
            });
            const searches = () =>
                vi.mocked(invoke).mock.calls.filter(([c]) => c === 'detect_vertical_faces').length;

            const wrapper = mountWith({ clips: [clip], vertical: false });
            await vi.advanceTimersByTimeAsync(2000);
            expect(searches()).toBe(0);

            await wrapper.setProps({ state: { ...wrapper.props('state'), vertical: true } });
            await vi.advanceTimersByTimeAsync(2000);
            expect(searches()).toBe(1);

            // Nothing changed: no second search.
            await wrapper.setProps({ state: { ...wrapper.props('state'), captions: false } });
            await vi.advanceTimersByTimeAsync(2000);
            expect(searches()).toBe(1);

            const trimmed = { ...clip, ranges: [{ ...clip.ranges[0], end: 12.5 }] };
            await wrapper.setProps({ state: { ...wrapper.props('state'), clips: [trimmed] } });
            await vi.advanceTimersByTimeAsync(2000);
            expect(searches()).toBe(2);
            wrapper.unmount();
        } finally {
            vi.useRealTimers();
        }
    });

    it("doesn't search again for clips whose faces were saved", async () => {
        vi.useFakeTimers();
        try {
            const [clip] = fromCandidates([candidate]);
            const signature = `/tmp/source.mp4|${clip.ranges.map((r) => `${r.start}-${r.end}`).join(',')}`;
            const wrapper = mountWith({
                clips: [clip],
                vertical: true,
                faceSearch: { signature, faces: [] },
            });
            await vi.advanceTimersByTimeAsync(2000);
            const searched = vi
                .mocked(invoke)
                .mock.calls.some(([command]) => command === 'detect_vertical_faces');
            expect(searched).toBe(false);
            wrapper.unmount();
        } finally {
            vi.useRealTimers();
        }
    });

    it("shows a clip's text and corrects a word in the transcript", async () => {
        const [clip] = fromCandidates([candidate]);
        const wrapper = mountWith({ clips: [clip] });
        expect(wrapper.find('[data-testid="clip-text"]').exists()).toBe(false);

        await wrapper.get(`[data-testid="clip-text-toggle-${clip.id}"]`).trigger('click');
        expect(wrapper.get('[data-testid="clip-text"]').text()).toContain('one two three');

        await wrapper.get('[data-testid="clip-word-1"]').trigger('click');
        const input = wrapper.get('[data-testid="clip-text-input"]');
        await input.setValue('TWO');
        await input.trigger('keydown', { key: 'Enter' });

        const [edited] = wrapper.emitted('update:segments')!.at(-1) as [TranscriptSegment[]];
        expect(edited[0].text).toBe('one TWO three');
        expect(edited[0].words?.[1]).toEqual({ start: '00:11.000', end: '00:11.900', text: 'TWO' });
    });

    it('cuts a word out of a clip without touching the transcript', async () => {
        const [clip] = fromCandidates([candidate]);
        const wrapper = mountWith({ clips: [clip] });
        await wrapper.get(`[data-testid="clip-text-toggle-${clip.id}"]`).trigger('click');
        await wrapper.get('[data-testid="clip-text-mode-cut"]').trigger('click');
        await wrapper.get('[data-testid="clip-word-1"]').trigger('click');

        const cut = lastState(wrapper).clips[0];
        // "two" (11-11.9 s) and the pause up to "three".
        expect(cut.cuts).toEqual([{ start: 11, end: 12.1 }]);
        expect(wrapper.emitted('update:segments')).toBeUndefined();

        await wrapper.setProps({ state: lastState(wrapper) });
        await wrapper.get('[data-testid="clips-export-selected"]').trigger('click');
        await flushPromises();
        // What goes to tightening (and from there to the export) skips the cut.
        const call = vi.mocked(invoke).mock.calls.find(([command]) => command === 'tighten_clips');
        expect(call?.[1]).toMatchObject({
            segments: [
                {
                    segments: [
                        { start: '00:10.000', end: '00:11.000' },
                        { start: '00:12.100', end: '00:13.000' },
                    ],
                },
            ],
        });
    });

    it('auto-edits new clips when the toggle is on, keeping them if that fails', async () => {
        const defaultInvoke = vi.mocked(invoke).getMockImplementation()!;
        vi.mocked(invoke).mockImplementation((command, args) => {
            if (command === 'suggest_clip_cuts')
                return Promise.resolve([[{ from: 1, to: 1, reason: 'Wiederholung' }]]);
            return defaultInvoke(command, args);
        });
        const wrapper = mountWith({ autoEdit: true });
        await wrapper.get('[data-testid="clips-generate"]').trigger('click');
        await flushPromises();

        const call = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'suggest_clip_cuts');
        expect(call?.[1]).toMatchObject({
            clips: [
                {
                    title: 'The reveal',
                    looped: false,
                    words: [
                        { text: 'one', speaker: 'Host', splice: false },
                        { text: 'two', speaker: 'Host', splice: false },
                        { text: 'three', speaker: 'Host', splice: false },
                    ],
                },
            ],
        });
        const [clip] = lastState(wrapper).clips;
        expect(clip.cutWords).toEqual([
            { start: 11, end: 11.9, auto: true, reason: 'Wiederholung' },
        ]);
        expect(wrapper.emitted('update:status')!.at(-1)).toEqual([
            'Found 1 clip; auto editing cut 1 word.',
        ]);

        vi.mocked(invoke).mockImplementation((command, args) => {
            if (command === 'suggest_clip_cuts') return Promise.reject(new Error('quota'));
            return defaultInvoke(command, args);
        });
        await wrapper.get('[data-testid="clips-generate"]').trigger('click');
        await flushPromises();
        expect(lastState(wrapper).clips).toHaveLength(1);
        expect(wrapper.emitted('update:status')!.at(-1)?.[0]).toContain(
            'Found 1 clip, but auto editing failed',
        );
    });

    it("doesn't auto-edit when the toggle is off", async () => {
        const wrapper = mountWith();
        await wrapper.get('[data-testid="clips-generate"]').trigger('click');
        await flushPromises();
        const commands = vi.mocked(invoke).mock.calls.map(([command]) => command);
        expect(commands).not.toContain('suggest_clip_cuts');
    });

    it('lists detected faces and saves turning one off or naming one', async () => {
        const [clip] = fromCandidates([candidate]);
        const anchor = { time: 11, x: 0.8, y: 0.3 };
        vi.mocked(invoke).mockImplementation((command) => {
            if (command === 'begin_run') return Promise.resolve(123);
            if (command === 'detect_vertical_faces')
                return Promise.resolve([
                    {
                        anchors: [anchor],
                        thumbnail: 'data:image/png;base64,AA==',
                        speaker: 'Host',
                        confident: false,
                        views: 1,
                        seconds: 3,
                        applied: null,
                    },
                ]);
            return Promise.resolve(null);
        });
        const wrapper = mountWith({ clips: [clip], vertical: true });

        await wrapper.get('[data-testid="clips-detect-faces"]').trigger('click');
        await flushPromises();
        const call = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'detect_vertical_faces');
        expect(call?.[1]).toMatchObject({ request: { faces: [] } });
        // The search is kept in the state, with the clips it was made for.
        expect(lastState(wrapper).faceSearch?.signature).toContain('/tmp/source.mp4|');
        await wrapper.setProps({ state: lastState(wrapper) });
        const speaker = wrapper.get('[data-testid="clips-face-speaker-0"]');
        expect(speaker.text()).toContain('Auto (Host, unsure)');

        await wrapper.get('[data-testid="clips-face-speaker-0"]').setValue('named:Host');
        expect(lastState(wrapper).faces).toEqual([
            { anchors: [anchor], ignored: false, speaker: { kind: 'named', name: 'Host' } },
        ]);
        // The face now points at its override.
        expect(lastState(wrapper).faceSearch?.faces[0].applied).toBe(0);

        await wrapper.setProps({ state: lastState(wrapper) });
        await wrapper.get('[data-testid="clips-face-enabled-0"]').setValue(false);
        // The same override changes; the face doesn't get a second one.
        expect(lastState(wrapper).faces).toEqual([
            {
                anchors: [anchor, anchor],
                ignored: true,
                speaker: { kind: 'named', name: 'Host' },
            },
        ]);
        await wrapper.setProps({ state: lastState(wrapper) });
        expect(
            (wrapper.get('[data-testid="clips-face-speaker-0"]').element as HTMLSelectElement)
                .disabled,
        ).toBe(true);
    });

    it('previews the planned 9:16 framing when vertical is on', async () => {
        const [clip] = fromCandidates([candidate]);
        vi.mocked(invoke).mockImplementation((command) => {
            if (command === 'begin_run') return Promise.resolve(123);
            if (command === 'plan_vertical_clips')
                return Promise.resolve({
                    sourceWidth: 1280,
                    sourceHeight: 720,
                    outputWidth: 1080,
                    outputHeight: 1920,
                    captions: [[[{ start: 10, end: 10.8, text: 'one' }]]],
                    clips: [
                        [
                            {
                                start: 10,
                                end: 13,
                                fit: false,
                                keys: [{ time: 0, centerX: 640, centerY: 360, height: 480 }],
                                zoom: 1,
                            },
                        ],
                    ],
                });
            return Promise.resolve(null);
        });
        const wrapper = mountWith({ clips: [clip], vertical: true });
        const video = wrapper.get('[data-testid="clips-player"]').element as HTMLVideoElement;
        video.play = () => Promise.resolve();

        await wrapper.get(`[data-testid="clip-preview-${clip.id}"]`).trigger('click');
        await flushPromises();

        const call = vi
            .mocked(invoke)
            .mock.calls.find(([command]) => command === 'plan_vertical_clips');
        expect(call?.[1]).toMatchObject({
            request: {
                inputPath: '/tmp/source.mp4',
                segments: [{ segments: [{ start: '00:10.000', end: '00:13.000' }] }],
                turns: [{ start: 10, end: 13, speaker: 'Host' }],
            },
        });
        // 9:16 frame; the 270x480 crop at (505, 120) fills it 1:1.
        const frame = wrapper.get('[data-testid="clips-player-frame"]').element as HTMLElement;
        expect(frame.style.width).toBe('270px');
        expect(frame.style.height).toBe('480px');
        expect(video.style.left).toBe('-505px');
        expect(video.style.top).toBe('-120px');
        expect(video.controls).toBe(false);
        // Captions as the export burns them in.
        expect(wrapper.get('[data-testid="clips-player-caption"]').text()).toBe('one');

        // Stopping returns to the normal player.
        await wrapper.get(`[data-testid="clip-preview-${clip.id}"]`).trigger('click');
        expect(frame.style.width).toBe('');
        expect(video.controls).toBe(true);
    });

    it('does not start while another job is running', async () => {
        const wrapper = mountWith({}, true);
        expect(wrapper.get('[data-testid="clips-generate"]').attributes('disabled')).toBeDefined();
    });
});
