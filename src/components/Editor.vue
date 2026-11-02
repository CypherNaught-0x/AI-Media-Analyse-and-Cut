<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import { ask } from '@tauri-apps/plugin-dialog';
import {
    Check,
    CheckCheck,
    ChevronDown,
    ChevronUp,
    CirclePlay,
    Combine,
    Crosshair,
    List,
    ListFilter,
    Pause,
    Pencil,
    Replace,
    RotateCcw,
    Scissors,
    Search,
    Square,
    SquarePlay,
    Trash2,
    TriangleAlert,
    Volume2,
    WholeWord,
    X,
} from '@lucide/vue';
import EmptyState from './ui/EmptyState.vue';
import { speakerColor } from '../utils/speakerColors';
import type {
    TranscriptAlternativeSource,
    TranscriptMergeStatus,
    TranscriptSegment,
    TranscriptWord,
} from '../types';
import type { TranscriptBlacklistMatch } from '../utils/transcriptBlacklist';
import { parseTime, formatTime } from '../composables/useTimeFormat';

interface SplitDraft {
    time: number;
    firstText: string;
    secondText: string;
    secondSpeaker: string;
}

const props = withDefaults(
    defineProps<{
        segments: TranscriptSegment[];
        showOnlyReviewSegments?: boolean;
        reviewThreshold?: number;
        hideBlacklistFromReview?: boolean;
        blacklistMatchesBySegment?: Record<number, TranscriptBlacklistMatch[]>;
        speakerVisibility?: Record<string, boolean>;
        audioAvailable?: boolean;
        previewIndex?: number | null;
        videoAvailable?: boolean;
        videoPreviewIndex?: number | null;
        getPlayhead?: () => number | null;
        /**
         * Only the wording can be edited (used for translated views): splitting,
         * merging, deleting, timing and speaker changes belong to the original
         * transcript, which drives the cut.
         */
        lockStructure?: boolean;
        /** Current media playhead in seconds, to highlight the segment being played. */
        playhead?: number | null;
        /** Speaker order that decides each speaker's colour (matches the speaker list). */
        speakerOrder?: string[];
    }>(),
    {
        playhead: null,
        speakerOrder: undefined,
        showOnlyReviewSegments: false,
        reviewThreshold: 0.85,
        hideBlacklistFromReview: false,
        blacklistMatchesBySegment: () => ({}),
        speakerVisibility: () => ({}),
        audioAvailable: false,
        previewIndex: null,
        videoAvailable: false,
        videoPreviewIndex: null,
        lockStructure: false,
    },
);

const emit = defineEmits<{
    (e: 'jump-to', time: number): void;
    (e: 'preview', payload: { start: string; end: string; index: number }): void;
    (e: 'preview-video', payload: { start: string; end: string; index: number }): void;
    (e: 'update:segments', segments: TranscriptSegment[]): void;
}>();

const editingIndex = ref<number | null>(null);
const tempSegment = ref<TranscriptSegment | null>(null);
// Sentinel select value that switches the speaker field from the existing-speaker
// dropdown to a free-text input for adding a brand-new speaker.
const NEW_SPEAKER_OPTION = '__add_new_speaker__';
const addingNewSpeaker = ref(false);
const newSpeakerInput = ref<HTMLInputElement | null>(null);
const addingNewSplitSpeaker = ref(false);
const newSplitSpeakerInput = ref<HTMLInputElement | null>(null);

// Both new-speaker inputs live inside the segment `v-for`, where a plain string
// `ref` collects an *array* of elements rather than the element itself (so
// `.focus()` threw). Function refs, as used for `setSegmentRef` below, keep the
// element itself; only one edit form and one split panel are open at a time.
const setNewSpeakerInput = (element: unknown) => {
    newSpeakerInput.value = element instanceof HTMLInputElement ? element : null;
};
const setNewSplitSpeakerInput = (element: unknown) => {
    newSplitSpeakerInput.value = element instanceof HTMLInputElement ? element : null;
};
const splittingIndex = ref<number | null>(null);
const splitDraft = ref<SplitDraft | null>(null);
const selectedIndices = ref<Set<number>>(new Set());
const alternativeSources: TranscriptAlternativeSource[] = ['google', 'local'];
const searchQuery = ref('');
const replaceQuery = ref('');
const wholeWordMatch = ref(false);
const currentMatchIndex = ref(0);
const segmentRefs = new Map<number, HTMLDivElement>();
// The row the user last clicked or focused: highlighted, and its actions stay visible.
const activeIndex = ref<number | null>(null);
const showReplace = ref(false);
const uid = Math.random().toString(36).slice(2, 8);

interface SearchMatch {
    matchIndex: number;
    visibleIndex: number;
    originalIndex: number;
    start: number;
    end: number;
}

const reviewThreshold = computed(() => {
    if (Number.isNaN(props.reviewThreshold)) return 0.85;
    return Math.min(Math.max(props.reviewThreshold, 0), 1);
});

const getBlacklistMatches = (originalIndex: number): TranscriptBlacklistMatch[] =>
    props.blacklistMatchesBySegment[originalIndex] ?? [];

const segmentNeedsReview = (segment: TranscriptSegment, originalIndex: number): boolean => {
    if (segment.reviewResolved) return false;

    const belowThreshold =
        segment.similarityScore != null
            ? segment.similarityScore < reviewThreshold.value
            : segment.mergeStatus === 'missing_google' || segment.mergeStatus === 'missing_local';

    if (belowThreshold) return true;
    if (props.hideBlacklistFromReview) return false;

    return getBlacklistMatches(originalIndex).length > 0;
};

// Unique, sorted list of speaker names already present in the transcript, used to
// populate the speaker dropdown when editing a segment.
const existingSpeakers = computed(() => {
    const names = new Set<string>();
    for (const segment of props.segments) {
        const name = segment.speaker?.trim();
        if (name) names.add(name);
    }
    return Array.from(names).sort((a, b) => a.localeCompare(b));
});

const visibleSegments = computed(() =>
    props.segments
        .map((segment, originalIndex) => ({ segment, originalIndex }))
        .filter(({ segment }) => props.speakerVisibility[segment.speaker] ?? true)
        .filter(
            ({ segment, originalIndex }) =>
                !props.showOnlyReviewSegments || segmentNeedsReview(segment, originalIndex),
        ),
);

/**
 * Whether a row opens a new speaker run.
 *
 * Consecutive segments from the same speaker repeat the name for no
 * information gain, and the transcript is the surface people scroll most. Only
 * labelling the first row of a run removes a line from most rows and makes
 * speaker changes far easier to spot while scanning.
 */
function startsSpeakerRun(visibleIndex: number): boolean {
    const rows = visibleSegments.value;
    if (visibleIndex <= 0) return true;
    return rows[visibleIndex - 1]?.segment.speaker !== rows[visibleIndex]?.segment.speaker;
}

/** Badges force the header line even mid-run, so they are never hidden. */
function hasRowBadges(segment: TranscriptSegment, originalIndex: number): boolean {
    return (
        !!segment.mergeStatus ||
        segment.similarityScore != null ||
        segment.reviewResolved === true ||
        getBlacklistMatches(originalIndex).length > 0
    );
}

watch(visibleSegments, (segments) => {
    const visibleIndices = new Set(segments.map(({ originalIndex }) => originalIndex));

    selectedIndices.value = new Set(
        Array.from(selectedIndices.value).filter((index) => visibleIndices.has(index)),
    );

    if (activeIndex.value !== null && !visibleIndices.has(activeIndex.value)) {
        activeIndex.value = null;
    }

    if (editingIndex.value !== null && !visibleIndices.has(editingIndex.value)) {
        cancelEdit();
    }

    if (splittingIndex.value !== null && !visibleIndices.has(splittingIndex.value)) {
        cancelSplit();
    }
});

const escapeRegExp = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

const escapeHtml = (value: string): string =>
    value
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');

