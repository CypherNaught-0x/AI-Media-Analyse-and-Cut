<script setup lang="ts">
import { save } from '@tauri-apps/plugin-dialog';
import { ref, computed, onBeforeUnmount, onMounted } from 'vue';
import { CircleAlert, CircleCheck, Download, ShieldCheck, TriangleAlert, X } from '@lucide/vue';
import type { TranscriptSegment } from '../types';
import {
    processSubtitlesForDisplay,
    type SubtitleSplitOptions,
    type SubtitleValidationError,
} from '../utils/subtitleValidation';
import { formatSubtitleTimeRange } from '../utils/subtitle';
import { remapSegmentsToCutTimeline } from '../utils/subtitleTimeline';
import { commands } from '../bindings';
import { errorMessage } from '../utils/appError';
import UiSegmented, { type SegmentedOption } from './ui/UiSegmented.vue';

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
type SubtitleFormat = 'srt' | 'vtt' | 'txt';

const formats: SubtitleFormat[] = ['srt', 'vtt', 'txt'];

const timelineOptions: SegmentedOption<SubtitleTimeline>[] = [
    {
        value: 'source',
        label: 'Source',
        title: 'Timestamps match the media file you selected.',
        testId: 'subtitle-timeline-source',
    },
    {
        value: 'cut',
        label: 'Cut',
        title: 'Re-timed for the _cut export, which contains only the transcript segments and therefore starts at 00:00.',
        testId: 'subtitle-timeline-cut',
    },
];

const timeline = ref<SubtitleTimeline>('source');
const status = ref('');
const statusTone = ref<'success' | 'error'>('success');
let statusTimer: ReturnType<typeof setTimeout> | null = null;
const showValidationPanel = ref(false);
const validationErrors = ref<SubtitleValidationError[]>([]);
const root = ref<HTMLElement | null>(null);
const splitOptions = ref<SubtitleSplitOptions>({
    maxCharsPerLine: 42,
    maxLines: 2,
    minDurationMs: 500,
    maxDurationMs: 7000,
    minCps: 8,
    maxCps: 25,
});

const hasErrors = computed(() => validationErrors.value.some((e) => e.severity === 'error'));
const hasWarnings = computed(() => validationErrors.value.some((e) => e.severity === 'warning'));
const errorCount = computed(
    () => validationErrors.value.filter((e) => e.severity === 'error').length,
);
const warningCount = computed(
    () => validationErrors.value.filter((e) => e.severity === 'warning').length,
);
const severity = computed<'error' | 'warning' | 'none'>(() =>
    hasErrors.value ? 'error' : hasWarnings.value ? 'warning' : 'none',
);

const limitsSummary = computed(() => {
    const o = splitOptions.value;
    return `${o.maxCharsPerLine} chars per line, ${o.maxLines} lines, ${o.minDurationMs / 1000}–${o.maxDurationMs / 1000} s on screen, ${o.minCps}–${o.maxCps} chars/s`;
});

// Timestamps to export: the source timeline as-is, or re-timed for the cut
// export, whose first cue starts at 00:00 because it contains only the
// transcript segments.
const exportSegments = computed(() =>
    timeline.value === 'cut'
        ? remapSegmentsToCutTimeline(props.segments, props.cutSegments ?? props.segments)
        : props.segments,
);

function clearStatus() {
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = null;
    status.value = '';
}

function setStatus(message: string, tone: 'success' | 'error') {
    clearStatus();
    status.value = message;
    statusTone.value = tone;
    // Success fades on its own; an error stays until the next action.
    if (tone === 'success') {
        statusTimer = setTimeout(() => {
            status.value = '';
            statusTimer = null;
        }, 3000);
    }
}

function validateAndShow() {
    clearStatus();
    const { errors } = processSubtitlesForDisplay(exportSegments.value, splitOptions.value);
    validationErrors.value = errors;
    showValidationPanel.value = true;
}

