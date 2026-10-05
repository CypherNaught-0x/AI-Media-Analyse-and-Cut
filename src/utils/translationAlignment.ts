import type { TranscriptSegment } from '../types';
import { parseTime } from '../composables/useTimeFormat';

function sameStructure(original: TranscriptSegment[], translation: TranscriptSegment[]): boolean {
    return (
        original.length === translation.length &&
        original.every(
            (segment, index) =>
                segment.start === translation[index].start &&
                segment.end === translation[index].end,
        )
    );
}

function safeParse(time: string): number {
    try {
        return parseTime(time);
    } catch {
        return Number.NaN;
    }
}

/**
 * Re-fit a translation onto the current structure of the original transcript.
 *
 * Translations keep the original segments' timestamps, so after a split, merge
 * or delete in the original the translated segments can be matched by time
 * instead of by (now shifted) index: each original segment takes the text of
 * every translated segment whose midpoint falls inside it. A segment with no
 * match (e.g. the second half of a split) falls back to the original wording
 * until it is translated again. Timing and speaker always come from the
 * original, so the cut and the preview stay authoritative.
 */
export function realignTranslation(
    original: TranscriptSegment[],
    translation: TranscriptSegment[],
): TranscriptSegment[] {
    if (sameStructure(original, translation)) {
        return translation;
    }

    const translated = translation
        .map((segment) => ({
            midpoint: (safeParse(segment.start) + safeParse(segment.end)) / 2,
            text: segment.text,
        }))
        .filter((segment) => Number.isFinite(segment.midpoint));

    return original.map((segment, index) => {
        const start = safeParse(segment.start);
        const end = safeParse(segment.end);
        const isLast = index === original.length - 1;
        const texts = translated
            .filter(
                ({ midpoint }) =>
                    midpoint >= start && (midpoint < end || (isLast && midpoint <= end)),
            )
            .map(({ text }) => text.trim())
            .filter(Boolean);

        return {
            start: segment.start,
            end: segment.end,
            speaker: segment.speaker,
            text: texts.length > 0 ? texts.join(' ') : segment.text,
        };
    });
}