const buildSearchPattern = (query: string, wholeWord: boolean): RegExp | null => {
    if (!query) return null;
    const source = wholeWord ? `\\b${escapeRegExp(query)}\\b` : escapeRegExp(query);
    return new RegExp(source, 'g');
};

const getMatchRanges = (text: string, query: string, wholeWord: boolean) => {
    const pattern = buildSearchPattern(query, wholeWord);
    if (!pattern) return [];

    const matches: Array<{ start: number; end: number }> = [];
    let match: RegExpExecArray | null;

    while ((match = pattern.exec(text)) !== null) {
        matches.push({
            start: match.index,
            end: match.index + match[0].length,
        });
    }

    return matches;
};

const searchMatches = computed<SearchMatch[]>(() => {
    if (!searchQuery.value) return [];

    const matches: SearchMatch[] = [];

    visibleSegments.value.forEach(({ segment, originalIndex }, visibleIndex) => {
        for (const range of getMatchRanges(segment.text, searchQuery.value, wholeWordMatch.value)) {
            matches.push({
                matchIndex: matches.length,
                visibleIndex,
                originalIndex,
                start: range.start,
                end: range.end,
            });
        }
    });

    return matches;
});

const searchMatchesBySegment = computed(() => {
    const map = new Map<number, SearchMatch[]>();

    for (const match of searchMatches.value) {
        const segmentMatches = map.get(match.originalIndex);
        if (segmentMatches) {
            segmentMatches.push(match);
        } else {
            map.set(match.originalIndex, [match]);
        }
    }

    return map;
});

const currentSearchMatch = computed(() =>
    searchMatches.value.length > 0 ? searchMatches.value[currentMatchIndex.value] : null,
);

const searchStatusLabel = computed(() => {
    if (!searchQuery.value) return 'Enter text to search';
    if (searchMatches.value.length === 0) return 'No matches';
    return `${currentMatchIndex.value + 1} of ${searchMatches.value.length} matches`;
});

const setSegmentRef = (originalIndex: number, element: HTMLDivElement | null) => {
    if (element) {
        segmentRefs.set(originalIndex, element);
    } else {
        segmentRefs.delete(originalIndex);
    }
};

const scrollToMatch = (match: SearchMatch | null) => {
    if (!match) return;

    nextTick(() => {
        const element = segmentRefs.get(match.originalIndex);
        if (element && typeof element.scrollIntoView === 'function') {
            element.scrollIntoView({
                block: 'nearest',
                behavior: 'smooth',
            });
        }
    });
};

const goToMatch = (nextIndex: number) => {
    if (searchMatches.value.length === 0) return;

    const normalizedIndex = (nextIndex + searchMatches.value.length) % searchMatches.value.length;
    currentMatchIndex.value = normalizedIndex;
    scrollToMatch(searchMatches.value[normalizedIndex]);
};

const goToPreviousMatch = () => {
    goToMatch(currentMatchIndex.value - 1);
};

const goToNextMatch = () => {
    goToMatch(currentMatchIndex.value + 1);
};

watch([searchQuery, wholeWordMatch], () => {
    currentMatchIndex.value = 0;
    scrollToMatch(searchMatches.value[0] ?? null);
});

watch(searchMatches, (matches) => {
    if (matches.length === 0) {
        currentMatchIndex.value = 0;
        return;
    }

    if (currentMatchIndex.value >= matches.length) {
        currentMatchIndex.value = matches.length - 1;
    }
});

const jumpTo = (timeStr: string) => {
    emit('jump-to', parseTime(timeStr));
};

const requestPreview = (originalIndex: number) => {
    const segment = props.segments[originalIndex];
    if (!segment) return;
    emit('preview', { start: segment.start, end: segment.end, index: originalIndex });
};

const requestVideoPreview = (originalIndex: number) => {
    const segment = props.segments[originalIndex];
    if (!segment) return;
    emit('preview-video', { start: segment.start, end: segment.end, index: originalIndex });
};

const requestPlayFrom = (originalIndex: number) => {
    const segment = props.segments[originalIndex];
    if (!segment) return;
    jumpTo(segment.start);
};

const toggleSelection = (originalIndex: number) => {
    if (selectedIndices.value.has(originalIndex)) {
        selectedIndices.value.delete(originalIndex);
    } else {
        selectedIndices.value.add(originalIndex);
    }
};

const handleSegmentClick = (originalIndex: number, event: MouseEvent) => {
    // Plain clicks never control playback (use the per-segment buttons); they
    // make the row the current one. Shift-click toggles multi-segment selection.
    if (!event.shiftKey) {
        activeIndex.value = originalIndex;
        return;
    }

    toggleSelection(originalIndex);
};

const startEditing = (originalIndex: number) => {
    cancelSplit();
    editingIndex.value = originalIndex;
    tempSegment.value = { ...props.segments[originalIndex] };
    addingNewSpeaker.value = false;
};

// Handle a choice in the speaker dropdown: either adopt an existing speaker or
// switch to the free-text field for a new one.
const onSpeakerSelect = (value: string) => {
    if (!tempSegment.value) return;
    if (value === NEW_SPEAKER_OPTION) {
        addingNewSpeaker.value = true;
        tempSegment.value.speaker = '';
        void nextTick(() => newSpeakerInput.value?.focus());
        return;
    }
    addingNewSpeaker.value = false;
    tempSegment.value.speaker = value;
};

// Leave the new-speaker input and return to the dropdown, restoring the segment's
// original speaker so cancelling the add does not blank the field.
const cancelNewSpeaker = () => {
    if (!tempSegment.value) return;
    addingNewSpeaker.value = false;
    const original =
        editingIndex.value !== null ? props.segments[editingIndex.value]?.speaker : undefined;
    tempSegment.value.speaker = original?.trim() || existingSpeakers.value[0] || '';
};

// Partition a segment's word-level timing at a split point so the text follows
// the exact audio switch. Returns null when the segment has no word timing.
const deriveSplitText = (
    segment: TranscriptSegment,
    splitSec: number,
): { first: string; second: string } | null => {
    if (!segment.words?.length) return null;
    const join = (words: TranscriptWord[]) =>
        words
            .map((word) => word.text)
            .join(' ')
            .replace(/\s+/g, ' ')
            .trim();
    return {
        first: join(segment.words.filter((word) => parseTime(word.start) < splitSec)),
        second: join(segment.words.filter((word) => parseTime(word.start) >= splitSec)),
    };
};

const splitBounds = computed(() => {
    if (splittingIndex.value === null) return { start: 0, end: 0 };
    const segment = props.segments[splittingIndex.value];
    if (!segment) return { start: 0, end: 0 };
    return { start: parseTime(segment.start), end: parseTime(segment.end) };
});

const canConfirmSplit = computed(() => {
    if (!splitDraft.value) return false;
    const { start, end } = splitBounds.value;
    return splitDraft.value.time > start && splitDraft.value.time < end;
});

const startSplitting = (originalIndex: number) => {
    if (props.lockStructure) return;
    const segment = props.segments[originalIndex];
    if (!segment) return;
    cancelEdit();
    const start = parseTime(segment.start);
    const end = parseTime(segment.end);
    const midpoint = (start + end) / 2;
    const derived = deriveSplitText(segment, midpoint);
    splittingIndex.value = originalIndex;
    splitDraft.value = {
        time: midpoint,
        firstText: derived ? derived.first : segment.text,
        secondText: derived ? derived.second : '',
        secondSpeaker: segment.speaker,
    };
    addingNewSplitSpeaker.value = false;
};

const cancelSplit = () => {
    splittingIndex.value = null;
    splitDraft.value = null;
    addingNewSplitSpeaker.value = false;
};

