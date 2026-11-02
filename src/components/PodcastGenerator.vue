<script setup lang="ts">
import { ref, computed, watch, nextTick } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import {
    FileAudio,
    FolderOpen,
    GripVertical,
    Loader2,
    Mic,
    Plus,
    Podcast,
    Search,
    Sparkles,
    X,
} from '@lucide/vue';
import type {
    PodcastScript,
    PodcastSegment,
    PodcastWorkspaceState,
    TranscriptSegment,
} from '../types';
import { useSettings } from '../composables/useSettings';
import { formatDuration, calculateSegmentsDuration } from '../composables/useTimeFormat';
import { beginRun, isRunCancelled } from '../composables/useRunCancellation';
import { exportFolderOf } from '../utils/filePath';

import { commands } from '../bindings';
import { errorMessage } from '../utils/appError';
import { speakerColor } from '../utils/speakerColors';
import PageHeader from './ui/PageHeader.vue';
import EmptyState from './ui/EmptyState.vue';
import UiDialog from './ui/UiDialog.vue';

interface Props {
    segments: TranscriptSegment[];
    inputPath: string;
    hasMediaFile: boolean;
    state: PodcastWorkspaceState;
    cancelGeneration: number;
    /** The analysis context the user described for this media, if any. */
    context: string;
    /** Another job (analysis, translation, export, ...) is running app-wide. */
    busy: boolean;
    /** The user-managed speaker order (Home's speaker list), so a speaker keeps
     *  the same colour as on the Transcript step. Falls back to the order of
     *  first appearance in the transcript. */
    speakerOrder?: string[];
}

const props = withDefaults(defineProps<Props>(), { speakerOrder: undefined });

const emit = defineEmits<{
    'update:status': [message: string];
    'update:processing': [isProcessing: boolean];
    'update:state': [state: PodcastWorkspaceState];
}>();

const { settings } = useSettings();

const isGenerating = ref(false);
// Which export is running, so each button shows its own busy state.
const exportingKind = ref<'podcast' | 'clips' | null>(null);
const isExporting = computed(() => exportingKind.value !== null);
const activeRunId = ref<number | null>(null);

// Segment being edited (by row key, so a reorder doesn't move the editor).
const editingKey = ref<number | null>(null);
const editingField = ref<'start' | 'end' | null>(null);
const editingValue = ref('');
const editError = ref('');
const editInput = ref<HTMLInputElement | HTMLInputElement[] | null>(null);

// Add segment modal
const showAddSegmentModal = ref(false);
const addSearch = ref('');
const addedCount = ref(0);

// Drag state
const draggedIndex = ref<number | null>(null);

// Stable keys for the script rows (segments carry no ids). Kept in step with
// every local reorder/insert/remove; reset when the script is replaced.
let nextRowKey = 1;
const rowKeys = ref<number[]>([]);

function invalidateRun() {
    activeRunId.value = null;
    isGenerating.value = false;
    exportingKind.value = null;
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
    const contentSegments = podcastScript.value.segments.filter((s) => s.type === 'content');
    return calculateSegmentsDuration(contentSegments);
});

const voiceoverCount = computed(() => {
    if (!podcastScript.value) return 0;
    return podcastScript.value.segments.filter((s) => s.type === 'voiceover').length;
});

const durationStatus = computed(() => {
    const duration = currentDuration.value;
    const min = minDurationSeconds.value;
    const max = maxDurationSeconds.value;

    if (duration < min) return 'short';
    if (duration > max) return 'long';
    return 'ok';
});

const durationExplanation = computed(() => {
    if (rangeError.value) return 'Fix the target length to compare';
    const range = `${minDurationMinutes.value}–${maxDurationMinutes.value} min`;
    if (durationStatus.value === 'short') return `Shorter than the ${range} target`;
    if (durationStatus.value === 'long') return `Longer than the ${range} target`;
    return `Within the ${range} target`;
});

/** The target range is unusable: min above max, or not a positive number. */
const rangeError = computed(() => {
    const min = minDurationMinutes.value;
    const max = maxDurationMinutes.value;
    if (typeof min !== 'number' || typeof max !== 'number' || !(min > 0) || !(max > 0)) {
        return 'Enter a length of at least 1 minute.';
    }
    if (min > max) return 'Minimum is longer than the maximum.';
    return '';
});

const generateDisabledReason = computed(() => {
    if (props.busy) return 'Another job is running.';
    if (isExporting.value) return 'Wait for the export to finish.';
    if (props.segments.length === 0) return 'The transcript has no segments.';
    if (rangeError.value) return 'Fix the target length first.';
    return '';
});

const exportDisabledReason = computed(() => {
    if (!props.hasMediaFile) return 'The media file is missing. Pick it again on the Source step.';
    if (isGenerating.value) return 'Wait for the script to finish generating.';
    if (props.busy && !isExporting.value) return 'Another job is running.';
    return '';
});

const colourOrder = computed(() => {
    if (props.speakerOrder?.length) return props.speakerOrder;
    const seen: string[] = [];
    for (const segment of props.segments) {
        if (!seen.includes(segment.speaker)) seen.push(segment.speaker);
    }
    return seen;
});

function colorOf(speaker: string) {
    return speakerColor(speaker, colourOrder.value);
}

