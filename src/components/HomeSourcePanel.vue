<script setup lang="ts">
// The Source page: pick a recording, choose how to analyse it, and run the
// analysis. Presentational — props in, events out; Home owns the state.
import { computed } from 'vue';
import { ArrowRight, Info, Loader2, Settings2, Sparkles, TriangleAlert } from '@lucide/vue';
import type { LocalEngine, TranscriptionBackend } from '../types';
import FileSelector from './FileSelector.vue';
import AnalysisSettings from './AnalysisSettings.vue';
import SessionControls from './SessionControls.vue';
import PageHeader from './ui/PageHeader.vue';
import { useWorkspaceNav } from '../composables/useWorkspaceNav';

const props = defineProps<{
    currentEngineLabel: string;
    currentModelDisplay: string;
    inputPath: string;
    hasMediaFile: boolean;
    isProcessing: boolean;
    hasBackendConfiguration: boolean;
    hasTranscript: boolean;
    settingsChanged: boolean;
    transcriptionBackend: TranscriptionBackend;
    localEngine: LocalEngine;
    context: string;
    glossary: string;
    speakerCount: number | null;
    removeFillerWords: boolean;
    trimSilence: boolean;
}>();

defineEmits<{
    (e: 'update:inputPath', value: string): void;
    (e: 'update:transcriptionBackend', value: TranscriptionBackend): void;
    (e: 'update:localEngine', value: LocalEngine): void;
    (e: 'update:context', value: string): void;
    (e: 'update:glossary', value: string): void;
    (e: 'update:speakerCount', value: number | null): void;
    (e: 'update:removeFillerWords', value: boolean): void;
    (e: 'update:trimSilence', value: boolean): void;
    (e: 'invalid-selection', message: string): void;
    (e: 'save-session'): void;
    (e: 'load-session'): void;
    (e: 'open-settings'): void;
    (e: 'process'): void;
}>();

const { selectStep } = useWorkspaceNav();

const mediaMissing = computed(() => !!props.inputPath && !props.hasMediaFile);

/** Nothing to re-run: the transcript matches the current options. */
const upToDate = computed(
    () => props.hasTranscript && !props.settingsChanged && !props.isProcessing,
);

const canAnalyze = computed(
    () => !props.isProcessing && props.hasMediaFile && props.hasBackendConfiguration,
);

const analyzeLabel = computed(() => {
    if (props.isProcessing) return 'Working…';
    return props.hasTranscript ? 'Re-analyze media' : 'Analyze media';
});

/** Why the primary action is unavailable, or what it will do. */
const analyzeNote = computed<{ tone: 'muted' | 'warning'; text: string } | null>(() => {
    if (props.isProcessing) {
        return {
            tone: 'muted',
            text: 'Another task is running. Follow it in the activity bar below.',
        };
    }
    if (upToDate.value) {
        return {
            tone: 'muted',
            text: 'The transcript matches these options. Change an option to analyze again.',
        };
    }
    if (!props.inputPath) return { tone: 'muted', text: 'Choose a media file to analyze.' };
    if (mediaMissing.value) {
        return { tone: 'warning', text: 'The media file is missing. Locate it to analyze.' };
    }
    if (!props.hasBackendConfiguration) {
        return {
            tone: 'warning',
            text: 'This pipeline needs an API key. Add one in Settings.',
        };
    }
    if (props.hasTranscript) {
        return { tone: 'muted', text: 'Re-analyzing replaces the current transcript.' };
    }
    return null;
});
</script>

