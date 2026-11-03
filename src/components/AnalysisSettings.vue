<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue';
import { Info, TriangleAlert } from '@lucide/vue';
import type { LocalEngine, TranscriptionBackend } from '../types';
import { usesLocalEngine, usesRemoteModel } from '../types';
import { runtimeDownload } from '../utils/runtimeLicenses';
import { useAppleSpeech } from '../composables/useAppleSpeech';
import OptionCardGroup, { type OptionCard } from './settings/OptionCardGroup.vue';
import SettingsSwitchRow from './settings/SettingsSwitchRow.vue';

const props = defineProps<{
    transcriptionBackend: TranscriptionBackend;
    localEngine: LocalEngine;
    context: string;
    glossary: string;
    speakerCount: number | null;
    removeFillerWords: boolean;
    trimSilence: boolean;
}>();

const emit = defineEmits<{
    (e: 'update:transcriptionBackend', value: TranscriptionBackend): void;
    (e: 'update:localEngine', value: LocalEngine): void;
    (e: 'update:context', value: string): void;
    (e: 'update:glossary', value: string): void;
    (e: 'update:speakerCount', value: number | null): void;
    (e: 'update:removeFillerWords', value: boolean): void;
    (e: 'update:trimSilence', value: boolean): void;
}>();

const contextTextarea = ref<HTMLTextAreaElement | null>(null);
const glossaryTextarea = ref<HTMLTextAreaElement | null>(null);

/** The engine row only matters when a local engine actually runs. */
const showsLocalEngine = computed(() => usesLocalEngine(props.transcriptionBackend));
/** Context, glossary and speaker count are only sent to a remote LLM. */
const usesLlmAssist = computed(() => usesRemoteModel(props.transcriptionBackend));
/**
 * CrisperWhisper removes fillers itself (it transcribes them verbatim with
 * timings, so they can be cut from the video), and the LLM stages can too.
 * Parakeet alone has no such pass.
 */
const supportsFillerRemoval = computed(
    () =>
        usesLlmAssist.value || props.localEngine === 'crisper' || props.localEngine === 'nemotron',
);
/** Nemotron and cloud speech-to-text take the glossary even without an LLM. */
const engineUsesGlossary = computed(
    () =>
        showsLocalEngine.value &&
        (props.localEngine === 'nemotron' || props.localEngine === 'cloud'),
);
const isCrisperEngine = computed(() => showsLocalEngine.value && props.localEngine === 'crisper');
/** The licence notice the runtime licence register requires for the weights. */
const crisperWeightsNotice = runtimeDownload('crisperwhisper-weights').notice;

/** Why filler removal is off for this pipeline/engine pair. */
const fillerDisabledReason = computed(() => {
    if (props.localEngine === 'apple-speech')
        return 'Apple Speech already drops most fillers; cutting them from the video needs CrisperWhisper or an LLM pass.';
    if (props.localEngine === 'cloud')
        return 'Cloud speech-to-text has no filler pass; use Nemotron, CrisperWhisper or an LLM pass.';
    return 'Parakeet keeps every word; filler removal needs Nemotron, CrisperWhisper or an LLM pass.';
});
const fillerDescription = computed(() => {
    if (isCrisperEngine.value)
        return 'Cuts [UM] / [UH] out of the transcript and the exported video.';
    if (showsLocalEngine.value && props.localEngine === 'nemotron' && !usesLlmAssist.value)
        return 'Nemotron writes the transcript without fillers and accidental repetitions.';
    return 'The AI pass drops fillers like “um” and “uh” from the transcript.';
});

const PIPELINES: OptionCard<TranscriptionBackend>[] = [
    {
        value: 'llm',
        title: 'LLM Only',
        description: 'Sends the audio to your API model. Uses context, glossary and speakers.',
    },
    {
        value: 'local',
        title: 'Speech Engine Only',
        description: 'Just the engine below. With an on-device engine nothing leaves this machine.',
    },
    {
        value: 'hybrid',
        title: 'Hybrid Cleanup',
        description: 'Engine timings, then an LLM pass tidies wording and punctuation.',
    },
    {
        value: 'hybrid-merge',
        title: 'Hybrid Merge',
        description: 'The engine and the LLM both transcribe, merged onto the engine timings.',
    },
];

