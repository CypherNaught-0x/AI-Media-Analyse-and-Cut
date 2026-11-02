<script setup lang="ts">
// Modal dialog: teleported to <body>, Escape and backdrop close it, focus
// moves in on open and back to the opener on close. Tests can stub Teleport
// (`global: { stubs: { teleport: true } }`) or pass `:teleport="false"`.
import { nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { X } from '@lucide/vue';

const props = withDefaults(
    defineProps<{
        open: boolean;
        title: string;
        description?: string;
        size?: 'sm' | 'md' | 'lg' | 'xl';
        tone?: 'default' | 'danger';
        dismissible?: boolean;
        teleport?: boolean;
    }>(),
    {
        description: undefined,
        size: 'md',
        tone: 'default',
        dismissible: true,
        teleport: true,
    },
);

const emit = defineEmits<{
    (e: 'close'): void;
}>();

const panel = ref<HTMLElement | null>(null);
let opener: HTMLElement | null = null;
const titleId = `dlg-${Math.random().toString(36).slice(2, 9)}`;

function close() {
    if (props.dismissible) emit('close');
}

function focusables(): HTMLElement[] {
    if (!panel.value) return [];
    return Array.from(
        panel.value.querySelectorAll<HTMLElement>(
            'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
    );
}

function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
        event.stopPropagation();
        close();
        return;
    }
    if (event.key !== 'Tab') return;
    const items = focusables();
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
    }
}

watch(
    () => props.open,
    async (open) => {
        if (open) {
            opener = document.activeElement as HTMLElement | null;
            await nextTick();
            const autofocus = panel.value?.querySelector<HTMLElement>(
                '[autofocus], [data-autofocus]',
            );
            (autofocus ?? focusables()[0] ?? panel.value)?.focus();
        } else {
            opener?.focus?.();
            opener = null;
        }
    },
    { immediate: true },
);

onBeforeUnmount(() => opener?.focus?.());

const widths = { sm: 'max-w-sm', md: 'max-w-lg', lg: 'max-w-2xl', xl: 'max-w-4xl' };
</script>

<template>
    <Teleport to="body" :disabled="!teleport">
        <Transition name="fade">
            <div
                v-if="open"
                class="fixed inset-0 z-[200] flex items-center justify-center bg-black/60 p-6 backdrop-blur-[2px]"
                @mousedown.self="close"
            >
                <Transition name="pop" appear>
                    <div
                        ref="panel"
                        role="dialog"
                        aria-modal="true"
                        :aria-labelledby="titleId"
                        tabindex="-1"
                        class="popover flex max-h-[85vh] w-full flex-col overflow-hidden outline-none"
                        :class="widths[size]"
                        @keydown="onKeydown"
                    >
                        <header class="flex items-start gap-3 border-b border-line px-5 py-4">
                            <slot name="icon" />
                            <div class="min-w-0 flex-1">
                                <h2
                                    :id="titleId"
                                    class="text-[15px] font-semibold"
                                    :class="tone === 'danger' ? 'text-red-300' : 'text-fg'"
                                >
                                    {{ title }}
                                </h2>
                                <p v-if="description" class="mt-1 hint">{{ description }}</p>
                            </div>
                            <button
                                v-if="dismissible"
                                type="button"
                                class="btn-ghost btn-sm btn-icon -mr-1.5 -mt-0.5"
                                aria-label="Close"
                                @click="close"
                            >
                                <X />
                            </button>
                        </header>
                        <div class="min-h-0 flex-1 overflow-y-auto px-5 py-4">
                            <slot />
                        </div>
                        <footer
                            v-if="$slots.footer"
                            class="flex items-center justify-end gap-2 border-t border-line bg-surface px-5 py-3"
                        >
                            <slot name="footer" />
                        </footer>
                    </div>
                </Transition>
            </div>
        </Transition>
    </Teleport>
</template>
