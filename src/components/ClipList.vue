<script setup lang="ts">
import { ref, computed, watch } from 'vue';
import FolderOpenIcon from '../assets/icons/folder-open.svg?component';
import type { Clip } from '../types';

const props = defineProps<{
  clips: Clip[];
  lastExportPath: string;
  isProcessing: boolean;
}>();

const emit = defineEmits<{
  (e: 'export', payload: { clips: Clip[], includeSubtitles: boolean }): void;
  (e: 'openFolder'): void;
}>();

const selectedIndices = ref<Set<number>>(new Set());
const includeSubtitles = ref(true);

const toggleSelection = (index: number) => {
    if (selectedIndices.value.has(index)) {
        selectedIndices.value.delete(index);
    } else {
        selectedIndices.value.add(index);
    }
};

const toggleAll = () => {
    if (selectedIndices.value.size === props.clips.length) {
        selectedIndices.value.clear();
    } else {
        props.clips.forEach((_, i) => selectedIndices.value.add(i));
    }
};

const handleExport = () => {
    const clipsToExport = selectedIndices.value.size > 0
        ? props.clips.filter((_, i) => selectedIndices.value.has(i))
        : props.clips;
    
    emit('export', { clips: clipsToExport, includeSubtitles: includeSubtitles.value });
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
                        :indeterminate="selectedIndices.size > 0 && selectedIndices.size < clips.length"
                        @change="toggleAll"
                        class="rounded bg-white/10 border-white/20 text-blue-500 focus:ring-blue-500/50" />
                    Select All
                </label>
                <label class="flex items-center gap-2 cursor-pointer text-sm text-gray-400 hover:text-gray-300">
                    <input type="checkbox" v-model="includeSubtitles"
                        class="rounded bg-white/10 border-white/20 text-blue-500 focus:ring-blue-500/50" />
                    Auto-export Subtitles
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
            <button @click="handleExport" :disabled="isProcessing"
                class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-bold py-4 px-6 rounded-2xl border border-gray-600 hover:border-gray-500 transition-all flex items-center justify-center gap-2">
                <span>{{ selectionLabel }}</span>
                <span v-if="includeSubtitles" class="text-xs bg-black/20 px-2 py-0.5 rounded text-gray-300">+ Subs</span>
            </button>
            <button v-if="lastExportPath" @click="$emit('openFolder')"
                class="px-6 bg-gray-800 hover:bg-gray-700 text-white font-bold rounded-2xl border border-gray-700 transition-all" title="Open Folder">
                <FolderOpenIcon class="h-6 w-6" />
            </button>
        </div>
    </div>
</template>
