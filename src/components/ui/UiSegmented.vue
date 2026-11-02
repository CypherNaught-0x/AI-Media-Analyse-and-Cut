<script setup lang="ts" generic="T extends string | number">
// Segmented control (radio group) for 2–5 mutually exclusive options.
import type { Component } from 'vue';

export interface SegmentedOption<V> {
    value: V;
    label: string;
    icon?: Component;
    disabled?: boolean;
    title?: string;
    testId?: string;
}

const props = withDefaults(
    defineProps<{
        modelValue: T;
        options: SegmentedOption<T>[];
        label?: string;
        size?: 'sm' | 'md';
        disabled?: boolean;
    }>(),
    { label: undefined, size: 'md', disabled: false },
);

const emit = defineEmits<{
    (e: 'update:modelValue', value: T): void;
}>();

function select(option: SegmentedOption<T>) {
    if (props.disabled || option.disabled) return;
    emit('update:modelValue', option.value);
}

function onKeydown(event: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
    event.preventDefault();
    const enabled = props.options.filter((o) => !o.disabled);
    const index = enabled.findIndex((o) => o.value === props.modelValue);
    const next =
        enabled[(index + (event.key === 'ArrowRight' ? 1 : -1) + enabled.length) % enabled.length];
    if (next) select(next);
}
</script>

<template>
    <div
        role="radiogroup"
        :aria-label="label"
        class="inline-flex items-center gap-0.5 rounded-lg border border-line bg-well p-0.5"
        :class="disabled ? 'opacity-45' : ''"
        @keydown="onKeydown"
    >
        <button
            v-for="option in options"
            :key="String(option.value)"
            type="button"
            role="radio"
            :aria-checked="modelValue === option.value ? 'true' : 'false'"
            :tabindex="modelValue === option.value ? 0 : -1"
            :disabled="disabled || option.disabled"
            :title="option.title"
            :data-testid="option.testId"
            class="inline-flex items-center gap-1.5 rounded-md font-medium whitespace-nowrap transition-colors duration-150 disabled:cursor-not-allowed"
            :class="[
                size === 'sm' ? 'h-6 px-2 text-[12px]' : 'h-7 px-2.5 text-[13px]',
                modelValue === option.value
                    ? 'bg-surface-3 text-fg shadow-[0_1px_2px_rgb(0_0_0/0.4),inset_0_0_0_1px_rgb(255_255_255/0.08)]'
                    : 'text-fg-subtle hover:text-fg',
            ]"
            @click="select(option)"
        >
            <component
                :is="option.icon"
                v-if="option.icon"
                class="h-3.5 w-3.5"
                aria-hidden="true"
            />
            {{ option.label }}
        </button>
    </div>
</template>
