<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import type { ClipWord } from '../utils/shortClips';

interface Props {
    words: ClipWord[];
    /** What plays (source seconds), once the preview is prepared; words outside are cut. */
    kept: { start: number; end: number }[] | null;
    /** Player time while this clip previews, else null. */
    playhead: number | null;
}

const props = defineProps<Props>();
const emit = defineEmits<{ edit: [word: ClipWord, text: string] }>();

const container = ref<HTMLElement | null>(null);
const editing = ref<number | null>(null);
const draft = ref('');

/** Cut by tightening: outside what the prepared preview plays. */
function tightened(word: ClipWord): boolean {
    if (!props.kept) return false;
    const middle = (word.start + word.end) / 2;
    return !props.kept.some((range) => range.start <= middle && middle < range.end);
}

const active = computed(() => {
    const time = props.playhead;
    if (time === null) return -1;
    return props.words.findIndex((word) => word.start <= time && time < word.end);
});

/** A speaker label goes before the first word and wherever the speaker changes. */
function speakerBefore(index: number): string | null {
    const speaker = props.words[index].speaker;
    if (!speaker) return null;
    return index === 0 || props.words[index - 1].speaker !== speaker ? speaker : null;
}

// Keep the spoken word in view, inside the text box only, unless the user
// scrolled it lately: following would snap their scrolling back.
const FOLLOW_PAUSE_MS = 4000;
let userScrolledAt = 0;

function onUserScroll() {
    userScrolledAt = Date.now();
}

watch(active, async (index) => {
    if (index < 0 || Date.now() - userScrolledAt < FOLLOW_PAUSE_MS) return;
    await nextTick();
    const box = container.value;
    const element = box?.querySelector<HTMLElement>(`[data-word="${index}"]`);
    if (!box || !element) return;
    // The box is the words' offset parent, so this is relative to its content.
    const top = element.offsetTop;
    if (top < box.scrollTop || top + element.offsetHeight > box.scrollTop + box.clientHeight) {
        box.scrollTop = top - box.clientHeight / 3;
    }
});

async function startEdit(index: number) {
    editing.value = index;
    draft.value = props.words[index].text;
    await nextTick();
    const input = container.value?.querySelector<HTMLInputElement>('input');
    input?.focus();
    input?.select();
}

function commit() {
    const index = editing.value;
    if (index === null) return;
    editing.value = null;
    const word = props.words[index];
    const text = draft.value.trim();
    if (word && text && text !== word.text) emit('edit', word, text);
}

function cancel() {
    editing.value = null;
}
</script>

<template>
    <div
        ref="container"
        class="relative max-h-48 overflow-y-auto rounded-lg bg-black/30 p-3 text-sm leading-relaxed text-gray-200"
        data-testid="clip-text"
        @wheel.passive="onUserScroll"
        @touchmove.passive="onUserScroll"
        @pointerdown="onUserScroll"
    >
        <p v-if="words.length === 0" class="text-xs text-gray-500">No transcript words here.</p>
        <template v-for="(word, index) in words" :key="`${word.segment}:${word.index}`">
            <span
                v-if="speakerBefore(index)"
                class="mr-1 mt-1 block text-[10px] font-semibold uppercase tracking-wider text-pink-300/80 first:mt-0"
                >{{ speakerBefore(index) }}</span
            >
            <input
                v-if="editing === index"
                v-model="draft"
                class="mx-0.5 rounded bg-white/15 px-1 text-white outline-none ring-1 ring-pink-400"
                :style="{ width: `${Math.max(draft.length, 2) + 1}ch` }"
                :aria-label="`Edit “${word.text}”`"
                autocapitalize="off"
                autocorrect="off"
                autocomplete="off"
                spellcheck="false"
                data-testid="clip-text-input"
                @keydown.enter.prevent="commit"
                @keydown.esc.prevent="cancel"
                @blur="commit"
            />
            <span
                v-else
                role="button"
                tabindex="0"
                class="cursor-text rounded px-px transition-colors hover:bg-white/10"
                :class="[
                    index === active ? 'bg-[#ffd60a] text-black' : '',
                    tightened(word) ? 'text-gray-500 line-through decoration-gray-500/70' : '',
                ]"
                :title="tightened(word) ? 'Cut by tightening; click to edit' : 'Click to edit'"
                :data-word="index"
                :data-testid="`clip-word-${index}`"
                @click="startEdit(index)"
                @keydown.enter.prevent="startEdit(index)"
                >{{ word.text }}</span
            >{{ ' ' }}
        </template>
    </div>
</template>
