<script setup lang="ts">
import { ref } from 'vue';

const props = defineProps<{
  context: string;
  glossary: string;
  speakerCount: number | null;
  removeFillerWords: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:context', value: string): void;
  (e: 'update:glossary', value: string): void;
  (e: 'update:speakerCount', value: number | null): void;
  (e: 'update:removeFillerWords', value: boolean): void;
}>();

const contextTextarea = ref<HTMLTextAreaElement | null>(null);
const glossaryTextarea = ref<HTMLTextAreaElement | null>(null);

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
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6 mb-8">
        <div class="md:col-span-2">
            <label class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">Context</label>
            <div class="relative">
                <textarea ref="contextTextarea" :value="context" @input="$emit('update:context', ($event.target as HTMLTextAreaElement).value)" rows="2"
                    class="w-full p-4 pb-8 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-colors text-gray-300 placeholder-gray-600 resize-none"
                    placeholder="Describe the video content to help the AI... Especially for translation"></textarea>
                <div @mousedown.prevent="startResize($event, contextTextarea)"
                    class="absolute bottom-0 left-0 right-0 h-6 cursor-ns-resize flex items-center justify-center hover:bg-white/5 rounded-b-2xl transition-colors group">
                    <div class="w-12 h-1 bg-white/10 rounded-full group-hover:bg-white/20 transition-colors"></div>
                </div>
            </div>
        </div>
        
        <div>
            <label class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">Glossary</label>
            <div class="relative">
                <textarea ref="glossaryTextarea" :value="glossary" @input="$emit('update:glossary', ($event.target as HTMLTextAreaElement).value)" rows="2"
                    class="w-full p-4 pb-8 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600 resize-none"
                    placeholder="Specific terms, names, acronyms..."></textarea>
                <div @mousedown.prevent="startResize($event, glossaryTextarea)"
                    class="absolute bottom-0 left-0 right-0 h-6 cursor-ns-resize flex items-center justify-center hover:bg-white/5 rounded-b-2xl transition-colors group">
                    <div class="w-12 h-1 bg-white/10 rounded-full group-hover:bg-white/20 transition-colors"></div>
                </div>
            </div>
        </div>

        <div>
            <label class="block text-sm font-medium text-gray-400 mb-2 uppercase tracking-wider">Speakers</label>
            <div class="relative">
                <input :value="speakerCount" @input="$emit('update:speakerCount', ($event.target as HTMLInputElement).valueAsNumber || null)" type="number" min="1"
                    class="w-full p-4 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 outline-none transition-all text-gray-300 placeholder-gray-600"
                    placeholder="Auto-detect" />
                <div class="absolute right-4 top-4 text-gray-600 text-xs pointer-events-none select-none">Optional</div>
            </div>
        </div>
    </div>

    <!-- Advanced Options -->
    <div class="mb-8 flex items-center gap-4">
        <div class="flex items-center gap-3 p-4 bg-black/20 rounded-xl border border-white/5 cursor-pointer hover:bg-black/30 transition-colors" @click="$emit('update:removeFillerWords', !removeFillerWords)">
            <div class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none"
                :class="removeFillerWords ? 'bg-blue-600' : 'bg-gray-700'">
                <span class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
                    :class="removeFillerWords ? 'translate-x-6' : 'translate-x-1'" />
            </div>
            <span class="text-sm font-medium text-gray-300">Remove Filler Words</span>
        </div>
    </div>
</template>