const audioSlots = computed(() => [
    {
        id: 'intro',
        label: 'Intro',
        path: introPath.value,
        browse: selectIntroFile,
        clear: () => (introPath.value = ''),
    },
    {
        id: 'outro',
        label: 'Outro',
        path: outroPath.value,
        browse: selectOutroFile,
        clear: () => (outroPath.value = ''),
    },
]);

function fileName(path: string) {
    return path.split(/[/\\]/).pop() ?? path;
}

const outputName = computed(() => fileName(props.inputPath.replace(/\.[^/\\.]+$/, '_podcast.m4a')));

watch(
    () => podcastScript.value,
    (script, previous) => {
        const length = script?.segments.length ?? 0;
        // A replaced script (generation, loaded session) or one whose length
        // changed outside our own edits gets fresh keys.
        if (!previous || !script || rowKeys.value.length !== length) {
            rowKeys.value = Array.from({ length }, () => nextRowKey++);
        }
    },
    { immediate: true },
);

function rowKey(index: number) {
    return rowKeys.value[index] ?? -index - 1;
}

async function selectIntroFile() {
    try {
        const selected = await open({
            multiple: false,
            filters: [
                {
                    name: 'Audio',
                    extensions: ['mp3', 'wav', 'aac', 'flac', 'ogg', 'm4a'],
                },
            ],
        });
        if (selected && typeof selected === 'string') {
            introPath.value = selected;
        }
    } catch (e) {
        console.error('Failed to select intro file:', e);
    }
}

async function selectOutroFile() {
    try {
        const selected = await open({
            multiple: false,
            filters: [
                {
                    name: 'Audio',
                    extensions: ['mp3', 'wav', 'aac', 'flac', 'ogg', 'm4a'],
                },
            ],
        });
        if (selected && typeof selected === 'string') {
            outroPath.value = selected;
        }
    } catch (e) {
        console.error('Failed to select outro file:', e);
    }
}

async function generatePodcast() {
    if (props.busy || props.segments.length === 0 || rangeError.value) return;

    const runId = await beginRun();
    activeRunId.value = runId;
    emit('update:status', 'Generating podcast script...');
    isGenerating.value = true;
    emit('update:processing', true);

    try {
        // Format transcript for LLM
        const transcript = props.segments
            .map((s) => `[${s.start}-${s.end}] ${s.speaker}: ${s.text}`)
            .join('\n');

        // Initial generation
        let response = await commands.generatePodcast(
            runId,
            settings.value.baseUrl,
            settings.value.model,
            transcript,
            minDurationSeconds.value,
            maxDurationSeconds.value,
            props.context.trim() || null,
        );
        assertActiveRun(runId);

        let script: PodcastScript;
        try {
            const parsedScript = parseScriptResponse(response);
            if (!parsedScript) {
                emit('update:status', 'Failed to parse podcast script from AI response.');
                return;
            }
            script = parsedScript;
        } catch (e) {
            emit('update:status', errorMessage(e));
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

            response = await commands.refinePodcast(
                runId,
                settings.value.baseUrl,
                settings.value.model,
                transcript,
                JSON.stringify(script),
                duration,
                minDurationSeconds.value,
                maxDurationSeconds.value,
            );
            assertActiveRun(runId);

            let refinedScript: PodcastScript | null = null;
            try {
                refinedScript = parseScriptResponse(response);
            } catch (e) {
                // A malformed refinement is non-fatal: keep the previous valid script.
                console.warn('Refined podcast script was malformed, keeping previous script:', e);
            }
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

        emit(
            'update:status',
            `Podcast script generated: "${script.title}" (${formatDuration(script.totalDuration)})`,
        );
    } catch (e) {
        if (isRunCancelled(e)) {
            emit('update:status', 'Run cancelled.');
            return;
        }
        emit('update:status', `Error generating podcast: ${errorMessage(e)}`);
        console.error('Podcast generation error:', e);
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isGenerating.value = false;
            emit('update:processing', false);
        }
    }
}

/** The script shape the LLM is asked for; validated field by field below. */
interface RawPodcastSegment {
    segment_type?: PodcastSegment['type'];
    type?: PodcastSegment['type'];
    start?: string;
    end?: string;
    text?: string;
    speaker?: string;
    include_reason?: string;
    includeReason?: string;
    transition_note?: string;
    transitionNote?: string;
}

interface RawPodcastScript {
    title?: string;
    summary?: string;
    segments?: RawPodcastSegment[];
}

