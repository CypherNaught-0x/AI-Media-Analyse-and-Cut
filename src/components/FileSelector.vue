<script setup lang="ts">
import { open } from '@tauri-apps/plugin-dialog';
import VideoFileIcon from '../assets/icons/video-file.svg?component';

const props = defineProps<{
  modelValue: string;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void;
}>();

async function selectFile() {
    try {
        const selected = await open({
            multiple: false,
            filters: [{
                name: 'Media',
                extensions: ['mp4', 'mkv', 'mov', 'avi', 'webm', 'flv', 'wmv', 'm4v', 'mp3', 'wav', 'aac', 'flac', 'ogg', 'm4a', 'wma']
            }]
        });

        if (selected && typeof selected === 'string') {
            emit('update:modelValue', selected);
        }
    } catch (e) {
        console.error("Failed to open dialog:", e);
    }
}
</script>

<template>
    <div class="mb-8">
        <label class="block text-sm font-medium text-gray-400 mb-3 uppercase tracking-wider">Source Media</label>
        <div class="flex gap-3">
            <div class="flex-1 relative group">
                <input :value="modelValue" @input="$emit('update:modelValue', ($event.target as HTMLInputElement).value)" type="text"
                    class="w-full p-4 pl-12 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 focus:bg-black/30 outline-none transition-all text-gray-300 placeholder-gray-600 font-mono text-sm"
                    placeholder="Select a media file..." readonly />
                <div class="absolute left-4 top-4 text-gray-500">
                    <VideoFileIcon class="h-5 w-5" />
                </div>
            </div>
            <button @click="selectFile"
                class="px-8 bg-blue-600 hover:bg-blue-500 text-white font-semibold rounded-2xl shadow-lg shadow-blue-900/20 transition-all transform active:scale-95">
                Browse
            </button>
        </div>
    </div>
</template>