const ENGINES: OptionCard<LocalEngine>[] = [
    {
        value: 'nemotron',
        title: 'Nemotron 3.5',
        badge: 'Recommended',
        description: 'Most accurate, ~40 languages, speakers, follows your glossary. On device.',
    },
    {
        value: 'parakeet',
        title: 'Parakeet',
        description: 'Fast, multilingual, with speaker labels. Auto-downloads.',
    },
    {
        value: 'crisper',
        title: 'CrisperWhisper',
        badge: 'EN / DE',
        description: 'Verbatim, ~30 ms word timings, keeps or cuts fillers.',
    },
    {
        value: 'apple-speech',
        title: 'Apple Speech',
        badge: 'macOS 26+',
        description: 'On-device, very fast, ~45 languages, drops fillers.',
    },
    {
        value: 'cloud',
        title: 'Cloud',
        badge: 'OpenRouter',
        description: 'Sends the audio to a speech-to-text API (Nemotron, Voxtral, …).',
    },
];

const { supported: appleSpeechSupported } = useAppleSpeech();
/** Apple Speech only exists on macOS; keep a stored choice visible anyway. */
const engines = computed(() =>
    ENGINES.filter(
        (engine) =>
            engine.value !== 'apple-speech' ||
            appleSpeechSupported.value ||
            props.localEngine === 'apple-speech',
    ),
);

/** Grow a textarea with its content (capped by max-height in CSS). */
function autosize(textarea: HTMLTextAreaElement | null) {
    if (!textarea) return;
    textarea.style.height = 'auto';
    if (textarea.scrollHeight > 0) textarea.style.height = `${textarea.scrollHeight + 2}px`;
}

function onTextInput(event: Event, field: 'context' | 'glossary') {
    const textarea = event.target as HTMLTextAreaElement;
    autosize(textarea);
    if (field === 'context') emit('update:context', textarea.value);
    else emit('update:glossary', textarea.value);
}

// Values set from outside (a restored session) resize too, and so do the
// fields when switching back from Local Only re-creates them.
watch(
    () => [props.context, props.glossary, usesLlmAssist.value, engineUsesGlossary.value],
    async () => {
        await nextTick();
        autosize(contextTextarea.value);
        autosize(glossaryTextarea.value);
    },
);
onMounted(() => {
    autosize(contextTextarea.value);
    autosize(glossaryTextarea.value);
});
</script>

