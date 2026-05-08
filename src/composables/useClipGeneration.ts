import { invoke } from '@tauri-apps/api/core';
import type { Ref } from 'vue';
import type { Clip, ClipExportPayload, SilenceInterval, TranscriptSegment } from '../types';
import type { LLMSettings } from './useSettings';
import { parseTime, formatTime } from './useTimeFormat';
import { generateSubtitleContent } from '../utils/subtitle';
import { trimClipBoundarySilence } from '../utils/clipSilence';
import { normalizeClips } from '../utils/clips';
import { beginRun, isRunCancelled } from './useRunCancellation';

interface UseClipGenerationOptions {
  settings: Ref<LLMSettings>;
  status: Ref<string>;
  isProcessing: Ref<boolean>;
  progressPercentage: Ref<number | null>;
  inputPath: Ref<string>;
  hasMediaFile: Ref<boolean>;
  segments: Ref<TranscriptSegment[]>;
  clipCount: Ref<number>;
  clipMinDuration: Ref<number>;
  clipMaxDuration: Ref<number>;
  clipTopic: Ref<string>;
  allowSplicing: Ref<boolean>;
  clips: Ref<Clip[]>;
  selectedClipIndices: Ref<number[]>;
  lastExportPath: Ref<string>;
  clipExportSilenceCache: Ref<{ path: string; intervals: SilenceInterval[] } | null>;
  estimateTime: (type: 'analysis' | 'generation', inputSize: number) => number;
  logExecution: (type: 'analysis' | 'generation', inputSize: number, duration: number) => void;
  startSimulatedProgress: (estimatedSeconds: number) => void;
  stopSimulatedProgress: () => void;
}

