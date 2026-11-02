<script setup lang="ts">
// A single toast in the bottom-right corner. Plain success messages dismiss
// themselves after a few seconds (hovering pauses that). Info toasts stay:
// callers use them for in-flight work ("Installing update..."), sometimes with
// progress=null. Errors, progress and toasts with an action stay too.
import { computed, inject, onBeforeUnmount, ref, watch } from 'vue';
import { routeLocationKey } from 'vue-router';
import { CircleAlert, CircleCheck, Info, X } from '@lucide/vue';

type ToastTone = 'info' | 'success' | 'error';

const AUTO_DISMISS_MS = 5000;

const props = withDefaults(
    defineProps<{
        show: boolean;
        message: string;
        tone?: ToastTone;
        progress?: number | null;
        actionLabel?: string;
    }>(),
    {
        tone: 'info',
        progress: null,
        actionLabel: '',
    },
);

const emit = defineEmits<{
    dismiss: [];
    action: [];
}>();

const toneStyles = computed(() => {
    switch (props.tone) {
        case 'success':
            return { icon: CircleCheck, iconClass: 'text-success', bar: 'bg-success' };
        case 'error':
            return { icon: CircleAlert, iconClass: 'text-danger', bar: 'bg-danger' };
        default:
            return { icon: Info, iconClass: 'text-info', bar: 'bg-accent' };
    }
});

const autoDismisses = computed(
    () => props.show && props.tone === 'success' && props.progress === null && !props.actionLabel,
);

// Home has the activity bar along the bottom; elsewhere sit in the corner.
// inject with a default so Toast still works when mounted without a router.
const route = inject(routeLocationKey, null);
const offsetClass = computed(() => (route?.path === '/' ? 'bottom-14' : 'bottom-5'));

let timer: ReturnType<typeof setTimeout> | null = null;
const hovered = ref(false);

function clearTimer() {
    if (timer) clearTimeout(timer);
    timer = null;
}

function schedule() {
    clearTimer();
    if (!autoDismisses.value || hovered.value) return;
    timer = setTimeout(() => {
        timer = null;
        emit('dismiss');
    }, AUTO_DISMISS_MS);
}

// Restart the countdown whenever the toast's content changes.
watch(() => [autoDismisses.value, props.message, props.tone] as const, schedule, {
    immediate: true,
});

function onEnter() {
    hovered.value = true;
    clearTimer();
}

function onLeave() {
    hovered.value = false;
    schedule();
}

onBeforeUnmount(clearTimer);
</script>

<template>
    <Transition name="slide-up">
        <div
            v-if="show"
            class="fixed right-5 z-[210] w-[340px] max-w-[calc(100vw-2.5rem)]"
            :class="offsetClass"
            role="status"
            aria-live="polite"
            data-testid="toast"
            @mouseenter="onEnter"
            @mouseleave="onLeave"
        >
            <div class="popover flex items-start gap-3 p-3.5 pr-2.5">
                <component
                    :is="toneStyles.icon"
                    class="mt-px h-[18px] w-[18px] shrink-0"
                    :class="toneStyles.iconClass"
                    aria-hidden="true"
                />
                <div class="min-w-0 flex-1">
                    <p class="text-[13px] leading-snug text-fg">{{ message }}</p>
                    <div v-if="progress !== null" class="mt-2.5 flex items-center gap-2.5">
                        <div class="h-1 flex-1 overflow-hidden rounded-full bg-surface-3">
                            <div
                                data-testid="toast-progress"
                                class="h-full rounded-full transition-[width] duration-300 ease-out"
                                :class="toneStyles.bar"
                                :style="{ width: `${progress}%` }"
                            />
                        </div>
                        <span class="w-8 text-right font-mono text-[11px] text-fg-subtle">
                            {{ Math.round(progress) }}%
                        </span>
                    </div>
                    <button
                        v-if="actionLabel"
                        type="button"
                        class="btn-primary btn-sm mt-3"
                        @click="emit('action')"
                    >
                        {{ actionLabel }}
                    </button>
                </div>
                <button
                    type="button"
                    class="btn-ghost btn-sm btn-icon -mt-0.5 shrink-0"
                    aria-label="Dismiss"
                    @click="emit('dismiss')"
                >
                    <X aria-hidden="true" />
                </button>
            </div>
        </div>
    </Transition>
</template>