<template>
    <div class="@container space-y-5">
        <p class="hint">
            These options apply to this project only and override your app defaults from Settings.
        </p>

        <!-- Pipeline -->
        <div>
            <p id="analysis-pipeline-label" class="field-label">Transcription pipeline</p>
            <OptionCardGroup
                class="grid-cols-1 @lg:grid-cols-2"
                aria-labelledby="analysis-pipeline-label"
                :model-value="transcriptionBackend"
                :options="PIPELINES"
                dense
                @update:model-value="emit('update:transcriptionBackend', $event)"
            />
        </div>

        <!-- The engine is independent of the pipeline: every non-LLM pipeline
             runs whichever engine is picked here. -->
        <Transition name="fade">
            <div v-if="showsLocalEngine" data-testid="engine-picker">
                <p id="analysis-engine-label" class="field-label">Speech engine</p>
                <OptionCardGroup
                    class="grid-cols-1"
                    :class="
                        engines.length > 2 ? '@lg:grid-cols-2 @3xl:grid-cols-3' : '@lg:grid-cols-2'
                    "
                    aria-labelledby="analysis-engine-label"
                    :model-value="localEngine"
                    :options="engines"
                    dense
                    @update:model-value="emit('update:localEngine', $event)"
                />

                <!-- Shown on selection: the weights carry a licence restriction the app cannot enforce. -->
                <div
                    v-if="isCrisperEngine"
                    class="mt-2.5 flex gap-2.5 rounded-md border border-warning/25 bg-warning/[0.06] px-3 py-2.5"
                    role="note"
                >
                    <TriangleAlert
                        class="mt-0.5 h-3.5 w-3.5 shrink-0 text-warning"
                        aria-hidden="true"
                    />
                    <div>
                        <p class="text-[12px] font-semibold text-warning">
                            Non-commercial use only &middot; English and German only
                        </p>
                        <p class="hint mt-0.5">
                            {{ crisperWeightsNotice }} The model card is published for English and
                            German only.
                        </p>
                    </div>
                </div>
            </div>
        </Transition>

        <!-- LLM assist: only sent to a remote model, so hidden for Local Only. -->
        <div
            v-if="usesLlmAssist"
            class="grid grid-cols-1 items-start gap-4 @lg:grid-cols-[minmax(0,1fr)_9rem]"
            data-testid="llm-assist"
        >
            <div class="@lg:col-span-2">
                <label for="analysis-context" class="field-label">Context</label>
                <textarea
                    id="analysis-context"
                    ref="contextTextarea"
                    :value="context"
                    rows="2"
                    class="input max-h-60 !min-h-[3.75rem] resize-none"
                    placeholder="Describe the video content to help the AI… Especially for translation"
                    @input="onTextInput($event, 'context')"
                ></textarea>
            </div>

            <div>
                <label for="analysis-glossary" class="field-label">Glossary</label>
                <textarea
                    id="analysis-glossary"
                    ref="glossaryTextarea"
                    :value="glossary"
                    rows="2"
                    class="input max-h-60 !min-h-[3.75rem] resize-none"
                    placeholder="Specific terms, names, acronyms…"
                    @input="onTextInput($event, 'glossary')"
                ></textarea>
            </div>

            <div>
                <label for="analysis-speakers" class="field-label">Speakers</label>
                <input
                    id="analysis-speakers"
                    :value="speakerCount"
                    type="number"
                    min="1"
                    class="input"
                    placeholder="Auto"
                    title="Leave empty to detect the number of speakers"
                    @input="
                        emit(
                            'update:speakerCount',
                            ($event.target as HTMLInputElement).valueAsNumber || null,
                        )
                    "
                />
            </div>
        </div>
        <template v-else>
            <div v-if="engineUsesGlossary" data-testid="engine-glossary">
                <label for="analysis-glossary" class="field-label">Glossary</label>
                <textarea
                    id="analysis-glossary"
                    ref="glossaryTextarea"
                    :value="glossary"
                    rows="2"
                    class="input max-h-60 !min-h-[3.75rem] resize-none"
                    placeholder="Names and terms to spell right, one per line…"
                    @input="onTextInput($event, 'glossary')"
                ></textarea>
            </div>
            <p
                class="flex items-center gap-2 rounded-md border border-line bg-surface-2 px-3 py-2 text-[12px] text-fg-muted"
                data-testid="llm-assist-hint"
            >
                <Info class="h-3.5 w-3.5 shrink-0 text-info" aria-hidden="true" />
                {{
                    engineUsesGlossary
                        ? 'The engine uses the glossary to spell names and terms. Context and speaker count only apply to LLM and hybrid pipelines.'
                        : 'Context, glossary and speaker count only apply to LLM and hybrid pipelines.'
                }}
            </p>
        </template>

        <!-- Clean-up -->
        <div class="grid grid-cols-1 gap-2 @lg:grid-cols-2">
            <SettingsSwitchRow
                test-id="remove-filler-words"
                title="Remove filler words"
                :model-value="removeFillerWords"
                :disabled="!supportsFillerRemoval"
                :description="fillerDescription"
                :disabled-reason="fillerDisabledReason"
                @update:model-value="emit('update:removeFillerWords', $event)"
            />
            <SettingsSwitchRow
                test-id="trim-silence"
                title="Trim silence"
                description="Removes long pauses from the audio before it is transcribed."
                :model-value="trimSilence"
                @update:model-value="emit('update:trimSilence', $event)"
            />
        </div>
    </div>
</template>
