<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { save } from '@tauri-apps/plugin-dialog';
import { ref, computed } from 'vue';
import type { TranscriptSegment } from '../types';
import DownloadIcon from '../assets/icons/download.svg?component';
import { 
  processSubtitlesForDisplay, 
  type SubtitleSplitOptions,
  type SubtitleValidationError 
} from '../utils/subtitleValidation';
import { formatSubtitleTimeRange } from '../utils/subtitle';
import { remapSegmentsToCutTimeline } from '../utils/subtitleTimeline';

const props = defineProps<{
  segments: TranscriptSegment[];
  inputPath: string;
  /**
   * The segment list "Export Video" cuts with. Subtitles exported on the cut
   * timeline are re-timed against it; defaults to the displayed segments.
   */
  cutSegments?: TranscriptSegment[];
  language?: string;
  disabled?: boolean;
}>();

type SubtitleTimeline = 'source' | 'cut';

const timeline = ref<SubtitleTimeline>('source');
const status = ref("");
const showValidationPanel = ref(false);
const validationErrors = ref<SubtitleValidationError[]>([]);
const splitOptions = ref<SubtitleSplitOptions>({
  maxCharsPerLine: 42,
  maxLines: 2,
  minDurationMs: 500,
  maxDurationMs: 7000,
  minCps: 8,
  maxCps: 25,
});

const hasErrors = computed(() => validationErrors.value.some(e => e.severity === 'error'));
const hasWarnings = computed(() => validationErrors.value.some(e => e.severity === 'warning'));

// Timestamps to export: the source timeline as-is, or re-timed for the cut
// export, whose first cue starts at 00:00 because it contains only the
// transcript segments.
const exportSegments = computed(() =>
  timeline.value === 'cut'
    ? remapSegmentsToCutTimeline(props.segments, props.cutSegments ?? props.segments)
    : props.segments
);

function validateAndShow() {
  const { errors } = processSubtitlesForDisplay(exportSegments.value, splitOptions.value);
  validationErrors.value = errors;
  showValidationPanel.value = true;
}

async function exportSubtitles(format: 'srt' | 'vtt' | 'txt', manualSave: boolean = false) {
    if (props.segments.length === 0 || props.disabled) return;

    // Process segments for comfortable display (split long entries)
    const { segments: processedSegments, errors } = processSubtitlesForDisplay(
        exportSegments.value,
        splitOptions.value
    );

    // Update validation state
    validationErrors.value = errors;

    try {
        let content = "";
        // Robustly remove extension
        const baseName = props.inputPath.replace(/\.[^/\\.]+$/, "");
        // Match the cut video's file name (`<base>_cut.mp4`) so players pick the
        // subtitle file up automatically next to it.
        const timelineSuffix = timeline.value === 'cut' ? '_cut' : '';
        let suffix = props.language && props.language !== 'Original' ? `.${props.language}` : '';
        let outputPath = `${baseName}${timelineSuffix}${suffix}.${format}`;
        
        if (format === 'srt') {
            content = processedSegments.map((s, i) => {
                const { start, end } = formatSubtitleTimeRange(s.start, s.end, ',');
                // Handle multi-line text properly
                const text = s.text.split('\n').join('\n');
                return `${i + 1}\n${start} --> ${end}\n${s.speaker}: ${text}\n`;
            }).join('\n');
        } else if (format === 'vtt') {
            content = "WEBVTT\n\n" + processedSegments.map((s) => {
                const { start, end } = formatSubtitleTimeRange(s.start, s.end, '.');
                // Convert newlines to <br> for VTT
                const text = s.text.split('\n').join('<br>');
                return `${start} --> ${end}\n<v ${s.speaker}>${text}`;
            }).join('\n\n');
        } else {
            content = processedSegments.map(s => `[${s.start} - ${s.end}] ${s.speaker}: ${s.text}`).join('\n');
        }

        if (manualSave) {
            const saved = await save({
                defaultPath: outputPath,
                filters: [{
                    name: format.toUpperCase(),
                    extensions: [format]
                }]
            });
            if (!saved) return;
            outputPath = saved;
        }
        
        await invoke("write_text_file", { path: outputPath, content });
        status.value = timeline.value === 'cut'
            ? `Exported ${format.toUpperCase()} (cut timeline)`
            : `Exported ${format.toUpperCase()}`;
        setTimeout(() => status.value = "", 3000);
    } catch (e) {
        console.error(e);
        status.value = `Error: ${e}`;
    }
}
</script>

