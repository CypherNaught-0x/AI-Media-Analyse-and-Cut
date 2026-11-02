<script setup lang="ts" generic="T extends string">
// A radio group of option cards (title, optional badge, description). Arrow
// keys, Home and End move the selection, and only the selected card is in
// the tab order.
import { ref } from 'vue';

export interface OptionCard<V> {
    value: V;
    title: string;
    description?: string;
    badge?: string;
    testId?: string;
}

const props = withDefaults(
    defineProps<{
        modelValue: T;
        options: OptionCard<T>[];
        /** Accessible name when no visible label is linked via aria-labelledby. */
        label?: string;
        dense?: boolean;
    }>(),
    { label: undefined, dense: false },
);

const emit = defineEmits<{
    (e: 'update:modelValue', value: T): void;
}>();

const root = ref<HTMLElement | null>(null);

function onKeydown(event: KeyboardEvent) {
    const count = props.options.length;
    if (count === 0) return;
    const index = Math.max(
        props.options.findIndex((o) => o.value === props.modelValue),
        0,
    );
    let nextIndex: number;
    if (['ArrowRight', 'ArrowDown'].includes(event.key)) nextIndex = (index + 1) % count;
    else if (['ArrowLeft', 'ArrowUp'].includes(event.key)) nextIndex = (index - 1 + count) % count;
    else if (event.key === 'Home') nextIndex = 0;
    else if (event.key === 'End') nextIndex = count - 1;
    else return;
    event.preventDefault();
    emit('update:modelValue', props.options[nextIndex].value);
    root.value?.querySelectorAll<HTMLButtonElement>('[role="radio"]')[nextIndex]?.focus();
}
</script>

<template>
    <div ref="root" role="radiogroup" :aria-label="label" class="grid gap-2" @keydown="onKeydown">
        <button
            v-for="(option, index) in options"
            :key="option.value"
            type="button"
            role="radio"
            class="option-card flex h-full flex-col"
            :class="[
                dense ? '!px-3 !py-2' : '',
                // Keep the selection visible while the pointer is still on it.
                modelValue === option.value ? 'hover:border-accent/70 hover:bg-accent/15' : '',
            ]"
            :aria-checked="modelValue === option.value ? 'true' : 'false'"
            :tabindex="
                modelValue === option.value ||
                (index === 0 && !options.some((o) => o.value === modelValue))
                    ? 0
                    : -1
            "
            :data-testid="option.testId"
            @click="emit('update:modelValue', option.value)"
        >
            <span class="flex items-center gap-2">
                <span
                    class="flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-full border transition-colors duration-150"
                    :class="
                        modelValue === option.value
                            ? 'border-accent bg-accent'
                            : 'border-line-strong bg-well'
                    "
                    aria-hidden="true"
                >
                    <span
                        v-if="modelValue === option.value"
                        class="h-1.5 w-1.5 rounded-full bg-fg"
                    />
                </span>
                <span class="text-[13px] font-semibold text-fg">{{ option.title }}</span>
                <span v-if="option.badge" class="chip ml-auto">{{ option.badge }}</span>
            </span>
            <span
                v-if="option.description"
                class="mt-1 pl-[22px] text-[12px] leading-snug text-fg-subtle"
                >{{ option.description }}</span
            >
        </button>
    </div>
</template>