// Speaker-dropdown handling for the split panel's "second part" speaker, mirroring
// the edit form: pick an existing speaker or switch to a free-text new-speaker field.
const onSplitSpeakerSelect = (value: string) => {
    if (!splitDraft.value) return;
    if (value === NEW_SPEAKER_OPTION) {
        addingNewSplitSpeaker.value = true;
        splitDraft.value.secondSpeaker = '';
        void nextTick(() => newSplitSpeakerInput.value?.focus());
        return;
    }
    addingNewSplitSpeaker.value = false;
    splitDraft.value.secondSpeaker = value;
};

const cancelNewSplitSpeaker = () => {
    if (!splitDraft.value) return;
    addingNewSplitSpeaker.value = false;
    const original =
        splittingIndex.value !== null ? props.segments[splittingIndex.value]?.speaker : undefined;
    splitDraft.value.secondSpeaker = original?.trim() || existingSpeakers.value[0] || '';
};

// Re-derive the text boundary as the split point moves (only when word timing exists).
const onSplitTimeChange = () => {
    if (splittingIndex.value === null || !splitDraft.value) return;
    const { start, end } = splitBounds.value;
    splitDraft.value.time = Math.min(Math.max(splitDraft.value.time, start), end);
    const derived = deriveSplitText(props.segments[splittingIndex.value], splitDraft.value.time);
    if (derived) {
        splitDraft.value.firstText = derived.first;
        splitDraft.value.secondText = derived.second;
    }
};

const useCurrentPlayhead = () => {
    if (!props.getPlayhead || !splitDraft.value) return;
    const playhead = props.getPlayhead();
    if (playhead === null || !Number.isFinite(playhead)) return;
    const { start, end } = splitBounds.value;
    splitDraft.value.time = Math.min(Math.max(playhead, start), end);
    onSplitTimeChange();
};

const confirmSplit = () => {
    if (splittingIndex.value === null || !splitDraft.value || !canConfirmSplit.value) return;

    const originalIndex = splittingIndex.value;
    const segment = props.segments[originalIndex];
    const splitSec = splitDraft.value.time;
    const splitStamp = formatTime(splitSec);
    const secondSpeaker = splitDraft.value.secondSpeaker.trim() || segment.speaker;

    let firstWords: TranscriptWord[] | undefined;
    let secondWords: TranscriptWord[] | undefined;
    if (segment.words?.length) {
        const first = segment.words.filter((word) => parseTime(word.start) < splitSec);
        const second = segment.words.filter((word) => parseTime(word.start) >= splitSec);
        firstWords = first.length ? first : undefined;
        secondWords = second.length
            ? secondSpeaker !== segment.speaker
                ? second.map((word) => ({ ...word, speaker: secondSpeaker }))
                : second
            : undefined;
    }

    const firstSegment = stripMergeMetadata({
        ...segment,
        end: splitStamp,
        text: splitDraft.value.firstText.trim(),
        words: firstWords,
    });
    const secondSegment = stripMergeMetadata({
        ...segment,
        start: splitStamp,
        speaker: secondSpeaker,
        text: splitDraft.value.secondText.trim(),
        words: secondWords,
    });

    const newSegments = [...props.segments];
    newSegments.splice(originalIndex, 1, firstSegment, secondSegment);
    emit('update:segments', newSegments);
    cancelSplit();
};

const stripMergeMetadata = (segment: TranscriptSegment): TranscriptSegment => ({
    ...segment,
    alternatives: undefined,
    mergeStatus: undefined,
    activeSource: undefined,
    similarityScore: undefined,
});

const mergeWords = (segmentsToMerge: TranscriptSegment[]): TranscriptWord[] | undefined => {
    const mergedWords = segmentsToMerge.flatMap((segment) => segment.words ?? []);
    return mergedWords.length > 0 ? mergedWords : undefined;
};

const getAlternativeText = (
    segment: TranscriptSegment,
    source: TranscriptAlternativeSource,
): string => {
    return segment.alternatives?.find((alternative) => alternative.source === source)?.text ?? '';
};

const hasAlternativeText = (
    segment: TranscriptSegment,
    source: TranscriptAlternativeSource,
): boolean => {
    return getAlternativeText(segment, source).trim().length > 0;
};

const sourceLabel = (source: TranscriptAlternativeSource): string => {
    // "Local" rather than an engine name: the merge works with any local engine.
    return source === 'google' ? 'Remote' : 'Local';
};

const renderSegmentText = (text: string, originalIndex: number): string => {
    const segmentMatches = searchMatchesBySegment.value.get(originalIndex) ?? [];
    if (segmentMatches.length === 0) return escapeHtml(text);

    const parts: string[] = [];
    let cursor = 0;

    for (const match of segmentMatches) {
        parts.push(escapeHtml(text.slice(cursor, match.start)));
        parts.push(
            `<mark class="${
                match.matchIndex === currentMatchIndex.value
                    ? 'rounded-[3px] bg-accent px-0.5 text-accent-fg'
                    : 'rounded-[3px] bg-warning/20 px-0.5 text-fg'
            }">${escapeHtml(text.slice(match.start, match.end))}</mark>`,
        );
        cursor = match.end;
    }

    parts.push(escapeHtml(text.slice(cursor)));
    return parts.join('');
};

const isSearchResultSegment = (originalIndex: number): boolean =>
    searchMatchesBySegment.value.has(originalIndex);

const isCurrentSearchSegment = (originalIndex: number): boolean =>
    currentSearchMatch.value?.originalIndex === originalIndex;

const mergeStatusLabel = (status?: TranscriptMergeStatus): string => {
    if (status === 'missing_google') return 'Missing in remote';
    if (status === 'missing_local') return 'Missing in local';
    if (status === 'conflict') return 'Review needed';
    return 'Aligned';
};

const mergeStatusClass = (status?: TranscriptMergeStatus): string => {
    if (status === 'missing_google') return 'chip-danger';
    if (status === 'missing_local' || status === 'conflict') return 'chip-warning';
    return 'chip-success';
};

// ---------------------------------------------------------------- Presentation

// Speaker colours follow the speaker list's order so a speaker has the same
// colour in the list and in the rows; without it, order of first appearance.
const colourOrder = computed(() => {
    if (props.speakerOrder?.length) return props.speakerOrder;
    const seen: string[] = [];
    for (const segment of props.segments) {
        if (!seen.includes(segment.speaker)) seen.push(segment.speaker);
    }
    return seen;
});

const colourFor = (speaker: string): string => speakerColor(speaker, colourOrder.value);

// Segment bounds in seconds, parsed once per transcript change rather than on
// every playhead tick.
const segmentBounds = computed(() =>
    props.segments.map((segment) => ({
        start: parseTime(segment.start),
        end: parseTime(segment.end),
    })),
);

const playingIndex = computed<number | null>(() => {
    const t = props.playhead;
    if (t === null || t === undefined || !Number.isFinite(t)) return null;
    const index = segmentBounds.value.findIndex(({ start, end }) => t >= start && t < end);
    return index >= 0 ? index : null;
});

const rowStateClass = (originalIndex: number): string => {
    // Accent is reserved for the selection; search results use the warning
    // tint (the current match is also marked by its accent highlight).
    if (selectedIndices.value.has(originalIndex)) return 'border-l-accent bg-accent/[0.12]';
    if (isCurrentSearchSegment(originalIndex)) return 'border-l-warning bg-warning/[0.08]';
    if (isSearchResultSegment(originalIndex)) return 'border-l-warning/40 bg-warning/[0.04]';
    if (activeIndex.value === originalIndex) return 'border-l-fg-subtle bg-surface-2';
    if (playingIndex.value === originalIndex) return 'border-l-success/70 bg-success/[0.04]';
    return 'border-l-transparent hover:bg-surface-2';
};

const showRowActions = (originalIndex: number): boolean =>
    activeIndex.value === originalIndex || selectedIndices.value.has(originalIndex);

const searchCounter = computed(() => {
    if (!searchQuery.value) return '';
    if (searchMatches.value.length === 0) return 'No matches';
    return `${currentMatchIndex.value + 1} of ${searchMatches.value.length}`;
});

