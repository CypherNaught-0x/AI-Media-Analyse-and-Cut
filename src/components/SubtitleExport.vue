<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { save } from '@tauri-apps/plugin-dialog';
import { ref } from 'vue';
import type { TranscriptSegment } from '../types';
import DownloadIcon from '../assets/icons/download.svg?component';

const props = defineProps<{
  segments: TranscriptSegment[];
  inputPath: string;
  language?: string;
}>();

const status = ref("");

async function exportSubtitles(format: 'srt' | 'vtt' | 'txt', manualSave: boolean = false) {
    if (props.segments.length === 0) return;
    
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
            content = props.segments.map((s, i) => {
                const start = formatTime(s.start, ',');
                const end = formatTime(s.end, ',');
                return `${i + 1}\n${start} --> ${end}\n${s.speaker}: ${s.text}\n`;
            }).join('\n');
        } else if (format === 'vtt') {
            content = "WEBVTT\n\n" + props.segments.map((s) => {
                const start = formatTime(s.start, '.');
                const end = formatTime(s.end, '.');
                return `${start} --> ${end}\n<v ${s.speaker}>${s.text}`;
            }).join('\n\n');
        } else {
            content = props.segments.map(s => `[${s.start} - ${s.end}] ${s.speaker}: ${s.text}`).join('\n');
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
        <span v-if="status" class="text-xs text-emerald-400 animate-pulse">{{ status }}</span>
    </div>
</template>
