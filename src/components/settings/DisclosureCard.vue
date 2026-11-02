<script setup lang="ts">
// A collapsible panel whose open state the parent owns (`v-model:open`), so
// it can open a card from outside without remounting it and without undoing
// what the user opened or closed by hand. Chips in `#actions` sit beside the
// toggle, not inside it.
import { useId } from 'vue';
import { ChevronDown } from '@lucide/vue';

defineProps<{
    title: string;
    description?: string;
    open: boolean;
}>();

const emit = defineEmits<{
    (e: 'update:open', value: boolean): void;
}>();

const bodyId = `disclosure-${useId()}`;
</script>

<template>
    <section class="panel">
        <div
            class="flex items-center gap-3 rounded-t-lg pr-4 transition-colors duration-150 hover:bg-surface-2/60"
            :class="open ? '' : 'rounded-b-lg'"
        >
            <button
                type="button"
                class="flex min-w-0 flex-1 cursor-pointer items-center gap-3 rounded-lg py-3.5 pl-4 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
                :aria-expanded="open ? 'true' : 'false'"
                :aria-controls="bodyId"
                @click="emit('update:open', !open)"
            >
                <ChevronDown
                    class="h-4 w-4 shrink-0 text-fg-subtle transition-transform duration-200"
                    :class="open ? '' : '-rotate-90'"
                    aria-hidden="true"
                />
                <span class="min-w-0 flex-1">
                    <span class="section-title block">{{ title }}</span>
                    <span v-if="description" class="hint mt-0.5 block">{{ description }}</span>
                </span>
            </button>
            <div v-if="$slots.actions" class="flex shrink-0 items-center gap-2">
                <slot name="actions" />
            </div>
        </div>
        <div v-show="open" :id="bodyId" class="px-4 pt-1 pb-4">
            <slot />
        </div>
    </section>
</template>