<template>
    <div class="flex flex-col gap-2">
        <div class="flex flex-wrap items-center gap-2">
            <div class="flex rounded-lg bg-white/5 border border-white/10 overflow-hidden">
                <button @click="exportSubtitles('srt')" :disabled="disabled" class="px-3 py-1.5 hover:bg-white/10 text-xs text-gray-300 transition-colors border-r border-white/10 disabled:opacity-50 disabled:cursor-not-allowed focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none">SRT</button>
                <button @click="exportSubtitles('srt', true)" :disabled="disabled" class="px-2 py-1.5 hover:bg-white/10 text-gray-300 transition-colors disabled:opacity-50 disabled:cursor-not-allowed focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none" title="Save SRT as...">
                    <DownloadIcon class="h-3 w-3" />
                </button>
            </div>
            <div class="flex rounded-lg bg-white/5 border border-white/10 overflow-hidden">
                <button @click="exportSubtitles('vtt')" :disabled="disabled" class="px-3 py-1.5 hover:bg-white/10 text-xs text-gray-300 transition-colors border-r border-white/10 disabled:opacity-50 disabled:cursor-not-allowed focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none">VTT</button>
                <button @click="exportSubtitles('vtt', true)" :disabled="disabled" class="px-2 py-1.5 hover:bg-white/10 text-gray-300 transition-colors disabled:opacity-50 disabled:cursor-not-allowed focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none" title="Save VTT as...">
                    <DownloadIcon class="h-3 w-3" />
                </button>
            </div>
            <div class="flex rounded-lg bg-white/5 border border-white/10 overflow-hidden">
                <button @click="exportSubtitles('txt')" :disabled="disabled" class="px-3 py-1.5 hover:bg-white/10 text-xs text-gray-300 transition-colors border-r border-white/10 disabled:opacity-50 disabled:cursor-not-allowed focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none">TXT</button>
                <button @click="exportSubtitles('txt', true)" :disabled="disabled" class="px-2 py-1.5 hover:bg-white/10 text-gray-300 transition-colors disabled:opacity-50 disabled:cursor-not-allowed focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none" title="Save TXT as...">
                    <DownloadIcon class="h-3 w-3" />
                </button>
            </div>
            <div
                class="flex rounded-lg bg-white/5 border border-white/10 px-1"
                title="Which timeline the exported timestamps use. Source file: matches the media you selected. Cut export: re-timed for the _cut file, which contains only the transcript segments and therefore starts at 00:00."
            >
                <select
                    v-model="timeline"
                    aria-label="Subtitle timeline"
                    data-testid="subtitle-timeline"
                    class="bg-transparent text-xs text-gray-300 outline-none border-none py-1.5 px-1 cursor-pointer [&>option]:bg-gray-900 focus-visible:ring-2 focus-visible:ring-blue-500/50"
                >
                    <option value="source">Source timeline</option>
                    <option value="cut">Cut timeline</option>
                </select>
            </div>
            <button
                @click="validateAndShow"
                class="px-3 py-1.5 text-xs rounded-lg border transition-colors focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none"
                :class="{
                    'bg-red-500/20 border-red-500/30 text-red-400 hover:bg-red-500/30': hasErrors,
                    'bg-yellow-500/20 border-yellow-500/30 text-yellow-400 hover:bg-yellow-500/30': hasWarnings && !hasErrors,
                    'bg-white/5 border-white/10 text-gray-300 hover:bg-white/10': !hasErrors && !hasWarnings
                }"
            >
                Validate
            </button>
            <span v-if="status" class="text-xs text-emerald-400 animate-pulse">{{ status }}</span>
        </div>
        
        <!-- Validation Panel -->
        <div v-if="showValidationPanel" class="mt-2 p-3 rounded-lg bg-white/5 border border-white/10 max-h-48 overflow-y-auto">
            <div class="flex items-center justify-between mb-2">
                <span class="text-xs font-medium text-gray-300">Validation Results</span>
                <button @click="showValidationPanel = false" class="text-xs text-gray-500 hover:text-gray-300 focus-visible:ring-2 focus-visible:ring-blue-500/50 focus-visible:outline-none">×</button>
            </div>
            <div v-if="validationErrors.length === 0" class="text-xs text-emerald-400">
                ✓ All subtitles meet comfortable display requirements
            </div>
            <div v-else class="space-y-1">
                <div 
                    v-for="(error, idx) in validationErrors" 
                    :key="idx"
                    class="text-xs py-1 px-2 rounded"
                    :class="{
                        'bg-red-500/10 text-red-400': error.severity === 'error',
                        'bg-yellow-500/10 text-yellow-400': error.severity === 'warning'
                    }"
                >
                    <span class="font-medium">Segment {{ error.segmentIndex + 1 }} ({{ error.field }}):</span>
                    {{ error.message }}
                </div>
            </div>
        </div>
    </div>
</template>
