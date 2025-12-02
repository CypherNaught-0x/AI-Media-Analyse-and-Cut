<script setup lang="ts">
import type { TranscriptSegment } from '../types';

defineProps<{
  segments: TranscriptSegment[];
}>();

const emit = defineEmits<{
  (e: 'jump-to', time: number): void;
}>();

const parseTime = (timeStr: string): number => {
  const [mm, ss] = timeStr.split(':').map(Number);
  return mm * 60 + ss;
};

const jumpTo = (timeStr: string) => {
  emit('jump-to', parseTime(timeStr));
};
</script>

<template>
  <div class="editor-container p-4 bg-gray-800 rounded-lg overflow-y-auto max-h-[600px]">
    <div v-for="(segment, index) in segments" :key="index" 
         class="segment mb-4 p-3 bg-gray-700 rounded hover:bg-gray-600 cursor-pointer transition-colors"
         @click="jumpTo(segment.start)">
      <div class="flex justify-between text-sm text-gray-400 mb-1">
        <span class="font-bold text-blue-400">{{ segment.speaker }}</span>
        <span>{{ segment.start }} - {{ segment.end }}</span>
      </div>
      <p class="text-gray-200">{{ segment.text }}</p>
    </div>
  </div>
</template>

<style scoped>
/* Custom scrollbar if needed */
</style>
