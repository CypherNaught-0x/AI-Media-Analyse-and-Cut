<script setup lang="ts">
// Header at the top of each workspace page: title, one-line description and
// the page's primary actions on the right.
defineProps<{
    title: string;
    description?: string;
    eyebrow?: string;
}>();
</script>

<template>
    <header class="flex flex-wrap items-end justify-between gap-x-6 gap-y-3 pb-5">
        <div class="min-w-0">
            <p v-if="eyebrow" class="eyebrow mb-1.5">{{ eyebrow }}</p>
            <h1 class="text-[22px] leading-tight font-semibold tracking-[-0.01em] text-fg">
                {{ title }}
            </h1>
            <p v-if="description" class="mt-1 text-[13px] text-fg-subtle">{{ description }}</p>
        </div>
        <div v-if="$slots.actions" class="flex flex-wrap items-center gap-2">
            <slot name="actions" />
        </div>
    </header>
</template>