export function useClipGeneration(options: UseClipGenerationOptions) {
  async function generateClips() {
    if (options.segments.value.length === 0) return;

    const runId = await beginRun();
    options.status.value = 'Generating clips...';
    options.isProcessing.value = true;
    options.progressPercentage.value = null;

    try {
      const transcript = options.segments.value
        .map((s) => `[${s.start}-${s.end}] ${s.speaker}: ${s.text}`)
        .join('\n');

      const estimatedTime = options.estimateTime('generation', transcript.length);
      options.status.value = `Generating clips... (Est. ${estimatedTime.toFixed(0)}s)`;
      const startTime = Date.now();

      options.startSimulatedProgress(estimatedTime);
      let response: string;
      try {
        response = await invoke<string>('generate_clips', {
          runId,
          apiKey: options.settings.value.apiKey,
          baseUrl: options.settings.value.baseUrl,
          model: options.settings.value.model,
          transcript,
          count: options.clipCount.value,
          minDuration: options.clipMinDuration.value,
          maxDuration: options.clipMaxDuration.value,
          topic: options.clipTopic.value || null,
          splicing: options.allowSplicing.value,
        });
      } finally {
        options.stopSimulatedProgress();
      }

      const duration = (Date.now() - startTime) / 1000;
      options.logExecution('generation', transcript.length, duration);

      const jsonMatch = response.match(/\[[\s\S]*\]/);
      if (jsonMatch) {
        try {
          const parsed = JSON.parse(jsonMatch[0]);
          options.clips.value = normalizeClips(parsed);
          options.selectedClipIndices.value = [];
          options.status.value = `Found ${options.clips.value.length} clips.`;
        } catch (error) {
          console.error('JSON Parse Error', error);
          options.status.value = 'Failed to parse clips from AI response. Check console for details.';
        }
      } else {
        options.status.value = 'Failed to find JSON in AI response.';
        console.error(response);
      }
    } catch (error) {
      if (isRunCancelled(error)) {
        options.status.value = 'Run cancelled.';
      } else {
        options.status.value = `Error generating clips: ${error}`;
      }
    } finally {
      options.isProcessing.value = false;
      options.progressPercentage.value = null;
    }
  }

  async function getClipExportSilenceIntervals(runId: number): Promise<SilenceInterval[]> {
    if (options.clipExportSilenceCache.value?.path === options.inputPath.value) {
      return options.clipExportSilenceCache.value.intervals;
    }

    options.status.value = 'Detecting clip boundary silence...';
    const intervals = await invoke<SilenceInterval[]>('detect_silence', {
      runId,
      path: options.inputPath.value,
    });
    options.clipExportSilenceCache.value = { path: options.inputPath.value, intervals };
    return intervals;
  }

  async function exportClips(payload?: ClipExportPayload) {
    if (!options.hasMediaFile.value) {
      options.status.value = 'Select a valid media file before exporting clips.';
      return;
    }

    const clipsToExport = payload?.clips || options.clips.value;
    const includeSubtitlesValue = payload?.includeSubtitles || false;
    const fastModeValue = payload?.fastMode || false;
    const trimBoundarySilenceValue = payload?.trimBoundarySilence || false;

    if (clipsToExport.length === 0) return;

    const runId = await beginRun();
    options.status.value = 'Exporting clips...';
    options.isProcessing.value = true;
    options.progressPercentage.value = null;

    try {
      const outputDir = options.inputPath.value.replace(/\.[^/\\.]+$/, '') + '_clips';

      const prePadding = options.settings.value.preClipPadding || 0;
      const postPadding = options.settings.value.postClipPadding || 0;
      const maxDuration = Number.POSITIVE_INFINITY;

      let clipSegments = clipsToExport.map((clip) => ({
        segments: clip.segments.map((segment) => {
          const start = Math.max(0, parseTime(segment.start) - prePadding);
          const end = Math.min(maxDuration, parseTime(segment.end) + postPadding);
          return {
            start: formatTime(start),
            end: formatTime(end),
          };
        }),
        label: clip.title,
        reason: clip.reason,
      }));

      if (trimBoundarySilenceValue) {
        try {
          const silenceIntervals = await getClipExportSilenceIntervals(runId);
          clipSegments = clipSegments.map((clip) => ({
            ...clip,
            segments: trimClipBoundarySilence(clip.segments, silenceIntervals),
          }));
        } catch (error) {
          if (isRunCancelled(error)) throw error;
          console.warn('Failed to detect silence for clip export', error);
          options.status.value = 'Silence detection failed, exporting without boundary trimming...';
        }
      }

      options.status.value = `Exporting to ${outputDir}...`;
      await invoke('export_clips', {
        runId,
        inputPath: options.inputPath.value,
        segments: clipSegments,
        outputDir,
        fastMode: fastModeValue,
      });

      if (includeSubtitlesValue) {
        options.status.value = 'Generating subtitles...';
        for (let i = 0; i < clipSegments.length; i++) {
          const clip = clipSegments[i];
          const suffix = clip.label ? clip.label.replace(/[^a-zA-Z0-9-_]/g, '') : '';
          const indexStr = (i + 1).toString().padStart(3, '0');
          const filename = suffix ? `clip_${indexStr}_${suffix}.srt` : `clip_${indexStr}.srt`;
          const outputPath = `${outputDir}\\${filename}`;

          const clipTranscript: TranscriptSegment[] = [];
          let currentOffset = 0;

          for (const seg of clip.segments) {
            const segStart = parseTime(seg.start);
            const segEnd = parseTime(seg.end);
            const duration = segEnd - segStart;

            const overlapping = options.segments.value.filter((transcriptSegment) => {
              const tStart = parseTime(transcriptSegment.start);
              const tEnd = parseTime(transcriptSegment.end);
              return Math.max(tStart, segStart) < Math.min(tEnd, segEnd);
            });

            for (const transcriptSegment of overlapping) {
              const tStart = parseTime(transcriptSegment.start);
              const tEnd = parseTime(transcriptSegment.end);
              const effStart = Math.max(tStart, segStart);
              const effEnd = Math.min(tEnd, segEnd);

              if (effEnd > effStart) {
                const relStart = currentOffset + (effStart - segStart);
                const relEnd = currentOffset + (effEnd - segStart);

                clipTranscript.push({
                  start: formatTime(relStart),
                  end: formatTime(relEnd),
                  text: transcriptSegment.text,
                  speaker: transcriptSegment.speaker,
                });
              }
            }
            currentOffset += duration;
          }

          if (clipTranscript.length > 0) {
            const srtContent = generateSubtitleContent(clipTranscript, 'srt');
            await invoke('write_text_file', { path: outputPath, content: srtContent });
          }
        }
      }

      options.lastExportPath.value = outputDir;
      options.status.value = `Clips exported to ${outputDir}`;
    } catch (error) {
      if (isRunCancelled(error)) {
        options.status.value = 'Run cancelled.';
      } else {
        options.status.value = `Error exporting clips: ${error}`;
      }
    } finally {
      options.isProcessing.value = false;
      options.progressPercentage.value = null;
    }
  }

  async function openExportFolder() {
    if (options.lastExportPath.value) {
      await invoke('open_folder', { path: options.lastExportPath.value });
    }
  }

  return {
    generateClips,
    exportClips,
    openExportFolder,
  };
}
