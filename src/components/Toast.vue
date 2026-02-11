<script setup lang="ts">
import { computed } from 'vue';

type ToastTone = 'info' | 'success' | 'error';

const props = withDefaults(defineProps<{
  show: boolean;
  message: string;
  tone?: ToastTone;
  progress?: number | null;
  actionLabel?: string;
}>(), {
  tone: 'info',
  progress: null,
  actionLabel: '',
});

const emit = defineEmits<{
  dismiss: [];
  action: [];
}>();

const toneStyles = computed(() => {
  switch (props.tone) {
    case 'success':
      return {
        ring: 'border-emerald-400/30',
        icon: 'bg-emerald-500/15 text-emerald-300',
        bar: 'bg-emerald-400',
      };
    case 'error':
      return {
        ring: 'border-red-400/30',
        icon: 'bg-red-500/15 text-red-300',
        bar: 'bg-red-400',
      };
    default:
      return {
        ring: 'border-blue-400/30',
        icon: 'bg-blue-500/15 text-blue-300',
        bar: 'bg-blue-400',
      };
  }
});
</script>

<template>
  <transition name="toast-slide">
    <div
      v-if="show"
      class="fixed top-6 right-6 z-[200] w-80 max-w-[calc(100vw-3rem)]"
      role="status"
      aria-live="polite"
    >
      <div
        class="rounded-2xl border bg-gray-950/95 shadow-2xl shadow-black/40 backdrop-blur-md p-4"
        :class="toneStyles.ring"
      >
        <div class="flex items-start gap-3">
          <div class="mt-0.5 h-8 w-8 rounded-full flex items-center justify-center" :class="toneStyles.icon">
            <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M13 16h-1v-4h-1m1-4h.01M12 8a4 4 0 00-4 4v1a4 4 0 004 4h0a4 4 0 004-4v-1a4 4 0 00-4-4z"
              />
            </svg>
          </div>
          <div class="flex-1">
            <p class="text-sm text-gray-100 leading-snug">{{ message }}</p>
            <div v-if="progress !== null" class="mt-3">
              <div class="h-1.5 w-full rounded-full bg-white/10 overflow-hidden">
                <div class="h-full transition-all duration-300" :class="toneStyles.bar" :style="{ width: `${progress}%` }" />
              </div>
              <p class="mt-1 text-xs text-gray-400">{{ Math.round(progress) }}%</p>
            </div>
            <div v-if="actionLabel" class="mt-3">
              <button
                class="px-3 py-1.5 text-xs font-semibold uppercase tracking-wider rounded-lg bg-white/10 text-gray-100 hover:bg-white/20 transition"
                @click="emit('action')"
              >
                {{ actionLabel }}
              </button>
            </div>
          </div>
          <button
            class="h-7 w-7 rounded-lg text-gray-400 hover:text-gray-200 hover:bg-white/10 transition"
            @click="emit('dismiss')"
            aria-label="Dismiss"
          >
            <svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
      </div>
    </div>
  </transition>
</template>

<style scoped>
.toast-slide-enter-active,
.toast-slide-leave-active {
  transition: opacity 0.25s ease, transform 0.25s ease;
}

.toast-slide-enter-from,
.toast-slide-leave-to {
  opacity: 0;
  transform: translateY(-8px);
}
</style>
