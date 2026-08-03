<script setup lang="ts">
import { computed, ref } from 'vue';
import type { LocalEngine, TranscriptionBackend } from '../types';
import { usesLocalEngine, usesRemoteModel } from '../types';

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
    () => usesLlmAssist.value || props.localEngine === 'crisper',
);
const isCrisperEngine = computed(
    () => showsLocalEngine.value && props.localEngine === 'crisper',
);

const PIPELINES: { value: TranscriptionBackend; title: string; description: string }[] = [
    {
        value: 'llm',
        title: 'LLM Only',
        description: 'Sends the audio to your configured API model. Uses context, glossary and speaker count.',
    },
    {
        value: 'local',
        title: 'Local Only',
        description: 'Runs entirely on this machine with the engine below. No API key, nothing leaves the device.',
    },
    {
        value: 'hybrid',
        title: 'Hybrid Cleanup',
        description: 'Keeps the local engine’s timings, then an LLM pass tidies wording and punctuation.',
    },
    {
        value: 'hybrid-merge',
        title: 'Hybrid Merge',
        description: 'Transcribes locally and remotely, then merges the strengths of both onto the local timings.',
    },
];

const ENGINES: { value: LocalEngine; title: string; badge?: string; description: string }[] = [
    {
        value: 'parakeet',
        title: 'Parakeet',
        description: 'Parakeet TDT + Sortformer diarization. Fast, multilingual, auto-downloads.',
    },
    {
        value: 'crisper',
        title: 'CrisperWhisper',
        badge: 'EN / DE',
        description: 'Verbatim, ~30 ms word timings, keeps or cuts fillers.',
    },
];

function startResize(e: MouseEvent, textarea: HTMLTextAreaElement | null) {
    if (!textarea) return;

    const startY = e.clientY;
    const startHeight = textarea.offsetHeight;

    function onMouseMove(e: MouseEvent) {
        const newHeight = startHeight + (e.clientY - startY);
        if (newHeight > 60) { // Minimum height
            textarea!.style.height = `${newHeight}px`;
        }
    }

    function onMouseUp() {
        document.removeEventListener('mousemove', onMouseMove);
        document.removeEventListener('mouseup', onMouseUp);
    }

    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
}
</script>

