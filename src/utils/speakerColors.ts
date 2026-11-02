// Stable per-speaker colours from the design system's speaker palette
// (--color-speaker-1..8). The same speaker keeps its colour everywhere: it is
// picked by the speaker's position in the transcript's speaker order.

export const SPEAKER_COLOR_COUNT = 8;

/** CSS colour (a theme variable) for `speaker`, given the ordered speakers. */
export function speakerColor(speaker: string, speakers: readonly string[]): string {
    let index = speakers.indexOf(speaker);
    if (index < 0) {
        // Unknown speaker: hash the name so it's still stable.
        index = Array.from(speaker).reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7);
    }
    return `var(--color-speaker-${(index % SPEAKER_COLOR_COUNT) + 1})`;
}
