import { ref } from 'vue';
import { SETUP_WIZARD_VERSION, useSettings } from './useSettings';

// One wizard for the whole app: App renders it, Settings can reopen it.
const open = ref(false);

export function useSetupWizard() {
    const { settings } = useSettings();
    return {
        open,
        /** Show it on start when this version of the wizard was never completed or skipped. */
        openIfNeeded() {
            if ((settings.value.setupWizardVersion ?? 0) < SETUP_WIZARD_VERSION) open.value = true;
        },
        show() {
            open.value = true;
        },
        close() {
            open.value = false;
        },
    };
}
