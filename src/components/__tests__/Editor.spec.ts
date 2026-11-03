import { describe, it, expect, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { nextTick } from 'vue';
import Editor from '../Editor.vue';
import type { TranscriptSegment } from '../../types';
import { ask } from '@tauri-apps/plugin-dialog';

// Mock Tauri dialog plugin
vi.mock('@tauri-apps/plugin-dialog', () => ({
    ask: vi.fn(() => Promise.resolve(true)),
}));

describe('Editor.vue', () => {
    const mockSegments: TranscriptSegment[] = [
        { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'Hello world' },
        { start: '00:10', end: '00:20', speaker: 'Speaker 2', text: 'How are you?' },
    ];

    it('renders segments correctly', () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
            },
        });

        const segments = wrapper.findAll('.segment');
        expect(segments).toHaveLength(2);
        expect(wrapper.text()).toContain('Hello world');
        expect(wrapper.text()).toContain('How are you?');
    });

    it('sizes the time gutter to the timestamps and shows milliseconds on focus only', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: [
                    { start: '01:18.400', end: '01:22.640', speaker: 'Speaker 1', text: 'Heute' },
                    {
                        start: '01:01:06.720',
                        end: '01:01:09.000',
                        speaker: 'Speaker 1',
                        text: 'Später',
                    },
                ],
            },
        });

        // 12 characters (hh:mm:ss.mmm) of 11px monospace, plus slack.
        const list = wrapper.get('.segment').element.parentElement as HTMLElement;
        expect(list.style.getPropertyValue('--time-col')).toBe('5.075rem');

        const fraction = wrapper.findAll('[data-testid="segment-time-fraction"]');
        expect(fraction[0].text()).toBe('.400');
        expect(fraction[0].classes()).toContain('opacity-0');
        expect(fraction[0].classes()).toContain('group-hover:opacity-100');
        expect(wrapper.findAll('.segment')[0].text()).toContain('01:18.400');
    });

    it('does not control playback when a segment body is clicked', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                videoAvailable: true,
                audioAvailable: true,
            },
        });

        await wrapper.find('.segment').trigger('click');

        expect(wrapper.emitted('jump-to')).toBeFalsy();
        expect(wrapper.emitted('preview')).toBeFalsy();
        expect(wrapper.emitted('preview-video')).toBeFalsy();
    });

    it('emits jump-to from the "start from here" button using the segment start', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                videoAvailable: true,
            },
        });

        await wrapper.get('[data-testid="segment-play-from-1"]').trigger('click');

        expect(wrapper.emitted('jump-to')).toBeTruthy();
        expect(wrapper.emitted('jump-to')![0]).toEqual([10]); // 00:10 is 10 seconds
    });

    it('emits a video preview request with the segment bounds', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                videoAvailable: true,
            },
        });

        await wrapper.get('[data-testid="segment-play-video-0"]').trigger('click');

        expect(wrapper.emitted('preview-video')).toBeTruthy();
        expect(wrapper.emitted('preview-video')![0]).toEqual([
            { start: '00:00', end: '00:10', index: 0 },
        ]);
        expect(wrapper.emitted('jump-to')).toBeFalsy();
    });

    it('hides the video playback buttons when no media file is available', () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                videoAvailable: false,
            },
        });

        expect(wrapper.find('[data-testid="segment-play-video-0"]').exists()).toBe(false);
        expect(wrapper.find('[data-testid="segment-play-from-0"]').exists()).toBe(false);
    });

    it('enters edit mode when edit button is clicked', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
            },
        });

        // The edit button is an icon button that shows on hover/focus; it is
        // always in the DOM.
        const editButton = wrapper.find('[data-testid^="segment-edit-"]');
        expect(editButton.exists()).toBe(true);

        await editButton.trigger('click');

        // Check if inputs appear
        expect(wrapper.find('input[placeholder="MM:SS"]').exists()).toBe(true);
        expect(wrapper.find('textarea').exists()).toBe(true);
    });

    it('offers existing speakers in a dropdown preselected to the current speaker', async () => {
        const wrapper = mount(Editor, {
            props: { segments: mockSegments },
        });

        const editButton = wrapper.find('[data-testid^="segment-edit-"]');
        await editButton.trigger('click');

        const select = wrapper.get('[data-testid="edit-speaker-select"]');
        const optionValues = select.findAll('option').map((o) => o.attributes('value'));
        // Both existing speakers plus the "add new" affordance are offered.
        expect(optionValues).toContain('Speaker 1');
        expect(optionValues).toContain('Speaker 2');
        expect(optionValues).toContain('__add_new_speaker__');
        // The dropdown starts on the segment's current speaker.
        expect((select.element as HTMLSelectElement).value).toBe('Speaker 1');
    });

    it('reassigns a segment to another existing speaker via the dropdown', async () => {
        const wrapper = mount(Editor, {
            props: { segments: mockSegments },
        });

        const editButton = wrapper.find('[data-testid^="segment-edit-"]');
        await editButton.trigger('click');

        await wrapper.get('[data-testid="edit-speaker-select"]').setValue('Speaker 2');
        const saveButton = wrapper.get('[data-testid="edit-save"]');
        await saveButton.trigger('click');

        const emitted = wrapper.emitted('update:segments');
        expect(emitted).toBeTruthy();
        const updated = emitted![emitted!.length - 1][0] as TranscriptSegment[];
        expect(updated[0].speaker).toBe('Speaker 2');
    });

    it('adds a brand-new speaker via the free-text input', async () => {
        const wrapper = mount(Editor, {
            props: { segments: mockSegments },
        });

        const editButton = wrapper.find('[data-testid^="segment-edit-"]');
        await editButton.trigger('click');

        // Switching to the "add new" option reveals a text field in place of the dropdown.
        await wrapper.get('[data-testid="edit-speaker-select"]').setValue('__add_new_speaker__');
        const input = wrapper.get('[data-testid="edit-speaker-input"]');
        expect(wrapper.find('[data-testid="edit-speaker-select"]').exists()).toBe(false);

        await input.setValue('Alice Example');
        const saveButton = wrapper.get('[data-testid="edit-save"]');
        await saveButton.trigger('click');

        const emitted = wrapper.emitted('update:segments');
        expect(emitted).toBeTruthy();
        const updated = emitted![emitted!.length - 1][0] as TranscriptSegment[];
        expect(updated[0].speaker).toBe('Alice Example');
    });

    it('focuses the new-speaker inputs, which live inside the segment v-for', async () => {
        // A plain string `ref` inside a `v-for` collects an array of elements, not
        // the element, so `.focus()` threw an unhandled rejection and the field was
        // never focused. Both inputs use function refs instead.
        const focusSpy = vi.spyOn(HTMLInputElement.prototype, 'focus');

        try {
            const editWrapper = mount(Editor, { props: { segments: mockSegments } });
            const editButton = editWrapper.find('[data-testid^="segment-edit-"]');
            await editButton.trigger('click');
            await editWrapper
                .get('[data-testid="edit-speaker-select"]')
                .setValue('__add_new_speaker__');
            await nextTick();

            expect(focusSpy).toHaveBeenCalledTimes(1);
            expect(focusSpy.mock.instances[0]).toBe(
                editWrapper.get('[data-testid="edit-speaker-input"]').element,
            );

            const splitWrapper = mount(Editor, { props: { segments: mockSegments } });
            await splitWrapper.get('[data-testid="segment-split-0"]').trigger('click');
            await splitWrapper
                .get('[data-testid="split-second-speaker"]')
                .setValue('__add_new_speaker__');
            await nextTick();

            expect(focusSpy).toHaveBeenCalledTimes(2);
            expect(focusSpy.mock.instances[1]).toBe(
                splitWrapper.get('[data-testid="split-second-speaker-input"]').element,
            );
        } finally {
            focusSpy.mockRestore();
        }
    });

    it('keeps the segment header and playback buttons visible while editing', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                audioAvailable: true,
                videoAvailable: true,
            },
        });

        const editButton = wrapper.find('[data-testid^="segment-edit-"]');
        await editButton.trigger('click');

        // The edit form is shown...
        expect(wrapper.find('textarea').exists()).toBe(true);
        // ...but the header info and per-segment playback buttons remain.
        expect(wrapper.text()).toContain('Speaker 1');
        expect(wrapper.find('[data-testid="segment-preview-0"]').exists()).toBe(true);
        expect(wrapper.find('[data-testid="segment-play-video-0"]').exists()).toBe(true);
        expect(wrapper.find('[data-testid="segment-play-from-0"]').exists()).toBe(true);
    });

    it('saves edits correctly', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
            },
        });

        const editButton = wrapper.find('[data-testid^="segment-edit-"]');
        await editButton.trigger('click');

        const textarea = wrapper.find('textarea');
        await textarea.setValue('Hello updated world');

        const saveButton = wrapper.get('[data-testid="edit-save"]');
        await saveButton.trigger('click');

        expect(wrapper.emitted('update:segments')).toBeTruthy();
        const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updatedSegments[0].text).toBe('Hello updated world');
    });

    it('deletes a segment', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
            },
        });

        const deleteButton = wrapper.get('[data-testid="segment-delete-0"]');
        await deleteButton.trigger('click');

        expect(ask).toHaveBeenCalled();
        expect(wrapper.emitted('update:segments')).toBeTruthy();
        const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updatedSegments).toHaveLength(1);
        expect(updatedSegments[0].text).toBe('How are you?');
    });

    it('merges segments', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
            },
        });

        const mergeButton = wrapper.get('[data-testid="segment-merge-0"]');
        await mergeButton.trigger('click');

        expect(wrapper.emitted('update:segments')).toBeTruthy();
        const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updatedSegments).toHaveLength(1);
        expect(updatedSegments[0].text).toBe('Hello world How are you?');
    });

    it('merges only the selected adjacent segments into one', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
            },
        });

        const segs = wrapper.findAll('.segment');
        await segs[0].trigger('click', { shiftKey: true });
        await segs[1].trigger('click', { shiftKey: true });

        const mergeButton = wrapper.get('[data-testid="selection-merge"]');
        await mergeButton.trigger('click');

        const updated = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updated).toHaveLength(1);
        expect(updated[0].start).toBe('00:00');
        expect(updated[0].end).toBe('00:20');
        expect(updated[0].text).toBe('Hello world How are you?');
    });

    it('absorbs unselected in-between segments so the merge stays contiguous', async () => {
        const threeSegments: TranscriptSegment[] = [
            { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'First' },
            { start: '00:10', end: '00:20', speaker: 'Speaker 2', text: 'Second' },
            { start: '00:20', end: '00:30', speaker: 'Speaker 1', text: 'Third' },
        ];

        const wrapper = mount(Editor, {
            props: {
                segments: threeSegments,
            },
        });

        const segs = wrapper.findAll('.segment');
        // Select only the first and last, skipping the middle segment.
        await segs[0].trigger('click', { shiftKey: true });
        await segs[2].trigger('click', { shiftKey: true });

        const mergeButton = wrapper.get('[data-testid="selection-merge"]');
        await mergeButton.trigger('click');

        const updated = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        // The in-between segment is absorbed, leaving a single non-overlapping segment.
        expect(updated).toHaveLength(1);
        expect(updated[0].start).toBe('00:00');
        expect(updated[0].end).toBe('00:30');
        expect(updated[0].text).toBe('First Second Third');
    });

    it('filters to segments that require review below the selected threshold', () => {
        const reviewSegments: TranscriptSegment[] = [
            {
                start: '00:00',
                end: '00:05',
                speaker: 'Speaker 1',
                text: 'Aligned',
                mergeStatus: 'matched',
                similarityScore: 0.96,
            },
            {
                start: '00:05',
                end: '00:10',
                speaker: 'Speaker 2',
                text: 'Needs review',
                mergeStatus: 'matched',
                similarityScore: 0.62,
            },
            {
                start: '00:10',
                end: '00:15',
                speaker: 'Speaker 3',
                text: 'Missing in Google',
                mergeStatus: 'missing_google',
            },
        ];

        const wrapper = mount(Editor, {
            props: {
                segments: reviewSegments,
                showOnlyReviewSegments: true,
                reviewThreshold: 0.8,
            },
        });

        const segments = wrapper.findAll('.segment');
        expect(segments).toHaveLength(2);
        expect(wrapper.text()).toContain('Needs review');
        expect(wrapper.text()).toContain('Missing in Google');
        expect(segments[0].text()).toContain('Needs review');
        expect(segments[1].text()).toContain('Missing in Google');
    });

    it('treats blacklist hits as review items and renders a warning badge', () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                showOnlyReviewSegments: true,
                blacklistMatchesBySegment: {
                    1: [
                        {
                            languageCode: 'de',
                            matchedText: 'Arsch',
                            normalizedWord: 'arsch',
                            segmentIndex: 1,
                            speaker: 'Speaker 2',
                            start: '00:10',
                            end: '00:11',
                            segmentText: 'How are you?',
                        },
                    ],
                },
            },
        });

        const segments = wrapper.findAll('.segment');
        expect(segments).toHaveLength(1);
        expect(segments[0].text()).toContain('blacklist match');
        expect(segments[0].text()).toContain('Matched words: Arsch');
    });

    it('does not show higher-similarity conflict segments below a stricter threshold', () => {
        const reviewSegments: TranscriptSegment[] = [
            {
                start: '00:00',
                end: '00:05',
                speaker: 'Speaker 1',
                text: 'Conflict but acceptable',
                mergeStatus: 'conflict',
                similarityScore: 0.72,
            },
            {
                start: '00:05',
                end: '00:10',
                speaker: 'Speaker 2',
                text: 'Actually low confidence',
                mergeStatus: 'conflict',
                similarityScore: 0.41,
            },
            {
                start: '00:10',
                end: '00:15',
                speaker: 'Speaker 3',
                text: 'Missing in Google',
                mergeStatus: 'missing_google',
            },
        ];

        const wrapper = mount(Editor, {
            props: {
                segments: reviewSegments,
                showOnlyReviewSegments: true,
                reviewThreshold: 0.5,
            },
        });

        const segments = wrapper.findAll('.segment');
        expect(segments).toHaveLength(2);
        expect(wrapper.text()).toContain('Actually low confidence');
        expect(wrapper.text()).toContain('Missing in Google');
        expect(wrapper.text()).not.toContain('Conflict but acceptable');
    });

    it('edits the correct original segment while filtered', async () => {
        const reviewSegments: TranscriptSegment[] = [
            {
                start: '00:00',
                end: '00:05',
                speaker: 'Speaker 1',
                text: 'Aligned',
                mergeStatus: 'matched',
                similarityScore: 0.96,
            },
            {
                start: '00:05',
                end: '00:10',
                speaker: 'Speaker 2',
                text: 'Needs review',
                mergeStatus: 'matched',
                similarityScore: 0.62,
            },
            {
                start: '00:10',
                end: '00:15',
                speaker: 'Speaker 3',
                text: 'Also aligned',
                mergeStatus: 'matched',
                similarityScore: 0.92,
            },
        ];

        const wrapper = mount(Editor, {
            props: {
                segments: reviewSegments,
                showOnlyReviewSegments: true,
                reviewThreshold: 0.8,
            },
        });

        const editButton = wrapper.find('[data-testid^="segment-edit-"]');
        await editButton.trigger('click');

        const textarea = wrapper.find('textarea');
        await textarea.setValue('Updated reviewed segment');

        const saveButton = wrapper.get('[data-testid="edit-save"]');
        await saveButton.trigger('click');

        const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updatedSegments[0].text).toBe('Aligned');
        expect(updatedSegments[1].text).toBe('Updated reviewed segment');
        expect(updatedSegments[2].text).toBe('Also aligned');
    });

    it('selects transcript alternatives against the original segment while filtered', async () => {
        const reviewSegments: TranscriptSegment[] = [
            {
                start: '00:00',
                end: '00:05',
                speaker: 'Speaker 1',
                text: 'Aligned',
                mergeStatus: 'matched',
                similarityScore: 0.96,
            },
            {
                start: '00:05',
                end: '00:10',
                speaker: 'Speaker 2',
                text: 'Parakeet text',
                mergeStatus: 'conflict',
                similarityScore: 0.51,
                alternatives: [
                    { source: 'google', text: 'Google text', speaker: 'Named Speaker' },
                    { source: 'local', text: 'Parakeet text', speaker: 'Speaker 2' },
                ],
            },
        ];

        const wrapper = mount(Editor, {
            props: {
                segments: reviewSegments,
                showOnlyReviewSegments: true,
                reviewThreshold: 0.8,
            },
        });

        const useButton = wrapper.findAll('button').find((button) => button.text() === 'Use');
        await useButton!.trigger('click');

        const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updatedSegments[1].text).toBe('Google text');
        expect(updatedSegments[1].speaker).toBe('Named Speaker');
        expect(updatedSegments[1].activeSource).toBe('google');
    });

    it('replaces all matches in visible transcript segments', async () => {
        const segments: TranscriptSegment[] = [
            { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'world news world' },
            { start: '00:10', end: '00:20', speaker: 'Speaker 2', text: 'around the world' },
        ];

        const wrapper = mount(Editor, {
            props: {
                segments,
            },
        });

        await wrapper.get('[data-testid="editor-search-input"]').setValue('world');
        await wrapper.get('[data-testid="editor-replace-input"]').setValue('planet');
        await wrapper.get('[data-testid="editor-replace-all"]').trigger('click');

        const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updatedSegments[0].text).toBe('planet news planet');
        expect(updatedSegments[1].text).toBe('around the planet');
    });

    it('supports optional whole-word matching for replace all', async () => {
        const segments: TranscriptSegment[] = [
            { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'test contest testing' },
            { start: '00:10', end: '00:20', speaker: 'Speaker 2', text: 'another test case' },
        ];

        const wrapper = mount(Editor, {
            props: {
                segments,
            },
        });

        await wrapper.get('[data-testid="editor-search-input"]').setValue('test');
        await wrapper.get('[data-testid="editor-replace-input"]').setValue('exam');
        await wrapper.get('[data-testid="editor-whole-word-toggle"]').setValue(true);
        await wrapper.get('[data-testid="editor-replace-all"]').trigger('click');

        const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updatedSegments[0].text).toBe('exam contest testing');
        expect(updatedSegments[1].text).toBe('another exam case');
    });

    it('replaces the correct original segment while filtered', async () => {
        const reviewSegments: TranscriptSegment[] = [
            {
                start: '00:00',
                end: '00:05',
                speaker: 'Speaker 1',
                text: 'Aligned',
                mergeStatus: 'matched',
                similarityScore: 0.96,
            },
            {
                start: '00:05',
                end: '00:10',
                speaker: 'Speaker 2',
                text: 'Needs review review',
                mergeStatus: 'matched',
                similarityScore: 0.62,
            },
            {
                start: '00:10',
                end: '00:15',
                speaker: 'Speaker 3',
                text: 'Also aligned',
                mergeStatus: 'matched',
                similarityScore: 0.92,
            },
        ];

        const wrapper = mount(Editor, {
            props: {
                segments: reviewSegments,
                showOnlyReviewSegments: true,
                reviewThreshold: 0.8,
            },
        });

        await wrapper.get('[data-testid="editor-search-input"]').setValue('review');
        await wrapper.get('[data-testid="editor-replace-input"]').setValue('checked');
        await wrapper.get('[data-testid="editor-replace-all"]').trigger('click');

        const updatedSegments = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updatedSegments[0].text).toBe('Aligned');
        expect(updatedSegments[1].text).toBe('Needs checked checked');
        expect(updatedSegments[2].text).toBe('Also aligned');
    });

    it('emits a preview request with the segment timestamps when audio is available', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                audioAvailable: true,
            },
        });

        const previewButton = wrapper.get('[data-testid="segment-preview-1"]');
        await previewButton.trigger('click');

        expect(wrapper.emitted('preview')).toBeTruthy();
        expect(wrapper.emitted('preview')![0]).toEqual([
            { start: '00:10', end: '00:20', index: 1 },
        ]);
        // Previewing must not double as a jump-to seek on the segment body.
        expect(wrapper.emitted('jump-to')).toBeFalsy();
    });

    it('hides the preview button when no extracted audio is available', () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                audioAvailable: false,
            },
        });

        expect(wrapper.find('[data-testid="segment-preview-0"]').exists()).toBe(false);
    });

    it('splits a segment at the chosen point, partitioning word-timed text', async () => {
        const wordSegments: TranscriptSegment[] = [
            {
                start: '00:00',
                end: '00:10',
                speaker: 'Speaker 1',
                text: 'hello world',
                words: [
                    { start: '00:00', end: '00:01', text: 'hello' },
                    { start: '00:05', end: '00:06', text: 'world' },
                ],
            },
        ];

        const wrapper = mount(Editor, { props: { segments: wordSegments } });

        await wrapper.get('[data-testid="segment-split-0"]').trigger('click');
        await wrapper.get('[data-testid="split-time-input"]').setValue(3);
        await wrapper.get('[data-testid="confirm-split"]').trigger('click');

        const updated = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updated).toHaveLength(2);

        expect(updated[0].start).toBe('00:00');
        expect(updated[0].end).toBe('00:03.000');
        expect(updated[0].text).toBe('hello');
        expect(updated[0].words).toEqual([{ start: '00:00', end: '00:01', text: 'hello' }]);

        expect(updated[1].start).toBe('00:03.000');
        expect(updated[1].end).toBe('00:10');
        expect(updated[1].text).toBe('world');
        expect(updated[1].speaker).toBe('Speaker 1');
    });

    it('assigns a new speaker to the second half when splitting', async () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });

        await wrapper.get('[data-testid="segment-split-0"]').trigger('click');
        await wrapper.get('[data-testid="split-time-input"]').setValue(5);
        await wrapper.get('[data-testid="split-second-speaker"]').setValue('Speaker 2');
        await wrapper.get('[data-testid="confirm-split"]').trigger('click');

        const updated = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updated).toHaveLength(3);
        expect(updated[0].speaker).toBe('Speaker 1');
        expect(updated[1].start).toBe('00:05.000');
        expect(updated[1].speaker).toBe('Speaker 2');
        // The untouched segment is preserved after the split pair.
        expect(updated[2].text).toBe('How are you?');
    });

    it('adds a brand-new speaker to the second half when splitting', async () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });

        await wrapper.get('[data-testid="segment-split-0"]').trigger('click');
        await wrapper.get('[data-testid="split-time-input"]').setValue(5);
        await wrapper.get('[data-testid="split-second-speaker"]').setValue('__add_new_speaker__');
        await wrapper.get('[data-testid="split-second-speaker-input"]').setValue('Bob Example');
        await wrapper.get('[data-testid="confirm-split"]').trigger('click');

        const updated = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updated).toHaveLength(3);
        expect(updated[1].speaker).toBe('Bob Example');
    });

    it('disables the split confirmation when the point is at a boundary', async () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });

        await wrapper.get('[data-testid="segment-split-0"]').trigger('click');
        await wrapper.get('[data-testid="split-time-input"]').setValue(0); // equal to start

        expect(wrapper.get('[data-testid="confirm-split"]').attributes('disabled')).toBeDefined();
    });

    it('captures the playback position as the split point', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                getPlayhead: () => 4.2,
            },
        });

        await wrapper.get('[data-testid="segment-split-0"]').trigger('click');
        await wrapper.get('[data-testid="split-use-playhead"]').trigger('click');

        expect(wrapper.get('[data-testid="split-time-display"]').text()).toBe('00:04.200');
    });

    it('hides segments for speakers marked invisible', () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                speakerVisibility: {
                    'Speaker 1': false,
                    'Speaker 2': true,
                },
            },
        });

        const segments = wrapper.findAll('.segment');
        expect(segments).toHaveLength(1);
        expect(wrapper.text()).not.toContain('Hello world');
        expect(wrapper.text()).toContain('How are you?');
    });
    it('only allows wording edits when the structure is locked', async () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                lockStructure: true,
            },
        });

        expect(wrapper.find('[data-testid="segment-split-0"]').exists()).toBe(false);
        expect(wrapper.find('[data-testid="segment-delete-0"]').exists()).toBe(false);
        expect(wrapper.find('[data-testid^="segment-merge-"]').exists()).toBe(false);

        const editButton = wrapper.find('[data-testid^="segment-edit-"]');
        await editButton.trigger('click');
        expect(wrapper.find('input[placeholder="MM:SS"]').exists()).toBe(false);
        expect(wrapper.find('[data-testid="edit-speaker-select"]').exists()).toBe(false);

        await wrapper.find('textarea').setValue('Hola mundo');
        await wrapper.get('[data-testid="edit-save"]').trigger('click');
        const emitted = wrapper.emitted('update:segments')!.slice(-1)[0][0] as TranscriptSegment[];
        expect(emitted.map((segment) => segment.text)).toEqual(['Hola mundo', 'How are you?']);
    });
    it('makes a plainly clicked row current without emitting, and keeps shift-click for selection', async () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });
        const rows = wrapper.findAll('.segment');

        await rows[1].trigger('click');
        expect(rows[1].attributes('data-active')).toBe('true');
        expect(rows[0].attributes('data-active')).toBeUndefined();
        expect(wrapper.emitted()).not.toHaveProperty('update:segments');
        expect(wrapper.find('[data-testid="selection-bar"]').exists()).toBe(false);

        await rows[0].trigger('click', { shiftKey: true });
        // Shift-click selects but does not move the current row.
        expect(rows[1].attributes('data-active')).toBe('true');
        expect(wrapper.get('[data-testid="selection-bar"]').text()).toContain('1 selected');
    });

    it('moves between rows with the arrow keys and opens the editor with Enter', async () => {
        const wrapper = mount(Editor, {
            props: { segments: mockSegments },
            attachTo: document.body,
        });
        const rows = wrapper.findAll('.segment');

        (rows[0].element as HTMLElement).focus();
        await rows[0].trigger('keydown', { key: 'ArrowDown' });
        expect(document.activeElement).toBe(rows[1].element);
        expect(rows[1].attributes('data-active')).toBe('true');

        await rows[1].trigger('keydown', { key: 'ArrowUp' });
        expect(document.activeElement).toBe(rows[0].element);

        await rows[0].trigger('keydown', { key: 'Enter' });
        await nextTick();
        const textarea = wrapper.get('textarea');
        expect((textarea.element as HTMLTextAreaElement).value).toBe('Hello world');
        expect(document.activeElement).toBe(textarea.element);
        wrapper.unmount();
    });

    it('toggles selection with Space and clears it from the selection bar', async () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });
        const rows = wrapper.findAll('.segment');

        await rows[0].trigger('keydown', { key: ' ' });
        await rows[1].trigger('keydown', { key: ' ' });
        expect(wrapper.get('[data-testid="selection-bar"]').text()).toContain('2 selected');

        await wrapper.get('[data-testid="selection-clear"]').trigger('click');
        expect(wrapper.find('[data-testid="selection-bar"]').exists()).toBe(false);
    });

    it('needs two selected segments before merging', async () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });
        await wrapper.findAll('.segment')[0].trigger('click', { shiftKey: true });

        expect(wrapper.get('[data-testid="selection-merge"]').attributes('disabled')).toBeDefined();
    });

    it('saves the edit form with Cmd+Enter and cancels it with Escape', async () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });

        await wrapper.get('[data-testid="segment-edit-0"]').trigger('click');
        await wrapper.get('textarea').setValue('Saved by keyboard');
        await wrapper.get('textarea').trigger('keydown', { key: 'Enter', metaKey: true });
        const updated = wrapper.emitted('update:segments')![0][0] as TranscriptSegment[];
        expect(updated[0].text).toBe('Saved by keyboard');
        expect(wrapper.find('textarea').exists()).toBe(false);

        await wrapper.get('[data-testid="segment-edit-1"]').trigger('click');
        await wrapper.get('textarea').trigger('keydown', { key: 'Escape' });
        expect(wrapper.find('textarea').exists()).toBe(false);
        expect(wrapper.emitted('update:segments')).toHaveLength(1);
    });

    it('steps through matches with Enter and Shift+Enter and highlights the current one', async () => {
        const segments: TranscriptSegment[] = [
            { start: '00:00', end: '00:10', speaker: 'Speaker 1', text: 'one <b>one</b>' },
            { start: '00:10', end: '00:20', speaker: 'Speaker 2', text: 'one more' },
        ];
        const wrapper = mount(Editor, { props: { segments } });
        const search = wrapper.get('[data-testid="editor-search-input"]');

        await search.setValue('one');
        const counter = () => wrapper.get('[data-testid="editor-search-counter"]').text();
        expect(counter()).toBe('1 of 3');

        await search.trigger('keydown', { key: 'Enter' });
        expect(counter()).toBe('2 of 3');
        await search.trigger('keydown', { key: 'Enter', shiftKey: true });
        await search.trigger('keydown', { key: 'Enter', shiftKey: true });
        expect(counter()).toBe('3 of 3');

        const marks = wrapper.findAll('mark');
        expect(marks).toHaveLength(3);
        expect(marks[2].classes()).toContain('bg-accent');
        expect(marks[0].classes()).toContain('bg-warning/20');
        // Segment text stays escaped around the highlights.
        expect(wrapper.findAll('.segment')[0].html()).toContain('&lt;b&gt;');
        expect(wrapper.find('.segment b').exists()).toBe(false);

        await search.trigger('keydown', { key: 'Escape' });
        expect((search.element as HTMLInputElement).value).toBe('');
    });

    it('shows an empty state when no segment is visible', () => {
        const wrapper = mount(Editor, {
            props: {
                segments: mockSegments,
                speakerVisibility: { 'Speaker 1': false, 'Speaker 2': false },
            },
        });

        expect(wrapper.findAll('.segment')).toHaveLength(0);
        expect(wrapper.get('[data-testid="editor-empty"]').text()).toContain('No segments to show');
    });

    it('marks the row under the playhead', () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments, playhead: 12.5 } });
        const rows = wrapper.findAll('.segment');

        expect(rows[0].attributes('data-playing')).toBeUndefined();
        expect(rows[1].attributes('data-playing')).toBe('true');
    });

    it('colours speaker names by the given speaker order', () => {
        const wrapper = mount(Editor, {
            props: { segments: mockSegments, speakerOrder: ['Speaker 2', 'Speaker 1'] },
        });

        const name = wrapper
            .findAll('.segment')[0]
            .findAll('span')
            .find((span) => span.text() === 'Speaker 1');
        expect(name?.attributes('style')).toContain('var(--color-speaker-2)');
    });

    it('renders the language controls passed into the toolbar slot', () => {
        const wrapper = mount(Editor, {
            props: { segments: mockSegments },
            slots: { toolbar: '<button data-testid="slot-control">Translate</button>' },
        });

        expect(wrapper.find('[data-testid="slot-control"]').exists()).toBe(true);
    });
    it('names each row by its time and describes it by its text', () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });
        const row = wrapper.findAll('.segment')[0];

        expect(row.attributes('role')).toBe('article');
        expect(row.attributes('aria-label')).toBeUndefined();
        const label = wrapper.get(`[id="${row.attributes('aria-labelledby')}"]`);
        expect(label.text()).toContain('Speaker 1');
        expect(label.text()).toContain('00:00');
        const description = wrapper.get(`[id="${row.attributes('aria-describedby')}"]`);
        expect(description.text()).toBe('Hello world');
    });

    it('badges selected rows and keeps the selection tint off search results', async () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });
        const rows = wrapper.findAll('.segment');

        await rows[0].trigger('click', { shiftKey: true });
        expect(rows[0].get('[title="Selected"]').text()).toContain('selected');
        expect(rows[0].classes()).toContain('border-l-accent');

        await wrapper.get('[data-testid="editor-search-input"]').setValue('How');
        expect(rows[1].classes()).toContain('border-l-warning');
        expect(rows[1].classes()).not.toContain('border-l-accent');
        expect(rows[1].find('[title="Selected"]').exists()).toBe(false);
    });

    it("shows the current row's actions and reserves room for the selection bar", async () => {
        const wrapper = mount(Editor, { props: { segments: mockSegments } });
        const rows = wrapper.findAll('.segment');
        const toolbar = () => rows[0].get('[role="toolbar"]');

        expect(toolbar().classes()).toContain('hidden');
        await rows[0].trigger('click');
        expect(toolbar().classes()).toContain('flex');
        expect(toolbar().classes()).not.toContain('hidden');

        await rows[1].trigger('click', { shiftKey: true });
        expect(wrapper.find('[data-testid="selection-bar"]').exists()).toBe(true);
        expect(rows[0].element.parentElement?.classList.contains('pb-28')).toBe(true);
    });

    it('renders the status slot in the toolbar', () => {
        const wrapper = mount(Editor, {
            props: { segments: mockSegments },
            slots: { status: '<span data-testid="slot-status">2 segments</span>' },
        });

        expect(wrapper.get('[data-testid="slot-status"]').text()).toBe('2 segments');
    });
});
