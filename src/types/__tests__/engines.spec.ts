import { describe, it, expect } from 'vitest';
import { isLocalEngine, LOCAL_ENGINE_LABELS, LOCAL_ENGINES } from '../index';

describe('local engines', () => {
    it('recognises every engine and nothing else', () => {
        for (const engine of LOCAL_ENGINES) {
            expect(isLocalEngine(engine)).toBe(true);
            expect(LOCAL_ENGINE_LABELS[engine]).toBeTruthy();
        }
        expect(isLocalEngine('apple-speech')).toBe(true);
        expect(isLocalEngine('whisper')).toBe(false);
        expect(isLocalEngine(undefined)).toBe(false);
    });
});
