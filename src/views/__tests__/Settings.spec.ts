import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, flushPromises, enableAutoUnmount } from '@vue/test-utils';
import { ref } from 'vue';
import Settings from '../Settings.vue';
import { createRouter, createWebHistory } from 'vue-router';
import { runtimeLicenses } from '../../utils/runtimeLicenses';

// Mock Tauri APIs
vi.mock('@tauri-apps/api/app', () => ({
    getVersion: vi.fn(() => Promise.resolve('1.0.0')),
}));

vi.mock('@tauri-apps/plugin-updater', () => ({
    check: vi.fn(() => Promise.resolve({ available: false })),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
    open: vi.fn(),
    save: vi.fn(),
    ask: vi.fn(() => Promise.resolve(false)),
    message: vi.fn(() => Promise.resolve()),
}));

const invokeMock = vi.fn((command: string, _args?: unknown) => {
    if (command === 'crisper_environment_status') {
        return Promise.resolve({
            pythonPath: '/usr/bin/python3',
            python: '3.12.1',
            pythonSupported: true,
            minimumPython: '3.10',
            installed: true,
            crisperwhisperVersion: '2.0.1',
            backends: ['transformers'],
            torchVersion: '2.6.0',
            cuda: false,
            mps: true,
            environmentDir: '/tmp/env',
            managedEnvironmentExists: true,
            ready: true,
            message: null,
        });
    }
    if (command === 'list_models') {
        return Promise.resolve({ supported: true, models: ['gemini-pro'] });
    }
    return Promise.resolve(undefined);
});

vi.mock('@tauri-apps/api/core', () => ({
    invoke: (command: string, args?: unknown) => invokeMock(command, args),
}));

vi.mock('@tauri-apps/api/event', () => ({
    listen: vi.fn(() => Promise.resolve(() => {})),
}));

// Mock useSettings
const updateSettingsMock = vi.fn();
const updateModelFetchStateMock = vi.fn();
const settingsRef = ref({
    baseUrl: 'https://test.url',
    model: 'test-model',
    enforceJsonSchema: true,
    glossary: '',
    preClipPadding: 0,
    postClipPadding: 0,
    transcriptionBackend: 'llm',
    parakeetModelPath: '',
    sortformerModelPath: '',
    crisperModel: 'large',
    crisperLanguage: 'en',
    crisperMode: 'verbatim',
    crisperBackend: 'auto',
    crisperDevice: 'auto',
    crisperComputeType: 'auto',
    crisperRemoveVocalEvents: false,
    crisperDiarize: true,
    crisperPythonPath: '',
});
const modelFetchStateRef = ref({
    availableModels: [],
    supportsModelFetch: true,
});
const apiKeyStoredRef = ref(true);
const openRouterKeyStoredRef = ref(false);
const refreshApiKeyStatusMock = vi.fn(() => Promise.resolve());
vi.mock('../../composables/useSettings', () => ({
    SETUP_WIZARD_VERSION: 1,
    refreshApiKeyStatus: () => refreshApiKeyStatusMock(),
    useSettings: () => ({
        settings: settingsRef,
        apiKeyStored: apiKeyStoredRef,
        openRouterKeyStored: openRouterKeyStoredRef,
        updateSettings: updateSettingsMock,
        modelFetchState: modelFetchStateRef,
        updateModelFetchState: updateModelFetchStateMock,
    }),
}));

const router = createRouter({
    history: createWebHistory(),
    routes: [
        { path: '/', component: { template: '<div>Home</div>' } },
        { path: '/settings', component: Settings },
    ],
});

// Each mounted Settings registers a leave guard on the shared router; unmount
// between tests so a stale dirty form can't block another test's navigation.
enableAutoUnmount(afterEach);

const saveButton = (wrapper: ReturnType<typeof mount>) =>
    wrapper.findAll('button').find((b) => b.text() === 'Save settings');

