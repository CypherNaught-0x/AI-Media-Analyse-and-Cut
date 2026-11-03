import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import SetupWizard from '../SetupWizard.vue';
import { useSetupWizard } from '../../composables/useSetupWizard';
import { useSettings } from '../../composables/useSettings';
import type { NemotronStatus } from '../../bindings';

let nemotronRating: NemotronStatus['recommendation']['rating'] = 'good';

function nemotronStatus(): NemotronStatus {
    return {
        platformSupported: true,
        runtimeInstalled: false,
        runtimeDownloadBytes: 7_239_335,
        encoderInstalled: false,
        variants: [
            {
                variant: 'IQ4_NL',
                label: 'IQ4_NL (recommended)',
                downloadBytes: 21_349_066_752,
                requiredMemoryGb: 21.4,
                installed: false,
            },
        ],
        recommendation: {
            rating: nemotronRating,
            variant: nemotronRating === 'unsupported' ? null : 'IQ4_NL',
            device: 'metal',
            message:
                nemotronRating === 'unsupported'
                    ? 'Needs a Mac with at least 24 GB of memory; this one has 16 GB.'
                    : 'Runs on the GPU with the recommended IQ4_NL build.',
        },
        device: {
            os: 'macos',
            arch: 'aarch64',
            cpu: 'Apple M3 Pro',
            cpuCores: 12,
            totalMemoryGb: nemotronRating === 'unsupported' ? 16 : 36,
            appleSilicon: true,
            gpus: [],
            freeDiskGb: 120,
        },
        installing: false,
    };
}

const invokeMock = vi.fn((command: string, _args?: unknown): Promise<unknown> => {
    switch (command) {
        case 'nemotron_status':
            return Promise.resolve(nemotronStatus());
        case 'apple_speech_status':
            return Promise.resolve({
                supportedPlatform: false,
                available: false,
                supportedLocales: [],
                installedLocales: [],
                message: null,
            });
        case 'list_transcription_models':
            return Promise.resolve([
                {
                    id: 'mistralai/voxtral-small-transcribe-3-0',
                    name: 'Voxtral Small Transcribe',
                    usdPerMinute: 0.0044,
                },
            ]);
        case 'list_models':
            return Promise.resolve({ supported: true, models: ['google/gemini-3.5-flash-lite'] });
        case 'has_api_key':
        case 'has_openrouter_api_key':
            return Promise.resolve(false);
        default:
            return Promise.resolve(null);
    }
});

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (command: string, args?: unknown) => invokeMock(command, args),
}));
vi.mock('@tauri-apps/api/event', () => ({
    listen: vi.fn(() => Promise.resolve(() => {})),
}));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));

const button = (wrapper: ReturnType<typeof mount>, testId: string) =>
    wrapper.get(`[data-testid="${testId}"]`);

async function mountWizard() {
    useSetupWizard().open.value = true;
    const wrapper = mount(SetupWizard, { props: { teleport: false } });
    await flushPromises();
    return wrapper;
}

describe('SetupWizard', () => {
    beforeEach(() => {
        localStorage.clear();
        invokeMock.mockClear();
        nemotronRating = 'good';
        useSetupWizard().open.value = false;
        const { settings } = useSettings();
        settings.value = {
            ...settings.value,
            baseUrl: 'https://generativelanguage.googleapis.com',
            model: 'gemini-2.5-flash',
            openRouterEuOnly: false,
            setupWizardVersion: 0,
            localEngine: 'parakeet',
            transcriptionBackend: 'llm',
            asrLanguage: '',
        };
    });

    it('detects the machine and recommends Nemotron on a GPU that holds it', async () => {
        const wrapper = await mountWizard();

        expect(wrapper.get('[data-testid="wizard-device"]').text()).toContain(
            'Apple M3 Pro · 36 GB unified memory',
        );
        const nemotron = wrapper.get('[data-testid="wizard-assessment-nemotron"]');
        expect(nemotron.text()).toContain('Recommended');
        expect(nemotron.text()).toContain('Runs well');

        await button(wrapper, 'wizard-next').trigger('click');
        const checked = wrapper.get('[role="radio"][aria-checked="true"]');
        expect(checked.text()).toContain('Nemotron 3.5 Transcribe (on this device)');
    });

    it('applies the choices: OpenRouter in the EU, language and the key', async () => {
        const wrapper = await mountWizard();
        await button(wrapper, 'wizard-next').trigger('click'); // device -> engine
        await button(wrapper, 'wizard-next').trigger('click'); // engine -> ai
        await flushPromises();

        await wrapper.get('[data-testid="wizard-openrouter-key"]').setValue('sk-or-test');
        await button(wrapper, 'wizard-eu-only').trigger('click');
        await flushPromises();
        await button(wrapper, 'wizard-next').trigger('click'); // ai -> language
        await wrapper.get('#wizard-language').setValue('de');
        await button(wrapper, 'wizard-next').trigger('click'); // language -> finish

        expect(wrapper.text()).toContain('OpenRouter requests are processed in Europe only.');
        expect(wrapper.get('[data-testid="wizard-nemotron-download"]').text()).toContain(
            '19.9 GB to download',
        );

        await button(wrapper, 'wizard-finish').trigger('click');
        await flushPromises();

        expect(invokeMock).toHaveBeenCalledWith('set_openrouter_api_key', { key: 'sk-or-test' });
        const { settings } = useSettings();
        expect(settings.value).toMatchObject({
            localEngine: 'nemotron',
            transcriptionBackend: 'local',
            baseUrl: 'https://eu.openrouter.ai/api',
            model: 'google/gemini-3.5-flash-lite',
            openRouterEuOnly: true,
            cloudAsrModel: 'mistralai/voxtral-small-transcribe-3-0',
            asrLanguage: 'de',
            setupWizardVersion: 1,
        });
        expect(useSetupWizard().open.value).toBe(false);
        // Nothing large is downloaded unless asked for.
        expect(invokeMock).not.toHaveBeenCalledWith('install_nemotron', expect.anything());
    });

    it('falls back to the cloud, which needs an OpenRouter key', async () => {
        nemotronRating = 'unsupported';
        const wrapper = await mountWizard();
        expect(wrapper.get('[data-testid="wizard-assessment-nemotron"]').text()).toContain(
            'Not on this machine',
        );

        await button(wrapper, 'wizard-next').trigger('click');
        expect(wrapper.get('[role="radio"][aria-checked="true"]').text()).toContain('cloud');
        expect(wrapper.find('[data-testid="wizard-engine-nemotron"]').exists()).toBe(false);

        await button(wrapper, 'wizard-next').trigger('click');
        expect(button(wrapper, 'wizard-next').attributes('disabled')).toBeDefined();
        await wrapper.get('[data-testid="wizard-openrouter-key"]').setValue('sk-or-test');
        expect(button(wrapper, 'wizard-next').attributes('disabled')).toBeUndefined();
    });

    it('can be skipped and does not open again', async () => {
        const wrapper = await mountWizard();
        await button(wrapper, 'wizard-skip').trigger('click');

        expect(useSetupWizard().open.value).toBe(false);
        expect(useSettings().settings.value.setupWizardVersion).toBe(1);
        useSetupWizard().openIfNeeded();
        expect(useSetupWizard().open.value).toBe(false);
    });
});