function parseScriptResponse(response: string): PodcastScript | null {
    // Locate and parse the JSON object. A missing/invalid envelope is a soft
    // failure (return null); a well-formed object with malformed segments is a
    // hard failure (throw with details) so it never reaches parseTime later.
    const jsonMatch = response.match(/\{[\s\S]*\}/);
    if (!jsonMatch) return null;

    let parsed: RawPodcastScript;
    try {
        parsed = JSON.parse(jsonMatch[0]);
    } catch (e) {
        console.error('Failed to parse script response:', e);
        return null;
    }

    if (!parsed.title || !parsed.segments || !Array.isArray(parsed.segments)) {
        return null;
    }

    const problems: string[] = [];
    const segments: PodcastSegment[] = parsed.segments.map((s, index) => {
        const type: PodcastSegment['type'] = s.segment_type || s.type || 'content'; // Handle both snake_case and camelCase
        // Every segment needs string timestamps; parseTime throws on anything else.
        if (typeof s.start !== 'string' || typeof s.end !== 'string') {
            problems.push(`segment ${index + 1}: missing or non-string start/end`);
        }
        // Content segments must carry text; voiceover placeholders use transitionNote.
        if (type === 'content' && (typeof s.text !== 'string' || s.text.trim().length === 0)) {
            problems.push(`segment ${index + 1}: content segment has empty text`);
        }
        return {
            start: s.start as string,
            end: s.end as string,
            text: s.text || '',
            // The export commands require a speaker string.
            speaker: s.speaker ?? '',
            type,
            includeReason: s.include_reason || s.includeReason,
            transitionNote: s.transition_note || s.transitionNote,
        };
    });

    if (problems.length > 0) {
        throw new Error(`Podcast script has malformed segments: ${problems.join('; ')}`);
    }

    return {
        title: parsed.title,
        summary: parsed.summary || '',
        segments,
        totalDuration: 0,
    };
}

// Segment Editor Functions
function removeSegment(index: number) {
    if (editingKey.value === rowKey(index)) cancelEditing();
    rowKeys.value.splice(index, 1);
    updatePodcastScript((script) => {
        script.segments.splice(index, 1);
        script.totalDuration = calculateSegmentsDuration(script.segments);
    });
}

const TIME_PATTERN = /^\d{1,2}:\d{2}(:\d{2})?(\.\d+)?$/;

async function startEditing(index: number, field: 'start' | 'end') {
    const segment = podcastScript.value?.segments[index];
    if (!segment) return;
    editingKey.value = rowKey(index);
    editingField.value = field;
    editingValue.value = segment[field];
    editError.value = '';
    await nextTick();
    const input = Array.isArray(editInput.value) ? editInput.value[0] : editInput.value;
    input?.focus();
    input?.select();
}

function isEditing(index: number, field: 'start' | 'end') {
    return editingKey.value === rowKey(index) && editingField.value === field;
}

function cancelEditing() {
    editingKey.value = null;
    editingField.value = null;
    editError.value = '';
}

function finishEditing(index: number, field: 'start' | 'end', value: string) {
    // Ignore the blur that follows a cancel or an already committed edit.
    if (!isEditing(index, field)) return;
    // Validate time format; invalid input stays in the field with an error.
    const trimmed = value.trim();
    if (!TIME_PATTERN.test(trimmed)) {
        editError.value = 'Use mm:ss or hh:mm:ss, e.g. 01:23 or 1:02:03.5. Esc cancels.';
        return;
    }
    if (trimmed !== podcastScript.value?.segments[index]?.[field]) {
        updatePodcastScript((script) => {
            script.segments[index][field] = trimmed;
            script.totalDuration = calculateSegmentsDuration(script.segments);
        });
    }
    cancelEditing();
}

function moveSegment(from: number, to: number) {
    const segments = podcastScript.value?.segments;
    if (!segments || to < 0 || to >= segments.length || from === to) return;
    const [key] = rowKeys.value.splice(from, 1);
    rowKeys.value.splice(to, 0, key);
    updatePodcastScript((script) => {
        const [moved] = script.segments.splice(from, 1);
        script.segments.splice(to, 0, moved);
    });
}

// Keyboard reorder: Alt+ArrowUp/Down on a focused row moves it.
async function onRowKeydown(event: KeyboardEvent, index: number) {
    if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return;
    if (event.target !== event.currentTarget) return;
    event.preventDefault();
    const to = index + (event.key === 'ArrowUp' ? -1 : 1);
    const key = rowKey(index);
    moveSegment(index, to);
    await nextTick();
    const list = (event.currentTarget as HTMLElement | null)?.parentElement;
    list?.querySelector<HTMLElement>(`[data-row-key="${key}"]`)?.focus();
}

// Drag and Drop (started from the grip handle only)
function onDragStart(event: DragEvent, index: number) {
    draggedIndex.value = index;
    if (event.dataTransfer) {
        event.dataTransfer.effectAllowed = 'move';
        event.dataTransfer.setData('text/plain', String(index));
        const row = (event.currentTarget as HTMLElement | null)?.closest<HTMLElement>(
            '[data-row-key]',
        );
        if (row) event.dataTransfer.setDragImage(row, 24, 20);
    }
}

function onDragOver(e: DragEvent, index: number) {
    if (draggedIndex.value === null) return;
    e.preventDefault();
    if (draggedIndex.value === index) return;
    const from = draggedIndex.value;
    moveSegment(from, index);
    draggedIndex.value = index;
}

function onDragEnd() {
    draggedIndex.value = null;
}

// Add Segment from Transcript
function openAddSegmentModal() {
    addSearch.value = '';
    addedCount.value = 0;
    showAddSegmentModal.value = true;
}

function closeAddSegmentModal() {
    showAddSegmentModal.value = false;
}

const filteredTranscript = computed(() => {
    const query = addSearch.value.trim().toLowerCase();
    if (!query) return props.segments;
    return props.segments.filter(
        (segment) =>
            segment.text.toLowerCase().includes(query) ||
            segment.speaker.toLowerCase().includes(query) ||
            segment.start.includes(query),
    );
});

