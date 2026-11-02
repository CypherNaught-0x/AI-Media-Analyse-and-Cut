<script setup lang="ts">
import type { Component } from 'vue';

// Placeholder for an area with nothing in it yet: what goes here and how.
defineProps<{
    icon?: Component;
    title: string;
    description?: string;
    compact?: boolean;
}>();
</script>

<template>
    <div
        class="flex flex-col items-center justify-center rounded-lg border border-dashed border-line-strong text-center"
        :class="compact ? 'gap-2 px-4 py-6' : 'gap-3 px-6 py-12'"
    >
        <div
            v-if="icon"
            class="flex items-center justify-center rounded-xl border border-line bg-surface-2 text-fg-subtle"
            :class="compact ? 'h-9 w-9' : 'h-11 w-11'"
        >
            <component :is="icon" class="h-5 w-5" aria-hidden="true" />
        </div>
        <div>
            <p class="section-title">{{ title }}</p>
            <p v-if="description" class="hint mx-auto mt-1 max-w-sm">{{ description }}</p>
        </div>
        <div v-if="$slots.default" class="mt-1 flex flex-wrap justify-center gap-2">
            <slot />
        </div>
    </div>
</template>
