<script setup lang="ts">
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { TranscriptSegment, SilenceInterval, ViralClipsWorkspaceState } from '../types';
import { useSettings } from '../composables/useSettings';
import { trimClipBoundarySilence } from '../utils/clipSilence';

import FolderOpenIcon from '../assets/icons/folder-open.svg?component';

interface Props {
  segments: TranscriptSegment[];
  inputPath: string;
  hasMediaFile: boolean;
  state: ViralClipsWorkspaceState;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  'update:status': [message: string];
  'update:processing': [isProcessing: boolean];
  'update:state': [state: ViralClipsWorkspaceState];
}>();

const { settings } = useSettings();

const isProcessing = ref(false);
const silenceIntervalsCache = ref<SilenceInterval[] | null>(null);

function updateState(patch: Partial<ViralClipsWorkspaceState>) {
  emit('update:state', {
    ...props.state,
    ...patch,
  });
}

const clips = computed(() => props.state.clips);
const clipCount = computed({
  get: () => props.state.count,
  set: (value: number) => updateState({ count: value }),
});
const clipMinDuration = computed({
  get: () => props.state.minDuration,
  set: (value: number) => updateState({ minDuration: value }),
});
const clipMaxDuration = computed({
  get: () => props.state.maxDuration,
  set: (value: number) => updateState({ maxDuration: value }),
});
const clipTopic = computed({
  get: () => props.state.topic,
  set: (value: string) => updateState({ topic: value }),
});
const allowSplicing = computed({
  get: () => props.state.allowSplicing,
  set: (value: boolean) => updateState({ allowSplicing: value }),
});
const lastExportPath = computed(() => props.state.lastExportPath);
const trimBoundarySilence = computed({
  get: () => props.state.trimBoundarySilence,
  set: (value: boolean) => updateState({ trimBoundarySilence: value }),
});

function showError(message: string, rawResponse: string, parseError: string = "") {
  // Emit to parent for now - can be improved later
  emit('update:status', message);
  console.error("Error:", message, parseError, rawResponse);
}

async function generateClips() {
  if (props.segments.length === 0) return;

  emit('update:status', "Generating clips...");
  isProcessing.value = true;
  emit('update:processing', true);

  try {
    const transcript = props.segments
      .map(s => `[${s.start}-${s.end}] ${s.speaker}: ${s.text}`)
      .join("\n");

    const response = await invoke<string>("generate_clips", {
      apiKey: settings.value.apiKey,
      baseUrl: settings.value.baseUrl,
      model: settings.value.model,
      transcript,
      count: clipCount.value,
      minDuration: clipMinDuration.value,
      maxDuration: clipMaxDuration.value,
      topic: clipTopic.value || null,
      splicing: allowSplicing.value
    });

    const jsonMatch = response.match(/\[[\s\S]*\]/);
    if (jsonMatch) {
      try {
        const parsed = JSON.parse(jsonMatch[0]);
        if (!Array.isArray(parsed)) throw new Error("Response is not an array");

        // Normalize clips to always have 'segments'
        updateState({
          clips: parsed.map((c: any) => {
          if (c.segments) return c;
          // Backward compatibility for AI response without segments
          return {
            ...c,
            segments: [{ start: c.start, end: c.end }]
          };
          }),
        });

        emit('update:status', `Found ${parsed.length} clips.`);
      } catch (e) {
        console.error("JSON Parse Error", e);
        showError(
          "Failed to parse clips from AI response.",
          response,
          e instanceof Error ? e.message : String(e)
        );
      }
    } else {
      console.error(response);
      showError(
        "Failed to find JSON in AI response.",
        response
      );
    }
  } catch (e) {
    emit('update:status', `Error generating clips: ${e}`);
  } finally {
    isProcessing.value = false;
    emit('update:processing', false);
  }
}

async function exportClips() {
  if (clips.value.length === 0) return;
  if (!props.hasMediaFile) {
    emit('update:status', "Select a valid media file before exporting clips.");
    return;
  }

  emit('update:status', "Exporting clips...");
  isProcessing.value = true;
  emit('update:processing', true);

  try {
    // Robust extension replacement
    const outputDir = props.inputPath.replace(/\.[^/\\.]+$/, "") + "_clips";
    let clipSegments = clips.value.map(c => ({
      segments: c.segments,
      label: c.title,
      reason: c.reason
    }));

    if (trimBoundarySilence.value) {
      emit('update:status', "Detecting clip boundary silence...");

      if (!silenceIntervalsCache.value) {
        silenceIntervalsCache.value = await invoke<SilenceInterval[]>("detect_silence", {
          path: props.inputPath,
        });
      }

      clipSegments = clipSegments.map((clip) => ({
        ...clip,
        segments: trimClipBoundarySilence(clip.segments, silenceIntervalsCache.value ?? []),
      }));
    }

    emit('update:status', `Exporting to ${outputDir}...`);
    await invoke("export_clips", {
      inputPath: props.inputPath,
      segments: clipSegments,
      outputDir
    });

    updateState({ lastExportPath: outputDir });
    emit('update:status', `Clips exported to ${outputDir}`);
  } catch (e) {
    emit('update:status', `Error exporting clips: ${e}`);
  } finally {
    isProcessing.value = false;
    emit('update:processing', false);
  }
}

