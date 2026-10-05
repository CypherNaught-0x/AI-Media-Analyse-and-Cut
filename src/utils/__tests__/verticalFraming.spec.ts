import { describe, expect, it } from 'vitest';
import type { PreviewPiece } from '../../bindings';
import { cropAt, pieceAt, videoBox } from '../verticalFraming';

const SOURCE = { width: 1280, height: 720 };
const PORTRAIT = 9 / 16;
const key = (time: number, centerX: number, height = 720) => ({
    time,
    centerX,
    centerY: 360,
    height,
});

describe('verticalFraming', () => {
    it('interpolates and clamps crops like the renderer', () => {
        const keys = [key(0, 300), key(2, 900)];
        expect(cropAt(keys, 0, SOURCE, PORTRAIT)).toEqual({
            x: 97.5,
            y: 0,
            width: 405,
            height: 720,
        });
        expect(cropAt(keys, 1, SOURCE, PORTRAIT).x).toBeCloseTo(397.5);
        // Held after the last key; kept inside the frame at the edge.
        expect(cropAt(keys, 5, SOURCE, PORTRAIT).x).toBeCloseTo(697.5);
        const edge = cropAt([key(0, 1270)], 0, SOURCE, PORTRAIT);
        expect(edge.x + edge.width).toBe(1280);
    });

    it('finds the piece playing at a source time', () => {
        const pieces: PreviewPiece[] = [
            { start: 10, end: 12, fit: true, keys: [] },
            { start: 12, end: 15, fit: false, keys: [key(0, 640)] },
        ];
        expect(pieceAt(pieces, 11)?.fit).toBe(true);
        expect(pieceAt(pieces, 12)?.fit).toBe(false);
        expect(pieceAt(pieces, 15)).toBeNull();
    });

    it('places the video so the crop fills the frame', () => {
        const pieces: PreviewPiece[] = [
            { start: 10, end: 20, fit: false, keys: [key(0, 640, 480)] },
        ];
        const plan = { sourceWidth: 1280, sourceHeight: 720 };
        // 270x480 crop centred at (640, 360) in a 270x480 frame: 1:1.
        const box = videoBox(plan, pieces, 12, { width: 270, height: 480 });
        expect(box).toEqual({ width: 1280, height: 720, left: -505, top: -120 });
    });

    it('fits the whole picture for fit pieces', () => {
        const pieces: PreviewPiece[] = [{ start: 0, end: 5, fit: true, keys: [] }];
        const plan = { sourceWidth: 1280, sourceHeight: 720 };
        const box = videoBox(plan, pieces, 1, { width: 270, height: 480 });
        expect(box.width).toBeCloseTo(270);
        expect(box.height).toBeCloseTo(151.875);
        expect(box.left).toBeCloseTo(0);
        expect(box.top).toBeCloseTo((480 - 151.875) / 2);
    });
});
