<script setup lang="ts">
// Quick setup: checks what this machine can run, picks a speech engine, an
// AI provider (with EU-only routing) and the transcript language, and
// downloads Nemotron if that is the choice. Shown once on first start and
// from Settings → Run setup wizard.
import { computed, ref, watch } from 'vue';
import {
    Check,
    Cloud,
    Cpu,
    Download,
    ExternalLink,
    KeyRound,
    Languages,
    Loader2,
    Sparkles,
    TriangleAlert,
} from '@lucide/vue';
import { openUrl } from '@tauri-apps/plugin-opener';
import UiDialog from './ui/UiDialog.vue';
import OptionCardGroup, { type OptionCard } from './settings/OptionCardGroup.vue';
import SettingsSwitchRow from './settings/SettingsSwitchRow.vue';
import { commands, type TranscriptionModel } from '../bindings';
import type { LocalEngine } from '../types';
import { SETUP_WIZARD_VERSION, refreshApiKeyStatus, useSettings } from '../composables/useSettings';
import { useSetupWizard } from '../composables/useSetupWizard';
import { useNemotron } from '../composables/useNemotron';
import { useAppleSpeech } from '../composables/useAppleSpeech';
import {
    assessEngines,
    describeDevice,
    hasRoomForNemotron,
    recommendedEngine,
    type EngineRating,
} from '../utils/setupRecommendation';
import {
    ASR_LANGUAGES,
    DEFAULT_GEMINI_MODEL,
    GEMINI_BASE_URL,
    defaultCloudAsrModel,
    defaultOpenRouterModel,
    formatBytes,
    isOpenRouterUrl,
    openRouterBaseUrl,
    providerForBaseUrl,
} from '../utils/providers';
import { errorMessage } from '../utils/appError';

const props = withDefaults(defineProps<{ teleport?: boolean }>(), { teleport: true });

const { open, close } = useSetupWizard();
const { settings, updateSettings, apiKeyStored, openRouterKeyStored } = useSettings();
const nemotron = useNemotron();
const { status: appleSpeechStatus } = useAppleSpeech();

type Step = 'device' | 'engine' | 'ai' | 'language' | 'finish';
const STEPS: { id: Step; label: string }[] = [
    { id: 'device', label: 'Your machine' },
    { id: 'engine', label: 'Speech engine' },
    { id: 'ai', label: 'Cloud & AI' },
    { id: 'language', label: 'Language' },
    { id: 'finish', label: 'Finish' },
];
const step = ref<Step>('device');
const stepIndex = computed(() => STEPS.findIndex((entry) => entry.id === step.value));

// ---------------------------------------------------------------- Choices
type AiChoice = 'openrouter' | 'gemini' | 'none';
const engine = ref<LocalEngine>('nemotron');
const engineTouched = ref(false);
const ai = ref<AiChoice>('openrouter');
const euOnly = ref(false);
const openRouterKey = ref('');
const geminiKey = ref('');
const cloudModel = ref('');
const llmModel = ref('');
const language = ref('');
const diarize = ref(true);
const applyError = ref('');

/** Start from the current settings each time the wizard opens. */
function reset() {
    step.value = 'device';
    engineTouched.value = false;
    const current = settings.value;
    euOnly.value = current.openRouterEuOnly ?? false;
    const provider = providerForBaseUrl(current.baseUrl);
    ai.value =
        provider === 'openrouter'
            ? 'openrouter'
            : provider === 'gemini' && apiKeyStored.value
              ? 'gemini'
              : 'openrouter';
    openRouterKey.value = '';
    geminiKey.value = '';
    cloudModel.value = current.cloudAsrModel || defaultCloudAsrModel(euOnly.value);
    llmModel.value = isOpenRouterUrl(current.baseUrl)
        ? current.model
        : defaultOpenRouterModel(euOnly.value);
    language.value = current.asrLanguage ?? '';
    diarize.value = current.nemotronDiarize ?? true;
    applyError.value = '';
    engine.value = recommendedEngine(nemotron.status.value);
    void nemotron.refresh();
}

