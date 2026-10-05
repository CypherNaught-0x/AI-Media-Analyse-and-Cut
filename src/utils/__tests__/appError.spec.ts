import { describe, expect, it } from 'vitest';
import { errorMessage, isAppError } from '../appError';
import { isRunCancelled } from '../../composables/useRunCancellation';

describe('command errors', () => {
    const failed = { kind: 'failed', message: 'Failed to cut the video: disk full' };
    const cancelled = { kind: 'cancelled', message: 'Run cancelled.' };

    it('reads the message of command errors, Errors and anything else', () => {
        expect(errorMessage(failed)).toBe('Failed to cut the video: disk full');
        expect(errorMessage(new Error('boom'))).toBe('boom');
        expect(errorMessage('plain string')).toBe('plain string');
        expect(`${errorMessage(failed)}`).not.toContain('[object Object]');
    });

    it('recognises command errors by shape', () => {
        expect(isAppError(failed)).toBe(true);
        expect(isAppError(new Error('boom'))).toBe(false);
        expect(isAppError(null)).toBe(false);
    });

    it('detects cancellation by kind, not by message text', () => {
        expect(isRunCancelled(cancelled)).toBe(true);
        expect(isRunCancelled({ kind: 'failed', message: 'Run cancelled. (not really)' })).toBe(
            false,
        );
        // The frontend's own assertActiveRun still throws a plain Error.
        expect(isRunCancelled(new Error('Run cancelled.'))).toBe(true);
        expect(isRunCancelled(failed)).toBe(false);
    });
});
