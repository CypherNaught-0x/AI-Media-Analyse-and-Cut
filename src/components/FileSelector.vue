<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open } from '@tauri-apps/plugin-dialog';
import VideoFileIcon from '../assets/icons/video-file.svg?component';

const MEDIA_EXTENSIONS = [
    'mp4',
    'mkv',
    'mov',
    'avi',
    'webm',
    'flv',
    'wmv',
    'm4v',
    'mp3',
    'wav',
    'aac',
    'flac',
    'ogg',
    'm4a',
    'wma',
];
const SUPPORTED_MEDIA_EXTENSIONS = new Set(MEDIA_EXTENSIONS);

const props = defineProps<{
  modelValue: string;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void;
  (e: 'invalid-selection', message: string): void;
}>();

const dropZoneRef = ref<HTMLElement | null>(null);
const isDragActive = ref(false);

let unlistenDragDrop: (() => void) | null = null;
let tauriDragHasSupportedFile = false;
const appWindow = getCurrentWindow();

function isSupportedMediaPath(path: string) {
    const extension = path.split('.').pop()?.toLowerCase();
    return extension ? SUPPORTED_MEDIA_EXTENSIONS.has(extension) : false;
}

function getFirstSupportedPath(paths: string[]) {
    return paths.find(isSupportedMediaPath) ?? null;
}

function selectPath(path: string) {
    emit('update:modelValue', path);
}

function emitInvalidSelection() {
    emit('invalid-selection', 'Please select a supported video or audio file.');
}

function pointInsideZone(x: number, y: number) {
    if (!dropZoneRef.value) return false;

    const rect = dropZoneRef.value.getBoundingClientRect();
    return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

function pointTargetsDropZone(x: number, y: number) {
    if (
        !dropZoneRef.value ||
        !Number.isFinite(x) ||
        !Number.isFinite(y) ||
        typeof document.elementFromPoint !== 'function'
    ) {
        return false;
    }

    const target = document.elementFromPoint(x, y);
    return !!target && dropZoneRef.value.contains(target);
}

async function isDropInsideZone(position: { x: number; y: number }) {
    const scale = window.devicePixelRatio || 1;
    const candidates = [
        { x: position.x, y: position.y },
        { x: position.x / scale, y: position.y / scale },
    ];

    try {
        const windowPosition = await appWindow.innerPosition();
        const relativeX = position.x - windowPosition.x;
        const relativeY = position.y - windowPosition.y;
        candidates.push(
            { x: relativeX, y: relativeY },
            { x: relativeX / scale, y: relativeY / scale },
        );
    } catch (error) {
        console.debug('Unable to resolve window position for drag-and-drop hit testing.', error);
    }

    return candidates.some(({ x, y }) => pointInsideZone(x, y) || pointTargetsDropZone(x, y));
}

async function selectFile() {
    try {
        const selected = await open({
            multiple: false,
            filters: [{
                name: 'Media',
                extensions: MEDIA_EXTENSIONS
            }]
        });

        if (selected && typeof selected === 'string') {
            selectPath(selected);
        }
    } catch (e) {
        console.error("Failed to open dialog:", e);
    }
}

onMounted(async () => {
    try {
        unlistenDragDrop = await appWindow.onDragDropEvent(async ({ payload }) => {
            if (payload.type === 'leave') {
                tauriDragHasSupportedFile = false;
                isDragActive.value = false;
                return;
            }

            if (payload.type === 'enter') {
                tauriDragHasSupportedFile = payload.paths.some(isSupportedMediaPath);
                isDragActive.value = tauriDragHasSupportedFile && await isDropInsideZone(payload.position);
                return;
            }

            if (payload.type === 'over') {
                isDragActive.value = tauriDragHasSupportedFile && await isDropInsideZone(payload.position);
                return;
            }

            const selectedPath = getFirstSupportedPath(payload.paths);
            const droppedInsideZone = await isDropInsideZone(payload.position);

            tauriDragHasSupportedFile = false;
            isDragActive.value = false;

            if (!droppedInsideZone) return;

            if (selectedPath) {
                selectPath(selectedPath);
                return;
            }

            emitInvalidSelection();
        });
    } catch (error) {
        console.debug('Drag-and-drop listener unavailable.', error);
    }
});

onUnmounted(() => {
    unlistenDragDrop?.();
});
</script>

<template>
    <div class="mb-6">
        <label class="block text-sm font-medium text-gray-400 mb-3 uppercase tracking-wider">Source Media</label>
        <div
            ref="dropZoneRef"
            data-testid="file-drop-zone"
            :data-drag-active="isDragActive ? 'true' : 'false'"
            class="flex gap-3 rounded-3xl border border-transparent transition-all duration-200"
            :class="isDragActive ? 'bg-blue-500/10 border-blue-400/50 shadow-lg shadow-blue-900/20' : ''"
        >
            <div class="flex-1 relative group">
                <input :value="modelValue" @input="$emit('update:modelValue', ($event.target as HTMLInputElement).value)" type="text"
                    class="w-full p-4 pl-12 rounded-2xl bg-black/20 border border-white/10 focus:border-blue-500/50 focus:bg-black/30 outline-none transition-all text-gray-300 placeholder-gray-600 font-mono text-sm"
                    placeholder="Select a media file..." readonly />
                <div class="absolute left-4 top-4 text-gray-500">
                    <VideoFileIcon class="h-5 w-5" />
                </div>
            </div>
            <button @click="selectFile"
                class="btn-primary px-8 shrink-0">
                Browse
            </button>
        </div>
        <!-- Outside the flex row: inside it, the row's stretch alignment made the
             Browse button as tall as the input *plus* this hint. -->
        <p class="mt-2 px-1 text-xs text-gray-500">
            Browse or drop a video/audio file here
        </p>
    </div>
</template>
