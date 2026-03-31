<script setup lang="ts">
import { ref } from 'vue';
import { ask } from '@tauri-apps/plugin-dialog';
import type {
  TranscriptAlternativeSource,
  TranscriptMergeStatus,
  TranscriptSegment,
  TranscriptWord
} from '../types';

const props = defineProps<{
  segments: TranscriptSegment[];
}>();

const emit = defineEmits<{
  (e: 'jump-to', time: number): void;
  (e: 'update:segments', segments: TranscriptSegment[]): void;
}>();

const editingIndex = ref<number | null>(null);
const tempSegment = ref<TranscriptSegment | null>(null);
const selectedIndices = ref<Set<number>>(new Set());
const alternativeSources: TranscriptAlternativeSource[] = ['google', 'parakeet'];

const parseTime = (timeStr: string): number => {
  const [mm, ss] = timeStr.split(':').map(Number);
  return mm * 60 + ss;
};

const jumpTo = (timeStr: string) => {
  emit('jump-to', parseTime(timeStr));
};

const handleSegmentClick = (index: number, event: MouseEvent) => {
  if (event.shiftKey) {
    if (selectedIndices.value.has(index)) {
      selectedIndices.value.delete(index);
    } else {
      selectedIndices.value.add(index);
    }
  } else {
    // If we are not selecting, just jump
    jumpTo(props.segments[index].start);
  }
};

const startEditing = (index: number) => {
  editingIndex.value = index;
  tempSegment.value = { ...props.segments[index] };
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

const selectAlternative = (index: number, source: TranscriptAlternativeSource) => {
  const segment = props.segments[index];
  const text = getAlternativeText(segment, source).trim();
  if (!text) return;
  const speaker = segment.alternatives?.find((alternative) => alternative.source === source)?.speaker?.trim();

  const newSegments = [...props.segments];
  newSegments[index] = {
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

const saveEdit = () => {
  if (editingIndex.value !== null && tempSegment.value) {
    const newSegments = [...props.segments];
    newSegments[editingIndex.value] = stripMergeMetadata(tempSegment.value);
    emit('update:segments', newSegments);
    cancelEdit();
  }
};

const deleteSegment = async (index: number) => {
  const confirmed = await ask('Are you sure you want to delete this segment?', {
    title: 'Confirm Deletion',
    kind: 'warning'
  });

  if (confirmed) {
    const newSegments = [...props.segments];
    newSegments.splice(index, 1);
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

const mergeDown = (index: number) => {
  if (index >= props.segments.length - 1) return;
  
  const current = props.segments[index];
  const next = props.segments[index + 1];
  
  const merged: TranscriptSegment = {
    start: current.start,
    end: next.end,
    speaker: current.speaker,
    text: `${current.text} ${next.text}`,
    words: mergeWords([current, next])
  };
  
  const newSegments = [...props.segments];
  newSegments.splice(index, 2, merged);
  emit('update:segments', newSegments);
};
</script>

<template>
  <div class="editor-container p-4 bg-black/20 backdrop-blur-md border border-white/10 rounded-xl overflow-y-auto max-h-[600px] relative">
    
    <!-- Multi-selection Toolbar -->
    <div v-if="selectedIndices.size > 0" class="sticky top-0 z-50 mb-4 p-2 bg-blue-600/20 backdrop-blur-md border border-blue-500/30 rounded-lg flex items-center justify-between">
        <span class="text-sm text-blue-200 font-medium px-2">{{ selectedIndices.size }} selected</span>
        <div class="flex gap-2">
            <button @click="mergeSelected" class="px-3 py-1.5 bg-purple-500/20 text-purple-300 border border-purple-500/30 rounded text-xs hover:bg-purple-500/30 transition-colors font-medium">Merge Selected</button>
            <button @click="deleteSelected" class="px-3 py-1.5 bg-red-500/20 text-red-300 border border-red-500/30 rounded text-xs hover:bg-red-500/30 transition-colors font-medium">Delete Selected</button>
            <button @click="selectedIndices.clear()" class="px-3 py-1.5 bg-white/10 text-gray-300 border border-white/10 rounded text-xs hover:bg-white/20 transition-colors">Cancel</button>
        </div>
    </div>

    <div v-for="(segment, index) in segments" :key="index" 
         class="segment mb-4 p-4 rounded-lg transition-all duration-300 group relative border"
         :class="[
            selectedIndices.has(index) ? 'bg-blue-500/20 border-blue-500/50' : 'bg-white/5 border-white/5 hover:bg-white/10 hover:border-white/20'
         ]"
         @click="handleSegmentClick(index, $event)">
      
      <!-- Display Mode -->
      <div v-if="editingIndex !== index">
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
          </div>
          <span class="font-mono text-xs bg-black/30 px-2 py-0.5 rounded text-gray-500">{{ segment.start }} - {{ segment.end }}</span>
        </div>
        <p class="text-gray-200 cursor-pointer leading-relaxed">{{ segment.text }}</p>

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
                @click.stop="selectAlternative(index, source)"
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
          <button @click.stop="startEditing(index)" class="px-2 py-1 bg-blue-500/20 text-blue-300 border border-blue-500/30 rounded text-xs hover:bg-blue-500/30 transition-colors">Edit</button>
          <button v-if="index < segments.length - 1" @click.stop="mergeDown(index)" class="px-2 py-1 bg-purple-500/20 text-purple-300 border border-purple-500/30 rounded text-xs hover:bg-purple-500/30 transition-colors" title="Merge with next">Merge ↓</button>
          <button @click.stop="deleteSegment(index)" class="px-2 py-1 bg-red-500/20 text-red-300 border border-red-500/30 rounded text-xs hover:bg-red-500/30 transition-colors">Del</button>
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
