<script setup lang="ts">
// Accessible on/off switch. Use `label` when no visible text names it.
const props = withDefaults(
    defineProps<{
        modelValue: boolean;
        disabled?: boolean;
        label?: string;
        size?: 'sm' | 'md';
    }>(),
    { disabled: false, label: undefined, size: 'md' },
);

const emit = defineEmits<{
    (e: 'update:modelValue', value: boolean): void;
}>();

function toggle() {
    if (props.disabled) return;
    emit('update:modelValue', !props.modelValue);
}
</script>

<template>
    <button
        type="button"
        role="switch"
        :aria-checked="modelValue ? 'true' : 'false'"
        :aria-label="label"
        :disabled="disabled"
        class="group relative inline-flex shrink-0 cursor-pointer items-center rounded-full border transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-45"
        :class="[
            size === 'sm' ? 'h-4 w-7' : 'h-5 w-9',
            modelValue
                ? 'border-accent/60 bg-accent'
                : 'border-line-strong bg-surface-3 hover:bg-surface-hover',
        ]"
        @click.stop="toggle"
    >
        <span
            aria-hidden="true"
            class="pointer-events-none inline-block rounded-full bg-white shadow-sm transition-transform duration-200 ease-[var(--ease-out-expo)]"
            :class="[
                size === 'sm' ? 'h-3 w-3' : 'h-3.5 w-3.5',
                modelValue
                    ? size === 'sm'
                        ? 'translate-x-[13px]'
                        : 'translate-x-[17px]'
                    : 'translate-x-[2px] opacity-80',
            ]"
        />
    </button>
</template>
