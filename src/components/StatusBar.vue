<script setup lang="ts">
const props = defineProps<{
  status: string;
  isProcessing: boolean;
  progressPercentage: number | null;
  progressEtaSeconds?: number | null;
  isCancelling?: boolean;
}>();

const emit = defineEmits<{
  (e: 'cancel'): void;
}>();

function formatEta(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 1) {
    return '';
  }

  const rounded = Math.ceil(seconds);
  const minutes = Math.floor(rounded / 60);
  const remainingSeconds = rounded % 60;
  return minutes > 0
    ? `ETA ${minutes}:${remainingSeconds.toString().padStart(2, '0')}`
    : `ETA 0:${remainingSeconds.toString().padStart(2, '0')}`;
}
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
                <span v-if="progressEtaSeconds !== null && progressEtaSeconds !== undefined && progressPercentage !== null && progressPercentage < 100" class="text-xs font-mono text-gray-500 whitespace-nowrap">
                    {{ formatEta(progressEtaSeconds) }}
                </span>
                <button
                    v-if="isProcessing"
                    @click="emit('cancel')"
                    :disabled="isCancelling"
                    class="ml-auto rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-1 text-xs font-semibold text-red-300 transition-colors hover:bg-red-500/20 disabled:cursor-not-allowed disabled:opacity-60"
                >
                    {{ isCancelling ? 'Cancelling...' : 'Cancel' }}
                </button>
            </div>
        </div>
    </div>
</template>
