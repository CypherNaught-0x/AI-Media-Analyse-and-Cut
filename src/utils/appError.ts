import type { AppError } from '../bindings';

/** True for the `{ kind, message }` errors Tauri commands reject with. */
export function isAppError(error: unknown): error is AppError {
    return (
        typeof error === 'object' &&
        error !== null &&
        'kind' in error &&
        typeof (error as { message?: unknown }).message === 'string'
    );
}

/** A human-readable message for anything a command or the UI can throw. */
export function errorMessage(error: unknown): string {
    if (isAppError(error) || error instanceof Error) {
        return error.message;
    }
    return String(error);
}
