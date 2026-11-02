<script setup lang="ts">
// A titled panel section. `collapsible` turns the header into a disclosure.
import { ref } from 'vue';
import { ChevronDown } from '@lucide/vue';

const props = withDefaults(
    defineProps<{
        title?: string;
        description?: string;
        collapsible?: boolean;
        defaultOpen?: boolean;
        padded?: boolean;
    }>(),
    {
        title: undefined,
        description: undefined,
        collapsible: false,
        defaultOpen: true,
        padded: true,
    },
);

const open = ref(props.defaultOpen);
</script>

<template>
    <section class="panel">
        <component
            :is="collapsible ? 'button' : 'div'"
            v-if="title || $slots.actions"
            :type="collapsible ? 'button' : undefined"
            :aria-expanded="collapsible ? (open ? 'true' : 'false') : undefined"
            class="flex w-full items-center gap-3 px-4 pt-3.5 text-left"
            :class="[
                collapsible ? 'cursor-pointer rounded-t-lg pb-3.5 hover:bg-white/[0.02]' : 'pb-1',
                collapsible && !open ? 'rounded-b-lg' : '',
            ]"
            @click="collapsible && (open = !open)"
        >
            <div class="min-w-0 flex-1">
                <h3 v-if="title" class="section-title">{{ title }}</h3>
                <p v-if="description" class="hint mt-0.5">{{ description }}</p>
            </div>
            <div v-if="$slots.actions" class="flex items-center gap-2" @click.stop>
                <slot name="actions" />
            </div>
            <ChevronDown
                v-if="collapsible"
                class="h-4 w-4 shrink-0 text-fg-subtle transition-transform duration-200"
                :class="open ? 'rotate-180' : ''"
                aria-hidden="true"
            />
        </component>
        <div v-show="!collapsible || open" :class="padded ? 'px-4 pt-2.5 pb-4' : ''">
            <slot />
        </div>
    </section>
</template>
