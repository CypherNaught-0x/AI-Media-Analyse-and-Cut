<script setup lang="ts">
// A labelled on/off row: switch on the left, title and a description (or,
// while disabled, why it is off) on the right. The label is tied to the
// switch, so clicking the text toggles it too.
import { useId } from 'vue';
import UiSwitch from '../ui/UiSwitch.vue';

const props = withDefaults(
    defineProps<{
        modelValue: boolean;
        title: string;
        description?: string;
        /** Shown instead of the description while disabled: why it is off. */
        disabledReason?: string;
        disabled?: boolean;
        testId?: string;
    }>(),
    {
        description: undefined,
        disabledReason: undefined,
        disabled: false,
        testId: undefined,
    },
);

const emit = defineEmits<{
    (e: 'update:modelValue', value: boolean): void;
}>();

const id = `switch-row-${useId()}`;

function update(value: boolean) {
    if (props.disabled) return;
    emit('update:modelValue', value);
}
</script>

<template>
    <div
        class="flex items-start gap-3 rounded-md border border-line bg-surface-2 px-3 py-2.5 transition-colors duration-150"
        :class="disabled ? 'opacity-80' : 'hover:border-line-strong'"
    >
        <UiSwitch
            :id="id"
            class="mt-0.5"
            :model-value="modelValue"
            :disabled="disabled"
            :data-testid="testId"
            :aria-describedby="
                description || (disabled && disabledReason) ? `${id}-hint` : undefined
            "
            @update:model-value="update"
        />
        <div class="min-w-0 flex-1">
            <label
                :for="id"
                class="block text-[13px] font-medium"
                :class="disabled ? 'cursor-not-allowed text-fg-subtle' : 'cursor-pointer text-fg'"
            >
                {{ title }}
            </label>
            <p
                v-if="disabled && disabledReason"
                :id="`${id}-hint`"
                class="hint mt-0.5"
                :data-testid="testId ? `${testId}-reason` : undefined"
            >
                {{ disabledReason }}
            </p>
            <p v-else-if="description" :id="`${id}-hint`" class="hint mt-0.5">
                {{ description }}
            </p>
        </div>
        <slot />
    </div>
</template>
