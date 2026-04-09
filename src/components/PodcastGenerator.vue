<script setup lang="ts">
import { ref, computed, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { PodcastScript, PodcastSegment, PodcastWorkspaceState, TranscriptSegment } from '../types';
import { useSettings } from '../composables/useSettings';
import { formatDuration, calculateSegmentsDuration } from '../composables/useTimeFormat';
import { beginRun, isRunCancelled } from '../composables/useRunCancellation';

import FolderOpenIcon from '../assets/icons/folder-open.svg?component';
import SpinnerIcon from '../assets/icons/spinner.svg?component';

interface Props {
  segments: TranscriptSegment[];
  inputPath: string;
  hasMediaFile: boolean;
  state: PodcastWorkspaceState;
  cancelGeneration: number;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  'update:status': [message: string];
  'update:processing': [isProcessing: boolean];
  'update:state': [state: PodcastWorkspaceState];
}>();

const { settings } = useSettings();

const isGenerating = ref(false);
const isExporting = ref(false);
const activeRunId = ref<number | null>(null);

// Segment being edited
const editingSegmentIndex = ref<number | null>(null);
const editingField = ref<'start' | 'end' | null>(null);

// Add segment modal
const showAddSegmentModal = ref(false);

// Drag state
const draggedIndex = ref<number | null>(null);

function invalidateRun() {
  activeRunId.value = null;
  isGenerating.value = false;
  isExporting.value = false;
  emit('update:processing', false);
}

function assertActiveRun(runId: number) {
  if (activeRunId.value !== runId) {
    throw new Error('Run cancelled.');
  }
}

watch(() => props.cancelGeneration, invalidateRun);

function updateState(patch: Partial<PodcastWorkspaceState>) {
  emit('update:state', {
    ...props.state,
    ...patch,
  });
}

function clonePodcastScript(script: PodcastScript): PodcastScript {
  return {
    ...script,
    segments: script.segments.map((segment) => ({ ...segment })),
  };
}

function updatePodcastScript(mutator: (script: PodcastScript) => void) {
  if (!props.state.podcastScript) return;
  const next = clonePodcastScript(props.state.podcastScript);
  mutator(next);
  updateState({ podcastScript: next });
}

const minDurationMinutes = computed({
  get: () => props.state.minDurationMinutes,
  set: (value: number) => updateState({ minDurationMinutes: value }),
});
const maxDurationMinutes = computed({
  get: () => props.state.maxDurationMinutes,
  set: (value: number) => updateState({ maxDurationMinutes: value }),
});
const startPadding = computed({
  get: () => props.state.startPadding,
  set: (value: number) => updateState({ startPadding: value }),
});
const endPadding = computed({
  get: () => props.state.endPadding,
  set: (value: number) => updateState({ endPadding: value }),
});
const introPath = computed({
  get: () => props.state.introPath,
  set: (value: string) => updateState({ introPath: value }),
});
const outroPath = computed({
  get: () => props.state.outroPath,
  set: (value: string) => updateState({ outroPath: value }),
});
const podcastScript = computed(() => props.state.podcastScript);
const lastExportPath = computed(() => props.state.lastExportPath);

// Computed values (preview playback variables will be used in future implementation)
const minDurationSeconds = computed(() => minDurationMinutes.value * 60);
const maxDurationSeconds = computed(() => maxDurationMinutes.value * 60);

const currentDuration = computed(() => {
  if (!podcastScript.value) return 0;
  // Only count content segments, not voiceover placeholders
  const contentSegments = podcastScript.value.segments.filter(s => s.type === 'content');
  return calculateSegmentsDuration(contentSegments);
});

const voiceoverCount = computed(() => {
  if (!podcastScript.value) return 0;
  return podcastScript.value.segments.filter(s => s.type === 'voiceover').length;
});

const durationStatus = computed(() => {
  const duration = currentDuration.value;
  const min = minDurationSeconds.value;
  const max = maxDurationSeconds.value;

  if (duration < min) return 'short';
  if (duration > max) return 'long';
  return 'ok';
});

async function selectIntroFile() {
  try {
    const selected = await open({
      multiple: false,
      filters: [{
        name: 'Audio',
        extensions: ['mp3', 'wav', 'aac', 'flac', 'ogg', 'm4a']
      }]
    });
    if (selected && typeof selected === 'string') {
      introPath.value = selected;
    }
  } catch (e) {
    console.error("Failed to select intro file:", e);
  }
}

async function selectOutroFile() {
  try {
    const selected = await open({
      multiple: false,
      filters: [{
        name: 'Audio',
        extensions: ['mp3', 'wav', 'aac', 'flac', 'ogg', 'm4a']
      }]
    });
    if (selected && typeof selected === 'string') {
      outroPath.value = selected;
    }
  } catch (e) {
    console.error("Failed to select outro file:", e);
  }
}

async function generatePodcast() {
  if (props.segments.length === 0) return;

  const runId = await beginRun();
  activeRunId.value = runId;
  emit('update:status', "Generating podcast script...");
  isGenerating.value = true;
  emit('update:processing', true);

  try {
    // Format transcript for LLM
    const transcript = props.segments
      .map(s => `[${s.start}-${s.end}] ${s.speaker}: ${s.text}`)
      .join("\n");

    // Initial generation
    let response = await invoke<string>("generate_podcast", {
      runId,
      apiKey: settings.value.apiKey,
      baseUrl: settings.value.baseUrl,
      model: settings.value.model,
      transcript,
      minDuration: minDurationSeconds.value,
      maxDuration: maxDurationSeconds.value,
      context: null
    });
    assertActiveRun(runId);

    let script = parseScriptResponse(response);
    if (!script) {
      emit('update:status', "Failed to parse podcast script from AI response.");
      return;
    }

    // Validation loop - up to 3 iterations
    let iterations = 0;
    const MAX_ITERATIONS = 3;

    while (iterations < MAX_ITERATIONS) {
      const duration = calculateSegmentsDuration(script.segments);
      script.totalDuration = duration;

      if (duration >= minDurationSeconds.value && duration <= maxDurationSeconds.value) {
        // Duration is within target range
        break;
      }

      iterations++;
      emit('update:status', `Refining podcast script (iteration ${iterations})...`);

      response = await invoke<string>("refine_podcast", {
        runId,
        apiKey: settings.value.apiKey,
        baseUrl: settings.value.baseUrl,
        model: settings.value.model,
        originalTranscript: transcript,
        currentScript: JSON.stringify(script),
        currentDuration: duration,
        targetMin: minDurationSeconds.value,
        targetMax: maxDurationSeconds.value
      });
      assertActiveRun(runId);

      const refinedScript = parseScriptResponse(response);
      if (refinedScript) {
        script = refinedScript;
      } else {
        // Keep the previous script if parsing fails
        break;
      }
    }

    // Calculate final duration
    script.totalDuration = calculateSegmentsDuration(script.segments);
    assertActiveRun(runId);
    updateState({ podcastScript: script });

    emit('update:status', `Podcast script generated: "${script.title}" (${formatDuration(script.totalDuration)})`);
  } catch (e) {
    if (isRunCancelled(e)) {
      emit('update:status', 'Run cancelled.');
      return;
    }
    emit('update:status', `Error generating podcast: ${e}`);
    console.error("Podcast generation error:", e);
  } finally {
    if (activeRunId.value === runId) {
      activeRunId.value = null;
      isGenerating.value = false;
      emit('update:processing', false);
    }
  }
}

function parseScriptResponse(response: string): PodcastScript | null {
  try {
    // Try to find JSON object in response
    const jsonMatch = response.match(/\{[\s\S]*\}/);
    if (jsonMatch) {
      const parsed = JSON.parse(jsonMatch[0]);
      if (parsed.title && parsed.segments && Array.isArray(parsed.segments)) {
        return {
          title: parsed.title,
          summary: parsed.summary || "",
          segments: parsed.segments.map((s: any) => ({
            start: s.start,
            end: s.end,
            text: s.text || "",
            speaker: s.speaker,
            type: s.segment_type || s.type || 'content', // Handle both snake_case and camelCase
            includeReason: s.include_reason || s.includeReason,
            transitionNote: s.transition_note || s.transitionNote
          })),
          totalDuration: 0
        };
      }
    }
  } catch (e) {
    console.error("Failed to parse script response:", e);
  }
  return null;
}

// Segment Editor Functions
function removeSegment(index: number) {
  updatePodcastScript((script) => {
    script.segments.splice(index, 1);
    script.totalDuration = calculateSegmentsDuration(script.segments);
  });
}

function startEditing(index: number, field: 'start' | 'end') {
  editingSegmentIndex.value = index;
  editingField.value = field;
}

function finishEditing(index: number, field: 'start' | 'end', value: string) {
  // Validate time format
  const trimmed = value.trim();
  if (trimmed.match(/^\d{1,2}:\d{2}(:\d{2})?(\.\d+)?$/)) {
    updatePodcastScript((script) => {
      script.segments[index][field] = trimmed;
      script.totalDuration = calculateSegmentsDuration(script.segments);
    });
  }

  editingSegmentIndex.value = null;
  editingField.value = null;
}

// Drag and Drop
function onDragStart(index: number) {
  draggedIndex.value = index;
}

function onDragOver(e: DragEvent, index: number) {
  e.preventDefault();
  if (draggedIndex.value === null || draggedIndex.value === index) return;
  updatePodcastScript((script) => {
    const segments = script.segments;
    const draggedSegment = segments[draggedIndex.value!];
    segments.splice(draggedIndex.value!, 1);
    segments.splice(index, 0, draggedSegment);
    draggedIndex.value = index;
  });
}

function onDragEnd() {
  draggedIndex.value = null;
}

// Add Segment from Transcript
function openAddSegmentModal() {
  showAddSegmentModal.value = true;
}

function addSegmentFromTranscript(segment: TranscriptSegment) {
  const newSegment: PodcastSegment = {
    start: segment.start,
    end: segment.end,
    text: segment.text,
    speaker: segment.speaker,
    type: 'content',
    includeReason: "Manually added"
  };

  updatePodcastScript((script) => {
    script.segments.push(newSegment);
    const contentSegments = script.segments.filter(s => s.type === 'content');
    script.totalDuration = calculateSegmentsDuration(contentSegments);
  });
  showAddSegmentModal.value = false;
}

function addVoiceoverSegment(afterIndex: number) {
  const newSegment: PodcastSegment = {
    start: "00:00",
    end: "00:00",
    text: "",
    speaker: "Narrator",
    type: 'voiceover',
    transitionNote: "Add your transition text here..."
  };

  // Insert after the specified index
  updatePodcastScript((script) => {
    script.segments.splice(afterIndex + 1, 0, newSegment);
  });
}

// Check if a transcript segment is already in the podcast
function isSegmentIncluded(segment: TranscriptSegment): boolean {
  if (!podcastScript.value) return false;
  return podcastScript.value.segments.some(
    s => s.start === segment.start && s.end === segment.end
  );
}

// Export Functions
async function exportPodcast() {
  if (!podcastScript.value || podcastScript.value.segments.length === 0) return;
  if (!props.hasMediaFile) {
    emit('update:status', "Select a valid media file before exporting the podcast.");
    return;
  }

  // Check if there are any content segments
  const contentSegments = podcastScript.value.segments.filter(s => s.type === 'content');
  if (contentSegments.length === 0) {
    emit('update:status', "No content segments to export (only voiceover placeholders)");
    return;
  }

  const runId = await beginRun();
  activeRunId.value = runId;
  emit('update:status', "Exporting podcast...");
  isExporting.value = true;
  emit('update:processing', true);

  try {
    const outputPath = props.inputPath.replace(/\.[^/\\.]+$/, "_podcast.m4a");

    await invoke("export_podcast", {
      runId,
      inputPath: props.inputPath,
      segments: podcastScript.value.segments.map(s => ({
        start: s.start,
        end: s.end,
        text: s.text,
        speaker: s.speaker,
        segment_type: s.type,
        include_reason: s.includeReason,
        transition_note: s.transitionNote
      })),
      introPath: introPath.value || null,
      outroPath: outroPath.value || null,
      startPadding: startPadding.value,
      endPadding: endPadding.value,
      outputPath
    });
    assertActiveRun(runId);

    updateState({ lastExportPath: outputPath });
    emit('update:status', `Podcast exported to ${outputPath}`);
  } catch (e) {
    if (isRunCancelled(e)) {
      emit('update:status', 'Run cancelled.');
      return;
    }
    emit('update:status', `Error exporting podcast: ${e}`);
    console.error("Export error:", e);
  } finally {
    if (activeRunId.value === runId) {
      activeRunId.value = null;
      isExporting.value = false;
      emit('update:processing', false);
    }
  }
}

async function exportClips() {
  if (!podcastScript.value || podcastScript.value.segments.length === 0) return;
  if (!props.hasMediaFile) {
    emit('update:status', "Select a valid media file before exporting podcast clips.");
    return;
  }

  // Check if there are any content segments
  const contentSegments = podcastScript.value.segments.filter(s => s.type === 'content');
  if (contentSegments.length === 0) {
    emit('update:status', "No content segments to export (only voiceover placeholders)");
    return;
  }

  const runId = await beginRun();
  activeRunId.value = runId;
  emit('update:status', "Exporting podcast clips...");
  isExporting.value = true;
  emit('update:processing', true);

  try {
    const outputDir = props.inputPath.replace(/\.[^/\\.]+$/, "_podcast_clips");

    await invoke("export_podcast_clips", {
      runId,
      inputPath: props.inputPath,
      segments: podcastScript.value.segments.map(s => ({
        start: s.start,
        end: s.end,
        text: s.text,
        speaker: s.speaker,
        segment_type: s.type,
        include_reason: s.includeReason,
        transition_note: s.transitionNote
      })),
      startPadding: startPadding.value,
      endPadding: endPadding.value,
      outputDir
    });
    assertActiveRun(runId);

    updateState({ lastExportPath: outputDir });
    emit('update:status', `Podcast clips exported to ${outputDir}`);
  } catch (e) {
    if (isRunCancelled(e)) {
      emit('update:status', 'Run cancelled.');
      return;
    }
    emit('update:status', `Error exporting clips: ${e}`);
    console.error("Export clips error:", e);
  } finally {
    if (activeRunId.value === runId) {
      activeRunId.value = null;
      isExporting.value = false;
      emit('update:processing', false);
    }
  }
}

async function openExportFolder() {
  if (lastExportPath.value) {
    // Get directory from path
    const dir = lastExportPath.value.replace(/[/\\][^/\\]+$/, "");
    await invoke("open_folder", { path: dir });
  }
}
</script>

<template>
  <div class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl">
    <div class="flex justify-between items-center mb-6">
      <h2 class="text-2xl font-bold text-white">
        Podcast Generator
      </h2>
      <span v-if="podcastScript" class="px-3 py-1 rounded-full text-xs font-bold border"
        :class="{
          'bg-emerald-500/20 text-emerald-400 border-emerald-500/30': durationStatus === 'ok',
          'bg-yellow-500/20 text-yellow-400 border-yellow-500/30': durationStatus === 'short',
          'bg-red-500/20 text-red-400 border-red-500/30': durationStatus === 'long'
        }">
        {{ formatDuration(currentDuration) }}
      </span>
    </div>

    <!-- Settings -->
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6 mb-6">
      <div class="group">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Min Duration (minutes)</label>
        <input v-model.number="minDurationMinutes" type="number" min="1" max="60"
          class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-teal-500/50 outline-none text-white text-center" />
      </div>
      <div class="group">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Max Duration (minutes)</label>
        <input v-model.number="maxDurationMinutes" type="number" min="1" max="60"
          class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-teal-500/50 outline-none text-white text-center" />
      </div>
    </div>

    <!-- Intro/Outro Selection -->
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6 mb-6">
      <div class="group">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Intro Audio (Optional)</label>
        <div class="flex gap-2">
          <input :value="introPath ? introPath.split(/[/\\]/).pop() : ''" type="text" readonly
            class="flex-1 p-3 rounded-xl bg-black/20 border border-white/10 outline-none text-white placeholder-gray-600 text-sm"
            placeholder="No intro selected" />
          <button @click="selectIntroFile"
            class="px-4 py-2 bg-white/10 hover:bg-white/20 text-white rounded-xl transition-all border border-white/10">
            Browse
          </button>
          <button v-if="introPath" @click="introPath = ''"
            class="px-3 py-2 bg-red-500/20 hover:bg-red-500/30 text-red-400 rounded-xl transition-all border border-red-500/20">
            &times;
          </button>
        </div>
      </div>
      <div class="group">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Outro Audio (Optional)</label>
        <div class="flex gap-2">
          <input :value="outroPath ? outroPath.split(/[/\\]/).pop() : ''" type="text" readonly
            class="flex-1 p-3 rounded-xl bg-black/20 border border-white/10 outline-none text-white placeholder-gray-600 text-sm"
            placeholder="No outro selected" />
          <button @click="selectOutroFile"
            class="px-4 py-2 bg-white/10 hover:bg-white/20 text-white rounded-xl transition-all border border-white/10">
            Browse
          </button>
          <button v-if="outroPath" @click="outroPath = ''"
            class="px-3 py-2 bg-red-500/20 hover:bg-red-500/30 text-red-400 rounded-xl transition-all border border-red-500/20">
            &times;
          </button>
        </div>
      </div>
    </div>

    <!-- Clip Padding Settings -->
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6 mb-6">
      <div class="group">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Start Padding (seconds)</label>
        <input v-model.number="startPadding" type="number" min="0" max="10" step="0.1"
          class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-teal-500/50 outline-none text-white text-center" />
        <p class="text-xs text-gray-500 mt-1">Extra audio before each clip</p>
      </div>
      <div class="group">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">End Padding (seconds)</label>
        <input v-model.number="endPadding" type="number" min="0" max="10" step="0.1"
          class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-teal-500/50 outline-none text-white text-center" />
        <p class="text-xs text-gray-500 mt-1">Extra audio after each clip</p>
      </div>
    </div>

    <!-- Generate Button -->
    <button @click="generatePodcast" :disabled="isGenerating || isExporting"
      class="w-full mb-8 bg-gradient-to-r from-teal-600 to-cyan-600 hover:from-teal-500 hover:to-cyan-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg transition-all transform hover:-translate-y-0.5 active:translate-y-0 disabled:opacity-50 disabled:cursor-not-allowed flex items-center justify-center gap-2">
      <SpinnerIcon v-if="isGenerating" class="animate-spin h-5 w-5" />
      {{ isGenerating ? 'Generating...' : 'Generate Podcast Script' }}
    </button>

    <!-- Results -->
    <div v-if="podcastScript" class="space-y-6">
      <!-- Title & Summary -->
      <div class="p-6 bg-black/20 rounded-2xl border border-white/5">
        <h3 class="font-bold text-xl text-teal-400 mb-2">{{ podcastScript.title }}</h3>
        <p class="text-gray-400 text-sm leading-relaxed">{{ podcastScript.summary }}</p>
      </div>

      <!-- Segment List -->
      <div class="space-y-2">
        <div class="flex justify-between items-center mb-4">
          <div class="flex items-center gap-3">
            <h4 class="text-sm font-semibold text-gray-300 uppercase tracking-wider">
              Segments ({{ podcastScript.segments.length }})
            </h4>
            <span v-if="voiceoverCount > 0" class="px-2 py-0.5 rounded-full bg-purple-500/20 text-purple-400 text-xs border border-purple-500/30">
              {{ voiceoverCount }} voiceover{{ voiceoverCount > 1 ? 's' : '' }}
            </span>
          </div>
          <button @click="openAddSegmentModal"
            class="px-3 py-1 bg-teal-600/20 hover:bg-teal-600/30 text-teal-400 text-xs font-medium rounded-lg transition-all border border-teal-500/20">
            + Add Segment
          </button>
        </div>

        <div v-for="(segment, index) in podcastScript.segments" :key="index"
          class="p-4 rounded-xl border transition-colors cursor-move"
          :class="segment.type === 'voiceover'
            ? 'bg-purple-900/20 border-purple-500/30 hover:border-purple-500/50'
            : 'bg-black/20 border-white/5 hover:border-teal-500/30'"
          draggable="true"
          @dragstart="onDragStart(index)"
          @dragover="onDragOver($event, index)"
          @dragend="onDragEnd">
          <div class="flex items-start gap-4">
            <!-- Drag Handle -->
            <div class="text-gray-600 mt-1 cursor-move">
              <svg class="w-4 h-4" fill="currentColor" viewBox="0 0 24 24">
                <path d="M8 6a2 2 0 1 1-4 0 2 2 0 0 1 4 0zm0 6a2 2 0 1 1-4 0 2 2 0 0 1 4 0zm0 6a2 2 0 1 1-4 0 2 2 0 0 1 4 0zm8-12a2 2 0 1 1-4 0 2 2 0 0 1 4 0zm0 6a2 2 0 1 1-4 0 2 2 0 0 1 4 0zm0 6a2 2 0 1 1-4 0 2 2 0 0 1 4 0z"/>
              </svg>
            </div>

            <!-- Content -->
            <div class="flex-1 min-w-0">
              <div class="flex items-center gap-2 mb-2">
                <!-- Segment type badge -->
                <span v-if="segment.type === 'voiceover'" class="px-2 py-0.5 rounded bg-purple-500/20 text-purple-400 text-xs font-medium">
                  VOICEOVER
                </span>
                <span class="px-2 py-0.5 rounded text-xs font-mono"
                  :class="segment.type === 'voiceover' ? 'bg-purple-500/10 text-purple-300' : 'bg-teal-500/20 text-teal-400'">
                  {{ segment.speaker }}
                </span>

                <!-- Editable timestamps -->
                <div class="flex items-center gap-1 text-xs">
                  <input
                    v-if="editingSegmentIndex === index && editingField === 'start'"
                    :value="segment.start"
                    @blur="finishEditing(index, 'start', ($event.target as HTMLInputElement).value)"
                    @keyup.enter="finishEditing(index, 'start', ($event.target as HTMLInputElement).value)"
                    class="w-20 px-1 py-0.5 bg-black/50 border border-teal-500/50 rounded text-gray-300 font-mono text-center"
                    autofocus
                  />
                  <span v-else
                    @click="startEditing(index, 'start')"
                    class="px-2 py-0.5 rounded bg-white/5 text-gray-400 font-mono cursor-pointer hover:bg-white/10">
                    {{ segment.start }}
                  </span>
                  <span class="text-gray-600">-</span>
                  <input
                    v-if="editingSegmentIndex === index && editingField === 'end'"
                    :value="segment.end"
                    @blur="finishEditing(index, 'end', ($event.target as HTMLInputElement).value)"
                    @keyup.enter="finishEditing(index, 'end', ($event.target as HTMLInputElement).value)"
                    class="w-20 px-1 py-0.5 bg-black/50 border border-teal-500/50 rounded text-gray-300 font-mono text-center"
                    autofocus
                  />
                  <span v-else
                    @click="startEditing(index, 'end')"
                    class="px-2 py-0.5 rounded bg-white/5 text-gray-400 font-mono cursor-pointer hover:bg-white/10">
                    {{ segment.end }}
                  </span>
                </div>
              </div>

              <!-- Content segment: show transcript text -->
              <template v-if="segment.type === 'content'">
                <p class="text-gray-300 text-sm leading-relaxed line-clamp-2">{{ segment.text }}</p>
                <p v-if="segment.includeReason" class="mt-1 text-gray-500 text-xs italic">
                  {{ segment.includeReason }}
                </p>
              </template>

              <!-- Voiceover segment: show transition note -->
              <template v-else>
                <div class="flex items-start gap-2">
                  <svg class="w-4 h-4 text-purple-400 mt-0.5 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 11a7 7 0 01-7 7m0 0a7 7 0 01-7-7m7 7v4m0 0H8m4 0h4m-4-8a3 3 0 01-3-3V5a3 3 0 116 0v6a3 3 0 01-3 3z" />
                  </svg>
                  <p class="text-purple-300 text-sm leading-relaxed italic">
                    "{{ segment.transitionNote || segment.text }}"
                  </p>
                </div>
                <p class="mt-1 text-purple-500/70 text-xs">
                  Suggested voiceover text for transition (not exported - for future TTS or recording)
                </p>
              </template>
            </div>

            <!-- Action Buttons -->
            <div class="flex flex-col gap-1">
              <!-- Remove Button -->
              <button @click="removeSegment(index)"
                class="p-1.5 text-gray-500 hover:text-red-400 hover:bg-red-500/10 rounded-lg transition-colors"
                title="Remove segment">
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>

              <!-- Add Voiceover Button (only show for content segments, not after last segment) -->
              <button
                v-if="segment.type === 'content' && index < podcastScript.segments.length - 1"
                @click="addVoiceoverSegment(index)"
                class="p-1.5 text-gray-500 hover:text-purple-400 hover:bg-purple-500/10 rounded-lg transition-colors"
                title="Add voiceover transition after this segment">
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 11a7 7 0 01-7 7m0 0a7 7 0 01-7-7m7 7v4m0 0H8m4 0h4m-4-8a3 3 0 01-3-3V5a3 3 0 116 0v6a3 3 0 01-3 3z" />
                </svg>
              </button>
            </div>
          </div>
        </div>
      </div>

      <!-- Export Buttons -->
      <div class="flex gap-4 mt-6">
        <button @click="exportPodcast" :disabled="isExporting || !hasMediaFile"
          class="flex-1 bg-teal-600 hover:bg-teal-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg transition-all disabled:opacity-50 disabled:cursor-not-allowed flex items-center justify-center gap-2">
          <SpinnerIcon v-if="isExporting" class="animate-spin h-5 w-5" />
          Export Podcast
        </button>
        <button @click="exportClips" :disabled="isExporting || !hasMediaFile"
          class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-bold py-4 px-6 rounded-2xl border border-gray-600 transition-all disabled:opacity-50 disabled:cursor-not-allowed">
          Export Clips
        </button>
        <button v-if="lastExportPath" @click="openExportFolder"
          class="px-6 bg-gray-800 hover:bg-gray-700 text-white font-bold rounded-2xl border border-gray-700 transition-all" title="Open Folder">
          <FolderOpenIcon class="h-6 w-6" />
        </button>
      </div>
    </div>
  </div>

  <!-- Add Segment Modal -->
  <transition name="fade">
    <div v-if="showAddSegmentModal" class="fixed inset-0 z-[100] flex items-center justify-center p-4 bg-black/80 backdrop-blur-sm">
      <div class="w-full max-w-3xl max-h-[80vh] bg-gray-900 rounded-2xl border border-teal-500/30 shadow-2xl flex flex-col overflow-hidden">
        <!-- Header -->
        <div class="flex items-center justify-between p-6 border-b border-white/10">
          <h3 class="text-xl font-bold text-white">Add Segment from Transcript</h3>
          <button @click="showAddSegmentModal = false" class="p-2 hover:bg-white/10 rounded-lg transition-colors">
            <svg class="w-5 h-5 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <!-- Segment List -->
        <div class="flex-1 overflow-y-auto p-6 space-y-2">
          <div v-for="(segment, index) in segments" :key="index"
            class="p-4 rounded-xl border transition-colors cursor-pointer"
            :class="isSegmentIncluded(segment)
              ? 'bg-teal-500/10 border-teal-500/30 cursor-not-allowed opacity-50'
              : 'bg-black/20 border-white/5 hover:border-teal-500/30'"
            @click="!isSegmentIncluded(segment) && addSegmentFromTranscript(segment)">
            <div class="flex items-start gap-3">
              <span class="px-2 py-0.5 rounded bg-white/10 text-gray-400 text-xs font-mono">
                {{ segment.start }} - {{ segment.end }}
              </span>
              <span class="px-2 py-0.5 rounded bg-blue-500/20 text-blue-400 text-xs">
                {{ segment.speaker }}
              </span>
              <span v-if="isSegmentIncluded(segment)" class="px-2 py-0.5 rounded bg-teal-500/20 text-teal-400 text-xs">
                Already included
              </span>
            </div>
            <p class="mt-2 text-gray-300 text-sm line-clamp-2">{{ segment.text }}</p>
          </div>
        </div>
      </div>
    </div>
  </transition>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.line-clamp-2 {
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
</style>