async function exportSubtitles(format: SubtitleFormat, manualSave: boolean = false) {
    if (props.segments.length === 0 || props.disabled) return;
    clearStatus();

    // Process segments for comfortable display (split long entries)
    const { segments: processedSegments, errors } = processSubtitlesForDisplay(
        exportSegments.value,
        splitOptions.value,
    );

    // Update validation state
    validationErrors.value = errors;

    try {
        let content = '';
        // Robustly remove extension
        const baseName = props.inputPath.replace(/\.[^/\\.]+$/, '');
        // Match the cut video's file name (`<base>_cut.mp4`) so players pick the
        // subtitle file up automatically next to it.
        const timelineSuffix = timeline.value === 'cut' ? '_cut' : '';
        let suffix = props.language && props.language !== 'Original' ? `.${props.language}` : '';
        let outputPath = `${baseName}${timelineSuffix}${suffix}.${format}`;

        if (format === 'srt') {
            content = processedSegments
                .map((s, i) => {
                    const { start, end } = formatSubtitleTimeRange(s.start, s.end, ',');
                    // Handle multi-line text properly
                    const text = s.text.split('\n').join('\n');
                    return `${i + 1}\n${start} --> ${end}\n${s.speaker}: ${text}\n`;
                })
                .join('\n');
        } else if (format === 'vtt') {
            content =
                'WEBVTT\n\n' +
                processedSegments
                    .map((s) => {
                        const { start, end } = formatSubtitleTimeRange(s.start, s.end, '.');
                        // Convert newlines to <br> for VTT
                        const text = s.text.split('\n').join('<br>');
                        return `${start} --> ${end}\n<v ${s.speaker}>${text}`;
                    })
                    .join('\n\n');
        } else {
            content = processedSegments
                .map((s) => `[${s.start} - ${s.end}] ${s.speaker}: ${s.text}`)
                .join('\n');
        }

        if (manualSave) {
            const saved = await save({
                defaultPath: outputPath,
                filters: [
                    {
                        name: format.toUpperCase(),
                        extensions: [format],
                    },
                ],
            });
            if (!saved) return;
            outputPath = saved;
        }

        await commands.writeTextFile(outputPath, content);
        setStatus(
            timeline.value === 'cut'
                ? `Exported ${format.toUpperCase()} (cut timeline)`
                : `Exported ${format.toUpperCase()}`,
            'success',
        );
    } catch (e) {
        console.error(e);
        setStatus(`Error: ${errorMessage(e)}`, 'error');
    }
}

function closeValidationPanel() {
    showValidationPanel.value = false;
}

function onDocumentPointerDown(event: MouseEvent) {
    if (!showValidationPanel.value) return;
    if (root.value && event.target instanceof Node && root.value.contains(event.target)) return;
    closeValidationPanel();
}

function onDocumentKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && showValidationPanel.value) closeValidationPanel();
}

onMounted(() => {
    document.addEventListener('mousedown', onDocumentPointerDown);
    document.addEventListener('keydown', onDocumentKeydown);
});

onBeforeUnmount(() => {
    document.removeEventListener('mousedown', onDocumentPointerDown);
    document.removeEventListener('keydown', onDocumentKeydown);
    if (statusTimer) clearTimeout(statusTimer);
});
</script>