function addSegmentFromTranscript(segment: TranscriptSegment) {
    if (isSegmentIncluded(segment)) return;
    const newSegment: PodcastSegment = {
        start: segment.start,
        end: segment.end,
        text: segment.text,
        speaker: segment.speaker,
        type: 'content',
        includeReason: 'Manually added',
    };

    rowKeys.value.push(nextRowKey++);
    updatePodcastScript((script) => {
        script.segments.push(newSegment);
        const contentSegments = script.segments.filter((s) => s.type === 'content');
        script.totalDuration = calculateSegmentsDuration(contentSegments);
    });
    // The dialog stays open so several segments can be added in one go.
    addedCount.value++;
}

function addVoiceoverSegment(afterIndex: number) {
    const newSegment: PodcastSegment = {
        start: '00:00',
        end: '00:00',
        text: '',
        speaker: 'Narrator',
        type: 'voiceover',
        transitionNote: 'Add your transition text here...',
    };

    // Insert after the specified index
    rowKeys.value.splice(afterIndex + 1, 0, nextRowKey++);
    updatePodcastScript((script) => {
        script.segments.splice(afterIndex + 1, 0, newSegment);
    });
}

function updateTransitionNote(index: number, value: string) {
    if (podcastScript.value?.segments[index]?.transitionNote === value) return;
    updatePodcastScript((script) => {
        script.segments[index].transitionNote = value;
    });
}

// Check if a transcript segment is already in the podcast
function isSegmentIncluded(segment: TranscriptSegment): boolean {
    if (!podcastScript.value) return false;
    return podcastScript.value.segments.some(
        (s) => s.start === segment.start && s.end === segment.end,
    );
}

// Export Functions
async function exportPodcast() {
    if (props.busy || !podcastScript.value || podcastScript.value.segments.length === 0) return;
    if (!props.hasMediaFile) {
        emit('update:status', 'Select a valid media file before exporting the podcast.');
        return;
    }

    // Check if there are any content segments
    const contentSegments = podcastScript.value.segments.filter((s) => s.type === 'content');
    if (contentSegments.length === 0) {
        emit('update:status', 'No content segments to export (only voiceover placeholders)');
        return;
    }

    const runId = await beginRun();
    activeRunId.value = runId;
    emit('update:status', 'Exporting podcast...');
    exportingKind.value = 'podcast';
    emit('update:processing', true);

    try {
        const outputPath = props.inputPath.replace(/\.[^/\\.]+$/, '_podcast.m4a');

        await commands.exportPodcast(
            runId,
            props.inputPath,
            podcastScript.value.segments.map((s) => ({
                start: s.start,
                end: s.end,
                text: s.text,
                speaker: s.speaker,
                segment_type: s.type,
                include_reason: s.includeReason,
                transition_note: s.transitionNote,
            })),
            introPath.value || null,
            outroPath.value || null,
            startPadding.value,
            endPadding.value,
            outputPath,
        );
        assertActiveRun(runId);

        updateState({ lastExportPath: outputPath });
        emit('update:status', `Podcast exported to ${outputPath}`);
    } catch (e) {
        if (isRunCancelled(e)) {
            emit('update:status', 'Run cancelled.');
            return;
        }
        emit('update:status', `Error exporting podcast: ${errorMessage(e)}`);
        console.error('Export error:', e);
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            exportingKind.value = null;
            emit('update:processing', false);
        }
    }
}

async function exportClips() {
    if (props.busy || !podcastScript.value || podcastScript.value.segments.length === 0) return;
    if (!props.hasMediaFile) {
        emit('update:status', 'Select a valid media file before exporting podcast clips.');
        return;
    }

    // Check if there are any content segments
    const contentSegments = podcastScript.value.segments.filter((s) => s.type === 'content');
    if (contentSegments.length === 0) {
        emit('update:status', 'No content segments to export (only voiceover placeholders)');
        return;
    }

    const runId = await beginRun();
    activeRunId.value = runId;
    emit('update:status', 'Exporting podcast clips...');
    exportingKind.value = 'clips';
    emit('update:processing', true);

    try {
        const outputDir = props.inputPath.replace(/\.[^/\\.]+$/, '_podcast_clips');

        await commands.exportPodcastClips(
            runId,
            props.inputPath,
            podcastScript.value.segments.map((s) => ({
                start: s.start,
                end: s.end,
                text: s.text,
                speaker: s.speaker,
                segment_type: s.type,
                include_reason: s.includeReason,
                transition_note: s.transitionNote,
            })),
            startPadding.value,
            endPadding.value,
            outputDir,
        );
        assertActiveRun(runId);

        updateState({ lastExportPath: outputDir });
        emit('update:status', `Podcast clips exported to ${outputDir}`);
    } catch (e) {
        if (isRunCancelled(e)) {
            emit('update:status', 'Run cancelled.');
            return;
        }
        emit('update:status', `Error exporting clips: ${errorMessage(e)}`);
        console.error('Export clips error:', e);
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            exportingKind.value = null;
            emit('update:processing', false);
        }
    }
}

async function openExportFolder() {
    if (lastExportPath.value) {
        // The last export is either the podcast file or the clips directory.
        await commands.openFolder(exportFolderOf(lastExportPath.value));
    }
}
</script>