<template>
    <div class="mb-6 space-y-5">
        <div>
            <label class="mb-3 block text-sm font-medium uppercase tracking-wider text-gray-400">
                Transcription Pipeline
            </label>
            <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
                <button
                    v-for="pipeline in PIPELINES"
                    :key="pipeline.value"
                    type="button"
                    class="flex h-full flex-col rounded-2xl border p-4 text-left transition-all focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
                    :class="transcriptionBackend === pipeline.value
                        ? 'bg-blue-600/15 border-blue-500/40 text-white'
                        : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'"
                    @click="$emit('update:transcriptionBackend', pipeline.value)"
                >
                    <span class="text-sm font-semibold">{{ pipeline.title }}</span>
                    <span class="mt-1 text-xs leading-relaxed text-gray-400">{{ pipeline.description }}</span>
                </button>
            </div>
        </div>

        <!-- The engine is independent of the pipeline: every non-LLM pipeline
             runs whichever engine is picked here. -->
        <div v-if="showsLocalEngine">
            <label class="mb-2 block text-sm font-medium uppercase tracking-wider text-gray-400">
                Local Engine
            </label>
            <div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <button
                    v-for="engine in ENGINES"
                    :key="engine.value"
                    type="button"
                    class="flex h-full flex-col rounded-xl border px-4 py-3 text-left transition-all focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/60"
                    :class="localEngine === engine.value
                        ? 'bg-blue-600/15 border-blue-500/40 text-white'
                        : 'bg-black/20 border-white/10 text-gray-300 hover:bg-black/30'"
                    @click="$emit('update:localEngine', engine.value)"
                >
                    <span class="flex items-center gap-2">
                        <span class="text-sm font-semibold">{{ engine.title }}</span>
                        <span
                            v-if="engine.badge"
                            class="rounded bg-amber-500/20 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-amber-300"
                        >{{ engine.badge }}</span>
                    </span>
                    <span class="mt-0.5 text-xs text-gray-400">{{ engine.description }}</span>
                </button>
            </div>

            <!-- Shown on selection: the weights carry a licence restriction the app cannot enforce. -->
            <div
                v-if="isCrisperEngine"
                class="mt-3 rounded-2xl border border-amber-500/30 bg-amber-500/10 p-4"
            >
                <p class="mb-1 text-xs font-semibold text-amber-200">
                    Non-commercial use only &middot; English and German only
                </p>
                <p class="text-xs leading-relaxed text-amber-100/80">
                    The CrisperWhisper 2.0 weights are released under the Nyra Health
                    <strong>Non-Commercial Research License</strong>. Research and other
                    non-commercial use is free; <strong>commercial use requires a license from
                    Nyra Health</strong>. The model card is published for English and German only.
                </p>
            </div>
        </div>
    </div>

    <div class="grid grid-cols-1 md:grid-cols-2 gap-5 mb-6">
        <div class="md:col-span-2">
            <div class="flex items-center justify-between mb-2">
                <label class="block text-sm font-medium text-gray-400 uppercase tracking-wider">Context</label>
                <span v-if="!usesLlmAssist" class="text-[10px] uppercase tracking-wider text-gray-500">Hybrid / LLM</span>
            </div>
            <div class="relative">
                <textarea ref="contextTextarea" :value="context" @input="$emit('update:context', ($event.target as HTMLTextAreaElement).value)" rows="2"
                    class="w-full p-4 pb-8 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-colors text-gray-300 placeholder-gray-600 resize-none"
                    :disabled="!usesLlmAssist"
                    :class="{ 'opacity-60 cursor-not-allowed': !usesLlmAssist }"
                    placeholder="Describe the video content to help the AI... Especially for translation"></textarea>
                <div @mousedown.prevent="startResize($event, contextTextarea)"
                    class="absolute bottom-0 left-0 right-0 h-6 cursor-ns-resize flex items-center justify-center hover:bg-white/5 rounded-b-2xl transition-colors group">
                    <div class="w-12 h-1 bg-white/10 rounded-full group-hover:bg-white/20 transition-colors"></div>
                </div>
            </div>
        </div>
        
        <div>
            <div class="flex items-center justify-between mb-2">
                <label class="block text-sm font-medium text-gray-400 uppercase tracking-wider">Glossary</label>
                <span v-if="!usesLlmAssist" class="text-[10px] uppercase tracking-wider text-gray-500">Hybrid / LLM</span>
            </div>
            <div class="relative">
                <textarea ref="glossaryTextarea" :value="glossary" @input="$emit('update:glossary', ($event.target as HTMLTextAreaElement).value)" rows="2"
                    class="w-full p-4 pb-8 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600 resize-none"
                    :disabled="!usesLlmAssist"
                    :class="{ 'opacity-60 cursor-not-allowed': !usesLlmAssist }"
                    placeholder="Specific terms, names, acronyms..."></textarea>
                <div @mousedown.prevent="startResize($event, glossaryTextarea)"
                    class="absolute bottom-0 left-0 right-0 h-6 cursor-ns-resize flex items-center justify-center hover:bg-white/5 rounded-b-2xl transition-colors group">
                    <div class="w-12 h-1 bg-white/10 rounded-full group-hover:bg-white/20 transition-colors"></div>
                </div>
            </div>
        </div>

        <div>
            <div class="flex items-center justify-between mb-2">
                <label class="block text-sm font-medium text-gray-400 uppercase tracking-wider">Speakers</label>
                <span v-if="!usesLlmAssist" class="text-[10px] uppercase tracking-wider text-gray-500">Local diarization</span>
            </div>
            <div class="relative">
                <input :value="speakerCount" @input="$emit('update:speakerCount', ($event.target as HTMLInputElement).valueAsNumber || null)" type="number" min="1"
                    class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                    :disabled="!usesLlmAssist"
                    :class="{ 'opacity-60 cursor-not-allowed': !usesLlmAssist }"
                    placeholder="Auto-detect" />
                <div class="absolute right-4 top-4 text-gray-600 text-xs pointer-events-none select-none">Optional</div>
            </div>
        </div>
    </div>

    <!-- Advanced Options -->
    <div class="mb-6 flex flex-wrap items-center gap-3">
        <div
            class="flex items-center gap-3 p-4 rounded-xl border border-white/5 transition-colors"
            :class="supportsFillerRemoval
                ? 'bg-black/20 cursor-pointer hover:bg-black/30'
                : 'bg-black/10 opacity-60 cursor-not-allowed'"
            :title="isCrisperEngine
                ? 'Cuts [UM] / [UH] out of the transcript and the exported video.'
                : undefined"
            @click="supportsFillerRemoval && $emit('update:removeFillerWords', !removeFillerWords)"
        >
            <div class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none"
                :class="removeFillerWords ? 'bg-blue-600' : 'bg-gray-700'">
                <span class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                    :class="removeFillerWords ? 'translate-x-6' : 'translate-x-1'" />
            </div>
            <span class="text-sm font-medium text-gray-300">Remove Filler Words</span>
        </div>
        <div class="flex items-center gap-3 p-4 bg-black/20 rounded-xl border border-white/5 cursor-pointer hover:bg-black/30 transition-colors" @click="$emit('update:trimSilence', !trimSilence)">
            <div class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none"
                :class="trimSilence ? 'bg-blue-600' : 'bg-gray-700'">
                <span class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                    :class="trimSilence ? 'translate-x-6' : 'translate-x-1'" />
            </div>
            <span class="text-sm font-medium text-gray-300">Trim Silence</span>
        </div>
    </div>
</template>
