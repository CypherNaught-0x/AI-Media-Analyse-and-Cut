<script setup lang="ts">
defineProps<{
  inputPath: string;
  hasMediaFile: boolean;
}>();

defineEmits<{
  (e: 'save-session'): void;
  (e: 'load-session'): void;
}>();
</script>

<template>
  <div class="mb-6 flex flex-wrap gap-3">
    <button
      @click="$emit('save-session')"
      class="px-4 py-2 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-xl transition-all border border-white/10 focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none"
    >
      Save Session
    </button>
    <button
      @click="$emit('load-session')"
      class="px-4 py-2 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-xl transition-all border border-white/10 focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none"
    >
      Load Session
    </button>
    <span
      v-if="inputPath && !hasMediaFile"
      class="px-3 py-2 rounded-xl border border-amber-500/30 bg-amber-500/10 text-amber-300 text-sm"
    >
      Media file missing. Transcript state is restored, but media actions are disabled until you choose a valid file.
    </span>
  </div>
</template>
