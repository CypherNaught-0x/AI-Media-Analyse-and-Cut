<script setup lang="ts">
import FolderOpenIcon from '../assets/icons/folder-open.svg?component';
import type { Clip } from '../types';

const props = defineProps<{
  clips: Clip[];
  lastExportPath: string;
  isProcessing: boolean;
}>();

const emit = defineEmits<{
  (e: 'export'): void;
  (e: 'openFolder'): void;
}>();
</script>

<template>
    <div v-if="clips.length > 0" class="space-y-4">
        <div v-for="(clip, index) in clips" :key="index"
            class="p-6 bg-black/20 rounded-2xl border border-white/5 hover:border-pink-500/30 transition-colors">
            <div class="flex justify-between items-start mb-3">
                <h3 class="font-bold text-lg text-pink-400">{{ clip.title }}</h3>
                <div class="flex flex-col items-end gap-1">
                    <span v-for="(seg, i) in clip.segments" :key="i" class="px-2 py-1 rounded bg-white/5 text-xs text-gray-400 font-mono">
                        {{ seg.start }} - {{ seg.end }}
                    </span>
                </div>
            </div>
            <p class="text-gray-300 text-sm leading-relaxed">{{ clip.reason }}</p>
        </div>

        <div class="flex gap-4 mt-6">
            <button @click="$emit('export')" :disabled="isProcessing"
                class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-bold py-4 px-6 rounded-2xl border border-gray-600 hover:border-gray-500 transition-all">
                Export All Clips
            </button>
            <button v-if="lastExportPath" @click="$emit('openFolder')"
                class="px-6 bg-gray-800 hover:bg-gray-700 text-white font-bold rounded-2xl border border-gray-700 transition-all" title="Open Folder">
                <FolderOpenIcon class="h-6 w-6" />
            </button>
        </div>
    </div>
</template>
