<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import { ask } from '@tauri-apps/plugin-dialog';
import type {
  TranscriptAlternativeSource,
  TranscriptMergeStatus,
  TranscriptSegment,
  TranscriptWord
} from '../types';
import type { TranscriptBlacklistMatch } from '../utils/transcriptBlacklist';

const props = withDefaults(defineProps<{
  segments: TranscriptSegment[];
  showOnlyReviewSegments?: boolean;
  reviewThreshold?: number;
  blacklistMatchesBySegment?: Record<number, TranscriptBlacklistMatch[]>;
  speakerVisibility?: Record<string, boolean>;
}>(), {
  showOnlyReviewSegments: false,
  reviewThreshold: 0.85,
  blacklistMatchesBySegment: () => ({}),
  speakerVisibility: () => ({})
});

const emit = defineEmits<{
  (e: 'jump-to', time: number): void;
  (e: 'update:segments', segments: TranscriptSegment[]): void;
}>();

const editingIndex = ref<number | null>(null);
const tempSegment = ref<TranscriptSegment | null>(null);
const selectedIndices = ref<Set<number>>(new Set());
const alternativeSources: TranscriptAlternativeSource[] = ['google', 'parakeet'];
const searchQuery = ref('');
const replaceQuery = ref('');
const wholeWordMatch = ref(false);
const currentMatchIndex = ref(0);
const segmentRefs = new Map<number, HTMLDivElement>();

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
  if (segment.similarityScore !== undefined) {
    return (
      segment.similarityScore < reviewThreshold.value ||
      getBlacklistMatches(originalIndex).length > 0
    );
  }

  return (
    segment.mergeStatus === 'missing_google' ||
    segment.mergeStatus === 'missing_parakeet' ||
    getBlacklistMatches(originalIndex).length > 0
  );
};

const visibleSegments = computed(() =>
  props.segments
    .map((segment, originalIndex) => ({ segment, originalIndex }))
    .filter(({ segment }) => props.speakerVisibility[segment.speaker] ?? true)
    .filter(({ segment, originalIndex }) => !props.showOnlyReviewSegments || segmentNeedsReview(segment, originalIndex))
);

watch(visibleSegments, (segments) => {
  const visibleIndices = new Set(segments.map(({ originalIndex }) => originalIndex));

  selectedIndices.value = new Set(
    Array.from(selectedIndices.value).filter((index) => visibleIndices.has(index))
  );

  if (editingIndex.value !== null && !visibleIndices.has(editingIndex.value)) {
    cancelEdit();
  }
});

const escapeRegExp = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

const escapeHtml = (value: string): string => value
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
      end: match.index + match[0].length
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
        end: range.end
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
  searchMatches.value.length > 0 ? searchMatches.value[currentMatchIndex.value] : null
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
        behavior: 'smooth'
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

const parseTime = (timeStr: string): number => {
  const [mm, ss] = timeStr.split(':').map(Number);
  return mm * 60 + ss;
};

const jumpTo = (timeStr: string) => {
  emit('jump-to', parseTime(timeStr));
};

const handleSegmentClick = (originalIndex: number, event: MouseEvent) => {
  if (event.shiftKey) {
    if (selectedIndices.value.has(originalIndex)) {
      selectedIndices.value.delete(originalIndex);
    } else {
      selectedIndices.value.add(originalIndex);
    }
  } else {
    // If we are not selecting, just jump
    jumpTo(props.segments[originalIndex].start);
  }
};

const startEditing = (originalIndex: number) => {
  editingIndex.value = originalIndex;
  tempSegment.value = { ...props.segments[originalIndex] };
};

const stripMergeMetadata = (segment: TranscriptSegment): TranscriptSegment => ({
  ...segment,
  alternatives: undefined,
  mergeStatus: undefined,
  activeSource: undefined,
  similarityScore: undefined
});

const mergeWords = (segmentsToMerge: TranscriptSegment[]): TranscriptWord[] | undefined => {
  const mergedWords = segmentsToMerge.flatMap((segment) => segment.words ?? []);
  return mergedWords.length > 0 ? mergedWords : undefined;
};

