<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import type { CutWord } from '../types';
import type { ClipWord } from '../utils/shortClips';
import { Loader2, Undo2, WandSparkles } from '@lucide/vue';
import UiSegmented, { type SegmentedOption } from './ui/UiSegmented.vue';
import { speakerColor } from '../utils/speakerColors';

interface Props {
    words: ClipWord[];
    /** What plays (source seconds), once the preview is prepared; words outside are cut. */
    kept: { start: number; end: number }[] | null;
    /** Player time while this clip previews, else null. */
    playhead: number | null;
    /** Words cut out of the clip (by the user, or by auto editing). */
    cutWords?: CutWord[];
    /** Auto editing of this clip is running. */
    autoEditing?: boolean;
    /** Auto editing may start (nothing else runs). */
    canAutoEdit?: boolean;
    /** The transcript's speakers in order, so each keeps its app-wide colour. */
    speakerOrder?: string[];
}

const props = defineProps<Props>();
const emit = defineEmits<{
    edit: [word: ClipWord, text: string];
    /** Cut `words` out of the clip (or, with `cut` false, restore them). */
    cut: [words: ClipWord[], cut: boolean];
    /** Let an LLM propose cuts for this clip. */
    'auto-edit': [];
}>();

type Mode = 'edit' | 'cut';
const mode = ref<Mode>('edit');
const MODES: SegmentedOption<Mode>[] = [
    { value: 'edit', label: 'Edit words', testId: 'clip-text-mode-edit' },
    { value: 'cut', label: 'Cut out', testId: 'clip-text-mode-cut' },
];
const container = ref<HTMLElement | null>(null);
const editing = ref<number | null>(null);
const draft = ref('');
/** The word last clicked in cut mode, for Shift-click ranges. */
const anchor = ref<number | null>(null);

function cutOf(word: ClipWord): CutWord | undefined {
    return (props.cutWords ?? []).find((cut) => cut.start === word.start && cut.end === word.end);
}

function userCut(word: ClipWord): boolean {
    return cutOf(word) !== undefined;
}

function autoCut(word: ClipWord): boolean {
    return cutOf(word)?.auto === true;
}

function wordTitle(word: ClipWord): string {
    const cut = cutOf(word);
    if (cut?.auto) return `Cut by auto editing${cut.reason ? `: ${cut.reason}` : ''}`;
    if (cut) return 'Cut from this clip';
    if (tightened(word)) return 'Cut by tightening';
    return mode.value === 'cut' ? 'Click to cut' : 'Click to edit';
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

/** Speakers in order (the transcript's, else as they appear), for their colours. */
const speakers = computed(
    () =>
        props.speakerOrder ?? [
            ...new Set(props.words.map((word) => word.speaker).filter((name) => !!name)),
        ],
);

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
    <div
        class="overflow-hidden rounded-md border border-line bg-well"
        data-testid="clip-text-panel"
    >
        <div class="flex flex-wrap items-center gap-2 border-b border-line px-2.5 py-2">
            <UiSegmented v-model="mode" :options="MODES" label="Text mode" size="sm" />
            <button
                type="button"
                class="btn-ghost btn-sm ml-auto"
                :disabled="!canAutoEdit"
                :aria-busy="autoEditing ? 'true' : undefined"
                :title="
                    canAutoEdit
                        ? 'Let an LLM cut false starts, repetitions and asides; replaces earlier auto cuts'
                        : 'Wait for the running job to finish'
                "
                data-testid="clip-text-auto-edit"
                @click="emit('auto-edit')"
            >
                <Loader2 v-if="autoEditing" class="animate-spin" aria-hidden="true" />
                <WandSparkles v-else aria-hidden="true" />
                {{ autoEditing ? 'Auto editing' : 'Auto edit' }}
            </button>
            <button
                v-if="cutCount > 0"
                type="button"
                class="btn-secondary btn-sm"
                data-testid="clip-text-restore"
                @click="restoreAll"
            >
                <Undo2 aria-hidden="true" />
                Restore {{ cutCount }} cut word{{ cutCount === 1 ? '' : 's' }}
            </button>
            <p class="hint basis-full text-[11px]">
                {{
                    mode === 'edit'
                        ? 'Click a word to correct it in the transcript.'
                        : 'Click a word to cut it from this clip, Shift-click for a range. The transcript stays as it is.'
                }}
            </p>
        </div>
        <div
            ref="container"
            class="relative max-h-48 overflow-y-auto px-3 py-2.5 text-[13px] leading-relaxed text-fg"
            data-testid="clip-text"
            @wheel.passive="onUserScroll"
            @touchmove.passive="onUserScroll"
            @pointerdown="onUserScroll"
        >
            <p v-if="words.length === 0" class="hint">No transcript words here.</p>
            <template v-for="(word, index) in words" :key="`${word.segment}:${word.index}`">
                <span
                    v-if="speakerBefore(index)"
                    class="mt-1.5 mb-0.5 block text-[10px] font-semibold tracking-wider uppercase first:mt-0"
                    :style="{ color: speakerColor(speakerBefore(index)!, speakers) }"
                    >{{ speakerBefore(index) }}</span
                >
                <input
                    v-if="editing === index"
                    v-model="draft"
                    class="mx-0.5 rounded-xs bg-surface-3 px-1 text-fg ring-1 ring-accent outline-none"
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
                    class="rounded-xs px-px transition-colors duration-100"
                    :class="[
                        mode === 'cut'
                            ? 'cursor-pointer hover:bg-danger/20'
                            : 'cursor-text hover:bg-surface-hover',
                        index === active ? 'bg-accent text-accent-fg' : '',
                        autoCut(word)
                            ? 'bg-warning/10 text-warning/80 line-through decoration-warning'
                            : userCut(word)
                              ? 'bg-danger/10 text-danger/75 line-through decoration-danger'
                              : '',
                        tightened(word) ? 'text-fg-faint line-through decoration-fg-faint' : '',
                    ]"
                    :title="wordTitle(word)"
                    :data-word="index"
                    :data-cut="autoCut(word) ? 'auto' : userCut(word) || undefined"
                    :data-testid="`clip-word-${index}`"
                    @click="onWord(index, $event)"
                    @keydown.enter.prevent="onWord(index, $event)"
                    >{{ word.text }}</span
                >{{ ' ' }}
            </template>
        </div>
    </div>
</template>
