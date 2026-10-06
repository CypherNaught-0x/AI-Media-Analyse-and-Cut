<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import type { TimeSpan } from '../types';
import type { ClipWord } from '../utils/shortClips';

interface Props {
    words: ClipWord[];
    /** What plays (source seconds), once the preview is prepared; words outside are cut. */
    kept: { start: number; end: number }[] | null;
    /** Player time while this clip previews, else null. */
    playhead: number | null;
    /** Words the user cut out of the clip. */
    cutWords?: TimeSpan[];
}

const props = defineProps<Props>();
const emit = defineEmits<{
    edit: [word: ClipWord, text: string];
    /** Cut `words` out of the clip (or, with `cut` false, restore them). */
    cut: [words: ClipWord[], cut: boolean];
}>();

type Mode = 'edit' | 'cut';
const mode = ref<Mode>('edit');
const container = ref<HTMLElement | null>(null);
const editing = ref<number | null>(null);
const draft = ref('');
/** The word last clicked in cut mode, for Shift-click ranges. */
const anchor = ref<number | null>(null);

function userCut(word: ClipWord): boolean {
    return (props.cutWords ?? []).some((cut) => cut.start === word.start && cut.end === word.end);
}

/** Cut by tightening (outside what the prepared preview plays), not by the user. */
function tightened(word: ClipWord): boolean {
    if (!props.kept || userCut(word)) return false;
    const middle = (word.start + word.end) / 2;
    return !props.kept.some((range) => range.start <= middle && middle < range.end);
}

const active = computed(() => {
    const time = props.playhead;
    if (time === null) return -1;
    return props.words.findIndex((word) => word.start <= time && time < word.end);
});

const cutCount = computed(() => props.words.filter(userCut).length);

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

/** Cut mode: toggle a word, or with Shift everything back to the last one clicked. */
function toggleCut(index: number, extend: boolean) {
    const cut = !userCut(props.words[index]);
    const from = extend && anchor.value !== null ? Math.min(anchor.value, index) : index;
    const to = extend && anchor.value !== null ? Math.max(anchor.value, index) : index;
    anchor.value = index;
    emit('cut', props.words.slice(from, to + 1), cut);
}

function onWord(index: number, event: MouseEvent | KeyboardEvent) {
    if (mode.value === 'cut') toggleCut(index, event.shiftKey);
    else void startEdit(index);
}

function restoreAll() {
    emit('cut', props.words.filter(userCut), false);
}
</script>

<template>
    <div class="rounded-lg bg-black/30" data-testid="clip-text-panel">
        <div
            class="flex flex-wrap items-center gap-2 border-b border-white/5 px-3 py-1.5 text-[11px]"
        >
            <div
                class="flex overflow-hidden rounded-md border border-white/10"
                role="radiogroup"
                aria-label="Text mode"
            >
                <button
                    v-for="option in [
                        { value: 'edit', label: 'Edit words' },
                        { value: 'cut', label: 'Cut out' },
                    ] as const"
                    :key="option.value"
                    type="button"
                    role="radio"
                    :aria-checked="mode === option.value"
                    class="px-2 py-0.5 transition-colors"
                    :class="
                        mode === option.value
                            ? 'bg-pink-600/70 font-semibold text-white'
                            : 'text-gray-300 hover:bg-white/10'
                    "
                    :data-testid="`clip-text-mode-${option.value}`"
                    @click="mode = option.value"
                >
                    {{ option.label }}
                </button>
            </div>
            <button
                v-if="cutCount > 0"
                type="button"
                class="ml-auto rounded px-1.5 py-0.5 text-red-300 hover:bg-white/10"
                data-testid="clip-text-restore"
                @click="restoreAll"
            >
                Restore {{ cutCount }} cut word{{ cutCount === 1 ? '' : 's' }}
            </button>
            <span class="basis-full text-gray-500">
                {{
                    mode === 'edit'
                        ? 'Click a word to correct it in the transcript.'
                        : 'Click a word to cut it from this clip, Shift-click for a range. The transcript stays as it is.'
                }}
            </span>
        </div>
        <div
            ref="container"
            class="relative max-h-48 overflow-y-auto p-3 text-sm leading-relaxed text-gray-200"
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
                    class="rounded px-px transition-colors"
                    :class="[
                        mode === 'cut'
                            ? 'cursor-pointer hover:bg-red-500/20'
                            : 'cursor-text hover:bg-white/10',
                        index === active ? 'bg-[#ffd60a] text-black' : '',
                        userCut(word)
                            ? 'bg-red-500/10 text-red-300/70 line-through decoration-red-400'
                            : '',
                        tightened(word) ? 'text-gray-500 line-through decoration-gray-500/70' : '',
                    ]"
                    :title="
                        userCut(word)
                            ? 'Cut from this clip'
                            : tightened(word)
                              ? 'Cut by tightening'
                              : mode === 'cut'
                                ? 'Click to cut'
                                : 'Click to edit'
                    "
                    :data-word="index"
                    :data-cut="userCut(word) || undefined"
                    :data-testid="`clip-word-${index}`"
                    @click="onWord(index, $event)"
                    @keydown.enter.prevent="onWord(index, $event)"
                    >{{ word.text }}</span
                >{{ ' ' }}
            </template>
        </div>
    </div>
</template>
