import { computed, ref } from 'vue';
import { commands } from '../bindings';
import type { AppleSpeechStatus } from '../types';

// Shared across components: the status only changes when a locale is
// downloaded, so it is fetched once and refreshed explicitly.
const status = ref<AppleSpeechStatus | null>(null);
const checking = ref(false);
let pending: Promise<void> | null = null;

async function refresh(): Promise<void> {
    if (pending) return pending;
    checking.value = true;
    pending = (async () => {
        try {
            status.value = await commands.appleSpeechStatus();
        } catch (error) {
            console.error('Failed to check Apple Speech:', error);
            status.value = {
                supportedPlatform: false,
                available: false,
                supportedLocales: [],
                installedLocales: [],
                message: String(error),
            };
        } finally {
            checking.value = false;
            pending = null;
        }
    })();
    return pending;
}

/**
 * The locale to transcribe in: the configured one, else the system language.
 * The helper maps it to the closest supported locale (e.g. `de` to `de-DE`).
 */
export function appleSpeechLocale(setting: string): string {
    return setting.trim() || navigator.language || 'en-US';
}

/** Apple Speech availability (macOS 26+ only), fetched on first use. */
export function useAppleSpeech() {
    if (!status.value && !pending) {
        void refresh();
    }
    return {
        status,
        checking,
        /** True on macOS builds that include the helper, even if unusable. */
        supported: computed(() => status.value?.supportedPlatform ?? false),
        refresh,
        setStatus(next: AppleSpeechStatus) {
            status.value = next;
        },
    };
}
