<script setup lang="ts">
const props = defineProps<{
  count: number;
  minDuration: number;
  maxDuration: number;
  topic: string;
  splicing: boolean;
  isProcessing: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:count', value: number): void;
  (e: 'update:minDuration', value: number): void;
  (e: 'update:maxDuration', value: number): void;
  (e: 'update:topic', value: string): void;
  (e: 'update:splicing', value: boolean): void;
  (e: 'generate'): void;
}>();
</script>

<template>
    <div class="grid grid-cols-1 md:grid-cols-3 gap-6 mb-6">
        <div class="group">
            <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Count</label>
            <input :value="count" @input="$emit('update:count', ($event.target as HTMLInputElement).valueAsNumber)" type="number" min="1" max="10"
                class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white text-center" />
        </div>
        <div class="group">
            <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Min Sec</label>
            <input :value="minDuration" @input="$emit('update:minDuration', ($event.target as HTMLInputElement).valueAsNumber)" type="number" min="5"
                class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white text-center" />
        </div>
        <div class="group">
            <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Max Sec</label>
            <input :value="maxDuration" @input="$emit('update:maxDuration', ($event.target as HTMLInputElement).valueAsNumber)" type="number" min="10"
                class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white text-center" />
        </div>
    </div>

    <div class="mb-6">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Topic (Optional)</label>
        <input :value="topic" @input="$emit('update:topic', ($event.target as HTMLInputElement).value)" type="text"
            class="w-full p-4 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white placeholder-gray-600"
            placeholder="e.g. 'Funny moments', 'Technical explanation', 'Rants'..." />
    </div>

    <div class="mb-8 flex items-center justify-between p-4 bg-black/20 rounded-xl border border-white/5">
        <div>
            <h3 class="text-sm font-semibold text-gray-300">Smart Splicing</h3>
            <p class="text-xs text-gray-500">Allow AI to combine non-contiguous segments into one clip</p>
        </div>
        <button 
            @click="$emit('update:splicing', !splicing)"
            class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-pink-500 focus:ring-offset-2 focus:ring-offset-gray-900"
            :class="splicing ? 'bg-pink-600' : 'bg-gray-700'"
        >
            <span class="sr-only">Enable smart splicing</span>
            <span
                class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                :class="splicing ? 'translate-x-6' : 'translate-x-1'"
            />
        </button>
    </div>

    <button @click="$emit('generate')" :disabled="isProcessing"
        class="w-full mb-8 bg-gradient-to-r from-pink-600 to-purple-600 hover:from-pink-500 hover:to-purple-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg transition-all transform hover:-translate-y-0.5 active:translate-y-0">
        {{ isProcessing ? 'Processing...' : 'Generate Clips' }}
    </button>
</template>