const getAlternativeText = (segment: TranscriptSegment, source: TranscriptAlternativeSource): string => {
  return segment.alternatives?.find((alternative) => alternative.source === source)?.text ?? '';
};

const hasAlternativeText = (segment: TranscriptSegment, source: TranscriptAlternativeSource): boolean => {
  return getAlternativeText(segment, source).trim().length > 0;
};

const sourceLabel = (source: TranscriptAlternativeSource): string => {
  return source === 'google' ? 'Google' : 'Parakeet';
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
          ? 'rounded bg-teal-300 px-0.5 text-gray-950'
          : 'rounded bg-amber-300/70 px-0.5 text-gray-950'
      }">${escapeHtml(text.slice(match.start, match.end))}</mark>`
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
  if (status === 'missing_google') return 'Missing In Google';
  if (status === 'missing_parakeet') return 'Missing In Parakeet';
  if (status === 'conflict') return 'Review Needed';
  return 'Aligned';
};

const mergeStatusClass = (status?: TranscriptMergeStatus): string => {
  if (status === 'missing_google') return 'bg-rose-500/15 text-rose-200 border-rose-500/30';
  if (status === 'missing_parakeet') return 'bg-amber-500/15 text-amber-200 border-amber-500/30';
  if (status === 'conflict') return 'bg-orange-500/15 text-orange-200 border-orange-500/30';
  return 'bg-emerald-500/15 text-emerald-200 border-emerald-500/30';
};

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
  const speaker = segment.alternatives?.find((alternative) => alternative.source === source)?.speaker?.trim();

  const newSegments = [...props.segments];
  newSegments[originalIndex] = {
    ...segment,
    text,
    speaker: speaker || segment.speaker,
    activeSource: source
  };
  emit('update:segments', newSegments);
};

const cancelEdit = () => {
  editingIndex.value = null;
  tempSegment.value = null;
};