watch(
    open,
    (isOpen) => {
        if (isOpen) reset();
    },
    { immediate: true },
);

// The recommendation arrives with the device check; follow it until the user
// picks an engine themselves.
watch(
    () => nemotron.status.value?.recommendation.rating,
    () => {
        if (!engineTouched.value) engine.value = recommendedEngine(nemotron.status.value);
    },
);

// Switching the region swaps the defaults that only exist in the other one.
watch(euOnly, (eu, previous) => {
    if (cloudModel.value === defaultCloudAsrModel(previous)) {
        cloudModel.value = defaultCloudAsrModel(eu);
    }
    if (llmModel.value === defaultOpenRouterModel(previous)) {
        llmModel.value = defaultOpenRouterModel(eu);
    }
    void loadCloudModels();
    void loadLlmModels();
});

// ---------------------------------------------------------------- Device
const assessments = computed(() =>
    assessEngines(nemotron.status.value, appleSpeechStatus.value, euOnly.value),
);
const recommended = computed(() => recommendedEngine(nemotron.status.value));
const deviceLine = computed(() => describeDevice(nemotron.status.value));

const RATING_CHIPS: Record<EngineRating, { label: string; tone: string }> = {
    great: { label: 'Runs well', tone: 'chip-success' },
    ok: { label: 'Runs, memory is tight', tone: 'chip-warning' },
    slow: { label: 'Slow (CPU only)', tone: 'chip-warning' },
    unavailable: { label: 'Not on this machine', tone: 'chip' },
    'needs-key': { label: 'Needs an API key', tone: 'chip-accent' },
};

const engineOptions = computed<OptionCard<LocalEngine>[]>(() =>
    assessments.value
        .filter((entry) => entry.rating !== 'unavailable' || entry.engine === engine.value)
        .map((entry) => ({
            value: entry.engine,
            title: entry.title,
            description: entry.note,
            badge: entry.engine === recommended.value ? 'Recommended' : undefined,
            testId: `wizard-engine-${entry.engine}`,
        })),
);

function pickEngine(value: LocalEngine) {
    engine.value = value;
    engineTouched.value = true;
}

const nemotronVariant = computed(() => nemotron.status.value?.recommendation.variant ?? null);
const nemotronVariantInfo = computed(() =>
    nemotron.status.value?.variants.find((entry) => entry.variant === nemotronVariant.value),
);
const nemotronReady = computed(() => nemotron.isInstalled(nemotronVariant.value));
const nemotronDownloadBytes = computed(
    () =>
        (nemotronVariantInfo.value?.downloadBytes ?? 0) +
        (nemotron.status.value?.runtimeDownloadBytes ?? 0),
);
const diskTooSmall = computed(() => !hasRoomForNemotron(nemotron.status.value));

// ---------------------------------------------------------------- Cloud & AI
const AI_OPTIONS: OptionCard<AiChoice>[] = [
    {
        value: 'openrouter',
        title: 'OpenRouter',
        badge: 'Recommended',
        description:
            'One key for cloud speech-to-text and the AI features. Optional EU-only routing.',
        testId: 'wizard-ai-openrouter',
    },
    {
        value: 'gemini',
        title: 'Google Gemini',
        description: 'A Gemini API key from Google AI Studio (free tier available).',
        testId: 'wizard-ai-gemini',
    },
    {
        value: 'none',
        title: 'Not now',
        description: 'Transcribe on this machine only. Add a provider later in Settings.',
        testId: 'wizard-ai-none',
    },
];

const needsOpenRouter = computed(() => engine.value === 'cloud' || ai.value === 'openrouter');
const hasOpenRouterKey = computed(
    () => openRouterKeyStored.value || openRouterKey.value.trim().length > 0,
);
const hasGeminiKey = computed(() => apiKeyStored.value || geminiKey.value.trim().length > 0);

