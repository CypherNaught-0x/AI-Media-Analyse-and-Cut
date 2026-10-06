import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import ClipText from '../ClipText.vue';
import type { ClipWord } from '../../utils/shortClips';

const word = (index: number, start: number, text: string, speaker = 'Host'): ClipWord => ({
    segment: 0,
    index,
    start,
    end: start + 1,
    text,
    speaker,
});

const words = [
    word(0, 10, 'So'),
    word(1, 11, 'äh'),
    word(2, 12, 'yes'),
    word(3, 13, 'Right', 'Guest'),
];

describe('ClipText', () => {
    it('highlights the spoken word, strikes cut ones and labels speakers', () => {
        const wrapper = mount(ClipText, {
            props: {
                words,
                // Tightening cut the filler at 11-12 s.
                kept: [
                    { start: 10, end: 11 },
                    { start: 12, end: 14 },
                ],
                playhead: 12.4,
            },
        });
        expect(wrapper.get('[data-testid="clip-word-2"]').classes()).toContain('bg-[#ffd60a]');
        expect(wrapper.get('[data-testid="clip-word-1"]').classes()).toContain('line-through');
        expect(wrapper.get('[data-testid="clip-word-0"]').classes()).not.toContain('line-through');
        expect(wrapper.text()).toMatch(/Host.*So.*Guest.*Right/);
    });

    it('cancels an edit on Escape and ignores unchanged text', async () => {
        const wrapper = mount(ClipText, { props: { words, kept: null, playhead: null } });
        await wrapper.get('[data-testid="clip-word-0"]').trigger('click');
        await wrapper.get('[data-testid="clip-text-input"]').setValue('Nope');
        await wrapper.get('[data-testid="clip-text-input"]').trigger('keydown', { key: 'Escape' });
        expect(wrapper.find('[data-testid="clip-text-input"]').exists()).toBe(false);

        await wrapper.get('[data-testid="clip-word-0"]').trigger('click');
        await wrapper.get('[data-testid="clip-text-input"]').trigger('keydown', { key: 'Enter' });
        expect(wrapper.emitted('edit')).toBeUndefined();
    });

    it('cuts words in cut mode, a range with Shift, and restores them', async () => {
        const wrapper = mount(ClipText, {
            props: { words, kept: null, playhead: null, cutWords: [] },
        });
        await wrapper.get('[data-testid="clip-text-mode-cut"]').trigger('click');
        await wrapper.get('[data-testid="clip-word-0"]').trigger('click');
        await wrapper.get('[data-testid="clip-word-2"]').trigger('click', { shiftKey: true });
        const cuts = wrapper.emitted('cut')!;
        expect(cuts[0]).toEqual([[words[0]], true]);
        expect(cuts[1]).toEqual([[words[0], words[1], words[2]], true]);
        expect(wrapper.find('[data-testid="clip-text-input"]').exists()).toBe(false);

        await wrapper.setProps({ cutWords: [{ start: 11, end: 12 }] });
        expect(wrapper.get('[data-testid="clip-word-1"]').attributes('data-cut')).toBe('true');
        await wrapper.get('[data-testid="clip-word-1"]').trigger('click');
        expect(wrapper.emitted('cut')!.at(-1)).toEqual([[words[1]], false]);
        await wrapper.get('[data-testid="clip-text-restore"]').trigger('click');
        expect(wrapper.emitted('cut')!.at(-1)).toEqual([[words[1]], false]);
    });

    it('turns off autocorrect and capitalisation while editing', async () => {
        const wrapper = mount(ClipText, { props: { words, kept: null, playhead: null } });
        await wrapper.get('[data-testid="clip-word-0"]').trigger('click');
        const input = wrapper.get('[data-testid="clip-text-input"]');
        expect(input.attributes()).toMatchObject({
            autocapitalize: 'off',
            autocorrect: 'off',
            spellcheck: 'false',
        });
    });
});
