import type { PreviewKey, PreviewPiece, PreviewWord, VerticalPreview } from '../bindings';

/** A crop of the source, in source pixels. */
export interface CropRect {
    x: number;
    y: number;
    width: number;
    height: number;
}

/** Where the `<video>` element goes inside the 9:16 frame (CSS pixels). */
export interface VideoBox {
    width: number;
    height: number;
    left: number;
    top: number;
}

/** The piece playing at source time `time`, if any. */
export function pieceAt(pieces: PreviewPiece[], time: number): PreviewPiece | null {
    return pieces.find((piece) => piece.start <= time && time < piece.end) ?? null;
}

/**
 * The crop at `time` (relative to the piece start): linear between keys,
 * clamped to the source. Mirrors `crop_at` in `reframe.rs`, without the
 * even-pixel rounding the encoder needs.
 */
export function cropAt(
    keys: PreviewKey[],
    time: number,
    source: { width: number; height: number },
    aspect: number,
): CropRect {
    const next = keys.findIndex((key) => key.time > time);
    let key: PreviewKey;
    if (next === -1) key = keys[keys.length - 1];
    else if (next === 0) key = keys[0];
    else {
        const a = keys[next - 1];
        const b = keys[next];
        const t = (time - a.time) / (b.time - a.time);
        const lerp = (x: number, y: number) => x + (y - x) * t;
        key = {
            time,
            centerX: lerp(a.centerX, b.centerX),
            centerY: lerp(a.centerY, b.centerY),
            height: lerp(a.height, b.height),
        };
    }

    let height = Math.min(key.height, source.height);
    let width = height * aspect;
    if (width > source.width) {
        width = source.width;
        height = width / aspect;
    }
    const clamp = (value: number, max: number) => Math.min(Math.max(value, 0), max);
    return {
        x: clamp(key.centerX - width / 2, source.width - width),
        y: clamp(key.centerY - height / 2, source.height - height),
        width,
        height,
    };
}

/**
 * Size and offset of the source video inside a frame of `frame` CSS pixels
 * so that it shows what the export will show at source time `time`: the
 * crop scaled to fill the frame, or for `fit` pieces (and gaps) the whole
 * picture fitted to the width. A piece's `zoom` (punch-in) tightens either.
 */
export function videoBox(
    plan: Pick<VerticalPreview, 'sourceWidth' | 'sourceHeight'>,
    pieces: PreviewPiece[],
    time: number,
    frame: { width: number; height: number },
): VideoBox {
    const source = { width: plan.sourceWidth, height: plan.sourceHeight };
    const piece = pieceAt(pieces, time);
    const zoom = Math.max(piece?.zoom ?? 1, 1);
    if (!piece || piece.fit || piece.keys.length === 0) {
        const scale = Math.min(frame.width / source.width, frame.height / source.height) * zoom;
        return {
            width: source.width * scale,
            height: source.height * scale,
            left: (frame.width - source.width * scale) / 2,
            top: (frame.height - source.height * scale) / 2,
        };
    }
    const keys = piece.keys.map((key) => ({ ...key, height: key.height / zoom }));
    const crop = cropAt(keys, time - piece.start, source, frame.width / frame.height);
    const scale = frame.height / crop.height;
    return {
        width: source.width * scale,
        height: source.height * scale,
        left: -crop.x * scale,
        top: -crop.y * scale,
    };
}

/** A caption word as shown: highlighted while it is being spoken. */
export interface ShownWord {
    text: string;
    active: boolean;
}

/** Captions stay up this long (seconds) after their last word... */
const CAPTION_HOLD = 0.4;

/**
 * The caption chunk on screen at source time `time`, with the word being
 * spoken highlighted (the last one that has started), as the export burns
 * it in. A chunk stays up after its last word until the next one starts,
 * at most `CAPTION_HOLD`.
 */
export function captionAt(chunks: PreviewWord[][], time: number): ShownWord[] | null {
    const index = chunks.findIndex((chunk, i) => {
        const start = chunk[0]?.start ?? Infinity;
        const last = chunk[chunk.length - 1]?.end ?? -Infinity;
        const next = chunks[i + 1]?.[0]?.start ?? Infinity;
        return start <= time && time < Math.min(last + CAPTION_HOLD, next);
    });
    if (index === -1) return null;
    const chunk = chunks[index];
    let active = 0;
    chunk.forEach((word, i) => {
        if (word.start <= time) active = i;
    });
    return chunk.map((word, i) => ({ text: word.text, active: i === active }));
}