const cloudModels = ref<TranscriptionModel[]>([]);
const loadingCloudModels = ref(false);
async function loadCloudModels() {
    if (engine.value !== 'cloud') return;
    loadingCloudModels.value = true;
    try {
        cloudModels.value =
            (await commands.listTranscriptionModels(openRouterBaseUrl(euOnly.value))) ?? [];
    } catch (error) {
        console.warn('Could not load the speech-to-text models:', error);
        cloudModels.value = [];
    } finally {
        loadingCloudModels.value = false;
    }
}

const llmModels = ref<string[]>([]);
const loadingLlmModels = ref(false);
async function loadLlmModels() {
    if (ai.value !== 'openrouter' || !hasOpenRouterKey.value) return;
    loadingLlmModels.value = true;
    try {
        const list = await commands.listModels(
            openRouterBaseUrl(euOnly.value),
            openRouterKey.value.trim() || null,
        );
        llmModels.value = list.supported ? list.models : [];
    } catch (error) {
        console.warn('Could not load the OpenRouter models:', error);
        llmModels.value = [];
    } finally {
        loadingLlmModels.value = false;
    }
}

watch(step, (current) => {
    if (current === 'ai') {
        void loadCloudModels();
        void loadLlmModels();
    }
});

const cloudModelOptions = computed(() => {
    const ids = cloudModels.value.map((model) => model.id);
    const entries = cloudModels.value.map((model) => ({
        value: model.id,
        label:
            model.usdPerMinute != null
                ? `${model.name} · $${(model.usdPerMinute * 60).toFixed(2)}/h`
                : model.name,
    }));
    if (cloudModel.value && !ids.includes(cloudModel.value)) {
        entries.unshift({ value: cloudModel.value, label: cloudModel.value });
    }
    return entries;
});
const cloudModelUnavailable = computed(
    () =>
        cloudModels.value.length > 0 &&
        !cloudModels.value.some((model) => model.id === cloudModel.value),
);

const canContinue = computed(() => {
    if (step.value === 'ai') {
        if (needsOpenRouter.value && engine.value === 'cloud' && !hasOpenRouterKey.value)
            return false;
        if (ai.value === 'gemini' && !hasGeminiKey.value) return false;
    }
    return true;
});

// ---------------------------------------------------------------- Navigation
function next() {
    const index = stepIndex.value;
    if (index < STEPS.length - 1) step.value = STEPS[index + 1].id;
}
function back() {
    const index = stepIndex.value;
    if (index > 0) step.value = STEPS[index - 1].id;
}

/** Remember that the wizard was seen, so it doesn't open on every start. */
function skip() {
    updateSettings({ setupWizardVersion: SETUP_WIZARD_VERSION });
    close();
}

const applying = ref(false);
async function finish() {
    applying.value = true;
    applyError.value = '';
    try {
        if (openRouterKey.value.trim()) await commands.setOpenrouterApiKey(openRouterKey.value);
        if (ai.value === 'gemini' && geminiKey.value.trim())
            await commands.setApiKey(geminiKey.value);
        await refreshApiKeyStatus();
    } catch (error) {
        applyError.value = `Could not save the API key: ${errorMessage(error)}`;
        applying.value = false;
        return;
    }

    const current = settings.value;
    const llm =
        ai.value === 'openrouter'
            ? {
                  baseUrl: openRouterBaseUrl(euOnly.value),
                  model: llmModel.value.trim() || defaultOpenRouterModel(euOnly.value),
              }
            : ai.value === 'gemini'
              ? {
                    baseUrl: GEMINI_BASE_URL,
                    model:
                        providerForBaseUrl(current.baseUrl) === 'gemini'
                            ? current.model
                            : DEFAULT_GEMINI_MODEL,
                }
              : {};
    updateSettings({
        ...llm,
        localEngine: engine.value,
        transcriptionBackend: 'local',
        openRouterEuOnly: euOnly.value,
        cloudAsrModel: cloudModel.value || defaultCloudAsrModel(euOnly.value),
        asrLanguage: language.value,
        nemotronDiarize: diarize.value,
        cloudAsrDiarize: diarize.value,
        appleSpeechDiarize: diarize.value,
        setupWizardVersion: SETUP_WIZARD_VERSION,
    });
    applying.value = false;
    close();
}

