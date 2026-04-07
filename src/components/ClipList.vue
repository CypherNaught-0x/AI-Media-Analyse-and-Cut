<script setup lang="ts">
import { computed } from 'vue';
import FolderOpenIcon from '../assets/icons/folder-open.svg?component';
import type { Clip, ClipExportPayload } from '../types';

const props = defineProps<{
  clips: Clip[];
  lastExportPath: string;
  isProcessing: boolean;
  hasMediaFile: boolean;
  includeSubtitles: boolean;
  fastMode: boolean;
  trimBoundarySilence: boolean;
  selectedClipIndices: number[];
}>();

const emit = defineEmits<{
  (e: 'export', payload: ClipExportPayload): void;
  (e: 'openFolder'): void;
  (e: 'update:includeSubtitles', value: boolean): void;
  (e: 'update:fastMode', value: boolean): void;
  (e: 'update:trimBoundarySilence', value: boolean): void;
  (e: 'update:selectedClipIndices', value: number[]): void;
}>();

const selectedIndices = computed({
    get: () => new Set(props.selectedClipIndices),
    set: (indices: Set<number>) => emit('update:selectedClipIndices', Array.from(indices).sort((a, b) => a - b)),
});

const includeSubtitles = computed({
    get: () => props.includeSubtitles,
    set: (value: boolean) => emit('update:includeSubtitles', value),
});

const fastMode = computed({
    get: () => props.fastMode,
    set: (value: boolean) => emit('update:fastMode', value),
});

const trimBoundarySilence = computed({
    get: () => props.trimBoundarySilence,
    set: (value: boolean) => emit('update:trimBoundarySilence', value),
});

const toggleSelection = (index: number) => {
    const next = new Set(selectedIndices.value);
    if (next.has(index)) {
        next.delete(index);
    } else {
        next.add(index);
    }
    selectedIndices.value = next;
};

const toggleAll = () => {
    if (selectedIndices.value.size === props.clips.length) {
        selectedIndices.value = new Set();
    } else {
        selectedIndices.value = new Set(props.clips.map((_, i) => i));
    }
};

const handleExport = () => {
    const clipsToExport = selectedIndices.value.size > 0
        ? props.clips.filter((_, i) => selectedIndices.value.has(i))
        : props.clips;
    
    emit('export', {
        clips: clipsToExport,
        includeSubtitles: includeSubtitles.value,
        fastMode: fastMode.value,
        trimBoundarySilence: trimBoundarySilence.value
    });
};

const selectionLabel = computed(() => {
    if (selectedIndices.value.size === 0) return 'Export All Clips';
    return `Export ${selectedIndices.value.size} Selected Clip${selectedIndices.value.size > 1 ? 's' : ''}`;
});
</script>

<template>
    <div v-if="clips.length > 0" class="space-y-4">
        <!-- Toolbar -->
        <div class="flex items-center justify-between mb-4 px-2">
            <div class="flex items-center gap-4">
                <label class="flex items-center gap-2 cursor-pointer text-sm text-gray-400 hover:text-gray-300">
                    <input type="checkbox" 
                        :checked="selectedIndices.size === clips.length && clips.length > 0"
                        @change="toggleAll"
                        class="rounded bg-white/10 border-white/20 text-blue-500 focus:ring-blue-500/50" />
                    Select All
                </label>
                <label class="flex items-center gap-2 cursor-pointer text-sm text-gray-400 hover:text-gray-300">
                    <input type="checkbox" v-model="includeSubtitles"
                        class="rounded bg-white/10 border-white/20 text-blue-500 focus:ring-blue-500/50" />
                    Auto-export Subtitles
                </label>
                <label class="flex items-center gap-2 cursor-pointer text-sm text-gray-400 hover:text-gray-300" title="Use 'copy' codec for faster, lossless export (may be less precise)">
                    <input type="checkbox" v-model="fastMode"
                        class="rounded bg-white/10 border-white/20 text-blue-500 focus:ring-blue-500/50" />
                    Fast Mode (Lossless)
                </label>
                <label class="flex items-center gap-2 cursor-pointer text-sm text-gray-400 hover:text-gray-300" title="Trim only silence that touches the clip start or end">
                    <input data-testid="trim-boundary-silence" type="checkbox" v-model="trimBoundarySilence"
                        class="rounded bg-white/10 border-white/20 text-blue-500 focus:ring-blue-500/50" />
                    Trim Start/End Silence
                </label>
            </div>
            <span v-if="selectedIndices.size > 0" class="text-xs text-blue-400 font-medium">
                {{ selectedIndices.size }} selected
            </span>
        </div>

        <div v-for="(clip, index) in clips" :key="index"
            @click="toggleSelection(index)"
            class="p-6 bg-black/20 rounded-2xl border transition-colors cursor-pointer relative group"
            :class="selectedIndices.has(index) ? 'border-blue-500/50 bg-blue-500/10' : 'border-white/5 hover:border-pink-500/30'">
            
            <!-- Checkbox overlay -->
            <div class="absolute top-4 right-4">
                <input type="checkbox" :checked="selectedIndices.has(index)"
                    class="rounded-full w-5 h-5 bg-black/40 border-white/30 text-blue-500 focus:ring-offset-0 focus:ring-0 cursor-pointer" />
            </div>

            <div class="flex justify-between items-start mb-3 pr-8">
                <h3 class="font-bold text-lg" :class="selectedIndices.has(index) ? 'text-blue-300' : 'text-pink-400'">{{ clip.title }}</h3>
                <div class="flex flex-col items-end gap-1">
                    <span v-for="(seg, i) in clip.segments" :key="i" class="px-2 py-1 rounded bg-white/5 text-xs text-gray-400 font-mono">
                        {{ seg.start }} - {{ seg.end }}
                    </span>
                </div>
            </div>
            <p class="text-gray-300 text-sm leading-relaxed">{{ clip.reason }}</p>
        </div>

        <div class="flex gap-4 mt-6">
            <button @click="handleExport" :disabled="isProcessing || !hasMediaFile"
                class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-bold py-4 px-6 rounded-2xl border border-gray-600 hover:border-gray-500 transition-all flex items-center justify-center gap-2">
                <span>{{ selectionLabel }}</span>
                <span v-if="includeSubtitles" class="text-xs bg-black/20 px-2 py-0.5 rounded text-gray-300">+ Subs</span>
                <span v-if="trimBoundarySilence" class="text-xs bg-black/20 px-2 py-0.5 rounded text-gray-300">+ Trim</span>
            </button>
            <button v-if="lastExportPath" @click="$emit('openFolder')"
                class="px-6 bg-gray-800 hover:bg-gray-700 text-white font-bold rounded-2xl border border-gray-700 transition-all" title="Open Folder">
                <FolderOpenIcon class="h-6 w-6" />
            </button>
        </div>
    </div>
</template>
