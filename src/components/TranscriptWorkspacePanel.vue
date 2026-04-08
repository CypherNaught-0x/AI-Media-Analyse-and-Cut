<script setup lang="ts">
import { computed, ref } from 'vue';
import { convertFileSrc } from '@tauri-apps/api/core';
import Editor from './Editor.vue';
import SubtitleExport from './SubtitleExport.vue';
import type { TranscriptSegment } from '../types';
import { parseTime } from '../composables/useTimeFormat';
import { detectTranscriptBlacklistMatches } from '../utils/transcriptBlacklist';
import { SUPPORTED_TRANSCRIPT_LANGUAGES } from '../utils/transcriptLanguages';
import UserIcon from '../assets/icons/user.svg?component';
import TranslateIcon from '../assets/icons/translate.svg?component';
import CheckIcon from '../assets/icons/check.svg?component';
import ChevronDownIcon from '../assets/icons/chevron-down.svg?component';

const props = defineProps<{
  inputPath: string;
  hasMediaFile: boolean;
  displaySegments: TranscriptSegment[];
  originalSegments: TranscriptSegment[];
  translations: Record<string, TranscriptSegment[]>;
  currentLanguage: string;
  targetLanguage: string;
  isTranslating: boolean;
  isLlmOnlyBackend: boolean;
  useAdvancedAlignment: boolean;
  uniqueSpeakers: string[];
  isProcessing: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:currentLanguage', value: string): void;
  (e: 'update:targetLanguage', value: string): void;
  (e: 'translate'): void;
  (e: 'export-video'): void;
  (e: 'update:useAdvancedAlignment', value: boolean): void;
  (e: 'rename-speaker', payload: { oldName: string; newName: string; inputElement: HTMLInputElement }): void;
  (e: 'update:segments', value: TranscriptSegment[]): void;
}>();

const showLanguageDropdown = ref(false);
const videoRef = ref<HTMLVideoElement | null>(null);
const showOnlyReviewSegments = ref(false);
const reviewThresholdPercent = ref(85);

const segmentNeedsReview = (segment: TranscriptSegment): boolean => {
  if (segment.similarityScore !== undefined) {
    return segment.similarityScore < reviewThresholdPercent.value / 100;
  }

  return segment.mergeStatus === 'missing_google' || segment.mergeStatus === 'missing_parakeet';
};

const blacklistWarnings = computed(() =>
  detectTranscriptBlacklistMatches(props.displaySegments, props.currentLanguage),
);

const hasBlacklistWarnings = computed(() => blacklistWarnings.value.matches.length > 0);
const blacklistSegmentCount = computed(() => Object.keys(blacklistWarnings.value.matchesBySegment).length);
const previewBlacklistWords = computed(() => blacklistWarnings.value.uniqueWords.slice(0, 8));
const remainingBlacklistWordCount = computed(() =>
  Math.max(blacklistWarnings.value.uniqueWords.length - previewBlacklistWords.value.length, 0),
);

const segmentNeedsAttention = (segment: TranscriptSegment, index: number): boolean =>
  segmentNeedsReview(segment) || (blacklistWarnings.value.matchesBySegment[index]?.length ?? 0) > 0;

const hasReviewMetadata = computed(() =>
  props.displaySegments.some((segment, index) =>
    segmentNeedsAttention(segment, index) ||
    (segment.alternatives?.length ?? 0) > 0
  )
);

const reviewSegmentCount = computed(() =>
  props.displaySegments.filter((segment, index) => segmentNeedsAttention(segment, index)).length
);

function selectLanguage(langName: string) {
  emit('update:targetLanguage', langName);
  showLanguageDropdown.value = false;
  if (props.translations[langName]) {
    emit('update:currentLanguage', langName);
  }
}

function jumpTo(time: number) {
  if (!videoRef.value) return;
  videoRef.value.currentTime = time;
  void videoRef.value.play();
}

function onTimeUpdate() {
  if (!videoRef.value || props.originalSegments.length === 0) return;

  const currentTime = videoRef.value.currentTime;
  let inside = false;
  let nextStart = -1;

  for (const seg of props.originalSegments) {
    const start = parseTime(seg.start);
    const end = parseTime(seg.end);

    if (currentTime >= start && currentTime < end) {
      inside = true;
      break;
    }
    if (start > currentTime) {
      nextStart = start;
      break;
    }
  }

  if (!inside && nextStart !== -1) {
    videoRef.value.currentTime = nextStart;
  } else if (!inside && nextStart === -1) {
    const lastEnd = parseTime(props.originalSegments[props.originalSegments.length - 1].end);
    if (currentTime > lastEnd) {
      videoRef.value.pause();
    }
  }
}
</script>