const replaceDisabledReason = computed(() => {
    if (searchMatches.value.length === 0) return 'Search for text to replace first';
    if (editingIndex.value !== null) return 'Finish editing the open segment first';
    return '';
});

const emptyStateDescription = computed(() => {
    if (props.segments.length === 0) return 'This transcript has no segments yet.';
    if (props.showOnlyReviewSegments)
        return 'Nothing needs review with the current filter. Turn off "Show only review items" to see every segment.';
    return 'Every speaker is hidden. Show a speaker in the speaker list to see their segments.';
});

// ---------------------------------------------------------------- Keyboard

const onSearchKeydown = (event: KeyboardEvent) => {
    if (event.key === 'Enter') {
        event.preventDefault();
        if (event.shiftKey) goToPreviousMatch();
        else goToNextMatch();
    } else if (event.key === 'Escape' && searchQuery.value) {
        event.preventDefault();
        searchQuery.value = '';
    }
};

const focusRow = (visibleIndex: number) => {
    const row = visibleSegments.value[visibleIndex];
    if (!row) return;
    const element = segmentRefs.get(row.originalIndex);
    element?.focus();
    if (element && typeof element.scrollIntoView === 'function') {
        element.scrollIntoView({ block: 'nearest' });
    }
};

const onRowFocus = (originalIndex: number, event: FocusEvent) => {
    if (event.target === event.currentTarget) activeIndex.value = originalIndex;
};

// Shift-click is for selection: keep it from also moving focus or selecting text.
const onRowMousedown = (event: MouseEvent) => {
    if (!event.shiftKey) return;
    const target = event.target as HTMLElement | null;
    if (target?.closest('input, textarea, select, button')) return;
    event.preventDefault();
};

const onRowKeydown = (originalIndex: number, visibleIndex: number, event: KeyboardEvent) => {
    // Only when the row itself has focus, not a field or button inside it.
    if (event.target !== event.currentTarget) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        focusRow(visibleIndex + (event.key === 'ArrowDown' ? 1 : -1));
    } else if (event.key === 'Enter') {
        event.preventDefault();
        startEditing(originalIndex);
        void nextTick(() =>
            segmentRefs.get(originalIndex)?.querySelector<HTMLTextAreaElement>('textarea')?.focus(),
        );
    } else if (event.key === ' ') {
        event.preventDefault();
        toggleSelection(originalIndex);
    } else if (event.key === 'Escape') {
        if (selectedIndices.value.size > 0) selectedIndices.value.clear();
        else activeIndex.value = null;
    }
};

const onFormKeydown = (event: KeyboardEvent, save: () => void, cancel: () => void) => {
    if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        const row = (event.currentTarget as HTMLElement).closest<HTMLElement>('.segment');
        cancel();
        row?.focus();
    } else if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        save();
    }
};

const splitPercent = computed(() => {
    if (!splitDraft.value) return 0;
    const { start, end } = splitBounds.value;
    if (end <= start) return 0;
    return Math.min(Math.max(((splitDraft.value.time - start) / (end - start)) * 100, 0), 100);
});

const getUniqueBlacklistWords = (originalIndex: number): string[] => {
    const words: string[] = [];
    const seen = new Set<string>();

    for (const match of getBlacklistMatches(originalIndex)) {
        if (!seen.has(match.normalizedWord)) {
            seen.add(match.normalizedWord);
            words.push(match.matchedText);
        }
    }

    return words;
};

const selectAlternative = (originalIndex: number, source: TranscriptAlternativeSource) => {
    const segment = props.segments[originalIndex];
    const text = getAlternativeText(segment, source).trim();
    if (!text) return;
    const speaker = segment.alternatives
        ?.find((alternative) => alternative.source === source)
        ?.speaker?.trim();

    const newSegments = [...props.segments];
    newSegments[originalIndex] = {
        ...segment,
        text,
        speaker: speaker || segment.speaker,
        activeSource: source,
    };
    emit('update:segments', newSegments);
};

const cancelEdit = () => {
    editingIndex.value = null;
    tempSegment.value = null;
    addingNewSpeaker.value = false;
};

const emitUpdatedSegments = (updates: Array<{ originalIndex: number; text: string }>) => {
    if (updates.length === 0) return;

    const newSegments = [...props.segments];

    for (const update of updates) {
        newSegments[update.originalIndex] = stripMergeMetadata({
            ...props.segments[update.originalIndex],
            text: update.text,
        });
    }

    emit('update:segments', newSegments);
};

const replaceCurrentMatch = () => {
    if (editingIndex.value !== null) return;

    const match = currentSearchMatch.value;
    if (!match) return;

    const segment = props.segments[match.originalIndex];
    const updatedText = `${segment.text.slice(0, match.start)}${replaceQuery.value}${segment.text.slice(match.end)}`;

    emitUpdatedSegments([{ originalIndex: match.originalIndex, text: updatedText }]);
};

const replaceAllMatches = () => {
    if (editingIndex.value !== null || !searchQuery.value) return;

    const updates = visibleSegments.value
        .map(({ segment, originalIndex }) => {
            const ranges = getMatchRanges(segment.text, searchQuery.value, wholeWordMatch.value);
            if (ranges.length === 0) return null;

            const updatedText = segment.text.replace(
                buildSearchPattern(searchQuery.value, wholeWordMatch.value)!,
                () => replaceQuery.value,
            );

            return { originalIndex, text: updatedText };
        })
        .filter((update): update is { originalIndex: number; text: string } => update !== null);

    emitUpdatedSegments(updates);
};

const saveEdit = () => {
    if (editingIndex.value !== null && tempSegment.value) {
        const newSegments = [...props.segments];
        newSegments[editingIndex.value] = stripMergeMetadata(tempSegment.value);
        emit('update:segments', newSegments);
        cancelEdit();
    }
};

const toggleReviewResolved = (originalIndex: number) => {
    const segment = props.segments[originalIndex];
    if (!segment) return;

    const newSegments = [...props.segments];
    newSegments[originalIndex] = { ...segment, reviewResolved: !segment.reviewResolved };
    emit('update:segments', newSegments);
};

const deleteSegment = async (originalIndex: number) => {
    if (props.lockStructure) return;
    const confirmed = await ask('Are you sure you want to delete this segment?', {
        title: 'Confirm Deletion',
        kind: 'warning',
    });

    if (confirmed) {
        const newSegments = [...props.segments];
        newSegments.splice(originalIndex, 1);
        emit('update:segments', newSegments);
    }
};

const deleteSelected = async () => {
    if (props.lockStructure) return;
    const confirmed = await ask(
        `Are you sure you want to delete ${selectedIndices.value.size} segments?`,
        {
            title: 'Confirm Deletion',
            kind: 'warning',
        },
    );

    if (confirmed) {
        const indices = Array.from(selectedIndices.value).sort((a, b) => b - a);
        const newSegments = [...props.segments];
        for (const i of indices) {
            newSegments.splice(i, 1);
        }
        emit('update:segments', newSegments);
        selectedIndices.value.clear();
    }
};

const mergeSelected = () => {
    if (props.lockStructure) return;
    const indices = Array.from(selectedIndices.value).sort((a, b) => a - b);
    if (indices.length < 2) return;

    const firstIndex = indices[0];
    const lastIndex = indices[indices.length - 1];

    // Absorb every segment between the first and last selected (inclusive),
    // including unselected ones in between. Leaving those in place would create
    // a merged segment whose time range overlaps them and corrupts the timeline.
    const range: number[] = [];
    for (let i = firstIndex; i <= lastIndex; i++) range.push(i);

    const first = props.segments[firstIndex];
    const last = props.segments[lastIndex];

    const mergedText = range.map((i) => props.segments[i].text).join(' ');

    const merged: TranscriptSegment = {
        start: first.start,
        end: last.end,
        speaker: first.speaker,
        text: mergedText,
        words: mergeWords(range.map((i) => props.segments[i])),
    };

    const newSegments = [...props.segments];
    // Replace the whole contiguous range with the single merged segment.
    newSegments.splice(firstIndex, range.length, merged);

    emit('update:segments', newSegments);
    selectedIndices.value.clear();
};