const emitUpdatedSegments = (updates: Array<{ originalIndex: number; text: string }>) => {
  if (updates.length === 0) return;

  const newSegments = [...props.segments];

  for (const update of updates) {
    newSegments[update.originalIndex] = stripMergeMetadata({
      ...props.segments[update.originalIndex],
      text: update.text
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
        () => replaceQuery.value
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

const deleteSegment = async (originalIndex: number) => {
  const confirmed = await ask('Are you sure you want to delete this segment?', {
    title: 'Confirm Deletion',
    kind: 'warning'
  });

  if (confirmed) {
    const newSegments = [...props.segments];
    newSegments.splice(originalIndex, 1);
    emit('update:segments', newSegments);
  }
};

const deleteSelected = async () => {
  const confirmed = await ask(`Are you sure you want to delete ${selectedIndices.value.size} segments?`, {
    title: 'Confirm Deletion',
    kind: 'warning'
  });

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
  const indices = Array.from(selectedIndices.value).sort((a, b) => a - b);
  if (indices.length < 2) return;

  // Check for contiguity (optional, but good for sanity)
  // For now, we just merge everything between first and last selected?
  // Or just the selected ones.
  
  const first = props.segments[indices[0]];
  const last = props.segments[indices[indices.length - 1]];
  
  const mergedText = indices.map(i => props.segments[i].text).join(' ');
  
  const merged: TranscriptSegment = {
    start: first.start,
    end: last.end,
    speaker: first.speaker,
    text: mergedText,
    words: mergeWords(indices.map((i) => props.segments[i]))
  };
  
  const newSegments = [...props.segments];
  // Remove in reverse order
  for (let i = indices.length - 1; i >= 0; i--) {
    newSegments.splice(indices[i], 1);
  }
  // Insert at first index
  newSegments.splice(indices[0], 0, merged);
  
  emit('update:segments', newSegments);
  selectedIndices.value.clear();
};

const mergeDown = (originalIndex: number) => {
  if (originalIndex >= props.segments.length - 1) return;
  
  const current = props.segments[originalIndex];
  const next = props.segments[originalIndex + 1];
  
  const merged: TranscriptSegment = {
    start: current.start,
    end: next.end,
    speaker: current.speaker,
    text: `${current.text} ${next.text}`,
    words: mergeWords([current, next])
  };
  
  const newSegments = [...props.segments];
  newSegments.splice(originalIndex, 2, merged);
  emit('update:segments', newSegments);
};
</script>

<template>
  <div class="editor-container p-4 bg-black/20 backdrop-blur-md border border-white/10 rounded-xl overflow-y-auto max-h-[600px] relative">
    <div class="mb-4 rounded-lg border border-white/10 bg-black/30 p-3">
      <div class="grid gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]">
        <input
          v-model="searchQuery"
          data-testid="editor-search-input"
          type="text"
          placeholder="Search transcript"
          class="w-full rounded-lg border border-white/10 bg-black/40 px-3 py-2 text-sm text-white outline-none transition-all focus:border-blue-500/50 focus:ring-1 focus:ring-blue-500/50"
          @keydown.enter.prevent="goToNextMatch"
        >
        <input
          v-model="replaceQuery"
          data-testid="editor-replace-input"
          type="text"
          placeholder="Replace with"
          class="w-full rounded-lg border border-white/10 bg-black/40 px-3 py-2 text-sm text-white outline-none transition-all focus:border-blue-500/50 focus:ring-1 focus:ring-blue-500/50"
        >
        <label class="flex items-center gap-2 rounded-lg border border-white/10 bg-black/20 px-3 py-2 text-xs text-gray-300">
          <input
            v-model="wholeWordMatch"
            data-testid="editor-whole-word-toggle"
            type="checkbox"
            class="h-4 w-4 rounded border-white/10 bg-black/40 text-blue-500 focus:ring-blue-500/40"
          >
          Whole word
        </label>
      </div>

      <div class="mt-3 flex flex-wrap items-center justify-between gap-3">
        <span class="text-xs text-gray-400">{{ searchStatusLabel }}</span>
        <div class="flex flex-wrap gap-2">
          <button
            data-testid="editor-search-prev"
            class="rounded border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-gray-200 transition-colors hover:bg-white/10 disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="searchMatches.length === 0"
            @click="goToPreviousMatch"
          >
            Prev
          </button>
          <button
            data-testid="editor-search-next"
            class="rounded border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-gray-200 transition-colors hover:bg-white/10 disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="searchMatches.length === 0"
            @click="goToNextMatch"
          >
            Next
          </button>
          <button
            data-testid="editor-replace-current"
            class="rounded border border-emerald-500/30 bg-emerald-500/15 px-3 py-1.5 text-xs text-emerald-200 transition-colors hover:bg-emerald-500/25 disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="searchMatches.length === 0 || editingIndex !== null"
            @click="replaceCurrentMatch"
          >
            Replace
          </button>
          <button
            data-testid="editor-replace-all"
            class="rounded border border-blue-500/30 bg-blue-500/15 px-3 py-1.5 text-xs text-blue-200 transition-colors hover:bg-blue-500/25 disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="searchMatches.length === 0 || editingIndex !== null"
            @click="replaceAllMatches"
          >
            Replace All
          </button>
        </div>
      </div>
    </div>

    <!-- Multi-selection Toolbar -->
    <div v-if="selectedIndices.size > 0" class="sticky top-0 z-50 mb-4 p-2 bg-blue-600/20 backdrop-blur-md border border-blue-500/30 rounded-lg flex items-center justify-between">
        <span class="text-sm text-blue-200 font-medium px-2">{{ selectedIndices.size }} selected</span>
        <div class="flex gap-2">
            <button @click="mergeSelected" class="px-3 py-1.5 bg-purple-500/20 text-purple-300 border border-purple-500/30 rounded text-xs hover:bg-purple-500/30 transition-colors font-medium">Merge Selected</button>
            <button @click="deleteSelected" class="px-3 py-1.5 bg-red-500/20 text-red-300 border border-red-500/30 rounded text-xs hover:bg-red-500/30 transition-colors font-medium">Delete Selected</button>
            <button @click="selectedIndices.clear()" class="px-3 py-1.5 bg-white/10 text-gray-300 border border-white/10 rounded text-xs hover:bg-white/20 transition-colors">Cancel</button>
        </div>
    </div>

    <div v-for="{ segment, originalIndex } in visibleSegments" :key="`${originalIndex}-${segment.start}-${segment.end}`" 
         :ref="(element) => setSegmentRef(originalIndex, element as HTMLDivElement | null)"
         class="segment mb-4 p-4 rounded-lg transition-all duration-300 group relative border"
         :class="[
            selectedIndices.has(originalIndex)
              ? 'bg-blue-500/20 border-blue-500/50'
              : isCurrentSearchSegment(originalIndex)
                ? 'bg-teal-500/10 border-teal-500/40'
                : isSearchResultSegment(originalIndex)
                  ? 'bg-amber-500/10 border-amber-500/30'
                  : 'bg-white/5 border-white/5 hover:bg-white/10 hover:border-white/20'
         ]"
         @click="handleSegmentClick(originalIndex, $event)">
      
      <!-- Display Mode -->
      <div v-if="editingIndex !== originalIndex">
        <div class="flex justify-between text-sm text-gray-400 mb-2 cursor-pointer">
          <div class="flex items-center gap-2">
            <span class="font-bold text-blue-400">{{ segment.speaker }}</span>
            <span
              v-if="segment.mergeStatus"
              class="inline-flex items-center rounded-full border px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide"
              :class="mergeStatusClass(segment.mergeStatus)"
            >
              {{ mergeStatusLabel(segment.mergeStatus) }}
            </span>
            <span
              v-if="segment.similarityScore !== undefined"
              class="text-[10px] text-gray-500"
            >
              {{ Math.round(segment.similarityScore * 100) }}%
            </span>
            <span
              v-if="getBlacklistMatches(originalIndex).length > 0"
              class="inline-flex items-center rounded-full border border-amber-500/30 bg-amber-500/15 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-amber-100"
            >
              {{ getBlacklistMatches(originalIndex).length }} blacklist match{{ getBlacklistMatches(originalIndex).length > 1 ? 'es' : '' }}
            </span>
          </div>
          <span class="font-mono text-xs bg-black/30 px-2 py-0.5 rounded text-gray-500">{{ segment.start }} - {{ segment.end }}</span>
        </div>
        <p class="text-gray-200 cursor-pointer leading-relaxed" v-html="renderSegmentText(segment.text, originalIndex)"></p>

        <div
          v-if="getBlacklistMatches(originalIndex).length > 0"
          class="mt-3 rounded-lg border border-amber-500/20 bg-amber-500/10 p-3"
        >
          <div class="flex items-center justify-between gap-3">
            <span class="text-xs font-semibold uppercase tracking-wide text-amber-100">Blacklist Warning</span>
            <span class="text-[11px] text-amber-200/80">
              {{ getBlacklistMatches(originalIndex)[0].start }}{{ getBlacklistMatches(originalIndex).length > 1 ? ` +${getBlacklistMatches(originalIndex).length - 1}` : '' }}
            </span>
          </div>
          <p class="mt-1 text-sm text-amber-100">
            Matched words: {{ getUniqueBlacklistWords(originalIndex).join(', ') }}
          </p>
        </div>

        <div v-if="segment.alternatives?.length" class="mt-3 grid gap-2 md:grid-cols-2">
          <div
            v-for="source in alternativeSources"
            :key="source"
            class="rounded-lg border p-3"
            :class="segment.activeSource === source ? 'border-blue-500/40 bg-blue-500/10' : 'border-white/10 bg-black/20'"
          >
            <div class="mb-2 flex items-center justify-between gap-2">
              <span class="text-xs font-semibold uppercase tracking-wide text-gray-300">{{ sourceLabel(source) }}</span>
              <button
                class="rounded border px-2 py-1 text-[11px] transition-colors"
                :class="hasAlternativeText(segment, source)
                  ? (segment.activeSource === source ? 'border-blue-500/40 bg-blue-500/15 text-blue-200' : 'border-white/10 bg-white/5 text-gray-200 hover:bg-white/10')
                  : 'border-white/5 bg-white/5 text-gray-500 cursor-not-allowed'"
                :disabled="!hasAlternativeText(segment, source)"
                @click.stop="selectAlternative(originalIndex, source)"
              >
                {{ segment.activeSource === source ? 'Selected' : 'Use' }}
              </button>
            </div>
            <p class="text-sm leading-relaxed" :class="hasAlternativeText(segment, source) ? 'text-gray-200' : 'text-gray-500 italic'">
              {{ hasAlternativeText(segment, source) ? getAlternativeText(segment, source) : 'No matching sentence detected.' }}
            </p>
          </div>
        </div>
        
        <!-- Action Toolbar -->
        <div class="absolute top-2 right-2 hidden group-hover:flex gap-2 bg-black/60 backdrop-blur-md p-1.5 rounded-lg border border-white/10 shadow-xl">
          <button @click.stop="startEditing(originalIndex)" class="px-2 py-1 bg-blue-500/20 text-blue-300 border border-blue-500/30 rounded text-xs hover:bg-blue-500/30 transition-colors">Edit</button>
          <button v-if="originalIndex < segments.length - 1" @click.stop="mergeDown(originalIndex)" class="px-2 py-1 bg-purple-500/20 text-purple-300 border border-purple-500/30 rounded text-xs hover:bg-purple-500/30 transition-colors" title="Merge with next">Merge ↓</button>
          <button @click.stop="deleteSegment(originalIndex)" class="px-2 py-1 bg-red-500/20 text-red-300 border border-red-500/30 rounded text-xs hover:bg-red-500/30 transition-colors">Del</button>
        </div>
      </div>

      <!-- Edit Mode -->
      <div v-else-if="tempSegment" class="space-y-4 bg-black/40 p-4 rounded-lg border border-white/10">
        <div class="flex gap-4">
            <div class="flex flex-col gap-1.5">
                <label class="text-xs font-medium text-gray-400">Start</label>
                <input v-model="tempSegment.start" class="w-24 bg-black/40 border border-white/10 rounded-lg px-3 py-1.5 text-sm text-white focus:border-blue-500/50 focus:ring-1 focus:ring-blue-500/50 outline-none transition-all" placeholder="MM:SS">
            </div>
            <div class="flex flex-col gap-1.5">
                <label class="text-xs font-medium text-gray-400">End</label>
                <input v-model="tempSegment.end" class="w-24 bg-black/40 border border-white/10 rounded-lg px-3 py-1.5 text-sm text-white focus:border-blue-500/50 focus:ring-1 focus:ring-blue-500/50 outline-none transition-all" placeholder="MM:SS">
            </div>
            <div class="flex flex-col gap-1.5 flex-1">
                <label class="text-xs font-medium text-gray-400">Speaker</label>
                <input v-model="tempSegment.speaker" class="w-full bg-black/40 border border-white/10 rounded-lg px-3 py-1.5 text-sm text-white focus:border-blue-500/50 focus:ring-1 focus:ring-blue-500/50 outline-none transition-all" placeholder="Speaker Name">
            </div>
        </div>
        <div class="flex flex-col gap-1.5">
            <label class="text-xs font-medium text-gray-400">Content</label>
            <textarea v-model="tempSegment.text" rows="3" class="w-full bg-black/40 border border-white/10 rounded-lg px-3 py-1.5 text-sm text-white resize-none focus:border-blue-500/50 focus:ring-1 focus:ring-blue-500/50 outline-none transition-all"></textarea>
        </div>
        <div class="flex justify-end gap-3 pt-2">
            <button @click="cancelEdit" class="px-4 py-1.5 bg-white/5 border border-white/10 rounded-lg text-sm text-gray-300 hover:bg-white/10 transition-colors">Cancel</button>
            <button @click="saveEdit" class="px-4 py-1.5 bg-emerald-500/20 border border-emerald-500/30 rounded-lg text-sm text-emerald-300 hover:bg-emerald-500/30 transition-colors font-medium">Save Changes</button>
        </div>
      </div>

    </div>
  </div>
</template>

<style scoped>
/* Custom scrollbar if needed */
.editor-container::-webkit-scrollbar {
  width: 8px;
}
.editor-container::-webkit-scrollbar-track {
  background: #1f2937; 
}
.editor-container::-webkit-scrollbar-thumb {
  background: #4b5563; 
  border-radius: 4px;
}
.editor-container::-webkit-scrollbar-thumb:hover {
  background: #6b7280; 
}
</style>
