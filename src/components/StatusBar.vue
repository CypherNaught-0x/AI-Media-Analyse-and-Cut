<script setup lang="ts">
const props = defineProps<{
  status: string;
  isProcessing: boolean;
  progressPercentage: number | null;
}>();
</script>

<template>
    <div class="fixed bottom-0 left-0 right-0 p-4 bg-black/50 backdrop-blur-md border-t border-white/10 flex items-center justify-between z-50">
        <div class="max-w-5xl mx-auto w-full flex flex-col gap-2">
            <div v-if="progressPercentage !== null" class="w-full bg-gray-700 rounded-full h-1.5 overflow-hidden">
                <div class="bg-blue-500 h-full transition-all duration-300 ease-out" :style="{ width: `${progressPercentage}%` }"></div>
            </div>
            <div class="flex items-center gap-3">
                <div class="w-2 h-2 rounded-full"
                    :class="isProcessing ? 'bg-yellow-400 animate-pulse' : 'bg-emerald-400'"></div>
                <span class="text-sm font-mono text-gray-400 truncate">{{ status }}</span>
            </div>
        </div>
    </div>
</template>