describe('Settings.vue', () => {
    beforeEach(() => {
        vi.clearAllMocks();
        apiKeyStoredRef.value = true;
        settingsRef.value = {
            baseUrl: 'https://test.url',
            model: 'test-model',
            enforceJsonSchema: true,
            glossary: '',
            preClipPadding: 0,
            postClipPadding: 0,
            transcriptionBackend: 'llm',
            parakeetModelPath: '',
            sortformerModelPath: '',
            crisperModel: 'large',
            crisperLanguage: 'en',
            crisperMode: 'verbatim',
            crisperBackend: 'auto',
            crisperDevice: 'auto',
            crisperComputeType: 'auto',
            crisperRemoveVocalEvents: false,
            crisperDiarize: true,
            crisperPythonPath: '',
        };
        modelFetchStateRef.value = {
            availableModels: [],
            supportsModelFetch: true,
        };
        globalThis.fetch = vi.fn();
    });

    it('renders correctly', () => {
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });

        expect(wrapper.get('h1').text()).toBe('Settings');
        expect(wrapper.find('input[type="password"]').exists()).toBe(true);
    });

    it('lists every runtime download with its licence and notice', () => {
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });

        const list = wrapper.get('[data-testid="runtime-licenses"]');
        expect(list.findAll('li')).toHaveLength(runtimeLicenses.downloads.length);
        for (const download of runtimeLicenses.downloads) {
            expect(list.text()).toContain(download.name);
            if (download.notice) expect(list.text()).toContain(download.notice);
        }
    });

    it('fetches models correctly', async () => {
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });

        // Trigger fetch models
        // We need to make sure the button is enabled (apiKey is present in mock)
        const fetchButton = wrapper.findAll('button').find((b) => b.text() === 'Refresh models');
        await fetchButton!.trigger('click');

        await flushPromises();

        // The backend lists the models with the stored key; nothing is sent
        // from the webview.
        expect(invokeMock).toHaveBeenCalledWith('list_models', {
            baseUrl: 'https://test.url',
            apiKey: null,
        });
        expect(updateModelFetchStateMock).toHaveBeenCalledWith({
            supportsModelFetch: true,
            availableModels: ['gemini-pro'],
        });
    });

    it('saves the chosen video export quality', async () => {
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });

        const draft = wrapper.get('[data-testid="export-quality-draft"]');
        expect(draft.attributes('aria-checked')).toBe('false');
        await draft.trigger('click');
        expect(draft.attributes('aria-checked')).toBe('true');

        await saveButton(wrapper)!.trigger('click');
        expect(updateSettingsMock).toHaveBeenCalledWith(
            expect.objectContaining({ exportQuality: 'draft' }),
        );
    });

    it('saves settings', async () => {
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });

        const save = saveButton(wrapper);
        expect(save!.attributes('disabled')).toBeDefined();
        // Typing a new key counts as a change.
        const input = wrapper.find('input[type="password"]');
        await input.setValue('new-api-key');
        expect(save!.attributes('disabled')).toBeUndefined();

        await save!.trigger('click');
        await flushPromises();

        // The key goes to the keychain, not into the saved settings.
        expect(invokeMock).toHaveBeenCalledWith('set_api_key', { key: 'new-api-key' });
        expect(updateSettingsMock).toHaveBeenCalled();
        expect(updateSettingsMock.mock.calls[0][0]).not.toHaveProperty('apiKey');
        expect(refreshApiKeyStatusMock).toHaveBeenCalled();
        expect((input.element as HTMLInputElement).value).toBe('');
    });

    it('sets up OpenRouter with EU-only routing and its own keychain slot', async () => {
        const wrapper = mount(Settings, { global: { plugins: [router] } });

        await wrapper.get('[data-testid="provider-openrouter"]').trigger('click');
        // Presets own the base URL; only Custom shows the field.
        expect(wrapper.find('#settings-base-url').exists()).toBe(false);
        expect(wrapper.get('label[for="settings-api-key"]').text()).toBe('OpenRouter API key');

        await wrapper.get('[data-testid="openrouter-eu-only"]').trigger('click');
        await wrapper.get('[data-testid="api-key-input"]').setValue('sk-or-new');
        await saveButton(wrapper)!.trigger('click');
        await flushPromises();

        expect(invokeMock).toHaveBeenCalledWith('set_openrouter_api_key', { key: 'sk-or-new' });
        expect(invokeMock).not.toHaveBeenCalledWith('set_api_key', expect.anything());
        expect(updateSettingsMock.mock.calls[0][0]).toMatchObject({
            baseUrl: 'https://eu.openrouter.ai/api',
            model: 'google/gemini-3.5-flash-lite',
            openRouterEuOnly: true,
            cloudAsrModel: 'mistralai/voxtral-small-transcribe-3-0',
        });
    });

    it('offers Nemotron and cloud speech-to-text as speech engines', async () => {
        const wrapper = mount(Settings, { global: { plugins: [router] } });
        await flushPromises();
        const engines = wrapper
            .get('[data-testid="default-engine"]')
            .findAll('[role="radio"]')
            .map((radio) => radio.text());
        expect(engines[0]).toContain('Nemotron 3.5');
        expect(engines[0]).toContain('Recommended');
        expect(engines.some((text) => text.startsWith('Cloud'))).toBe(true);
        expect(wrapper.find('[data-testid="engine-card-nemotron"]').exists()).toBe(true);
        expect(wrapper.find('[data-testid="engine-card-cloud"]').exists()).toBe(true);
    });

    it('never shows the stored key and can remove it', async () => {
        const { ask } = await import('@tauri-apps/plugin-dialog');
        vi.mocked(ask).mockResolvedValueOnce(true);
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });

        const input = wrapper.get('[data-testid="api-key-input"]');
        expect((input.element as HTMLInputElement).value).toBe('');
        expect(input.attributes('placeholder')).toContain('keychain');

        await wrapper.get('[data-testid="api-key-remove"]').trigger('click');
        await flushPromises();
        expect(invokeMock).toHaveBeenCalledWith('clear_api_key', undefined);
        expect(refreshApiKeyStatusMock).toHaveBeenCalled();
    });

    it('probes the CrisperWhisper environment on mount and reports it as ready', async () => {
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });
        await flushPromises();

        expect(invokeMock).toHaveBeenCalledWith('crisper_environment_status', {
            pythonPath: '',
        });
        expect(wrapper.text()).toContain('Ready');
        expect(wrapper.text()).toContain('2.0.1');
    });

    it('states the English/German restriction and the non-commercial license', () => {
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });

        expect(wrapper.text()).toContain('English and German only');
        expect(wrapper.text()).toContain('non-commercial research use');
    });

    it('saves the CrisperWhisper options', async () => {
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });
        await flushPromises();

        const intendedButton = wrapper
            .findAll('button')
            .find((button) => button.text().startsWith('Intended'));
        expect(intendedButton).toBeTruthy();
        await intendedButton!.trigger('click');

        const vocalEvents = wrapper.get('[data-testid="crisper-remove-vocal-events"]');
        expect(vocalEvents.attributes('role')).toBe('switch');
        expect(vocalEvents.attributes('aria-checked')).toBe('false');
        await vocalEvents.trigger('click');
        expect(vocalEvents.attributes('aria-checked')).toBe('true');

        await saveButton(wrapper)!.trigger('click');

        expect(updateSettingsMock).toHaveBeenCalledWith(
            expect.objectContaining({
                crisperMode: 'intended',
                crisperRemoveVocalEvents: true,
                crisperLanguage: 'en',
                crisperModel: 'large',
            }),
        );
    });

    it('saves the JSON enforcement toggle', async () => {
        const wrapper = mount(Settings, {
            global: {
                plugins: [router],
            },
        });

        const toggle = wrapper.get('[data-testid="enforce-json-schema"]');
        // The visible label names the switch and toggles it.
        const label = wrapper.get(`label[for="${toggle.attributes('id')}"]`);
        expect(label.text()).toBe('Enforce structured JSON for transcript analysis');
        await label.trigger('click');
        expect(toggle.attributes('aria-checked')).toBe('false');

        await saveButton(wrapper)!.trigger('click');

        expect(updateSettingsMock).toHaveBeenCalledWith(
            expect.objectContaining({
                enforceJsonSchema: false,
            }),
        );
    });

    it('shows the stored-key state and can reveal a typed key', async () => {
        const wrapper = mount(Settings, { global: { plugins: [router] } });

        expect(wrapper.get('[data-testid="api-key-stored"]').text()).toContain('Saved in keychain');
        const input = wrapper.get('[data-testid="api-key-input"]');
        const toggle = wrapper.get('[data-testid="api-key-visibility"]');
        expect(toggle.attributes('aria-label')).toBe('Show API key');
        await toggle.trigger('click');
        expect(input.attributes('type')).toBe('text');
        expect(toggle.attributes('aria-label')).toBe('Hide API key');
        await toggle.trigger('click');
        expect(input.attributes('type')).toBe('password');

        apiKeyStoredRef.value = false;
        await flushPromises();
        expect(wrapper.find('[data-testid="api-key-stored"]').exists()).toBe(false);
        expect(wrapper.find('[data-testid="api-key-remove"]').exists()).toBe(false);
    });

    it('warns inline when the model list is a fallback', async () => {
        const defaultInvoke = invokeMock.getMockImplementation()!;
        invokeMock.mockImplementation((command: string, args?: unknown) =>
            command === 'list_models'
                ? (Promise.reject(new Error('boom')) as ReturnType<typeof defaultInvoke>)
                : defaultInvoke(command, args),
        );
        const wrapper = mount(Settings, { global: { plugins: [router] } });
        await flushPromises();
        invokeMock.mockImplementation(defaultInvoke);
        // The silent fetch on mount failed, so the list is the built-in one.
        expect(wrapper.get('[data-testid="model-fallback-warning"]').text()).toContain(
            'built-in fallback',
        );

        await wrapper.get('[data-testid="model-manual-toggle"]').trigger('click');
        expect(wrapper.find('select#settings-model').exists()).toBe(false);
        expect(wrapper.get('input#settings-model').element).toBeInstanceOf(HTMLInputElement);
    });

    it('flags a fallback model list kept from an earlier visit', () => {
        modelFetchStateRef.value = {
            availableModels: ['gpt-4', 'gpt-3.5-turbo'] as never,
            supportsModelFetch: false,
        };
        const wrapper = mount(Settings, { global: { plugins: [router] } });
        expect(wrapper.get('[data-testid="model-fallback-warning"]').text()).toContain(
            'built-in fallback',
        );
    });

    it('opens the card of a newly picked default engine without closing others', async () => {
        settingsRef.value = {
            ...settingsRef.value,
            transcriptionBackend: 'local',
            localEngine: 'parakeet',
        } as typeof settingsRef.value;
        const wrapper = mount(Settings, { global: { plugins: [router] } });
        const toggle = (engine: string) =>
            wrapper.get(`[data-testid="engine-card-${engine}"] button[aria-expanded]`);

        // Parakeet is the default engine, so only its card starts open.
        expect(toggle('parakeet').attributes('aria-expanded')).toBe('true');
        expect(toggle('crisper').attributes('aria-expanded')).toBe('false');

        // Open the advanced options inside CrisperWhisper by hand.
        await toggle('crisper').trigger('click');
        const advanced = wrapper
            .get('[data-testid="engine-card-crisper"]')
            .findAll('button[aria-expanded]')
            .find((button) => button.text().includes('Advanced runtime options'))!;
        await advanced.trigger('click');
        expect(advanced.attributes('aria-expanded')).toBe('true');

        // Changing the default pipeline or engine leaves those cards alone.
        await wrapper
            .findAll('[role="radio"]')
            .find((radio) => radio.text().startsWith('LLM Only'))!
            .trigger('click');
        await wrapper
            .findAll('[role="radio"]')
            .find((radio) => radio.text().startsWith('Hybrid Cleanup'))!
            .trigger('click');
        expect(toggle('crisper').attributes('aria-expanded')).toBe('true');
        expect(advanced.attributes('aria-expanded')).toBe('true');
        expect(toggle('parakeet').attributes('aria-expanded')).toBe('true');
    });

    it('gives the option cards radio semantics and labels every field', () => {
        const wrapper = mount(Settings, { global: { plugins: [router] } });

        const pipeline = wrapper.get('[data-testid="default-pipeline"]');
        expect(pipeline.attributes('role')).toBe('radiogroup');
        const checked = pipeline.findAll('[role="radio"][aria-checked="true"]');
        expect(checked).toHaveLength(1);
        expect(checked[0].text()).toContain('LLM Only');

        for (const field of wrapper.findAll('input, select')) {
            const id = field.attributes('id');
            expect(id, field.html()).toBeTruthy();
            expect(wrapper.find(`label[for="${id}"]`).exists(), id).toBe(true);
        }
    });

    it('notes that LLM-only sections are unused by a Speech Engine Only default', async () => {
        const wrapper = mount(Settings, { global: { plugins: [router] } });
        expect(wrapper.find('[data-testid="ai-provider-unused"]').exists()).toBe(false);
        // LLM Only as the default: the engines are the ones flagged instead.
        expect(wrapper.find('[data-testid="engines-unused"]').exists()).toBe(true);

        await wrapper
            .findAll('[role="radio"]')
            .find((radio) => radio.text().startsWith('Speech Engine Only'))!
            .trigger('click');
        expect(wrapper.get('[data-testid="ai-provider-unused"]').text()).toContain(
            'Only used by LLM and hybrid pipelines',
        );
        expect(wrapper.find('[data-testid="engines-unused"]').exists()).toBe(false);
    });

    it('shows a status chip for the CrisperWhisper runtime', async () => {
        const wrapper = mount(Settings, { global: { plugins: [router] } });
        await flushPromises();
        expect(wrapper.get('[data-testid="crisper-status-chip"]').text()).toBe('Ready');
    });

    describe('leaving with unsaved changes', () => {
        async function mountOnSettings() {
            await router.push('/settings');
            await router.isReady();
            const wrapper = mount(Settings, {
                global: { plugins: [router], stubs: { teleport: true } },
            });
            await flushPromises();
            return wrapper;
        }

        it('goes straight back without changes', async () => {
            const wrapper = await mountOnSettings();
            const cancel = wrapper.get('[data-testid="settings-cancel"]');
            expect(cancel.text()).toBe('Cancel');
            await cancel.trigger('click');
            await flushPromises();
            expect(router.currentRoute.value.path).toBe('/');
            expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
        });

        it('asks before discarding edits, and can keep editing', async () => {
            const wrapper = await mountOnSettings();
            await wrapper.get('[data-testid="export-quality-draft"]').trigger('click');
            expect(wrapper.get('[data-testid="settings-action-bar"]').text()).toContain(
                'Unsaved changes',
            );

            const discard = wrapper.get('[data-testid="settings-cancel"]');
            expect(discard.text()).toBe('Discard');
            await discard.trigger('click');
            await flushPromises();
            expect(wrapper.get('[role="dialog"]').text()).toContain('Discard unsaved changes?');

            await wrapper.get('[data-testid="leave-keep-editing"]').trigger('click');
            await flushPromises();
            expect(router.currentRoute.value.path).toBe('/settings');
            expect(wrapper.find('[role="dialog"]').exists()).toBe(false);

            await wrapper.get('[data-testid="settings-cancel"]').trigger('click');
            await flushPromises();
            await wrapper.get('[data-testid="leave-discard"]').trigger('click');
            await flushPromises();
            expect(router.currentRoute.value.path).toBe('/');
            expect(updateSettingsMock).not.toHaveBeenCalled();
        });

        it('puts the workspace step back when a rail navigation is cancelled', async () => {
            const { useWorkspaceNav, resetWorkspaceNav } =
                await import('../../composables/useWorkspaceNav');
            resetWorkspaceNav();
            const { state, selectStep } = useWorkspaceNav();
            state.hasTranscript = true;
            selectStep('transcript');
            const wrapper = await mountOnSettings();
            await wrapper.get('[data-testid="export-quality-draft"]').trigger('click');

            // The rail selects the step, then navigates.
            selectStep('clips');
            void router.push('/');
            await flushPromises();
            await wrapper.get('[data-testid="leave-keep-editing"]').trigger('click');
            await flushPromises();
            expect(router.currentRoute.value.path).toBe('/settings');
            expect(state.activeStep).toBe('transcript');
            resetWorkspaceNav();
        });

        it('floats the action bar only while there are unsaved changes', async () => {
            const wrapper = await mountOnSettings();
            const bar = () => wrapper.get('[data-testid="settings-action-bar"]');
            expect(bar().element.parentElement!.className).not.toContain('sticky');
            expect(bar().text()).toContain('No unsaved changes');

            await wrapper.get('[data-testid="export-quality-draft"]').trigger('click');
            expect(bar().element.parentElement!.className).toContain('sticky');
        });

        it('saves without asking', async () => {
            const wrapper = await mountOnSettings();
            await wrapper.get('[data-testid="export-quality-draft"]').trigger('click');
            await saveButton(wrapper)!.trigger('click');
            await flushPromises();
            expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
            expect(router.currentRoute.value.path).toBe('/');
        });
    });
});
