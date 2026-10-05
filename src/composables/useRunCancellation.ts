import { commands } from '../bindings';
import { errorMessage, isAppError } from '../utils/appError';

export const RUN_CANCELLED_MESSAGE = 'Run cancelled.';

/**
 * True when `error` means the run was cancelled: a command rejected with the
 * `cancelled` kind, or the frontend's own `assertActiveRun` noticed that a
 * newer run (or a cancel) replaced this one.
 */
export function isRunCancelled(error: unknown): boolean {
    if (isAppError(error)) {
        return error.kind === 'cancelled';
    }
    return errorMessage(error).includes(RUN_CANCELLED_MESSAGE);
}

export async function beginRun(): Promise<number> {
    return commands.beginRun();
}