<template>
    <!-- Controls on one line; below them a reserved status line, so a status
         message never reflows the header around it. -->
    <div ref="root" class="flex flex-col items-end gap-1">
        <div class="flex items-center gap-2">
            <UiSegmented
                v-model="timeline"
                :options="timelineOptions"
                size="sm"
                label="Subtitle timeline"
                data-testid="subtitle-timeline"
            />

            <div
                role="group"
                aria-label="Export subtitles"
                class="flex h-[1.875rem] items-center rounded-md border border-line-strong bg-surface-3 shadow-[inset_0_1px_0_rgb(255_255_255/0.04)]"
                :class="disabled ? 'opacity-45' : ''"
            >
                <template v-for="(format, index) in formats" :key="format">
                    <span
                        v-if="index > 0"
                        aria-hidden="true"
                        class="h-4 w-px bg-line-strong"
                    ></span>
                    <button
                        type="button"
                        :disabled="disabled"
                        :title="`Save ${format.toUpperCase()} next to the source file`"
                        class="h-full rounded-l-[5px] pr-1 pl-2.5 text-[12px] font-medium text-fg transition-colors duration-100 hover:bg-surface-hover disabled:cursor-not-allowed"
                        @click="exportSubtitles(format)"
                    >
                        {{ format.toUpperCase() }}
                    </button>
                    <button
                        type="button"
                        :disabled="disabled"
                        :aria-label="`Save ${format.toUpperCase()} as…`"
                        :title="`Save ${format.toUpperCase()} as…`"
                        class="flex h-full w-6 items-center justify-center text-fg-subtle transition-colors duration-100 hover:bg-surface-hover hover:text-fg disabled:cursor-not-allowed"
                        @click="exportSubtitles(format, true)"
                    >
                        <Download class="h-3 w-3" aria-hidden="true" />
                    </button>
                </template>
            </div>

            <!-- Validate anchors its results popover. -->
            <div class="relative">
                <button
                    type="button"
                    class="btn-secondary btn-sm h-[1.875rem]"
                    :data-severity="severity"
                    :aria-expanded="showValidationPanel ? 'true' : 'false'"
                    aria-controls="subtitle-validation-panel"
                    :class="{
                        'border-danger/40! bg-danger/10! text-danger!': severity === 'error',
                        'border-warning/40! bg-warning/10! text-warning!': severity === 'warning',
                    }"
                    title="Check subtitle line length, duration and reading speed"
                    @click="validateAndShow"
                >
                    <CircleAlert v-if="severity === 'error'" aria-hidden="true" />
                    <TriangleAlert v-else-if="severity === 'warning'" aria-hidden="true" />
                    <ShieldCheck v-else aria-hidden="true" />
                    Validate
                </button>

                <Transition name="pop">
                    <div
                        v-if="showValidationPanel"
                        id="subtitle-validation-panel"
                        data-testid="subtitle-validation-panel"
                        role="dialog"
                        aria-label="Validation results"
                        class="popover absolute top-full right-0 z-40 mt-2 w-[26rem] origin-top-right overflow-hidden"
                    >
                        <div class="flex items-start gap-3 border-b border-line px-3.5 py-3">
                            <div class="min-w-0 flex-1">
                                <p class="section-title">Validation results</p>
                                <p class="hint mt-0.5">Limits: {{ limitsSummary }}.</p>
                            </div>
                            <button
                                type="button"
                                data-testid="subtitle-validation-close"
                                class="btn-ghost btn-sm btn-icon -mt-0.5 -mr-1.5"
                                aria-label="Close validation results"
                                @click="closeValidationPanel"
                            >
                                <X aria-hidden="true" />
                            </button>
                        </div>
                        <div class="max-h-64 overflow-y-auto px-3.5 py-3">
                            <p
                                v-if="validationErrors.length === 0"
                                class="flex items-center gap-2 text-[13px] text-success"
                            >
                                <CircleCheck class="h-4 w-4 shrink-0" aria-hidden="true" />
                                All subtitles meet comfortable display requirements
                            </p>
                            <template v-else>
                                <p class="mb-2 flex gap-1.5">
                                    <span v-if="errorCount" class="chip-danger"
                                        >{{ errorCount }} error{{
                                            errorCount === 1 ? '' : 's'
                                        }}</span
                                    >
                                    <span v-if="warningCount" class="chip-warning"
                                        >{{ warningCount }} warning{{
                                            warningCount === 1 ? '' : 's'
                                        }}</span
                                    >
                                </p>
                                <ul class="space-y-1">
                                    <li
                                        v-for="(error, idx) in validationErrors"
                                        :key="idx"
                                        class="flex gap-2 rounded-md px-2 py-1.5 text-[12px] leading-snug"
                                        :class="
                                            error.severity === 'error'
                                                ? 'bg-danger/[0.08] text-fg-muted'
                                                : 'bg-warning/[0.08] text-fg-muted'
                                        "
                                    >
                                        <span
                                            class="shrink-0 font-medium"
                                            :class="
                                                error.severity === 'error'
                                                    ? 'text-danger'
                                                    : 'text-warning'
                                            "
                                            >Segment {{ error.segmentIndex + 1 }} ({{
                                                error.field
                                            }}):</span
                                        >
                                        <span class="min-w-0">{{ error.message }}</span>
                                    </li>
                                </ul>
                            </template>
                        </div>
                    </div>
                </Transition>
            </div>
        </div>

        <!-- Live region always mounted so screen readers announce changes. -->
        <span class="sr-only" role="status" aria-live="polite">{{ status }}</span>
        <div
            v-if="!disabled"
            class="flex h-4 items-center justify-end self-stretch [contain:inline-size]"
        >
            <Transition name="fade">
                <p
                    v-if="status"
                    aria-hidden="true"
                    data-testid="subtitle-export-status"
                    class="flex min-w-0 items-center gap-1.5 text-[12px] leading-4 font-medium whitespace-nowrap"
                    :class="statusTone === 'error' ? 'text-danger' : 'text-success'"
                    :title="status"
                >
                    <CircleAlert
                        v-if="statusTone === 'error'"
                        class="h-3.5 w-3.5 shrink-0"
                        aria-hidden="true"
                    />
                    <CircleCheck v-else class="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    <span class="truncate">{{ status }}</span>
                </p>
            </Transition>
        </div>
    </div>
</template>