<template>
    <div class="@container mx-auto w-full max-w-[1400px] px-8 pt-8 pb-10">
        <PageHeader
            title="Podcast"
            description="Condense the recording into an episode of the length you want."
        >
            <template v-if="podcastScript" #actions>
                <button
                    v-if="lastExportPath"
                    type="button"
                    class="btn-ghost btn-icon"
                    data-testid="podcast-open-folder"
                    aria-label="Open export folder"
                    :title="`Open ${exportFolderOf(lastExportPath)}`"
                    @click="openExportFolder"
                >
                    <FolderOpen aria-hidden="true" />
                </button>
                <button
                    type="button"
                    class="btn-secondary"
                    data-testid="podcast-export-clips"
                    :disabled="isExporting || isGenerating || busy || !hasMediaFile"
                    :aria-busy="exportingKind === 'clips' ? 'true' : undefined"
                    :title="exportDisabledReason || 'Export each segment as its own file'"
                    @click="exportClips"
                >
                    <Loader2
                        v-if="exportingKind === 'clips'"
                        class="animate-spin"
                        aria-hidden="true"
                    />
                    {{ exportingKind === 'clips' ? 'Exporting clips…' : 'Export clips' }}
                </button>
                <button
                    type="button"
                    class="btn-primary"
                    data-testid="podcast-export"
                    :disabled="isExporting || isGenerating || busy || !hasMediaFile"
                    :aria-busy="exportingKind === 'podcast' ? 'true' : undefined"
                    :title="exportDisabledReason || `Export as ${outputName}`"
                    @click="exportPodcast"
                >
                    <Loader2
                        v-if="exportingKind === 'podcast'"
                        class="animate-spin"
                        aria-hidden="true"
                    />
                    {{ exportingKind === 'podcast' ? 'Exporting…' : 'Export podcast' }}
                </button>
            </template>
        </PageHeader>

        <p
            v-if="podcastScript && !hasMediaFile"
            class="-mt-2 mb-4 text-[12px] text-warning"
            data-testid="podcast-export-hint"
        >
            {{ exportDisabledReason }}
        </p>

        <div class="grid items-start gap-5 @4xl:grid-cols-[340px_minmax(0,1fr)]">
            <!-- Setup inspector -->
            <aside
                class="panel grid gap-x-6 gap-y-5 p-4 @2xl:grid-cols-2 @4xl:sticky @4xl:top-4 @4xl:block @4xl:space-y-5"
                aria-label="Podcast setup"
                data-testid="podcast-setup"
            >
                <section>
                    <h2 class="section-title">Target length</h2>
                    <p class="hint mt-0.5">Only kept content counts towards the length.</p>
                    <div class="mt-3 grid grid-cols-2 gap-3">
                        <div>
                            <label for="podcast-min" class="field-label">Minimum</label>
                            <div class="relative">
                                <input
                                    id="podcast-min"
                                    v-model.number="minDurationMinutes"
                                    type="number"
                                    min="1"
                                    max="60"
                                    class="input pr-11 font-mono"
                                    :aria-invalid="rangeError ? 'true' : undefined"
                                    :aria-describedby="
                                        rangeError ? 'podcast-range-error' : undefined
                                    "
                                />
                                <span
                                    class="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-[12px] text-fg-subtle"
                                    aria-hidden="true"
                                    >min</span
                                >
                            </div>
                        </div>
                        <div>
                            <label for="podcast-max" class="field-label">Maximum</label>
                            <div class="relative">
                                <input
                                    id="podcast-max"
                                    v-model.number="maxDurationMinutes"
                                    type="number"
                                    min="1"
                                    max="60"
                                    class="input pr-11 font-mono"
                                    :aria-invalid="rangeError ? 'true' : undefined"
                                    :aria-describedby="
                                        rangeError ? 'podcast-range-error' : undefined
                                    "
                                />
                                <span
                                    class="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-[12px] text-fg-subtle"
                                    aria-hidden="true"
                                    >min</span
                                >
                            </div>
                        </div>
                    </div>
                    <p
                        v-if="rangeError"
                        id="podcast-range-error"
                        class="mt-1.5 text-[12px] text-danger"
                        role="alert"
                        data-testid="podcast-range-error"
                    >
                        {{ rangeError }}
                    </p>
                </section>

                <div class="divider hidden @4xl:block" />

                <section>
                    <h2 class="section-title">Intro and outro</h2>
                    <p class="hint mt-0.5">Optional audio played before and after the episode.</p>
                    <div class="mt-3 space-y-2">
                        <div
                            v-for="file in audioSlots"
                            :key="file.id"
                            class="well flex items-center gap-2.5 py-1.5 pr-1.5 pl-2.5"
                            :data-testid="`podcast-${file.id}`"
                        >
                            <FileAudio
                                class="h-4 w-4 shrink-0"
                                :class="file.path ? 'text-accent-soft' : 'text-fg-faint'"
                                aria-hidden="true"
                            />
                            <div class="min-w-0 flex-1">
                                <p class="eyebrow leading-none">{{ file.label }}</p>
                                <p
                                    class="mt-1 truncate text-[12.5px]"
                                    :class="file.path ? 'text-fg' : 'text-fg-faint'"
                                    :title="file.path || undefined"
                                >
                                    {{ file.path ? fileName(file.path) : `No ${file.id} selected` }}
                                </p>
                            </div>
                            <button
                                v-if="file.path"
                                type="button"
                                class="btn-ghost btn-sm btn-icon"
                                :aria-label="`Remove ${file.id}`"
                                :title="`Remove ${file.id}`"
                                @click="file.clear()"
                            >
                                <X aria-hidden="true" />
                            </button>
                            <button
                                type="button"
                                class="btn-secondary btn-sm"
                                :aria-label="`Browse for ${file.id} audio`"
                                @click="file.browse()"
                            >
                                Browse
                            </button>
                        </div>
                    </div>
                </section>

                <div class="divider hidden @4xl:block" />

                <section>
                    <h2 class="section-title">Padding</h2>
                    <p class="hint mt-0.5">Extra audio kept around each segment.</p>
                    <div class="mt-3 grid grid-cols-2 gap-3">
                        <div>
                            <label for="podcast-start-padding" class="field-label">Before</label>
                            <div class="relative">
                                <input
                                    id="podcast-start-padding"
                                    v-model.number="startPadding"
                                    type="number"
                                    min="0"
                                    max="10"
                                    step="0.1"
                                    class="input pr-7 font-mono"
                                    title="Extra audio before each clip"
                                />
                                <span
                                    class="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-[12px] text-fg-subtle"
                                    aria-hidden="true"
                                    >s</span
                                >
                            </div>
                        </div>
                        <div>
                            <label for="podcast-end-padding" class="field-label">After</label>
                            <div class="relative">
                                <input
                                    id="podcast-end-padding"
                                    v-model.number="endPadding"
                                    type="number"
                                    min="0"
                                    max="10"
                                    step="0.1"
                                    class="input pr-7 font-mono"
                                    title="Extra audio after each clip"
                                />
                                <span
                                    class="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-[12px] text-fg-subtle"
                                    aria-hidden="true"
                                    >s</span
                                >
                            </div>
                        </div>
                    </div>
                </section>

                <div class="self-end">
                    <button
                        type="button"
                        class="btn-primary btn-lg w-full"
                        data-testid="podcast-generate"
                        :disabled="isGenerating || !!generateDisabledReason"
                        :aria-busy="isGenerating ? 'true' : undefined"
                        :title="generateDisabledReason || undefined"
                        @click="generatePodcast"
                    >
                        <Loader2 v-if="isGenerating" class="animate-spin" aria-hidden="true" />
                        <Sparkles v-else aria-hidden="true" />
                        {{
                            isGenerating
                                ? 'Generating…'
                                : podcastScript
                                  ? 'Regenerate script'
                                  : 'Generate script'
                        }}
                    </button>
                    <p
                        v-if="generateDisabledReason && !isGenerating"
                        class="hint mt-2 text-center"
                        data-testid="podcast-generate-hint"
                    >
                        {{ generateDisabledReason }}
                    </p>
                    <p v-else-if="podcastScript && !isGenerating" class="hint mt-2 text-center">
                        Replaces the current script and your edits.
                    </p>
                </div>
            </aside>

            <!-- Script -->
            <section class="min-w-0" aria-label="Podcast script">
                <div v-if="!podcastScript">
                    <EmptyState
                        :icon="Podcast"
                        :title="isGenerating ? 'Writing the script…' : 'No script yet'"
                        :description="
                            isGenerating
                                ? 'Picking the strongest moments and fitting them into your target length. This can take a minute.'
                                : 'Set the target length, then generate a script. You can reorder, trim and add segments before exporting.'
                        "
                    />
                </div>

                <div v-else class="animate-rise-in space-y-4">
                    <!-- Script header -->
                    <div class="panel p-4" data-testid="podcast-script-header">
                        <div class="flex flex-wrap items-start justify-between gap-3">
                            <div class="min-w-0 flex-1">
                                <p class="eyebrow">Episode</p>
                                <h2 class="mt-1 text-[17px] leading-snug font-semibold text-fg">
                                    {{ podcastScript.title }}
                                </h2>
                            </div>
                            <div class="flex flex-col items-end gap-1">
                                <span
                                    data-testid="podcast-duration"
                                    :data-status="rangeError ? 'unknown' : durationStatus"
                                    class="font-mono"
                                    :class="{
                                        chip: !!rangeError,
                                        'chip-success': !rangeError && durationStatus === 'ok',
                                        'chip-warning': !rangeError && durationStatus === 'short',
                                        'chip-danger': !rangeError && durationStatus === 'long',
                                    }"
                                >
                                    {{ formatDuration(currentDuration) }}
                                </span>
                                <span class="text-[11px] text-fg-subtle">
                                    {{ durationExplanation }}
                                </span>
                            </div>
                        </div>
                        <p
                            v-if="podcastScript.summary"
                            class="mt-2 text-[13px] leading-relaxed text-fg-muted"
                        >
                            {{ podcastScript.summary }}
                        </p>
                    </div>

                    <!-- Timeline -->
                    <div class="panel">
                        <div class="flex items-center gap-2 border-b border-line px-4 py-2.5">
                            <h3 class="section-title">Segments</h3>
                            <span class="chip font-mono">{{ podcastScript.segments.length }}</span>
                            <span v-if="voiceoverCount > 0" class="chip-accent">
                                <Mic aria-hidden="true" />
                                {{ voiceoverCount }} voiceover{{ voiceoverCount > 1 ? 's' : '' }}
                            </span>
                            <span
                                class="hint ml-auto hidden items-center gap-1 whitespace-nowrap @5xl:inline-flex"
                            >
                                Reorder: drag or
                                <kbd class="kbd">Alt</kbd><kbd class="kbd">↑</kbd
                                ><kbd class="kbd">↓</kbd>
                            </span>
                            <button
                                type="button"
                                class="btn-subtle btn-sm ml-auto @5xl:ml-2"
                                data-testid="podcast-add-segment"
                                @click="openAddSegmentModal"
                            >
                                <Plus aria-hidden="true" />
                                Add segment
                            </button>
                        </div>

                        <p
                            v-if="podcastScript.segments.length === 0"
                            class="hint px-4 py-8 text-center"
                        >
                            All segments were removed. Add segments from the transcript or generate
                            a new script.
                        </p>

                        <TransitionGroup
                            :name="draggedIndex === null ? 'list' : 'list-drag'"
                            tag="ol"
                            class="relative space-y-1.5 p-2"
                            data-testid="podcast-segments"
                        >
                            <li
                                v-for="(segment, index) in podcastScript.segments"
                                :key="rowKey(index)"
                                :data-row-key="rowKey(index)"
                                :data-testid="`podcast-segment-${index}`"
                                tabindex="0"
                                aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
                                class="group flex items-start gap-2.5 rounded-md border [&.list-leave-active]:inset-x-2 px-2 py-2.5 transition-[background-color,border-color,opacity] duration-150 outline-none focus-visible:border-accent/60"
                                :class="[
                                    segment.type === 'voiceover'
                                        ? 'border-dashed border-accent/40 bg-accent/[0.06]'
                                        : 'border-transparent hover:border-line hover:bg-surface-2',
                                    draggedIndex === index ? 'opacity-50' : '',
                                ]"
                                @keydown="onRowKeydown($event, index)"
                                @dragover="onDragOver($event, index)"
                                @drop.prevent
                            >
                                <!-- Drag handle: the only place a drag starts -->
                                <span
                                    draggable="true"
                                    class="mt-0.5 flex h-6 w-5 shrink-0 cursor-grab items-center justify-center rounded text-fg-faint hover:bg-surface-hover hover:text-fg-muted active:cursor-grabbing"
                                    title="Drag to reorder"
                                    aria-hidden="true"
                                    data-testid="podcast-drag-handle"
                                    @dragstart="onDragStart($event, index)"
                                    @dragend="onDragEnd"
                                >
                                    <GripVertical class="h-4 w-4" />
                                </span>

                                <!-- Speaker colour bar / voiceover icon, in one fixed slot
                                     so labels and text line up across row types -->
                                <span
                                    class="mt-0.5 flex w-4 shrink-0 justify-center self-stretch"
                                    aria-hidden="true"
                                >
                                    <span
                                        v-if="segment.type === 'content'"
                                        class="h-full w-[3px] rounded-full"
                                        :style="{ backgroundColor: colorOf(segment.speaker) }"
                                    />
                                    <Mic v-else class="mt-0.5 h-4 w-4 text-accent-soft" />
                                </span>

                                <div class="min-w-0 flex-1">
                                    <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
                                        <span
                                            v-if="segment.type === 'voiceover'"
                                            class="text-[12px] font-semibold text-accent-soft"
                                        >
                                            Voiceover
                                        </span>
                                        <span
                                            v-else
                                            class="text-[12px] font-semibold"
                                            data-testid="podcast-speaker"
                                            :style="{ color: colorOf(segment.speaker) }"
                                        >
                                            {{ segment.speaker }}
                                        </span>

                                        <!-- Editable timestamps -->
                                        <span class="flex items-center gap-1 font-mono text-[12px]">
                                            <template
                                                v-for="field in ['start', 'end'] as const"
                                                :key="field"
                                            >
                                                <span
                                                    v-if="field === 'end'"
                                                    class="text-fg-faint"
                                                    aria-hidden="true"
                                                    >–</span
                                                >
                                                <input
                                                    v-if="isEditing(index, field)"
                                                    ref="editInput"
                                                    v-model="editingValue"
                                                    class="input input-sm w-[92px] font-mono"
                                                    :aria-label="`${field === 'start' ? 'Start' : 'End'} time`"
                                                    :aria-invalid="editError ? 'true' : undefined"
                                                    :aria-describedby="
                                                        editError ? 'podcast-time-error' : undefined
                                                    "
                                                    data-testid="podcast-time-input"
                                                    @blur="
                                                        finishEditing(index, field, editingValue)
                                                    "
                                                    @keydown.enter.prevent="
                                                        finishEditing(index, field, editingValue)
                                                    "
                                                    @keydown.esc.prevent.stop="cancelEditing"
                                                />
                                                <button
                                                    v-else
                                                    type="button"
                                                    class="rounded px-1.5 py-0.5 text-fg-muted transition-colors hover:bg-surface-hover hover:text-fg"
                                                    :aria-label="`Edit ${field} time ${segment[field]}`"
                                                    :data-testid="`podcast-time-${field}-${index}`"
                                                    @click="startEditing(index, field)"
                                                >
                                                    {{ segment[field] }}
                                                </button>
                                            </template>
                                        </span>
                                    </div>
                                    <p
                                        v-if="editingKey === rowKey(index) && editError"
                                        id="podcast-time-error"
                                        class="mt-1 text-[12px] text-danger"
                                        role="alert"
                                        data-testid="podcast-time-error"
                                    >
                                        {{ editError }}
                                    </p>

                                    <!-- Content segment: transcript text -->
                                    <template v-if="segment.type === 'content'">
                                        <p
                                            class="mt-1 text-[13px] leading-relaxed text-fg select-text"
                                        >
                                            {{ segment.text }}
                                        </p>
                                        <p
                                            v-if="segment.includeReason"
                                            class="mt-1 text-[12px] text-fg-subtle italic"
                                        >
                                            {{ segment.includeReason }}
                                        </p>
                                    </template>

                                    <!-- Voiceover segment: editable transition note -->
                                    <template v-else>
                                        <label
                                            :for="`podcast-note-${rowKey(index)}`"
                                            class="sr-only"
                                        >
                                            Voiceover text
                                        </label>
                                        <textarea
                                            :id="`podcast-note-${rowKey(index)}`"
                                            :value="segment.transitionNote || segment.text"
                                            rows="2"
                                            class="input mt-1.5 min-h-0 text-[13px] italic"
                                            placeholder="What the narrator says to bridge the topics"
                                            :data-testid="`podcast-note-${index}`"
                                            @change="
                                                updateTransitionNote(
                                                    index,
                                                    ($event.target as HTMLTextAreaElement).value,
                                                )
                                            "
                                        />
                                        <p class="hint mt-1">
                                            Suggested voiceover text for the transition. Not
                                            exported; for future TTS or recording.
                                        </p>
                                    </template>
                                </div>

                                <!-- Row actions -->
                                <div
                                    class="flex shrink-0 items-center gap-0.5 opacity-60 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100"
                                >
                                    <button
                                        v-if="
                                            segment.type === 'content' &&
                                            index < podcastScript.segments.length - 1
                                        "
                                        type="button"
                                        class="btn-ghost btn-sm btn-icon"
                                        aria-label="Add voiceover after this segment"
                                        title="Add voiceover transition after this segment"
                                        :data-testid="`podcast-add-voiceover-${index}`"
                                        @click="addVoiceoverSegment(index)"
                                    >
                                        <Mic aria-hidden="true" />
                                    </button>
                                    <button
                                        type="button"
                                        class="btn-ghost btn-sm btn-icon hover:text-danger"
                                        aria-label="Remove segment"
                                        title="Remove segment"
                                        :data-testid="`podcast-remove-${index}`"
                                        @click="removeSegment(index)"
                                    >
                                        <X aria-hidden="true" />
                                    </button>
                                </div>
                            </li>
                        </TransitionGroup>
                    </div>
                </div>
            </section>
        </div>

        <!-- Add Segment dialog -->
        <UiDialog
            :open="showAddSegmentModal"
            title="Add segments from the transcript"
            description="Added segments go to the end of the script."
            size="lg"
            @close="closeAddSegmentModal"
        >
            <div class="relative">
                <Search
                    class="pointer-events-none absolute top-1/2 left-2.5 h-4 w-4 -translate-y-1/2 text-fg-subtle"
                    aria-hidden="true"
                />
                <input
                    v-model="addSearch"
                    type="search"
                    class="input pl-8"
                    placeholder="Search text, speaker or time"
                    aria-label="Search transcript segments"
                    data-autofocus
                    data-testid="podcast-add-search"
                />
            </div>
            <ul class="mt-3 space-y-1" data-testid="podcast-add-list">
                <li
                    v-for="(segment, index) in filteredTranscript"
                    :key="`${segment.start}-${segment.end}-${index}`"
                    class="flex items-start gap-3 rounded-md px-2 py-2 hover:bg-surface-3"
                    :data-testid="`podcast-add-row-${index}`"
                >
                    <span class="mt-0.5 shrink-0 font-mono text-[12px] text-fg-subtle">
                        {{ segment.start }} – {{ segment.end }}
                    </span>
                    <div class="min-w-0 flex-1">
                        <p
                            class="text-[12px] font-semibold"
                            :style="{ color: colorOf(segment.speaker) }"
                        >
                            {{ segment.speaker }}
                        </p>
                        <p class="text-[13px] leading-relaxed text-fg-muted">{{ segment.text }}</p>
                    </div>
                    <span v-if="isSegmentIncluded(segment)" class="chip-success mt-0.5 shrink-0">
                        Included
                    </span>
                    <button
                        v-else
                        type="button"
                        class="btn-secondary btn-sm shrink-0"
                        :aria-label="`Add ${segment.speaker} at ${segment.start}`"
                        :data-testid="`podcast-add-row-button-${index}`"
                        @click="addSegmentFromTranscript(segment)"
                    >
                        <Plus aria-hidden="true" />
                        Add
                    </button>
                </li>
            </ul>
            <p v-if="filteredTranscript.length === 0" class="hint py-6 text-center">
                No segment matches “{{ addSearch }}”.
            </p>
            <template #footer>
                <span class="mr-auto text-[12px] text-fg-subtle" data-testid="podcast-added-count">
                    {{
                        addedCount === 0
                            ? 'Nothing added yet'
                            : `${addedCount} segment${addedCount > 1 ? 's' : ''} added`
                    }}
                </span>
                <button
                    type="button"
                    class="btn-primary"
                    data-testid="podcast-add-done"
                    @click="closeAddSegmentModal"
                >
                    Done
                </button>
            </template>
        </UiDialog>
    </div>
</template>
