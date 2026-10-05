import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

const STORAGE_KEY = 'llm-settings';

/** useSettings reads localStorage at import time, so load a fresh copy. */
async function loadFreshSettingsModule() {
    vi.resetModules();
    return import('../useSettings');
}

describe('API key migration', () => {
    beforeEach(() => {
        localStorage.clear();
        invoke.mockReset();
    });

    it('moves a key from localStorage into the keychain, then forgets it', async () => {
        localStorage.setItem(
            STORAGE_KEY,
            JSON.stringify({ apiKey: ' sk-legacy ', model: 'gemini-2.5-flash' }),
        );
        invoke.mockImplementation((command: string) =>
            Promise.resolve(command === 'has_api_key' ? true : null),
        );

        const { migrateLegacyApiKey, useSettings } = await loadFreshSettingsModule();
        const { settings, apiKeyStored } = useSettings();
        expect(settings.value).not.toHaveProperty('apiKey');

        await migrateLegacyApiKey();

        expect(invoke).toHaveBeenCalledWith('set_api_key', { key: 'sk-legacy' });
        expect(JSON.parse(localStorage.getItem(STORAGE_KEY)!)).not.toHaveProperty('apiKey');
        expect(JSON.parse(localStorage.getItem(STORAGE_KEY)!).model).toBe('gemini-2.5-flash');
        expect(apiKeyStored.value).toBe(true);
    });

    it('keeps the key in localStorage when the keychain is unavailable', async () => {
        localStorage.setItem(STORAGE_KEY, JSON.stringify({ apiKey: 'sk-legacy' }));
        invoke.mockImplementation((command: string) =>
            command === 'set_api_key'
                ? Promise.reject({ kind: 'failed', message: 'no keychain' })
                : Promise.resolve(false),
        );

        const { migrateLegacyApiKey } = await loadFreshSettingsModule();
        await migrateLegacyApiKey();

        expect(JSON.parse(localStorage.getItem(STORAGE_KEY)!).apiKey).toBe('sk-legacy');
    });

    it('does nothing without a legacy key', async () => {
        invoke.mockResolvedValue(false);
        const { migrateLegacyApiKey } = await loadFreshSettingsModule();
        await migrateLegacyApiKey();
        expect(invoke).not.toHaveBeenCalledWith('set_api_key', expect.anything());
    });
});