const mergeDown = (originalIndex: number) => {
    if (props.lockStructure) return;
    if (originalIndex >= props.segments.length - 1) return;

    const current = props.segments[originalIndex];
    const next = props.segments[originalIndex + 1];

    const merged: TranscriptSegment = {
        start: current.start,
        end: next.end,
        speaker: current.speaker,
        text: `${current.text} ${next.text}`,
        words: mergeWords([current, next]),
    };

    const newSegments = [...props.segments];
    newSegments.splice(originalIndex, 2, merged);
    emit('update:segments', newSegments);
};
</script>

<template>
    <!-- The page scrolls, not the editor: one scroll container, with the toolbar
       sticking to the top of the window while the transcript scrolls past. -->
    <div class="editor-container @container relative rounded-lg border border-line bg-surface">
        <!-- Masks the gap above the stuck toolbar so rows don't show through
             it; at rest it sits in the page gap above the card. -->
        <div
            aria-hidden="true"
            class="pointer-events-none sticky top-0 z-20 -mx-px -mt-[15px] mb-px h-[14px] bg-canvas"
        ></div>
        <!-- Sticky toolbar: find (and optional replace) on the left, the
         language controls from the parent on the right. -->
        <div
            class="sticky top-4 z-20 rounded-t-lg border-b border-line bg-surface px-3 py-2.5 shadow-[0_8px_16px_-12px_rgb(0_0_0/0.8)]"
        >
            <div class="flex flex-wrap items-center gap-2">
                <div class="relative min-w-[15rem] flex-1">
                    <Search
                        class="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-fg-subtle"
                        aria-hidden="true"
                    />
                    <input
                        v-model="searchQuery"
                        data-testid="editor-search-input"
                        type="text"
                        placeholder="Search transcript"
                        aria-label="Search transcript"
                        :aria-describedby="`${uid}-search-status`"
                        class="input input-sm h-7 pr-32 pl-8"
                        @keydown="onSearchKeydown"
                    />
                    <div class="absolute inset-y-0 right-1 flex items-center gap-0.5">
                        <span
                            v-if="searchCounter"
                            class="mr-1 font-mono text-[11px] whitespace-nowrap"
                            :class="searchMatches.length ? 'text-fg-muted' : 'text-fg-subtle'"
                            data-testid="editor-search-counter"
                            >{{ searchCounter }}</span
                        >
                        <button
                            type="button"
                            data-testid="editor-search-prev"
                            class="btn-ghost btn-sm btn-icon h-5.5! w-5.5!"
                            aria-label="Previous match"
                            title="Previous match (Shift+Enter)"
                            :disabled="searchMatches.length === 0"
                            @click="goToPreviousMatch"
                        >
                            <ChevronUp aria-hidden="true" />
                        </button>
                        <button
                            type="button"
                            data-testid="editor-search-next"
                            class="btn-ghost btn-sm btn-icon h-5.5! w-5.5!"
                            aria-label="Next match"
                            title="Next match (Enter)"
                            :disabled="searchMatches.length === 0"
                            @click="goToNextMatch"
                        >
                            <ChevronDown aria-hidden="true" />
                        </button>
                    </div>
                    <span :id="`${uid}-search-status`" class="sr-only" aria-live="polite">{{
                        searchStatusLabel
                    }}</span>
                </div>

                <label
                    class="relative inline-flex h-7 cursor-pointer items-center gap-1.5 rounded-md border px-2 text-[12px] font-medium transition-colors duration-100 has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent/70"
                    :class="
                        wholeWordMatch
                            ? 'border-accent/40 bg-accent/15 text-accent-soft'
                            : 'border-transparent text-fg-muted hover:bg-surface-hover hover:text-fg'
                    "
                    title="Match whole words only"
                >
                    <input
                        v-model="wholeWordMatch"
                        data-testid="editor-whole-word-toggle"
                        type="checkbox"
                        class="sr-only"
                    />
                    <WholeWord class="h-3.5 w-3.5" aria-hidden="true" />
                    Whole word
                </label>

                <button
                    type="button"
                    data-testid="editor-replace-toggle"
                    class="btn-ghost btn-sm h-7"
                    :data-active="showReplace ? 'true' : 'false'"
                    :aria-expanded="showReplace ? 'true' : 'false'"
                    :aria-controls="`${uid}-replace-row`"
                    title="Find and replace"
                    @click="showReplace = !showReplace"
                >
                    <Replace aria-hidden="true" />
                    Replace
                </button>

                <slot name="status" />

                <!-- Narrow: its own labelled line below the replace row. Wide:
                     inline at the end of the search line. -->
                <div
                    v-if="$slots.toolbar"
                    class="order-2 flex w-full flex-wrap items-center gap-2 border-t border-line pt-2 @4xl:order-none @4xl:ml-auto @4xl:w-auto @4xl:border-t-0 @4xl:border-l @4xl:pt-0 @4xl:pl-2"
                >
                    <span class="eyebrow mr-1 @4xl:hidden">Language</span>
                    <slot name="toolbar" />
                </div>

                <div
                    v-show="showReplace"
                    :id="`${uid}-replace-row`"
                    class="order-1 flex w-full flex-wrap items-center gap-2"
                >
                    <div class="relative min-w-[15rem] flex-1">
                        <Replace
                            class="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-fg-subtle"
                            aria-hidden="true"
                        />
                        <input
                            v-model="replaceQuery"
                            data-testid="editor-replace-input"
                            type="text"
                            placeholder="Replace with"
                            aria-label="Replace with"
                            class="input input-sm h-7 pl-8"
                            @keydown.enter.prevent="replaceCurrentMatch"
                        />
                    </div>
                    <button
                        type="button"
                        data-testid="editor-replace-current"
                        class="btn-secondary btn-sm h-7"
                        :disabled="searchMatches.length === 0 || editingIndex !== null"
                        :title="replaceDisabledReason || 'Replace the highlighted match'"
                        @click="replaceCurrentMatch"
                    >
                        Replace
                    </button>
                    <button
                        type="button"
                        data-testid="editor-replace-all"
                        class="btn-secondary btn-sm h-7"
                        :disabled="searchMatches.length === 0 || editingIndex !== null"
                        :title="
                            replaceDisabledReason || 'Replace every match in the visible segments'
                        "
                        @click="replaceAllMatches"
                    >
                        Replace all
                    </button>
                    <span v-if="editingIndex !== null && searchMatches.length" class="hint">
                        Finish editing the open segment to replace.
                    </span>
                </div>
            </div>
        </div>

        <!-- Rows -->
        <div
            class="relative px-1.5 pt-3 transition-[padding] duration-150"
            :class="selectedIndices.size > 0 && !lockStructure ? 'pb-28' : 'pb-1.5'"
        >
            <EmptyState
                v-if="visibleSegments.length === 0"
                :icon="ListFilter"
                title="No segments to show"
                :description="emptyStateDescription"
                compact
                class="m-2"
                data-testid="editor-empty"
            />

            <!-- Dense rows: state is a left accent bar and a tint. Speaker names
           only open a new speaker run, in that speaker's colour. -->
            <div
                v-for="({ segment, originalIndex }, visibleIndex) in visibleSegments"
                :key="`${originalIndex}-${segment.start}-${segment.end}`"
                :ref="(element) => setSegmentRef(originalIndex, element as HTMLDivElement | null)"
                tabindex="0"
                :data-active="activeIndex === originalIndex ? 'true' : undefined"
                :data-playing="playingIndex === originalIndex ? 'true' : undefined"
                :aria-current="activeIndex === originalIndex ? 'true' : undefined"
                role="article"
                :aria-labelledby="`${uid}-t-${originalIndex}`"
                :aria-describedby="`${uid}-x-${originalIndex}`"
                class="segment group relative grid scroll-mt-32 scroll-mb-32 grid-cols-[3.25rem_minmax(0,1fr)_auto] gap-x-3 rounded-md @4xl:grid-cols-[3.25rem_minmax(0,1fr)_auto_auto] border-l-2 py-2 pr-1.5 pl-2.5 transition-colors duration-100 focus-visible:-outline-offset-2"
                :class="[
                    rowStateClass(originalIndex),
                    startsSpeakerRun(visibleIndex) && visibleIndex > 0 ? 'mt-2.5' : '',
                ]"
                @click="handleSegmentClick(originalIndex, $event)"
                @mousedown="onRowMousedown"
                @focus="onRowFocus(originalIndex, $event)"
                @keydown="onRowKeydown(originalIndex, visibleIndex, $event)"
            >
                <!-- Time gutter: the anchor people scan by. -->
                <span
                    :id="`${uid}-t-${originalIndex}`"
                    class="col-start-1 row-start-1 flex flex-col items-start gap-1 pt-[3px] font-mono text-[11px] leading-5 select-none"
                    :class="playingIndex === originalIndex ? 'text-success' : 'text-fg-subtle'"
                    :title="`${segment.start} – ${segment.end}`"
                    ><span class="sr-only">{{ segment.speaker }}, </span>{{ segment.start
                    }}<span
                        v-if="selectedIndices.has(originalIndex)"
                        class="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-accent text-accent-fg"
                        title="Selected"
                        ><Check class="h-3 w-3" aria-hidden="true" /><span class="sr-only"
                            >, selected</span
                        ></span
                    ></span
                >

                <div class="col-start-2 row-start-1 min-w-0">
                    <!-- Header line: only when it carries information (a speaker
             change or a badge), so mid-run rows are a single line of text. -->
                    <div
                        v-if="
                            startsSpeakerRun(visibleIndex) ||
                            hasRowBadges(segment, originalIndex) ||
                            editingIndex === originalIndex ||
                            splittingIndex === originalIndex
                        "
                        class="mb-0.5 flex flex-wrap items-center gap-x-2 gap-y-1"
                    >
                        <span
                            class="text-[12px] font-semibold"
                            :style="{ color: colourFor(segment.speaker) }"
                            >{{ segment.speaker }}</span
                        >
                        <span
                            v-if="segment.mergeStatus"
                            :class="mergeStatusClass(segment.mergeStatus)"
                        >
                            {{ mergeStatusLabel(segment.mergeStatus) }}
                        </span>
                        <span
                            v-if="segment.similarityScore != null"
                            class="chip font-mono"
                            title="Similarity between the remote and local transcripts"
                        >
                            {{ Math.round(segment.similarityScore * 100) }}%
                        </span>
                        <span
                            v-if="getBlacklistMatches(originalIndex).length > 0"
                            class="chip-warning"
                        >
                            {{ getBlacklistMatches(originalIndex).length }} blacklist match{{
                                getBlacklistMatches(originalIndex).length > 1 ? 'es' : ''
                            }}
                        </span>
                        <span v-if="segment.reviewResolved" class="chip-success">
                            <CheckCheck aria-hidden="true" />
                            Resolved
                        </span>
                    </div>

                    <!-- Display mode -->
                    <div v-if="editingIndex !== originalIndex && splittingIndex !== originalIndex">
                        <p
                            :id="`${uid}-x-${originalIndex}`"
                            class="text-[14px] leading-[1.6] text-fg [overflow-wrap:anywhere]"
                            v-html="renderSegmentText(segment.text, originalIndex)"
                        ></p>

                        <p
                            v-if="getBlacklistMatches(originalIndex).length > 0"
                            class="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[12px] text-warning"
                        >
                            <TriangleAlert class="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                            <span class="font-medium">Blacklist</span>
                            <span class="font-mono text-[11px] text-fg-subtle">
                                {{ getBlacklistMatches(originalIndex)[0].start
                                }}{{
                                    getBlacklistMatches(originalIndex).length > 1
                                        ? ` +${getBlacklistMatches(originalIndex).length - 1}`
                                        : ''
                                }}
                            </span>
                            <span class="text-fg-muted"
                                >Matched words:
                                {{ getUniqueBlacklistWords(originalIndex).join(', ') }}</span
                            >
                        </p>

                        <div
                            v-if="segment.alternatives?.length"
                            class="mt-2 grid gap-2 @xl:grid-cols-2"
                        >
                            <div
                                v-for="source in alternativeSources"
                                :key="source"
                                class="rounded-md border p-2.5 transition-colors duration-100"
                                :class="
                                    segment.activeSource === source
                                        ? 'border-accent/45 bg-accent/[0.08]'
                                        : 'border-line bg-surface-2'
                                "
                            >
                                <div class="mb-1.5 flex items-center justify-between gap-2">
                                    <span class="eyebrow">{{ sourceLabel(source) }}</span>
                                    <button
                                        type="button"
                                        class="btn-sm"
                                        :class="
                                            segment.activeSource === source
                                                ? 'btn-subtle'
                                                : 'btn-secondary'
                                        "
                                        :disabled="!hasAlternativeText(segment, source)"
                                        :title="
                                            hasAlternativeText(segment, source)
                                                ? `Use the ${sourceLabel(source).toLowerCase()} wording`
                                                : 'This transcript has no sentence here'
                                        "
                                        @click.stop="selectAlternative(originalIndex, source)"
                                    >
                                        {{ segment.activeSource === source ? 'Selected' : 'Use' }}
                                    </button>
                                </div>
                                <p
                                    class="text-[13px] leading-relaxed"
                                    :class="
                                        hasAlternativeText(segment, source)
                                            ? 'text-fg-muted'
                                            : 'text-fg-subtle italic'
                                    "
                                >
                                    {{
                                        hasAlternativeText(segment, source)
                                            ? getAlternativeText(segment, source)
                                            : 'No matching sentence detected.'
                                    }}
                                </p>
                            </div>
                        </div>
                    </div>

                    <!-- Edit mode -->
                    <div
                        v-else-if="editingIndex === originalIndex && tempSegment"
                        class="mt-1.5 animate-pop-in space-y-3 rounded-md border border-line-strong bg-surface-2 p-3"
                        @keydown="onFormKeydown($event, saveEdit, cancelEdit)"
                    >
                        <div
                            v-if="!lockStructure"
                            class="grid grid-cols-[6.5rem_6.5rem_minmax(0,1fr)] gap-3"
                        >
                            <div>
                                <label :for="`${uid}-start-${originalIndex}`" class="field-label"
                                    >Start</label
                                >
                                <input
                                    :id="`${uid}-start-${originalIndex}`"
                                    v-model="tempSegment.start"
                                    class="input input-sm font-mono"
                                    placeholder="MM:SS"
                                />
                            </div>
                            <div>
                                <label :for="`${uid}-end-${originalIndex}`" class="field-label"
                                    >End</label
                                >
                                <input
                                    :id="`${uid}-end-${originalIndex}`"
                                    v-model="tempSegment.end"
                                    class="input input-sm font-mono"
                                    placeholder="MM:SS"
                                />
                            </div>
                            <div class="min-w-0">
                                <label :for="`${uid}-speaker-${originalIndex}`" class="field-label"
                                    >Speaker</label
                                >
                                <select
                                    v-if="!addingNewSpeaker"
                                    :id="`${uid}-speaker-${originalIndex}`"
                                    :value="tempSegment.speaker"
                                    data-testid="edit-speaker-select"
                                    class="input input-sm"
                                    @change="
                                        onSpeakerSelect(($event.target as HTMLSelectElement).value)
                                    "
                                >
                                    <option v-if="!tempSegment.speaker" value="" disabled>
                                        Select speaker
                                    </option>
                                    <option
                                        v-for="name in existingSpeakers"
                                        :key="name"
                                        :value="name"
                                    >
                                        {{ name }}
                                    </option>
                                    <option :value="NEW_SPEAKER_OPTION">Add new speaker…</option>
                                </select>
                                <div v-else class="flex gap-1.5">
                                    <input
                                        :id="`${uid}-speaker-${originalIndex}`"
                                        :ref="setNewSpeakerInput"
                                        v-model="tempSegment.speaker"
                                        data-testid="edit-speaker-input"
                                        class="input input-sm"
                                        placeholder="New speaker name"
                                    />
                                    <button
                                        type="button"
                                        class="btn-ghost btn-sm btn-icon shrink-0"
                                        aria-label="Choose an existing speaker instead"
                                        title="Choose an existing speaker instead"
                                        @click="cancelNewSpeaker"
                                    >
                                        <List aria-hidden="true" />
                                    </button>
                                </div>
                            </div>
                        </div>
                        <div>
                            <label :for="`${uid}-text-${originalIndex}`" class="field-label"
                                >Text</label
                            >
                            <textarea
                                :id="`${uid}-text-${originalIndex}`"
                                v-model="tempSegment.text"
                                rows="3"
                                class="input text-[14px]! leading-[1.6]!"
                            ></textarea>
                        </div>
                        <div class="flex items-center justify-end gap-2">
                            <span class="hint mr-auto hidden @lg:inline">
                                <kbd class="kbd">⌘</kbd> <kbd class="kbd">↵</kbd> save ·
                                <kbd class="kbd">Esc</kbd> cancel
                            </span>
                            <button
                                type="button"
                                data-testid="edit-cancel"
                                class="btn-secondary btn-sm"
                                @click="cancelEdit"
                            >
                                Cancel
                            </button>
                            <button
                                type="button"
                                data-testid="edit-save"
                                class="btn-primary btn-sm"
                                @click="saveEdit"
                            >
                                Save
                            </button>
                        </div>
                    </div>

                    <!-- Split mode -->
                    <div
                        v-else-if="splittingIndex === originalIndex && splitDraft"
                        class="mt-1.5 animate-pop-in space-y-3 rounded-md border border-line-strong bg-surface-2 p-3"
                        @keydown="onFormKeydown($event, confirmSplit, cancelSplit)"
                    >
                        <div>
                            <p class="section-title">Split segment</p>
                            <p class="hint mt-0.5">
                                Drag to the switch point, type it, or play the segment and capture
                                the playhead.
                            </p>
                        </div>

                        <!-- One slider; its thumb carries the time readout. -->
                        <div class="relative pt-7">
                            <span
                                data-testid="split-time-display"
                                class="pointer-events-none absolute top-0 -translate-x-1/2 rounded-sm bg-accent px-1.5 py-0.5 font-mono text-[11px] leading-4 text-accent-fg shadow-sm"
                                :style="{
                                    left: `calc(${splitPercent}% + ${8 - splitPercent * 0.16}px)`,
                                }"
                                >{{ formatTime(splitDraft.time) }}</span
                            >
                            <input
                                v-model.number="splitDraft.time"
                                type="range"
                                :min="splitBounds.start"
                                :max="splitBounds.end"
                                step="0.05"
                                class="block w-full"
                                aria-label="Split point"
                                data-testid="split-time-slider"
                                @input="onSplitTimeChange"
                            />
                            <div
                                class="mt-1 flex justify-between font-mono text-[11px] text-fg-subtle"
                            >
                                <span>{{ segment.start }}</span>
                                <span>{{ segment.end }}</span>
                            </div>
                        </div>

                        <div class="flex flex-wrap items-end gap-2">
                            <div>
                                <label :for="`${uid}-split-${originalIndex}`" class="field-label"
                                    >Split at (seconds)</label
                                >
                                <input
                                    :id="`${uid}-split-${originalIndex}`"
                                    v-model.number="splitDraft.time"
                                    type="number"
                                    :min="splitBounds.start"
                                    :max="splitBounds.end"
                                    step="0.05"
                                    class="input input-sm w-28 font-mono"
                                    data-testid="split-time-input"
                                    @input="onSplitTimeChange"
                                />
                            </div>
                            <button
                                v-if="getPlayhead"
                                type="button"
                                class="btn-secondary btn-sm"
                                data-testid="split-use-playhead"
                                title="Split where the audio or video player currently is"
                                @click.stop="useCurrentPlayhead"
                            >
                                <Crosshair aria-hidden="true" />
                                Use playhead
                            </button>
                        </div>

                        <div class="grid gap-3 @xl:grid-cols-2">
                            <div>
                                <label :for="`${uid}-first-${originalIndex}`" class="field-label"
                                    >First part</label
                                >
                                <textarea
                                    :id="`${uid}-first-${originalIndex}`"
                                    v-model="splitDraft.firstText"
                                    rows="3"
                                    class="input"
                                ></textarea>
                            </div>
                            <div>
                                <label :for="`${uid}-second-${originalIndex}`" class="field-label"
                                    >Second part</label
                                >
                                <textarea
                                    :id="`${uid}-second-${originalIndex}`"
                                    v-model="splitDraft.secondText"
                                    rows="3"
                                    class="input"
                                ></textarea>
                            </div>
                        </div>

                        <div class="max-w-xs">
                            <label
                                :for="`${uid}-second-speaker-${originalIndex}`"
                                class="field-label"
                                >Speaker for the second part</label
                            >
                            <select
                                v-if="!addingNewSplitSpeaker"
                                :id="`${uid}-second-speaker-${originalIndex}`"
                                :value="splitDraft.secondSpeaker"
                                data-testid="split-second-speaker"
                                class="input input-sm"
                                @change="
                                    onSplitSpeakerSelect(($event.target as HTMLSelectElement).value)
                                "
                            >
                                <option v-if="!splitDraft.secondSpeaker" value="" disabled>
                                    Select speaker
                                </option>
                                <option v-for="name in existingSpeakers" :key="name" :value="name">
                                    {{ name }}
                                </option>
                                <option :value="NEW_SPEAKER_OPTION">Add new speaker…</option>
                            </select>
                            <div v-else class="flex gap-1.5">
                                <input
                                    :id="`${uid}-second-speaker-${originalIndex}`"
                                    :ref="setNewSplitSpeakerInput"
                                    v-model="splitDraft.secondSpeaker"
                                    placeholder="New speaker name"
                                    data-testid="split-second-speaker-input"
                                    class="input input-sm"
                                />
                                <button
                                    type="button"
                                    class="btn-ghost btn-sm btn-icon shrink-0"
                                    aria-label="Choose an existing speaker instead"
                                    title="Choose an existing speaker instead"
                                    @click="cancelNewSplitSpeaker"
                                >
                                    <List aria-hidden="true" />
                                </button>
                            </div>
                        </div>

                        <div class="flex items-center justify-end gap-2">
                            <span v-if="!canConfirmSplit" class="hint mr-auto">
                                Pick a point strictly between {{ segment.start }} and
                                {{ segment.end }}.
                            </span>
                            <button
                                type="button"
                                data-testid="split-cancel"
                                class="btn-secondary btn-sm"
                                @click="cancelSplit"
                            >
                                Cancel
                            </button>
                            <button
                                type="button"
                                :disabled="!canConfirmSplit"
                                data-testid="confirm-split"
                                class="btn-primary btn-sm"
                                @click="confirmSplit"
                            >
                                <Scissors aria-hidden="true" />
                                Split
                            </button>
                        </div>
                    </div>
                </div>

                <!-- Playback: always visible, it is the main way people review. -->
                <div class="col-start-3 row-start-1 flex items-start gap-0.5 @4xl:col-start-4">
                    <button
                        v-if="audioAvailable"
                        type="button"
                        :data-testid="`segment-preview-${originalIndex}`"
                        class="btn-ghost btn-sm btn-icon"
                        :aria-pressed="previewIndex === originalIndex ? 'true' : 'false'"
                        :aria-label="
                            previewIndex === originalIndex
                                ? 'Stop audio preview'
                                : 'Play original audio for this segment'
                        "
                        :title="
                            previewIndex === originalIndex
                                ? 'Stop audio preview'
                                : 'Play original audio for this segment'
                        "
                        @click.stop="requestPreview(originalIndex)"
                    >
                        <Pause v-if="previewIndex === originalIndex" aria-hidden="true" />
                        <Volume2 v-else aria-hidden="true" />
                    </button>
                    <button
                        v-if="videoAvailable"
                        type="button"
                        :data-testid="`segment-play-video-${originalIndex}`"
                        class="btn-ghost btn-sm btn-icon"
                        :aria-pressed="videoPreviewIndex === originalIndex ? 'true' : 'false'"
                        :aria-label="
                            videoPreviewIndex === originalIndex
                                ? 'Stop video preview'
                                : 'Play this video segment'
                        "
                        :title="
                            videoPreviewIndex === originalIndex
                                ? 'Stop video preview'
                                : 'Play this video segment'
                        "
                        @click.stop="requestVideoPreview(originalIndex)"
                    >
                        <Square v-if="videoPreviewIndex === originalIndex" aria-hidden="true" />
                        <SquarePlay v-else aria-hidden="true" />
                    </button>
                    <button
                        v-if="videoAvailable"
                        type="button"
                        :data-testid="`segment-play-from-${originalIndex}`"
                        class="btn-ghost btn-sm btn-icon"
                        aria-label="Play video from here"
                        title="Play video from here"
                        @click.stop="requestPlayFrom(originalIndex)"
                    >
                        <CirclePlay aria-hidden="true" />
                    </button>
                </div>

                <!-- Editing actions. Wide editors: their own column before the
             playback buttons, faded in on hover / focus and for the current or
             selected row, with the width always reserved so nothing covers the
             text. Narrow editors: a line under the current row's text. -->
                <div
                    v-if="editingIndex !== originalIndex && splittingIndex !== originalIndex"
                    role="toolbar"
                    :aria-label="`Actions for segment at ${segment.start}`"
                    class="col-start-2 row-start-2 -ml-1.5 items-center gap-0.5 pt-1 transition-opacity duration-150 @4xl:col-start-3 @4xl:row-start-1 @4xl:ml-0 @4xl:flex @4xl:items-start @4xl:pt-0"
                    :class="[
                        activeIndex === originalIndex
                            ? 'flex animate-fade-in @4xl:animate-none'
                            : 'hidden',
                        showRowActions(originalIndex)
                            ? ''
                            : '@4xl:pointer-events-none @4xl:opacity-0 @4xl:group-focus-within:pointer-events-auto @4xl:group-focus-within:opacity-100 @4xl:group-hover:pointer-events-auto @4xl:group-hover:opacity-100',
                    ]"
                    @click.stop
                >
                    <button
                        v-if="segmentNeedsReview(segment, originalIndex) || segment.reviewResolved"
                        type="button"
                        :data-testid="`segment-mark-done-${originalIndex}`"
                        class="btn-ghost btn-sm btn-icon"
                        :class="segment.reviewResolved ? '' : 'text-success!'"
                        :aria-label="segment.reviewResolved ? 'Reopen for review' : 'Mark done'"
                        :title="
                            segment.reviewResolved
                                ? 'Return this segment to the review list'
                                : 'Mark this segment as reviewed and hide it from the review list'
                        "
                        @click.stop="toggleReviewResolved(originalIndex)"
                    >
                        <RotateCcw v-if="segment.reviewResolved" aria-hidden="true" />
                        <CheckCheck v-else aria-hidden="true" />
                    </button>
                    <button
                        type="button"
                        :data-testid="`segment-edit-${originalIndex}`"
                        class="btn-ghost btn-sm btn-icon"
                        aria-label="Edit"
                        title="Edit (Enter)"
                        @click.stop="startEditing(originalIndex)"
                    >
                        <Pencil aria-hidden="true" />
                    </button>
                    <template v-if="!lockStructure">
                        <button
                            type="button"
                            :data-testid="`segment-split-${originalIndex}`"
                            class="btn-ghost btn-sm btn-icon"
                            aria-label="Split"
                            title="Split this segment at a chosen point"
                            @click.stop="startSplitting(originalIndex)"
                        >
                            <Scissors aria-hidden="true" />
                        </button>
                        <button
                            v-if="originalIndex < segments.length - 1"
                            type="button"
                            :data-testid="`segment-merge-${originalIndex}`"
                            class="btn-ghost btn-sm btn-icon"
                            aria-label="Merge with next"
                            title="Merge with next"
                            @click.stop="mergeDown(originalIndex)"
                        >
                            <Combine aria-hidden="true" />
                        </button>
                        <span aria-hidden="true" class="mx-0.5 h-4 w-px bg-line-strong"></span>
                        <button
                            type="button"
                            :data-testid="`segment-delete-${originalIndex}`"
                            class="btn-ghost btn-sm btn-icon hover:bg-danger/15! hover:text-danger!"
                            aria-label="Delete"
                            title="Delete this segment"
                            @click.stop="deleteSegment(originalIndex)"
                        >
                            <Trash2 aria-hidden="true" />
                        </button>
                    </template>
                    <span
                        aria-hidden="true"
                        class="mx-1 mt-[5px] hidden h-4 w-px bg-line-strong @4xl:block"
                    ></span>
                </div>
            </div>

            <!-- Selection bar: floats at the bottom of the window while the
           transcript is on screen, so it never covers the search bar. -->
            <!-- The anchor sits after the last row and the bar hangs below it,
                 into the bottom padding reserved while a selection exists, so at
                 the end of the list it never covers the last row or the footer. -->
            <div class="pointer-events-none sticky bottom-[8rem] z-30 h-0">
                <div class="absolute inset-x-0 top-3 flex justify-center">
                    <Transition name="pop">
                        <div
                            v-if="selectedIndices.size > 0 && !lockStructure"
                            data-testid="selection-bar"
                            role="toolbar"
                            aria-label="Selected segments"
                            class="popover pointer-events-auto flex items-center gap-1.5 py-1.5 pr-1.5 pl-3.5"
                        >
                            <span class="text-[13px] font-medium whitespace-nowrap text-fg"
                                >{{ selectedIndices.size }} selected</span
                            >
                            <span class="hint mr-1 hidden whitespace-nowrap @2xl:inline"
                                >Shift-click rows to add or remove</span
                            >
                            <button
                                type="button"
                                data-testid="selection-merge"
                                class="btn-subtle btn-sm"
                                :disabled="selectedIndices.size < 2"
                                :title="
                                    selectedIndices.size < 2
                                        ? 'Select at least two segments to merge'
                                        : 'Merge everything from the first to the last selected segment'
                                "
                                @click="mergeSelected"
                            >
                                <Combine aria-hidden="true" />
                                Merge
                            </button>
                            <button
                                type="button"
                                data-testid="selection-delete"
                                class="btn-danger btn-sm"
                                @click="deleteSelected"
                            >
                                <Trash2 aria-hidden="true" />
                                Delete
                            </button>
                            <button
                                type="button"
                                data-testid="selection-clear"
                                class="btn-ghost btn-sm btn-icon"
                                aria-label="Clear selection"
                                title="Clear selection (Esc)"
                                @click="selectedIndices.clear()"
                            >
                                <X aria-hidden="true" />
                            </button>
                        </div>
                    </Transition>
                </div>
            </div>
        </div>

        <p
            v-if="visibleSegments.length > 0"
            class="hint flex flex-wrap items-center gap-x-3 gap-y-1 rounded-b-lg border-t border-line px-3 py-2"
        >
            <span>Click a row to focus it</span>
            <span><kbd class="kbd">↑</kbd> <kbd class="kbd">↓</kbd> move</span>
            <span><kbd class="kbd">↵</kbd> edit</span>
            <span><kbd class="kbd">Space</kbd> or Shift-click select</span>
        </p>
    </div>
</template>
