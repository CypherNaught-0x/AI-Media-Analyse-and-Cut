import { describe, expect, it } from 'vitest';
import { realignTranslation } from '../translationAlignment';
import type { TranscriptSegment } from '../../types';

const seg = (
    start: string,
    end: string,
    text: string,
    speaker = 'Speaker 1',
): TranscriptSegment => ({
    start,
    end,
    speaker,
    text,
});

const original = [
    seg('00:00', '00:04', 'Hello there.'),
    seg('00:04', '00:09', 'How are you?', 'Speaker 2'),
    seg('00:09', '00:13', 'Fine, thanks.'),
];
const spanish = [
    seg('00:00', '00:04', 'Hola.'),
    seg('00:04', '00:09', '¿Qué tal?', 'Speaker 2'),
    seg('00:09', '00:13', 'Bien, gracias.'),
];

describe('realignTranslation', () => {
    it('returns the translation untouched when the structure is unchanged', () => {
        expect(realignTranslation(original, spanish)).toBe(spanish);
    });

    it('drops the translation of a deleted segment instead of shifting the rest', () => {
        const afterDelete = [original[0], original[2]];
        expect(realignTranslation(afterDelete, spanish).map((s) => s.text)).toEqual([
            'Hola.',
            'Bien, gracias.',
        ]);
    });

    it('joins translations when segments are merged', () => {
        const afterMerge = [seg('00:00', '00:09', 'Hello there. How are you?'), original[2]];
        expect(realignTranslation(afterMerge, spanish).map((s) => s.text)).toEqual([
            'Hola. ¿Qué tal?',
            'Bien, gracias.',
        ]);
    });

    it('falls back to the original wording for the untranslated half of a split', () => {
        const afterSplit = [
            seg('00:00', '00:01.5', 'Hello'),
            seg('00:01.5', '00:04', 'there.'),
            original[1],
            original[2],
        ];
        expect(realignTranslation(afterSplit, spanish).map((s) => s.text)).toEqual([
            'Hello',
            'Hola.',
            '¿Qué tal?',
            'Bien, gracias.',
        ]);
    });

    it('takes timing and speaker from the original', () => {
        const retimed = [seg('00:00', '00:05', 'Hello there.', 'Host'), original[1], original[2]];
        const [first] = realignTranslation(retimed, spanish);
        expect(first).toEqual(seg('00:00', '00:05', 'Hola.', 'Host'));
    });
});
