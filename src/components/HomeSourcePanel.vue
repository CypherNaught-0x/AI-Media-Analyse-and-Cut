<script setup lang="ts">
import type { TranscriptionBackend } from '../types';
import FileSelector from './FileSelector.vue';
import AnalysisSettings from './AnalysisSettings.vue';
import SessionControls from './SessionControls.vue';
import LightningIcon from '../assets/icons/lightning.svg?component';
import SpinnerIcon from '../assets/icons/spinner.svg?component';

defineProps<{
  currentEngineLabel: string;
  currentModelDisplay: string;
  inputPath: string;
  hasMediaFile: boolean;
  isProcessing: boolean;
  hasBackendConfiguration: boolean;
  hasTranscript: boolean;
  settingsChanged: boolean;
  transcriptionBackend: TranscriptionBackend;
  context: string;
  glossary: string;
  speakerCount: number | null;
  removeFillerWords: boolean;
  trimSilence: boolean;
}>();

defineEmits<{
  (e: 'update:inputPath', value: string): void;
  (e: 'update:transcriptionBackend', value: TranscriptionBackend): void;
  (e: 'update:context', value: string): void;
  (e: 'update:glossary', value: string): void;
  (e: 'update:speakerCount', value: number | null): void;
  (e: 'update:removeFillerWords', value: boolean): void;
  (e: 'update:trimSilence', value: boolean): void;
  (e: 'invalid-selection', message: string): void;
  (e: 'save-session'): void;
  (e: 'load-session'): void;
  (e: 'open-settings'): void;
  (e: 'process'): void;
}>();
</script>

<template>
  <div class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl mb-8">
    <div class="mb-8 flex items-center justify-between bg-black/20 p-4 rounded-2xl border border-white/5">
      <div class="flex items-center gap-4">
        <div class="w-10 h-10 rounded-full bg-blue-500/20 flex items-center justify-center text-blue-400">
          <LightningIcon class="h-6 w-6" />
        </div>
        <div>
          <label class="block text-xs font-medium text-gray-400 uppercase tracking-wider">{{ currentEngineLabel }}</label>
          <div class="text-white font-medium">{{ currentModelDisplay }}</div>
        </div>
      </div>
      <button
        @click="$emit('open-settings')"
        class="px-6 py-2 bg-white/10 hover:bg-white/20 text-white text-sm font-medium rounded-xl transition-all border border-white/10"
      >
        Settings
      </button>
    </div>

    <FileSelector
      :modelValue="inputPath"
      @update:modelValue="$emit('update:inputPath', $event)"
      @invalid-selection="$emit('invalid-selection', $event)"
    />

    <SessionControls
      :inputPath="inputPath"
      :hasMediaFile="hasMediaFile"
      @save-session="$emit('save-session')"
      @load-session="$emit('load-session')"
    />

    <AnalysisSettings
      :transcriptionBackend="transcriptionBackend"
      :context="context"
      :glossary="glossary"
      :speakerCount="speakerCount"
      :removeFillerWords="removeFillerWords"
      :trimSilence="trimSilence"
      @update:transcriptionBackend="$emit('update:transcriptionBackend', $event)"
      @update:context="$emit('update:context', $event)"
      @update:glossary="$emit('update:glossary', $event)"
      @update:speakerCount="$emit('update:speakerCount', $event)"
      @update:removeFillerWords="$emit('update:removeFillerWords', $event)"
      @update:trimSilence="$emit('update:trimSilence', $event)"
    />

    <div class="flex gap-4 mb-6">
      <button
        @click="$emit('process')"
        :disabled="isProcessing || !hasMediaFile || !hasBackendConfiguration || (hasTranscript && !settingsChanged)"
        class="flex-1 bg-blue-600 hover:bg-blue-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg shadow-blue-900/20 disabled:opacity-50 disabled:cursor-not-allowed transition-all transform hover:-translate-y-0.5 active:translate-y-0 flex items-center justify-center gap-2"
      >
        <SpinnerIcon v-if="isProcessing" class="animate-spin h-5 w-5 text-white" />
        {{ isProcessing ? 'Processing...' : (hasTranscript && !settingsChanged ? 'Transcript Loaded' : (hasTranscript ? 'Re-analyze Media' : 'Analyze Media')) }}
      </button>
    </div>
  </div>
</template>
