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

const props = defineProps<{
  segments: TranscriptSegment[];
  inputPath: string;
  language?: string;
}>();

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

function validateAndShow() {
  const { errors } = processSubtitlesForDisplay(props.segments, splitOptions.value);
  validationErrors.value = errors;
  showValidationPanel.value = true;
}

async function exportSubtitles(format: 'srt' | 'vtt' | 'txt', manualSave: boolean = false) {
    if (props.segments.length === 0) return;
    
    // Process segments for comfortable display (split long entries)
    const { segments: processedSegments, errors } = processSubtitlesForDisplay(
        props.segments, 
        splitOptions.value
    );
    
    // Update validation state
    validationErrors.value = errors;
    
    // Helper to ensure timestamps are HH:MM:SS,mmm (SRT) or HH:MM:SS.mmm (VTT)
    const formatTime = (time: string, separator: string) => {
        let [base, ms] = time.split(/[.,]/);
        if (!ms) ms = "000";
        ms = ms.padEnd(3, '0').slice(0, 3);

        const parts = base.split(':');
        let h = "00";
        let m = "00";
        let s = "00";

        if (parts.length >= 3) {
            h = parts[parts.length - 3].padStart(2, '0');
            m = parts[parts.length - 2].padStart(2, '0');
            s = parts[parts.length - 1].padStart(2, '0');
        } else if (parts.length === 2) {
            m = parts[0].padStart(2, '0');
            s = parts[1].padStart(2, '0');
        } else {
            s = parts[0].padStart(2, '0');
        }

        return `${h}:${m}:${s}${separator}${ms}`;
    };
    
    try {
        let content = "";
        // Robustly remove extension
        const baseName = props.inputPath.replace(/\.[^/\\.]+$/, "");
        let suffix = props.language && props.language !== 'Original' ? `.${props.language}` : '';
        let outputPath = `${baseName}${suffix}.${format}`;
        
        if (format === 'srt') {
            content = processedSegments.map((s, i) => {
                const start = formatTime(s.start, ',');
                const end = formatTime(s.end, ',');
                // Handle multi-line text properly
                const text = s.text.split('\n').join('\n');
                return `${i + 1}\n${start} --> ${end}\n${s.speaker}: ${text}\n`;
            }).join('\n');
        } else if (format === 'vtt') {
            content = "WEBVTT\n\n" + processedSegments.map((s) => {
                const start = formatTime(s.start, '.');
                const end = formatTime(s.end, '.');
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
        status.value = `Exported ${format.toUpperCase()}`;
        setTimeout(() => status.value = "", 3000);
    } catch (e) {
        console.error(e);
        status.value = `Error: ${e}`;
    }
}
</script>

<template>
    <div class="flex flex-col gap-2">
        <div class="flex items-center gap-2">
            <div class="flex rounded-lg bg-white/5 border border-white/10 overflow-hidden">
                <button @click="exportSubtitles('srt')" class="px-3 py-1.5 hover:bg-white/10 text-xs text-gray-300 transition-colors border-r border-white/10">SRT</button>
                <button @click="exportSubtitles('srt', true)" class="px-2 py-1.5 hover:bg-white/10 text-gray-300 transition-colors" title="Save SRT as...">
                    <DownloadIcon class="h-3 w-3" />
                </button>
            </div>
            <div class="flex rounded-lg bg-white/5 border border-white/10 overflow-hidden">
                <button @click="exportSubtitles('vtt')" class="px-3 py-1.5 hover:bg-white/10 text-xs text-gray-300 transition-colors border-r border-white/10">VTT</button>
                <button @click="exportSubtitles('vtt', true)" class="px-2 py-1.5 hover:bg-white/10 text-gray-300 transition-colors" title="Save VTT as...">
                    <DownloadIcon class="h-3 w-3" />
                </button>
            </div>
            <div class="flex rounded-lg bg-white/5 border border-white/10 overflow-hidden">
                <button @click="exportSubtitles('txt')" class="px-3 py-1.5 hover:bg-white/10 text-xs text-gray-300 transition-colors border-r border-white/10">TXT</button>
                <button @click="exportSubtitles('txt', true)" class="px-2 py-1.5 hover:bg-white/10 text-gray-300 transition-colors" title="Save TXT as...">
                    <DownloadIcon class="h-3 w-3" />
                </button>
            </div>
            <button 
                @click="validateAndShow" 
                class="px-3 py-1.5 text-xs rounded-lg border transition-colors"
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
                <button @click="showValidationPanel = false" class="text-xs text-gray-500 hover:text-gray-300">×</button>
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
