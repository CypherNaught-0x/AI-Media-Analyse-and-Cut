import { computed, ref } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { commands, type NemotronStatus, type NemotronVariant } from '../bindings';
import { errorMessage } from '../utils/appError';

interface InstallProgress {
    message: string;
    downloadedBytes: number;
    totalBytes: number;
}

// Shared by Settings and the setup wizard: one download at a time, and both
// show the same progress.
const status = ref<NemotronStatus | null>(null);
const checking = ref(false);
const installing = ref(false);
const progress = ref<InstallProgress | null>(null);
const installError = ref('');
let pending: Promise<void> | null = null;
let listening: Promise<unknown> | null = null;

function listenForProgress() {
    if (listening) return;
    try {
        listening = Promise.resolve(
            listen<InstallProgress>('nemotron-install-progress', (event) => {
                progress.value = event.payload;
            }),
        ).catch((error) => {
            console.error('Failed to listen for Nemotron download progress:', error);
            listening = null;
        });
    } catch (error) {
        console.error('Failed to listen for Nemotron download progress:', error);
    }
}

async function refresh(): Promise<void> {
    if (pending) return pending;
    checking.value = true;
    pending = (async () => {
        try {
            const next = await commands.nemotronStatus();
            if (next) {
                status.value = next;
                installing.value = next.installing;
            }
        } catch (error) {
            console.error('Failed to check Nemotron:', error);
        } finally {
            checking.value = false;
            pending = null;
        }
    })();
    return pending;
}

/** Download the runtime and `variant`; resolves to whether it finished. */
async function install(variant: NemotronVariant): Promise<boolean> {
    if (installing.value) return false;
    listenForProgress();
    installing.value = true;
    installError.value = '';
    progress.value = { message: 'Starting download…', downloadedBytes: 0, totalBytes: 0 };
    try {
        status.value = await commands.installNemotron(variant);
        return true;
    } catch (error) {
        installError.value = errorMessage(error);
        return false;
    } finally {
        installing.value = false;
        progress.value = null;
        void refresh();
    }
}

async function cancel(): Promise<void> {
    await commands.cancelNemotronInstall();
}

async function remove(variant: NemotronVariant): Promise<void> {
    status.value = await commands.removeNemotronModel(variant);
}

/** The build a run uses when none is chosen: the recommended one, else any installed. */
export function effectiveNemotronVariant(
    current: NemotronStatus | null,
    chosen: NemotronVariant | '',
): NemotronVariant | null {
    if (chosen) return chosen;
    return current?.recommendation.variant ?? null;
}

/** Nemotron's status on this machine, fetched on first use. */
export function useNemotron() {
    if (!status.value && !pending) void refresh();
    listenForProgress();
    return {
        status,
        checking,
        installing,
        progress,
        installError,
        /** 0-100 while downloading, else null. */
        percent: computed(() => {
            const current = progress.value;
            if (!current || current.totalBytes <= 0) return null;
            return Math.min(100, (current.downloadedBytes / current.totalBytes) * 100);
        }),
        isInstalled(variant: NemotronVariant | null): boolean {
            if (!variant || !status.value?.runtimeInstalled) return false;
            return status.value.variants.some(
                (entry) => entry.variant === variant && entry.installed,
            );
        },
        refresh,
        install,
        cancel,
        remove,
    };
}