async function downloadNemotron() {
    if (!nemotronVariant.value) return;
    await nemotron.install(nemotronVariant.value);
}

const summary = computed(() => {
    const engineEntry = assessments.value.find((entry) => entry.engine === engine.value);
    const lines = [`Speech engine: ${engineEntry?.title ?? engine.value}`];
    if (engine.value === 'cloud') lines.push(`Speech model: ${cloudModel.value}`);
    lines.push(
        ai.value === 'openrouter'
            ? `AI features: OpenRouter${euOnly.value ? ' EU' : ''} · ${llmModel.value}`
            : ai.value === 'gemini'
              ? 'AI features: Google Gemini'
              : 'AI features: off (add a provider in Settings)',
    );
    if (needsOpenRouter.value && euOnly.value)
        lines.push('OpenRouter requests are processed in Europe only.');
    lines.push(
        `Language: ${ASR_LANGUAGES.find((entry) => entry.value === language.value)?.label ?? language.value}`,
    );
    lines.push(`Speakers: ${diarize.value ? 'labelled' : 'not labelled'}`);
    return lines;
});

const openKeysPage = () => openUrl('https://openrouter.ai/settings/keys');
</script>

<template>
    <UiDialog
        :open="open"
        :teleport="props.teleport"
        size="xl"
        title="Quick setup"
        :description="`Step ${stepIndex + 1} of ${STEPS.length} · ${STEPS[stepIndex].label}`"
        @close="skip"
    >
        <template #icon>
            <span
                class="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-accent/15 text-accent"
            >
                <Sparkles class="h-4 w-4" aria-hidden="true" />
            </span>
        </template>

        <ol class="mb-5 flex gap-1.5" aria-label="Setup steps">
            <li
                v-for="(entry, index) in STEPS"
                :key="entry.id"
                class="h-1 flex-1 rounded-full transition-colors duration-200"
                :class="index <= stepIndex ? 'bg-accent' : 'bg-surface-hover'"
                :aria-current="index === stepIndex ? 'step' : undefined"
                :aria-label="entry.label"
            />
        </ol>

        <!-- 1. Device -->
        <section v-if="step === 'device'" data-testid="wizard-step-device" class="space-y-4">
            <div class="well flex items-start gap-3 px-3 py-3">
                <Cpu class="mt-0.5 h-4 w-4 shrink-0 text-fg-subtle" aria-hidden="true" />
                <div class="min-w-0" aria-live="polite">
                    <p class="eyebrow">This machine</p>
                    <p
                        v-if="deviceLine"
                        class="mt-1 text-[13px] text-fg"
                        data-testid="wizard-device"
                    >
                        {{ deviceLine }}
                    </p>
                    <p v-else class="mt-1 flex items-center gap-2 text-[13px] text-fg-muted">
                        <Loader2 class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                        Checking memory, GPU and disk…
                    </p>
                </div>
            </div>

            <ul class="divide-y divide-line rounded-md border border-line">
                <li
                    v-for="entry in assessments"
                    :key="entry.engine"
                    class="flex items-start gap-3 px-3 py-2.5"
                    :data-testid="`wizard-assessment-${entry.engine}`"
                >
                    <component
                        :is="entry.onDevice ? Cpu : Cloud"
                        class="mt-0.5 h-4 w-4 shrink-0 text-fg-subtle"
                        aria-hidden="true"
                    />
                    <div class="min-w-0 flex-1">
                        <p class="text-[13px] font-medium text-fg">
                            {{ entry.title }}
                            <span
                                v-if="entry.engine === recommended"
                                class="chip-accent ml-1.5 align-middle"
                                >Recommended</span
                            >
                        </p>
                        <p class="hint mt-0.5">{{ entry.note }}</p>
                    </div>
                    <span :class="RATING_CHIPS[entry.rating].tone" class="shrink-0">
                        {{ RATING_CHIPS[entry.rating].label }}
                    </span>
                </li>
            </ul>
            <p class="hint">
                Nemotron 3.5 Transcribe is NVIDIA's speech-LLM: the most accurate engine here, with
                punctuation, speaker labels and your glossary. It needs a GPU with enough memory;
                otherwise the same model runs in the cloud.
            </p>
        </section>

        <!-- 2. Engine -->
        <section v-else-if="step === 'engine'" data-testid="wizard-step-engine" class="space-y-4">
            <p id="wizard-engine-label" class="field-label">Where should transcription run?</p>
            <OptionCardGroup
                class="grid-cols-1"
                aria-labelledby="wizard-engine-label"
                :model-value="engine"
                :options="engineOptions"
                @update:model-value="pickEngine"
            />
            <p
                v-if="engine === 'nemotron' && diskTooSmall"
                class="flex items-start gap-2 text-[12px] text-warning"
                role="alert"
            >
                <TriangleAlert class="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                Not enough free disk space for the {{ formatBytes(nemotronDownloadBytes) }}
                download. Free some space, or choose the cloud.
            </p>
            <p v-else-if="engine === 'nemotron' && !nemotronReady" class="hint">
                The model ({{ formatBytes(nemotronDownloadBytes) }}) downloads at the end of the
                setup, only when you start it.
            </p>
        </section>

        <!-- 3. Cloud & AI -->
        <section v-else-if="step === 'ai'" data-testid="wizard-step-ai" class="space-y-4">
            <p class="hint">
                Clip suggestions, translation, podcasts and the hybrid pipelines use an AI model.
                <template v-if="engine === 'cloud'"
                    >Cloud speech-to-text needs an OpenRouter key.</template
                >
            </p>
            <OptionCardGroup
                v-model="ai"
                class="grid-cols-1 @xl:grid-cols-3"
                label="AI provider"
                :options="AI_OPTIONS"
                dense
            />

            <div v-if="needsOpenRouter" class="panel divide-y divide-line">
                <div class="p-4">
                    <div class="mb-1.5 flex items-center justify-between gap-3">
                        <label for="wizard-openrouter-key" class="field-label !mb-0"
                            >OpenRouter API key</label
                        >
                        <span v-if="openRouterKeyStored" class="chip-success">
                            <KeyRound aria-hidden="true" />
                            Saved in keychain
                        </span>
                    </div>
                    <div class="flex gap-2">
                        <input
                            id="wizard-openrouter-key"
                            v-model="openRouterKey"
                            type="password"
                            autocomplete="off"
                            spellcheck="false"
                            class="input min-w-0 flex-1"
                            data-testid="wizard-openrouter-key"
                            :placeholder="
                                openRouterKeyStored
                                    ? 'Saved — enter a new key to replace it'
                                    : 'sk-or-…'
                            "
                            @change="loadLlmModels"
                        />
                        <button type="button" class="btn-secondary" @click="openKeysPage">
                            <ExternalLink aria-hidden="true" />
                            Get a key
                        </button>
                    </div>
                    <p class="hint mt-1.5">
                        Stored in your system keychain, never in the app's settings.
                    </p>
                </div>

                <div class="p-4">
                    <SettingsSwitchRow
                        v-model="euOnly"
                        test-id="wizard-eu-only"
                        title="Keep requests in the EU"
                        description="Uses eu.openrouter.ai: requests are processed in Europe only and fail rather than leave it. Needs OpenRouter's EU in-region routing on your account; fewer models are available."
                    />
                </div>

                <div v-if="engine === 'cloud'" class="p-4">
                    <label for="wizard-cloud-model" class="field-label">Speech-to-text model</label>
                    <select
                        id="wizard-cloud-model"
                        v-model="cloudModel"
                        class="input"
                        data-testid="wizard-cloud-model"
                    >
                        <option
                            v-for="option in cloudModelOptions"
                            :key="option.value"
                            :value="option.value"
                        >
                            {{ option.label }}
                        </option>
                    </select>
                    <p
                        v-if="cloudModelUnavailable"
                        class="mt-1.5 flex items-start gap-2 text-[12px] text-warning"
                    >
                        <TriangleAlert class="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        {{ cloudModel }} isn't served in this region.
                    </p>
                    <p v-else class="hint mt-1.5">
                        <Loader2
                            v-if="loadingCloudModels"
                            class="mr-1 inline h-3 w-3 animate-spin"
                            aria-hidden="true"
                        />
                        {{
                            euOnly
                                ? 'EU region: Nemotron is not served here; Voxtral is the recommended model.'
                                : 'Nemotron 3.5 Transcribe is recommended: word timings, ~40 languages.'
                        }}
                    </p>
                </div>

                <div v-if="ai === 'openrouter'" class="p-4">
                    <label for="wizard-llm-model" class="field-label">AI model</label>
                    <select
                        v-if="llmModels.length > 0"
                        id="wizard-llm-model"
                        v-model="llmModel"
                        class="input"
                    >
                        <option v-if="!llmModels.includes(llmModel)" :value="llmModel">
                            {{ llmModel }}
                        </option>
                        <option v-for="model in llmModels" :key="model" :value="model">
                            {{ model }}
                        </option>
                    </select>
                    <input
                        v-else
                        id="wizard-llm-model"
                        v-model="llmModel"
                        type="text"
                        spellcheck="false"
                        class="input font-mono text-[12px]"
                    />
                    <p class="hint mt-1.5">
                        <Loader2
                            v-if="loadingLlmModels"
                            class="mr-1 inline h-3 w-3 animate-spin"
                            aria-hidden="true"
                        />
                        Used for clips, translation and the hybrid pipelines.
                        <template v-if="!hasOpenRouterKey"> Enter a key to load the list.</template>
                    </p>
                </div>
            </div>

            <div v-if="ai === 'gemini'" class="panel p-4">
                <div class="mb-1.5 flex items-center justify-between gap-3">
                    <label for="wizard-gemini-key" class="field-label !mb-0">Gemini API key</label>
                    <span v-if="apiKeyStored" class="chip-success">
                        <KeyRound aria-hidden="true" />
                        Saved in keychain
                    </span>
                </div>
                <input
                    id="wizard-gemini-key"
                    v-model="geminiKey"
                    type="password"
                    autocomplete="off"
                    spellcheck="false"
                    class="input"
                    :placeholder="apiKeyStored ? 'Saved — enter a new key to replace it' : 'AIza…'"
                />
            </div>
        </section>

        <!-- 4. Language -->
        <section
            v-else-if="step === 'language'"
            data-testid="wizard-step-language"
            class="space-y-4"
        >
            <div>
                <label for="wizard-language" class="field-label">
                    <Languages class="mr-1 inline h-3.5 w-3.5" aria-hidden="true" />
                    Spoken language
                </label>
                <select id="wizard-language" v-model="language" class="input">
                    <option v-for="entry in ASR_LANGUAGES" :key="entry.value" :value="entry.value">
                        {{ entry.label }}
                    </option>
                </select>
                <p class="hint mt-1.5">
                    Naming the language helps with short or less common recordings. Apple Speech and
                    CrisperWhisper keep their own language setting in Settings.
                </p>
            </div>
            <SettingsSwitchRow
                v-model="diarize"
                test-id="wizard-diarize"
                title="Identify speakers"
                description="Labels who speaks when (Speaker 1, Speaker 2, …). Rename them in the editor."
            />
        </section>

        <!-- 5. Finish -->
        <section v-else data-testid="wizard-step-finish" class="space-y-4">
            <ul class="space-y-1.5">
                <li
                    v-for="line in summary"
                    :key="line"
                    class="flex items-start gap-2 text-[13px] text-fg"
                >
                    <Check class="mt-0.5 h-3.5 w-3.5 shrink-0 text-success" aria-hidden="true" />
                    {{ line }}
                </li>
            </ul>

            <div
                v-if="engine === 'nemotron' && nemotronVariant"
                class="well px-3 py-3"
                data-testid="wizard-nemotron-download"
            >
                <div class="flex items-start justify-between gap-3">
                    <div class="min-w-0" aria-live="polite">
                        <p class="eyebrow">Nemotron {{ nemotronVariant }}</p>
                        <p v-if="nemotronReady" class="mt-1 text-[13px] text-success">
                            Downloaded and ready.
                        </p>
                        <p
                            v-else-if="nemotron.installing.value"
                            class="mt-1 truncate text-[13px] text-fg-muted"
                        >
                            {{ nemotron.progress.value?.message ?? 'Downloading…' }}
                        </p>
                        <p v-else class="mt-1 text-[13px] text-fg-muted">
                            {{ formatBytes(nemotronDownloadBytes) }} to download. You can keep
                            working while it downloads; it resumes if interrupted.
                        </p>
                        <p
                            v-if="nemotron.installError.value"
                            class="mt-1 text-[12px] text-danger"
                            role="alert"
                        >
                            {{ nemotron.installError.value }}
                        </p>
                    </div>
                    <button
                        v-if="nemotron.installing.value"
                        type="button"
                        class="btn-secondary btn-sm shrink-0"
                        @click="nemotron.cancel"
                    >
                        Pause
                    </button>
                    <button
                        v-else-if="!nemotronReady"
                        type="button"
                        class="btn-primary btn-sm shrink-0"
                        data-testid="wizard-download"
                        :disabled="diskTooSmall"
                        @click="downloadNemotron"
                    >
                        <Download aria-hidden="true" />
                        Download now
                    </button>
                </div>
                <div
                    v-if="nemotron.installing.value"
                    class="mt-2.5 h-1.5 overflow-hidden rounded-full bg-accent/20"
                    role="progressbar"
                    aria-label="Downloading Nemotron"
                    :aria-valuenow="Math.round(nemotron.percent.value ?? 0)"
                    aria-valuemin="0"
                    aria-valuemax="100"
                >
                    <div
                        class="h-full bg-accent transition-[width] duration-300"
                        :style="{ width: `${nemotron.percent.value ?? 0}%` }"
                    />
                </div>
            </div>

            <p v-if="applyError" class="text-[12px] text-danger" role="alert">{{ applyError }}</p>
            <p class="hint">
                Change any of this later in Settings, or run this setup again from there.
            </p>
        </section>

        <template #footer>
            <button type="button" class="btn-ghost mr-auto" data-testid="wizard-skip" @click="skip">
                Skip setup
            </button>
            <button v-if="stepIndex > 0" type="button" class="btn-secondary" @click="back">
                Back
            </button>
            <button
                v-if="step !== 'finish'"
                type="button"
                class="btn-primary"
                data-testid="wizard-next"
                :disabled="!canContinue"
                @click="next"
            >
                Continue
            </button>
            <button
                v-else
                type="button"
                class="btn-primary"
                data-testid="wizard-finish"
                :disabled="applying"
                @click="finish"
            >
                <Loader2 v-if="applying" class="animate-spin" aria-hidden="true" />
                Finish
            </button>
        </template>
    </UiDialog>
</template>