async function openExportFolder() {
  if (lastExportPath.value) {
    await invoke("open_folder", { path: lastExportPath.value });
  }
}
</script>

<template>
  <div class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl">
    <div class="flex justify-between items-center mb-6">
      <h2 class="text-2xl font-bold text-white">
        Viral Clips Generator
      </h2>
    </div>

    <div class="grid grid-cols-1 md:grid-cols-3 gap-6 mb-6">
      <div class="group">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Count</label>
        <input v-model.number="clipCount" type="number" min="1" max="10"
          class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white text-center" />
      </div>
      <div class="group">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Min Sec</label>
        <input v-model.number="clipMinDuration" type="number" min="5"
          class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white text-center" />
      </div>
      <div class="group">
        <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Max Sec</label>
        <input v-model.number="clipMaxDuration" type="number" min="10"
          class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white text-center" />
      </div>
    </div>

    <div class="mb-6">
      <label class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider">Topic (Optional)</label>
      <input v-model="clipTopic" type="text"
        class="w-full p-4 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white placeholder-gray-600"
        placeholder="e.g. 'Funny moments', 'Technical explanation', 'Rants'..." />
    </div>

    <div class="mb-8 flex items-center justify-between p-4 bg-black/20 rounded-xl border border-white/5">
      <div>
        <h3 class="text-sm font-semibold text-gray-300">Smart Splicing</h3>
        <p class="text-xs text-gray-500">Allow AI to combine non-contiguous segments into one clip</p>
      </div>
      <button
        @click="allowSplicing = !allowSplicing"
        class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-pink-500 focus:ring-offset-2 focus:ring-offset-gray-900"
        :class="allowSplicing ? 'bg-pink-600' : 'bg-gray-700'"
      >
        <span class="sr-only">Enable smart splicing</span>
        <span
          class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform"
          :class="allowSplicing ? 'translate-x-6' : 'translate-x-1'"
        />
      </button>
    </div>

    <button @click="generateClips" :disabled="isProcessing"
      class="w-full mb-8 bg-gradient-to-r from-pink-600 to-purple-600 hover:from-pink-500 hover:to-purple-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg transition-all transform hover:-translate-y-0.5 active:translate-y-0 disabled:opacity-50 disabled:cursor-not-allowed">
      {{ isProcessing ? 'Processing...' : 'Generate Clips' }}
    </button>

    <div v-if="clips.length > 0" class="space-y-4">
      <label class="flex items-center gap-2 cursor-pointer text-sm text-gray-400 hover:text-gray-300">
        <input v-model="trimBoundarySilence" type="checkbox"
          class="rounded bg-white/10 border-white/20 text-pink-500 focus:ring-pink-500/50" />
        Trim Start/End Silence on Export
      </label>

      <div v-for="(clip, index) in clips" :key="index"
        class="p-6 bg-black/20 rounded-2xl border border-white/5 hover:border-pink-500/30 transition-colors">
        <div class="flex justify-between items-start mb-3">
          <h3 class="font-bold text-lg text-pink-400">{{ clip.title }}</h3>
          <div class="flex flex-col items-end gap-1">
            <span v-for="(seg, i) in clip.segments" :key="i" class="px-2 py-1 rounded bg-white/5 text-xs text-gray-400 font-mono">
              {{ seg.start }} - {{ seg.end }}
            </span>
          </div>
        </div>
        <p class="text-gray-300 text-sm leading-relaxed">{{ clip.reason }}</p>
      </div>

      <div class="flex gap-4 mt-6">
        <button @click="exportClips" :disabled="isProcessing || !hasMediaFile"
          class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-bold py-4 px-6 rounded-2xl border border-gray-600 hover:border-gray-500 transition-all disabled:opacity-50 disabled:cursor-not-allowed">
          Export All Clips
        </button>
        <button v-if="lastExportPath" @click="openExportFolder"
          class="px-6 bg-gray-800 hover:bg-gray-700 text-white font-bold rounded-2xl border border-gray-700 transition-all" title="Open Folder">
          <FolderOpenIcon class="h-6 w-6" />
        </button>
      </div>
    </div>
  </div>
</template>