<template>
    <div class="@container mx-auto w-full max-w-[1400px] px-8 pt-8 pb-10">
        <PageHeader
            title="Source"
            description="Pick a recording and choose how to transcribe it."
        />

        <div class="grid items-start gap-6 @4xl:grid-cols-[minmax(0,1fr)_320px]">
            <!-- Working area: media + analysis options -->
            <div class="min-w-0 space-y-6">
                <section aria-labelledby="source-media-title">
                    <h2 id="source-media-title" class="eyebrow mb-2">Media</h2>
                    <FileSelector
                        :modelValue="inputPath"
                        :missing="mediaMissing"
                        @update:modelValue="$emit('update:inputPath', $event)"
                        @invalid-selection="$emit('invalid-selection', $event)"
                    />
                </section>

                <section aria-labelledby="source-analysis-title">
                    <h2 id="source-analysis-title" class="eyebrow mb-2">Analysis options</h2>
                    <AnalysisSettings
                        :transcriptionBackend="transcriptionBackend"
                        :localEngine="localEngine"
                        :context="context"
                        :glossary="glossary"
                        :speakerCount="speakerCount"
                        :removeFillerWords="removeFillerWords"
                        :trimSilence="trimSilence"
                        @update:transcriptionBackend="$emit('update:transcriptionBackend', $event)"
                        @update:localEngine="$emit('update:localEngine', $event)"
                        @update:context="$emit('update:context', $event)"
                        @update:glossary="$emit('update:glossary', $event)"
                        @update:speakerCount="$emit('update:speakerCount', $event)"
                        @update:removeFillerWords="$emit('update:removeFillerWords', $event)"
                        @update:trimSilence="$emit('update:trimSilence', $event)"
                    />
                </section>
            </div>

            <!-- Inspector: pipeline, primary action, session -->
            <aside class="space-y-4 @4xl:sticky @4xl:top-8" aria-label="Analysis">
                <section class="panel overflow-hidden" data-testid="pipeline-card">
                    <div class="p-4">
                        <div class="flex items-center justify-between gap-2">
                            <p class="eyebrow">{{ currentEngineLabel }}</p>
                            <button
                                type="button"
                                class="btn-ghost btn-sm -mr-1.5"
                                @click="$emit('open-settings')"
                            >
                                <Settings2 aria-hidden="true" />
                                Change defaults
                            </button>
                        </div>
                        <p
                            class="mt-1.5 text-[14px] leading-snug font-medium break-words"
                            :class="hasBackendConfiguration ? 'text-fg' : 'text-warning'"
                        >
                            {{ currentModelDisplay }}
                        </p>
                    </div>

                    <div class="border-t border-line bg-surface-2/40 p-4">
                        <button
                            v-if="upToDate"
                            type="button"
                            class="btn-primary btn-lg w-full"
                            data-testid="open-transcript"
                            :aria-describedby="analyzeNote ? 'analyze-note' : undefined"
                            @click="selectStep('transcript')"
                        >
                            Open transcript
                            <ArrowRight aria-hidden="true" />
                        </button>
                        <button
                            v-else
                            type="button"
                            class="btn-primary btn-lg w-full"
                            data-testid="analyze-button"
                            :disabled="!canAnalyze"
                            :aria-describedby="analyzeNote ? 'analyze-note' : undefined"
                            @click="$emit('process')"
                        >
                            <Loader2 v-if="isProcessing" class="animate-spin" aria-hidden="true" />
                            <Sparkles v-else aria-hidden="true" />
                            {{ analyzeLabel }}
                        </button>
                        <p
                            v-if="analyzeNote"
                            id="analyze-note"
                            class="mt-2.5 flex items-start gap-1.5 text-[12px] leading-snug"
                            :class="
                                analyzeNote.tone === 'warning' ? 'text-warning' : 'text-fg-subtle'
                            "
                        >
                            <TriangleAlert
                                v-if="analyzeNote.tone === 'warning'"
                                class="mt-px h-3.5 w-3.5 shrink-0"
                                aria-hidden="true"
                            />
                            <Info v-else class="mt-px h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                            <span>{{ analyzeNote.text }}</span>
                        </p>
                    </div>
                </section>

                <SessionControls
                    :inputPath="inputPath"
                    :hasMediaFile="hasMediaFile"
                    @save-session="$emit('save-session')"
                    @load-session="$emit('load-session')"
                />
            </aside>
        </div>
    </div>
</template>