<template>
  <div class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl mb-8">
    <div v-if="hasMediaFile" class="mb-8 bg-black rounded-xl overflow-hidden border border-white/10 shadow-2xl">
      <video
        ref="videoRef"
        :src="convertFileSrc(inputPath)"
        class="w-full max-h-[500px] mx-auto"
        controls
        @timeupdate="onTimeUpdate"
      ></video>
    </div>
    <div v-else class="mb-8 rounded-xl border border-amber-500/20 bg-amber-500/10 p-4 text-sm text-amber-200">
      Media preview unavailable because the saved file path no longer exists. Select the source file again to re-enable playback and export actions.
    </div>

    <div class="flex justify-between items-center mb-6">
      <div class="flex items-center gap-4">
        <h2 class="text-2xl font-bold text-white">Transcript</h2>
        <span class="px-3 py-1 rounded-full bg-white/10 text-gray-300 text-xs font-bold border border-white/10">
          {{ displaySegments.length }} Segments
        </span>
      </div>
      <div class="flex items-center gap-3">
        <div class="flex items-center gap-2 bg-black/20 rounded-lg p-1 border border-white/10">
          <select
            :value="currentLanguage"
            @change="$emit('update:currentLanguage', ($event.target as HTMLSelectElement).value)"
            class="bg-transparent text-xs text-gray-300 outline-none border-none py-1 pl-2 pr-2 cursor-pointer [&>option]:bg-gray-900"
          >
            <option value="Original">Original</option>
            <option v-for="(_, lang) in translations" :key="lang" :value="lang">{{ lang }}</option>
          </select>
        </div>

        <div class="relative">
          <div class="flex items-center gap-2">
            <button
              @click="showLanguageDropdown = !showLanguageDropdown"
              class="flex items-center gap-2 w-32 bg-white/5 border border-white/10 rounded-lg px-2 py-1.5 text-xs text-gray-300 outline-none hover:bg-white/10 transition-colors"
            >
              <span class="truncate flex-1 text-left">{{ targetLanguage || 'Select Language' }}</span>
              <ChevronDownIcon class="h-3 w-3 text-gray-500" />
            </button>

            <button
              @click="$emit('translate')"
              :disabled="isTranslating || !targetLanguage || !!translations[targetLanguage]"
              class="p-1.5 bg-blue-600/20 hover:bg-blue-600/40 text-blue-400 rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed border border-blue-500/20"
              title="Translate"
            >
              <TranslateIcon class="h-4 w-4" :class="{ 'animate-pulse': isTranslating }" />
            </button>
          </div>

          <div
            v-if="showLanguageDropdown"
            class="absolute top-full left-0 mt-1 w-48 max-h-64 overflow-y-auto bg-gray-900 border border-white/10 rounded-lg shadow-xl z-50 py-1"
          >
            <button
              v-for="lang in SUPPORTED_TRANSCRIPT_LANGUAGES"
              :key="lang.code"
              @click="selectLanguage(lang.name)"
              class="w-full px-3 py-2 text-left text-xs text-gray-300 hover:bg-white/10 flex items-center justify-between group"
            >
              <span class="flex items-center gap-2">
                <span :class="`fi fi-${lang.country}`" class="rounded-sm"></span>
                <span>{{ lang.name }}</span>
              </span>
              <CheckIcon v-if="translations[lang.name]" class="h-3 w-3 text-emerald-400" />
            </button>
          </div>

          <div v-if="showLanguageDropdown" @click="showLanguageDropdown = false" class="fixed inset-0 z-40 bg-transparent"></div>
        </div>

        <div class="w-px h-6 bg-white/10 mx-1"></div>

        <button
          @click="$emit('export-video')"
          :disabled="originalSegments.length === 0 || isProcessing || !hasMediaFile"
          class="px-4 py-1.5 bg-emerald-600/20 hover:bg-emerald-600/40 text-emerald-400 text-xs font-bold rounded-lg border border-emerald-500/20 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
          title="Export the video with the current cuts applied"
        >
          Export Video
        </button>

        <SubtitleExport :segments="displaySegments" :inputPath="inputPath" :language="currentLanguage" :disabled="!hasMediaFile" />
      </div>
    </div>

    <div
      v-if="hasBlacklistWarnings"
      data-testid="blacklist-warnings"
      class="mb-4 rounded-xl border border-amber-500/20 bg-amber-500/10 p-4"
    >
      <div class="flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
        <div>
          <h3 class="text-sm font-semibold text-amber-100">Blacklist Warnings</h3>
          <p class="text-xs text-amber-200/80">
            {{ blacklistWarnings.matches.length }} word-level matches across {{ blacklistSegmentCount }} segments using the
            {{ blacklistWarnings.languageLabel ?? blacklistWarnings.languageCode }} list.
          </p>
        </div>
        <div class="flex flex-wrap gap-2">
          <span
            v-for="word in previewBlacklistWords"
            :key="word"
            class="rounded-full border border-amber-400/20 bg-amber-500/10 px-2.5 py-1 text-[11px] font-medium text-amber-100"
          >
            {{ word }}
          </span>
          <span
            v-if="remainingBlacklistWordCount > 0"
            class="rounded-full border border-amber-400/20 bg-black/20 px-2.5 py-1 text-[11px] font-medium text-amber-100"
          >
            +{{ remainingBlacklistWordCount }} more
          </span>
        </div>
      </div>
    </div>

    <div
      v-if="hasReviewMetadata"
      class="mb-4 rounded-xl border border-white/5 bg-black/20 p-4"
    >
      <div class="flex flex-col gap-4 lg:flex-row lg:items-center lg:justify-between">
        <div>
          <h3 class="text-sm font-semibold text-gray-300">Review Filter</h3>
          <p class="text-xs text-gray-500">
            {{ reviewSegmentCount }} segments need attention because they are missing a counterpart, below {{ reviewThresholdPercent }}% similarity, or match the blacklist.
          </p>
        </div>
        <div class="flex flex-col gap-3 sm:flex-row sm:items-center">
          <label class="flex items-center gap-2 text-xs text-gray-300">
            <input
              v-model="showOnlyReviewSegments"
              data-testid="review-filter-toggle"
              type="checkbox"
              class="h-4 w-4 rounded border-white/10 bg-black/40 text-blue-500 focus:ring-blue-500/40"
            />
            Show only review items
          </label>
          <label class="flex items-center gap-3 text-xs text-gray-300">
            <span>Review below</span>
            <input
              v-model.number="reviewThresholdPercent"
              data-testid="review-filter-threshold"
              type="range"
              min="0"
              max="100"
              step="1"
              class="w-32 accent-blue-500"
            />
            <span class="w-10 text-right font-mono text-gray-400">{{ reviewThresholdPercent }}%</span>
          </label>
        </div>
      </div>
    </div>

    <div v-if="isLlmOnlyBackend" class="mb-4 p-4 bg-black/20 rounded-xl border border-white/5 flex items-center justify-between">
      <div>
        <h3 class="text-sm font-semibold text-gray-300">Advanced Alignment</h3>
        <p class="text-xs text-gray-500">Align AI transcript with local timestamps (Coming Soon)</p>
      </div>
      <button
        @click="$emit('update:useAdvancedAlignment', !useAdvancedAlignment)"
        class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-gray-900"
        :class="useAdvancedAlignment ? 'bg-blue-600' : 'bg-gray-700'"
      >
        <span class="sr-only">Enable advanced alignment</span>
        <span class="inline-block h-4 w-4 transform rounded-full bg-white transition-transform" :class="useAdvancedAlignment ? 'translate-x-6' : 'translate-x-1'" />
      </button>
    </div>

    <div v-if="uniqueSpeakers.length > 0" class="mb-6 p-4 bg-black/20 rounded-xl border border-white/5">
      <h3 class="text-sm font-semibold text-gray-300 mb-3 uppercase tracking-wider">Speakers</h3>
      <div class="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 gap-3">
        <div v-for="speaker in uniqueSpeakers" :key="speaker" class="relative group">
          <div class="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
            <UserIcon class="h-4 w-4 text-gray-500" />
          </div>
          <input
            :value="speaker"
            @change="$emit('rename-speaker', { oldName: speaker, newName: ($event.target as HTMLInputElement).value, inputElement: $event.target as HTMLInputElement })"
            class="w-full pl-9 pr-3 py-2 rounded-lg bg-white/5 border border-white/10 focus:border-blue-500/50 focus:bg-black/30 outline-none text-sm text-gray-300 transition-all"
          />
        </div>
      </div>
    </div>

    <Editor
      :segments="displaySegments"
      :showOnlyReviewSegments="showOnlyReviewSegments"
      :reviewThreshold="reviewThresholdPercent / 100"
      :blacklistMatchesBySegment="blacklistWarnings.matchesBySegment"
      @jump-to="jumpTo"
      @update:segments="$emit('update:segments', $event)"
    />
  </div>
</template>
