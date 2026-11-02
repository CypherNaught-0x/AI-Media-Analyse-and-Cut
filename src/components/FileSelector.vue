<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open } from '@tauri-apps/plugin-dialog';
import { FileAudio, FileVideo, FolderSearch, RefreshCw, TriangleAlert, Upload } from '@lucide/vue';

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
const AUDIO_EXTENSIONS = new Set(['mp3', 'wav', 'aac', 'flac', 'ogg', 'm4a', 'wma']);

const props = withDefaults(
    defineProps<{
        modelValue: string;
        /** The chosen path no longer exists (e.g. a restored session). */
        missing?: boolean;
    }>(),
    { missing: false },
);

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
            filters: [
                {
                    name: 'Media',
                    extensions: MEDIA_EXTENSIONS,
                },
            ],
        });

        if (selected && typeof selected === 'string') {
            selectPath(selected);
        }
    } catch (e) {
        console.error('Failed to open dialog:', e);
    }
}

// The chosen file, split for display: name up front, folder muted.
const fileName = computed(() => props.modelValue.split(/[\\/]/).pop() ?? props.modelValue);
const folderPath = computed(() => {
    const name = fileName.value;
    return props.modelValue
        .slice(0, Math.max(0, props.modelValue.length - name.length))
        .replace(/[\\/]$/, '');
});
const extension = computed(() => fileName.value.split('.').pop()?.toLowerCase() ?? '');
const isAudio = computed(() => AUDIO_EXTENSIONS.has(extension.value));
const typeLabel = computed(() =>
    extension.value && extension.value !== fileName.value.toLowerCase()
        ? `${extension.value.toUpperCase()} · ${isAudio.value ? 'Audio' : 'Video'}`
        : 'Media',
);
const formatsHint = 'MP4, MOV, MKV, WebM, AVI · MP3, WAV, M4A, FLAC and more';

defineExpose({ selectFile });

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
                isDragActive.value =
                    tauriDragHasSupportedFile && (await isDropInsideZone(payload.position));
                return;
            }

            if (payload.type === 'over') {
                isDragActive.value =
                    tauriDragHasSupportedFile && (await isDropInsideZone(payload.position));
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
    <div
        ref="dropZoneRef"
        data-testid="file-drop-zone"
        :data-drag-active="isDragActive ? 'true' : 'false'"
        class="relative rounded-lg border transition-[border-color,background-color,box-shadow] duration-[var(--duration-base)]"
        :class="[
            !modelValue ? 'border-dashed' : '',
            isDragActive
                ? 'border-accent bg-accent/[0.07] shadow-glow'
                : missing && modelValue
                  ? 'border-warning/35 bg-warning/[0.04]'
                  : modelValue
                    ? 'border-line bg-surface'
                    : 'border-line-strong bg-surface/60 hover:border-accent/40',
        ]"
    >
        <!-- Empty: a real drop target. -->
        <div
            v-if="!modelValue"
            class="flex flex-col items-center justify-center gap-3 px-6 py-12 text-center"
        >
            <div
                class="flex h-12 w-12 items-center justify-center rounded-xl border transition-colors duration-[var(--duration-base)]"
                :class="
                    isDragActive
                        ? 'border-accent/50 bg-accent/15 text-accent-soft'
                        : 'border-line bg-surface-2 text-fg-subtle'
                "
            >
                <Upload class="h-5 w-5" aria-hidden="true" />
            </div>
            <div>
                <p class="text-[15px] font-semibold text-fg">
                    {{ isDragActive ? 'Release to use this file' : 'Drop a video or audio file' }}
                </p>
                <p class="hint mt-1">or choose one from your computer</p>
            </div>
            <button type="button" class="btn-primary" @click="selectFile">Browse</button>
            <p class="text-[11px] text-fg-faint">{{ formatsHint }}</p>
        </div>

        <!-- Chosen: file card. -->
        <div v-else class="flex items-center gap-3.5 p-3.5">
            <div
                class="flex h-11 w-11 shrink-0 items-center justify-center rounded-md border"
                :class="
                    missing
                        ? 'border-warning/30 bg-warning/10 text-warning'
                        : 'border-accent/25 bg-accent/10 text-accent-soft'
                "
            >
                <TriangleAlert v-if="missing" class="h-5 w-5" aria-hidden="true" />
                <FileAudio v-else-if="isAudio" class="h-5 w-5" aria-hidden="true" />
                <FileVideo v-else class="h-5 w-5" aria-hidden="true" />
            </div>
            <div class="min-w-0 flex-1">
                <div class="flex min-w-0 items-center gap-2">
                    <p class="truncate text-[14px] font-semibold text-fg" :title="modelValue">
                        {{ fileName }}
                    </p>
                    <span v-if="missing" class="chip-warning shrink-0">Missing</span>
                    <span v-else class="chip shrink-0">{{ typeLabel }}</span>
                </div>
                <p
                    v-if="folderPath"
                    class="mt-0.5 truncate font-mono text-[11.5px] text-fg-subtle"
                    :title="folderPath"
                >
                    {{ folderPath }}
                </p>
            </div>
            <button
                v-if="!missing"
                type="button"
                class="btn-secondary btn-sm shrink-0"
                @click="selectFile"
            >
                <RefreshCw aria-hidden="true" />
                Replace
            </button>
        </div>

        <!-- Missing media: say what still works and how to fix it. -->
        <div
            v-if="modelValue && missing"
            class="flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-warning/20 px-3.5 py-3"
            role="alert"
        >
            <p class="min-w-0 flex-1 text-[12.5px] leading-relaxed text-warning/90">
                <span class="font-semibold">Media file missing.</span>
                Transcript state is restored, but media actions are disabled until you choose a
                valid file.
            </p>
            <button type="button" class="btn-secondary btn-sm shrink-0" @click="selectFile">
                <FolderSearch aria-hidden="true" />
                Locate file
            </button>
        </div>

        <!-- Drop over a chosen file replaces it. -->
        <Transition name="fade">
            <div
                v-if="modelValue && isDragActive"
                class="pointer-events-none absolute inset-0 flex items-center justify-center rounded-lg bg-canvas/80 text-[13px] font-medium text-accent-soft"
            >
                Drop to replace
            </div>
        </Transition>
    </div>
</template>
