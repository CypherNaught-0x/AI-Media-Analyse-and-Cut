<script setup lang="ts">
// Activity bar pinned to the bottom of the workspace column: what the app is
// doing, how far along it is, and a short history of recent messages.
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { History } from '@lucide/vue';

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

const HISTORY_LIMIT = 20;

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

const etaText = computed(() =>
    props.progressEtaSeconds !== null &&
    props.progressEtaSeconds !== undefined &&
    props.progressPercentage !== null &&
    props.progressPercentage < 100
        ? formatEta(props.progressEtaSeconds)
        : '',
);

const showTrack = computed(() => props.progressPercentage !== null || props.isProcessing);
const indeterminate = computed(() => props.isProcessing && props.progressPercentage === null);

/** Status split so numbers (counts, percentages, times) render in mono. */
const statusParts = computed(() =>
    props.status
        .split(/(\d+(?:[.:,/]\d+)*%?)/)
        .filter((part) => part !== '')
        .map((text) => ({ text, numeric: /^\d/.test(text) })),
);

// ---------------------------------------------------------------- History
interface HistoryEntry {
    id: number;
    message: string;
    /** Message with numbers masked, so progress ticks collapse into one entry. */
    key: string;
    at: number;
}

function historyKey(text: string): string {
    return text.replace(/\d+(?:[.:,/]\d+)*%?/g, '#');
}

const history = ref<HistoryEntry[]>([]);
let nextId = 0;

watch(
    () => props.status,
    (message) => {
        const text = message.trim();
        if (!text || history.value[0]?.message === text) return;
        const key = historyKey(text);
        const [latest, ...rest] = history.value;
        if (latest?.key === key) {
            // Same message with new numbers (e.g. "Step 2/5" -> "Step 3/5"): update in place.
            history.value = [{ ...latest, message: text, at: Date.now() }, ...rest];
            return;
        }
        history.value = [
            { id: nextId++, message: text, key, at: Date.now() },
            ...history.value,
        ].slice(0, HISTORY_LIMIT);
    },
    { immediate: true },
);

const historyOpen = ref(false);
const now = ref(Date.now());
const rootRef = ref<HTMLElement | null>(null);
const toggleRef = ref<HTMLButtonElement | null>(null);
let clock: ReturnType<typeof setInterval> | null = null;

function relativeTime(at: number): string {
    const seconds = Math.max(0, Math.round((now.value - at) / 1000));
    if (seconds < 5) return 'just now';
    if (seconds < 60) return `${seconds}s ago`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m ago`;
    return `${Math.floor(minutes / 60)}h ago`;
}

function onDocumentPointerDown(event: MouseEvent) {
    if (rootRef.value && !rootRef.value.contains(event.target as Node)) historyOpen.value = false;
}

function onDocumentKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
        historyOpen.value = false;
        void nextTick(() => toggleRef.value?.focus());
    }
}

function stopListening() {
    if (clock) clearInterval(clock);
    clock = null;
    document.removeEventListener('mousedown', onDocumentPointerDown);
    document.removeEventListener('keydown', onDocumentKeydown);
}

watch(historyOpen, (open) => {
    stopListening();
    if (!open) return;
    now.value = Date.now();
    clock = setInterval(() => (now.value = Date.now()), 5000);
    document.addEventListener('mousedown', onDocumentPointerDown);
    document.addEventListener('keydown', onDocumentKeydown);
});

onBeforeUnmount(stopListening);
</script>

<template>
    <div
        ref="rootRef"
        class="sticky bottom-0 z-30 border-t border-line bg-rail shadow-[0_-8px_16px_-12px_rgba(0,0,0,0.6)]"
        data-testid="status-bar"
    >
        <!-- Screen readers hear new messages without focusing the bar. -->
        <span class="sr-only" role="status" aria-live="polite">{{ status }}</span>

        <div class="flex h-10 items-center gap-3 px-8">
            <span class="relative flex h-2 w-2 shrink-0" aria-hidden="true">
                <span
                    v-if="isProcessing"
                    class="absolute inset-0 animate-ping rounded-full bg-accent/60"
                />
                <span
                    class="relative h-2 w-2 rounded-full"
                    :class="isProcessing ? 'bg-accent' : 'bg-success'"
                />
            </span>

            <button
                ref="toggleRef"
                type="button"
                data-testid="status-text"
                class="group flex min-w-0 flex-1 items-center gap-2 rounded-sm text-left text-[12.5px] text-fg-muted transition-colors hover:text-fg"
                :aria-expanded="historyOpen ? 'true' : 'false'"
                aria-controls="status-history"
                title="Show recent activity"
                @click="historyOpen = !historyOpen"
            >
                <span class="truncate">
                    <template v-for="(part, index) in statusParts" :key="index">
                        <span v-if="part.numeric" class="font-mono text-[11.5px]">{{
                            part.text
                        }}</span>
                        <template v-else>{{ part.text }}</template>
                    </template>
                </span>
                <History
                    class="h-3.5 w-3.5 shrink-0 text-fg-faint opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
                    aria-hidden="true"
                />
            </button>

            <div v-if="showTrack" class="flex shrink-0 items-center gap-2">
                <div
                    class="relative h-1 w-36 overflow-hidden rounded-full bg-surface-3"
                    role="progressbar"
                    aria-label="Progress"
                    aria-valuemin="0"
                    aria-valuemax="100"
                    :aria-valuenow="progressPercentage ?? undefined"
                >
                    <div
                        v-if="indeterminate"
                        data-testid="status-progress-indeterminate"
                        class="shimmer absolute inset-0 bg-accent/25"
                    />
                    <div
                        v-else
                        data-testid="status-progress-fill"
                        class="h-full rounded-full bg-accent transition-[width] duration-300 ease-out"
                        :style="{ width: `${progressPercentage}%` }"
                    />
                </div>
                <span
                    v-if="progressPercentage !== null"
                    class="w-9 text-right font-mono text-[11px] text-fg-subtle"
                >
                    {{ Math.round(progressPercentage) }}%
                </span>
            </div>

            <span v-if="etaText" class="chip shrink-0 font-mono">{{ etaText }}</span>

            <button
                v-if="isProcessing"
                type="button"
                data-testid="status-cancel"
                class="btn-danger btn-sm shrink-0"
                :disabled="isCancelling"
                @click="emit('cancel')"
            >
                {{ isCancelling ? 'Cancelling...' : 'Cancel' }}
            </button>
        </div>

        <Transition name="pop">
            <div
                v-if="historyOpen"
                id="status-history"
                class="popover absolute bottom-full left-6 mb-2 w-[min(440px,calc(100%-2rem))] origin-bottom-left overflow-hidden"
                role="region"
                aria-label="Recent activity"
            >
                <p class="eyebrow border-b border-line px-3.5 py-2.5">Recent activity</p>
                <ol class="max-h-72 overflow-y-auto py-1">
                    <li
                        v-for="(entry, index) in history"
                        :key="entry.id"
                        class="flex items-baseline gap-3 px-3.5 py-1.5 text-[12.5px]"
                        :class="index === 0 ? 'text-fg' : 'text-fg-muted'"
                    >
                        <span class="min-w-0 flex-1 break-words">{{ entry.message }}</span>
                        <span class="shrink-0 font-mono text-[10.5px] text-fg-faint">
                            {{ relativeTime(entry.at) }}
                        </span>
                    </li>
                </ol>
            </div>
        </Transition>
    </div>
</template>
