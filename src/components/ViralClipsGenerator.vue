<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { mediaUrl } from '../utils/mediaUrl';
import type {
    ClipRole,
    ShortClip,
    ShortClipRange,
    SilenceInterval,
    TranscriptSegment,
    ViralClipsWorkspaceState,
} from '../types';
import { useSettings } from '../composables/useSettings';
import { trimClipBoundarySilence } from '../utils/clipSilence';
import { padClipSegments } from '../utils/clips';
import {
    clipDuration,
    fromCandidates,
    captionWords,
    playbackStep,
    speakerTurns,
    toExportSegments,
    trimClip,
    wordBoundaries,
} from '../utils/shortClips';
import { beginRun, isRunCancelled } from '../composables/useRunCancellation';
import { formatTime, parseTime } from '../composables/useTimeFormat';

import FolderOpenIcon from '../assets/icons/folder-open.svg?component';
import {
    commands,
    type ClipSegment,
    type Intensity,
    type VerticalPreview,
    type VerticalRequest,
} from '../bindings';
import { captionAt, videoBox, type ShownWord, type VideoBox } from '../utils/verticalFraming';
import { errorMessage } from '../utils/appError';

interface Props {
    segments: TranscriptSegment[];
    inputPath: string;
    hasMediaFile: boolean;
    state: ViralClipsWorkspaceState;
    cancelGeneration: number;
    /** Another job (analysis, translation, export, ...) is running app-wide. */
    busy: boolean;
}

const props = defineProps<Props>();

const emit = defineEmits<{
    'update:status': [message: string];
    'update:processing': [isProcessing: boolean];
    'update:state': [state: ViralClipsWorkspaceState];
}>();

const { settings } = useSettings();

const isProcessing = ref(false);
const silenceIntervalsCache = ref<{ path: string; intervals: SilenceInterval[] } | null>(null);
const activeRunId = ref<number | null>(null);

function invalidateRun() {
    activeRunId.value = null;
    isProcessing.value = false;
    emit('update:processing', false);
}

function assertActiveRun(runId: number) {
    if (activeRunId.value !== runId) {
        throw new Error('Run cancelled.');
    }
}

watch(() => props.cancelGeneration, invalidateRun);

function updateState(patch: Partial<ViralClipsWorkspaceState>) {
    emit('update:state', {
        ...props.state,
        ...patch,
    });
}

function setting<K extends keyof ViralClipsWorkspaceState>(key: K) {
    return computed({
        get: () => props.state[key],
        set: (value: ViralClipsWorkspaceState[K]) => updateState({ [key]: value }),
    });
}

const clips = computed(() => props.state.clips);
const clipCount = setting('count');
const clipMinDuration = setting('minDuration');
const clipMaxDuration = setting('maxDuration');
const clipTopic = setting('topic');
const lastExportPath = computed(() => props.state.lastExportPath);
const selectedClips = computed(() => clips.value.filter((clip) => clip.selected));

const TOGGLES = [
    {
        key: 'allowSplicing',
        title: 'Smart splicing',
        text: 'Combine separate moments, e.g. a cold-open hook.',
    },
    {
        key: 'looped',
        title: 'Looped shorts',
        text: 'Open on the resolution; the ending leads back into it.',
    },
    {
        key: 'trimBoundarySilence',
        title: 'Trim edge silence',
        text: 'Remove silence at clip starts and ends on export.',
    },
    {
        key: 'vertical',
        title: 'Vertical 9:16',
        text: 'Export portrait videos that follow whoever is speaking.',
    },
    {
        key: 'captions',
        title: 'Captions',
        text: 'Burn word-by-word captions into 9:16 exports.',
    },
] as const;

const INTENSITIES: { value: Intensity; label: string; text: string }[] = [
    { value: 'off', label: 'Off', text: 'Keep every pause and filler.' },
    { value: 'chill', label: 'Chill', text: 'Pauses up to 350 ms, no fillers.' },
    { value: 'punchy', label: 'Punchy', text: 'Pauses up to 180 ms, no fillers or stutters.' },
    { value: 'hyper', label: 'Hyper', text: 'Pauses up to 90 ms: rapid-fire.' },
];
const intensity = setting('intensity');
const intensityText = computed(
    () => INTENSITIES.find((option) => option.value === intensity.value)?.text ?? '',
);

const boundaries = computed(() => wordBoundaries(props.segments));

/** Transcript words around `ranges`, for fillers and captions. */
function wordsAround(ranges: { start: number; end: number }[]) {
    // A margin covers the export padding added later.
    return captionWords(
        props.segments,
        ranges.map((range) => ({ start: range.start - 2, end: range.end + 2 })),
    );
}

function verticalRequest(clipSegments: ClipSegment[], ranges: ShortClipRange[]): VerticalRequest {
    return {
        inputPath: props.inputPath,
        segments: clipSegments,
        turns: speakerTurns(props.segments),
        words: wordsAround(ranges),
        intensity: props.state.intensity,
        captions: props.state.captions,
    };
}

function updateClip(id: string, change: (clip: ShortClip) => ShortClip) {
    updateState({
        clips: clips.value.map((clip) => (clip.id === id ? change(clip) : clip)),
    });
}

function trim(clip: ShortClip, edge: 'start' | 'end', direction: -1 | 1) {
    updateClip(clip.id, (current) => trimClip(current, edge, direction, boundaries.value));
}

function setSelected(clip: ShortClip, selected: boolean) {
    updateClip(clip.id, (current) => ({ ...current, selected }));
}

const ROLE_LABELS: Record<ClipRole, string> = {
    hook: 'Hook',
    body: 'Body',
    payoff: 'Payoff',
    loop_opener: 'Loop opener',
    closing: 'Closing',
};

const RATING_LABELS = [
    ['hook', 'Hook'],
    ['standalone', 'Standalone'],
    ['emotion', 'Emotion'],
    ['info', 'Info'],
] as const;

function clock(seconds: number): string {
    // Milliseconds are noise on a card; show whole seconds.
    return formatTime(Math.round(seconds)).replace(/\.000$/, '');
}

// ---- Preview: plays what the export will contain (tightened ranges and,
// in 9:16 mode, the planned framing) in the player above. ----
/** CSS size of the 9:16 preview frame. */
const VERTICAL_FRAME = { width: 270, height: 480 };

interface PlaybackRange {
    start: number;
    end: number;
}

/** What a clip's preview plays, valid for the settings it was made with. */
interface PreparedPreview {
    signature: string;
    ranges: PlaybackRange[];
    plan: VerticalPreview | null;
}

const player = ref<HTMLVideoElement | null>(null);
const previewing = ref<{ id: string; rangeIndex: number; ranges: PlaybackRange[] } | null>(null);
const mediaSrc = computed(() => (props.hasMediaFile ? mediaUrl(props.inputPath) : ''));
const prepared = ref(new Map<string, PreparedPreview>());
const verticalBox = ref<VideoBox | null>(null);
const verticalCaption = ref<ShownWord[] | null>(null);
let frameRequest: number | null = null;

function previewSignature(clip: ShortClip): string {
    const ranges = clip.ranges.map((range) => `${range.start}-${range.end}`).join(',');
    return `${props.state.vertical ? '9:16' : 'source'}|${props.state.intensity}|${ranges}`;
}

function clipSegment(clip: ShortClip): ClipSegment {
    return { segments: toExportSegments(clip), label: clip.title, reason: clip.reason };
}

/** Consecutive pieces of the plan as playback ranges, joining touching ones. */
function piecesAsRanges(plan: VerticalPreview): PlaybackRange[] {
    const ranges: PlaybackRange[] = [];
    for (const piece of plan.clips[0] ?? []) {
        const last = ranges[ranges.length - 1];
        if (last && Math.abs(last.end - piece.start) < 1e-6) last.end = piece.end;
        else ranges.push({ start: piece.start, end: piece.end });
    }
    return ranges;
}

const verticalPlan = computed(() => {
    const current = previewing.value;
    if (!props.state.vertical || !current) return null;
    return prepared.value.get(current.id)?.plan ?? null;
});

function updateVerticalBox() {
    const video = player.value;
    const plan = verticalPlan.value;
    verticalBox.value =
        video && plan ? videoBox(plan, plan.clips[0], video.currentTime, VERTICAL_FRAME) : null;
    verticalCaption.value =
        video && plan ? captionAt(plan.captions[0] ?? [], video.currentTime) : null;
}

function followFrames() {
    const video = player.value as
        | (HTMLVideoElement & { requestVideoFrameCallback?: (callback: () => void) => number })
        | null;
    if (!video || !verticalPlan.value) return;
    updateVerticalBox();
    // Per decoded frame where supported, so the crop moves with the picture.
    frameRequest = video.requestVideoFrameCallback
        ? video.requestVideoFrameCallback(followFrames)
        : requestAnimationFrame(followFrames);
}

function stopFollowingFrames() {
    const video = player.value as
        (HTMLVideoElement & { cancelVideoFrameCallback?: (handle: number) => void }) | null;
    if (frameRequest !== null) {
        if (video?.cancelVideoFrameCallback) video.cancelVideoFrameCallback(frameRequest);
        else cancelAnimationFrame(frameRequest);
        frameRequest = null;
    }
}

onBeforeUnmount(stopFollowingFrames);

/**
 * What the clip's preview plays: its ranges as they are, or (tightened or
 * in 9:16) as the backend prepares them for export.
 */
async function preparePreview(clip: ShortClip): Promise<PreparedPreview | null> {
    const signature = previewSignature(clip);
    const existing = prepared.value.get(clip.id);
    if (existing?.signature === signature) return existing;
    if (!props.state.vertical && props.state.intensity === 'off') {
        return { signature, ranges: clip.ranges, plan: null };
    }
    if (props.busy || isProcessing.value) return null;

    const runId = await beginRun();
    activeRunId.value = runId;
    isProcessing.value = true;
    emit('update:processing', true);
    emit(
        'update:status',
        props.state.vertical ? 'Planning the 9:16 framing...' : 'Tightening the clip...',
    );
    try {
        let entry: PreparedPreview;
        if (props.state.vertical) {
            const plan = await commands.planVerticalClips(
                runId,
                verticalRequest([clipSegment(clip)], clip.ranges),
            );
            entry = { signature, ranges: piecesAsRanges(plan), plan };
        } else {
            const [tightened] = await commands.tightenClips(
                runId,
                props.inputPath,
                [clipSegment(clip)],
                wordsAround(clip.ranges),
                props.state.intensity,
            );
            const ranges = (tightened?.segments ?? []).map((segment) => ({
                start: parseTime(segment.start),
                end: parseTime(segment.end),
            }));
            entry = { signature, ranges: ranges.length ? ranges : clip.ranges, plan: null };
        }
        assertActiveRun(runId);
        const next = new Map(prepared.value);
        next.set(clip.id, entry);
        prepared.value = next;
        emit(
            'update:status',
            props.state.vertical ? 'Previewing the 9:16 framing.' : 'Previewing.',
        );
        return entry;
    } catch (e) {
        if (isRunCancelled(e)) {
            emit('update:status', 'Run cancelled.');
        } else {
            emit('update:status', `Error preparing the preview: ${errorMessage(e)}`);
        }
        return null;
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isProcessing.value = false;
            emit('update:processing', false);
        }
    }
}

async function preview(clip: ShortClip) {
    if (!player.value) return;
    if (previewing.value?.id === clip.id) {
        stopPreview();
        return;
    }
    const entry = await preparePreview(clip);
    const video = player.value;
    if (!entry || !video || entry.ranges.length === 0) return;
    previewing.value = { id: clip.id, rangeIndex: 0, ranges: entry.ranges };
    video.currentTime = entry.ranges[0].start;
    void video.play();
    stopFollowingFrames();
    followFrames();
}

function stopPreview() {
    previewing.value = null;
    player.value?.pause();
    stopFollowingFrames();
    verticalBox.value = null;
    verticalCaption.value = null;
}

function onTimeUpdate() {
    const video = player.value;
    const current = previewing.value;
    if (!video || !current) return;
    const step = playbackStep(video.currentTime, current.ranges, current.rangeIndex);
    if (step.action === 'seek') {
        previewing.value = { ...current, rangeIndex: step.rangeIndex };
        video.currentTime = step.to;
    } else if (step.action === 'stop') {
        stopPreview();
    }
}

// ---- Generation ----
async function generateClips() {
    if (props.busy || props.segments.length === 0) return;

    const runId = await beginRun();
    activeRunId.value = runId;
    emit('update:status', 'Finding clips...');
    isProcessing.value = true;
    emit('update:processing', true);
    stopPreview();

    try {
        const candidates = await commands.selectClips(
            runId,
            { baseUrl: settings.value.baseUrl, model: settings.value.model },
            props.segments,
            {
                count: clipCount.value,
                minSeconds: clipMinDuration.value,
                maxSeconds: clipMaxDuration.value,
                topic: clipTopic.value.trim() || null,
                allowSplicing: props.state.allowSplicing,
                looped: props.state.looped,
            },
        );
        assertActiveRun(runId);
        updateState({ clips: fromCandidates(candidates) });
        emit(
            'update:status',
            `Found ${candidates.length} clip${candidates.length === 1 ? '' : 's'}.`,
        );
    } catch (e) {
        if (isRunCancelled(e)) {
            emit('update:status', 'Run cancelled.');
            return;
        }
        emit('update:status', `Error finding clips: ${errorMessage(e)}`);
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isProcessing.value = false;
            emit('update:processing', false);
        }
    }
}

// ---- Export ----
async function exportClips(toExport: ShortClip[]) {
    if (props.busy || toExport.length === 0) return;
    if (!props.hasMediaFile) {
        emit('update:status', 'Select a valid media file before exporting clips.');
        return;
    }

    const runId = await beginRun();
    activeRunId.value = runId;
    emit('update:status', 'Exporting clips...');
    isProcessing.value = true;
    emit('update:processing', true);
    stopPreview();

    try {
        const outputDir = props.inputPath.replace(/\.[^/\\.]+$/, '') + '_clips';
        let clipSegments: ClipSegment[] = toExport.map((clip) => ({
            segments: toExportSegments(clip),
            label: clip.title,
            reason: clip.reason,
        }));

        if (props.state.trimBoundarySilence) {
            emit('update:status', 'Detecting clip boundary silence...');
            // Cache the detected silence per input path so a changed source file
            // never reuses stale intervals to mis-trim clips.
            if (
                !silenceIntervalsCache.value ||
                silenceIntervalsCache.value.path !== props.inputPath
            ) {
                const intervals = await commands.detectSilence(
                    runId,
                    props.inputPath,
                    null /* default minimum silence */,
                );
                assertActiveRun(runId);
                silenceIntervalsCache.value = { path: props.inputPath, intervals };
            }
            clipSegments = clipSegments.map((clip) => ({
                ...clip,
                segments: trimClipBoundarySilence(
                    clip.segments,
                    silenceIntervalsCache.value?.intervals ?? [],
                ),
            }));
        }

        // Padding applies after the silence trim, so it adds deliberate breathing
        // room rather than restoring the silence that was just removed.
        clipSegments = clipSegments.map((clip) => ({
            ...clip,
            segments: padClipSegments(
                clip.segments,
                settings.value.preClipPadding,
                settings.value.postClipPadding,
            ),
        }));

        const allRanges = toExport.flatMap((clip) => clip.ranges);
        emit('update:status', `Exporting to ${outputDir}...`);
        if (props.state.vertical) {
            // Tightening happens in the backend, so captions can follow it.
            await commands.exportVerticalClips(
                runId,
                verticalRequest(clipSegments, allRanges),
                outputDir,
                settings.value.exportQuality,
            );
        } else {
            if (props.state.intensity !== 'off') {
                emit('update:status', 'Tightening clips...');
                clipSegments = await commands.tightenClips(
                    runId,
                    props.inputPath,
                    clipSegments,
                    wordsAround(allRanges),
                    props.state.intensity,
                );
                assertActiveRun(runId);
                emit('update:status', `Exporting to ${outputDir}...`);
            }
            await commands.exportClips(
                runId,
                props.inputPath,
                clipSegments,
                outputDir,
                // fastMode off: stream copy can only start on a keyframe, so
                // clips would open early or on a frame that can't be decoded.
                // Social clips need exact cuts.
                false,
                settings.value.exportQuality,
            );
        }
        assertActiveRun(runId);

        updateState({ lastExportPath: outputDir });
        emit(
            'update:status',
            `Exported ${toExport.length} clip${toExport.length === 1 ? '' : 's'} to ${outputDir}`,
        );
    } catch (e) {
        if (isRunCancelled(e)) {
            emit('update:status', 'Run cancelled.');
            return;
        }
        emit('update:status', `Error exporting clips: ${errorMessage(e)}`);
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isProcessing.value = false;
            emit('update:processing', false);
        }
    }
}

async function openExportFolder() {
    if (lastExportPath.value) {
        await commands.openFolder(lastExportPath.value);
    }
}
</script>

<template>
    <div class="backdrop-blur-md bg-white/5 border border-white/10 p-8 rounded-3xl shadow-2xl">
        <div class="flex justify-between items-center mb-6">
            <h2 class="text-2xl font-bold text-white">Viral Clips</h2>
        </div>

        <!-- Options -->
        <div class="grid grid-cols-1 md:grid-cols-3 gap-6 mb-6">
            <div>
                <label
                    for="clips-count"
                    class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >Count</label
                >
                <input
                    id="clips-count"
                    v-model.number="clipCount"
                    type="number"
                    min="1"
                    max="10"
                    class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white"
                />
            </div>
            <div>
                <label
                    for="clips-min"
                    class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >Min seconds</label
                >
                <input
                    id="clips-min"
                    v-model.number="clipMinDuration"
                    type="number"
                    min="5"
                    class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white"
                />
            </div>
            <div>
                <label
                    for="clips-max"
                    class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider"
                    >Max seconds</label
                >
                <input
                    id="clips-max"
                    v-model.number="clipMaxDuration"
                    type="number"
                    min="10"
                    class="w-full p-3 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white"
                />
            </div>
        </div>

        <div class="mb-6">
            <label
                for="clips-topic"
                class="block text-xs font-medium text-gray-400 mb-2 uppercase tracking-wider"
                >Topic (optional)</label
            >
            <input
                id="clips-topic"
                v-model="clipTopic"
                type="text"
                class="w-full p-4 rounded-xl bg-black/20 border border-white/10 focus:border-pink-500/50 outline-none text-white placeholder-gray-600"
                placeholder="e.g. 'Funny moments', 'Technical explanation', 'Rants'..."
            />
        </div>

        <div class="mb-8 grid grid-cols-1 gap-3 md:grid-cols-3">
            <label
                v-for="toggle in TOGGLES"
                :key="toggle.key"
                class="flex cursor-pointer items-start gap-3 rounded-xl border border-white/5 bg-black/20 p-4"
            >
                <input
                    type="checkbox"
                    role="switch"
                    :data-testid="`clips-toggle-${toggle.key}`"
                    :checked="state[toggle.key]"
                    class="mt-0.5 h-4 w-4 rounded border-white/20 bg-white/10 text-pink-500 focus:ring-pink-500/50"
                    @change="
                        updateState({
                            [toggle.key]: ($event.target as HTMLInputElement).checked,
                        })
                    "
                />
                <span>
                    <span class="block text-sm font-semibold text-gray-200">{{
                        toggle.title
                    }}</span>
                    <span class="block text-xs text-gray-500">{{ toggle.text }}</span>
                </span>
            </label>
        </div>

        <div class="mb-8 flex flex-wrap items-center gap-3">
            <span class="text-xs font-bold uppercase tracking-wider text-gray-400">Tightening</span>
            <div
                class="flex overflow-hidden rounded-xl border border-white/10"
                role="radiogroup"
                aria-label="Tightening"
            >
                <button
                    v-for="option in INTENSITIES"
                    :key="option.value"
                    type="button"
                    role="radio"
                    :aria-checked="intensity === option.value"
                    :data-testid="`clips-intensity-${option.value}`"
                    class="px-4 py-2 text-sm transition-colors"
                    :class="
                        intensity === option.value
                            ? 'bg-pink-600/80 font-semibold text-white'
                            : 'bg-black/20 text-gray-300 hover:bg-white/10'
                    "
                    @click="intensity = option.value"
                >
                    {{ option.label }}
                </button>
            </div>
            <span class="text-xs text-gray-500">{{ intensityText }}</span>
        </div>

        <button
            @click="generateClips"
            :disabled="isProcessing || busy"
            data-testid="clips-generate"
            class="w-full mb-8 bg-gradient-to-r from-pink-600 to-purple-600 hover:from-pink-500 hover:to-purple-500 text-white font-bold py-4 px-6 rounded-2xl shadow-lg transition-all disabled:opacity-50 disabled:cursor-not-allowed"
        >
            {{ isProcessing ? 'Processing...' : clips.length ? 'Find Clips Again' : 'Find Clips' }}
        </button>

        <div v-if="clips.length > 0">
            <!-- Preview player; in 9:16 mode it shows the planned framing. -->
            <div
                v-if="hasMediaFile"
                class="mb-6 overflow-hidden rounded-2xl border border-white/10 bg-black"
            >
                <div
                    :class="
                        verticalBox
                            ? 'relative mx-auto my-4 overflow-hidden rounded-lg ring-1 ring-white/20'
                            : ''
                    "
                    :style="
                        verticalBox
                            ? {
                                  width: `${VERTICAL_FRAME.width}px`,
                                  height: `${VERTICAL_FRAME.height}px`,
                              }
                            : undefined
                    "
                    data-testid="clips-player-frame"
                >
                    <video
                        ref="player"
                        :src="mediaSrc"
                        :class="verticalBox ? 'absolute max-w-none' : 'mx-auto max-h-80 w-full'"
                        :style="
                            verticalBox
                                ? {
                                      width: `${verticalBox.width}px`,
                                      height: `${verticalBox.height}px`,
                                      left: `${verticalBox.left}px`,
                                      top: `${verticalBox.top}px`,
                                  }
                                : undefined
                        "
                        preload="metadata"
                        :controls="!verticalBox"
                        data-testid="clips-player"
                        @timeupdate="onTimeUpdate"
                    />
                    <!-- Captions as the export burns them in (68% down). -->
                    <p
                        v-if="verticalBox && verticalCaption"
                        class="vertical-caption pointer-events-none absolute inset-x-3 text-center"
                        data-testid="clips-player-caption"
                    >
                        <span
                            v-for="(word, i) in verticalCaption"
                            :key="i"
                            :class="word.active ? 'text-[#ffd60a]' : 'text-white'"
                            >{{ word.text }}{{ i < verticalCaption.length - 1 ? ' ' : '' }}</span
                        >
                    </p>
                </div>
            </div>

            <!-- Clip cards -->
            <ul class="grid grid-cols-1 gap-4 lg:grid-cols-2" data-testid="clip-cards">
                <li
                    v-for="clip in clips"
                    :key="clip.id"
                    class="flex flex-col rounded-2xl border bg-black/20 p-5 transition-colors"
                    :class="
                        previewing?.id === clip.id
                            ? 'border-pink-500/60'
                            : clip.selected
                              ? 'border-white/10'
                              : 'border-white/5 opacity-60'
                    "
                    :data-testid="`clip-card-${clip.id}`"
                >
                    <div class="mb-2 flex items-start justify-between gap-3">
                        <h3 class="text-base font-bold text-pink-300">
                            {{ clip.title || 'Untitled clip' }}
                        </h3>
                        <span
                            v-if="clip.score !== null"
                            class="shrink-0 rounded-full bg-pink-500/15 px-2.5 py-0.5 text-sm font-bold text-pink-200"
                            title="Ranking score (0-100)"
                            >{{ clip.score }}</span
                        >
                    </div>

                    <p v-if="clip.hookLine" class="mb-2 text-sm italic text-gray-200">
                        “{{ clip.hookLine }}”
                    </p>
                    <p class="mb-3 text-xs leading-relaxed text-gray-400">{{ clip.reason }}</p>

                    <div class="mb-3 flex flex-wrap items-center gap-1.5 text-xs">
                        <span class="rounded bg-white/10 px-2 py-0.5 font-mono text-gray-200">{{
                            clock(clipDuration(clip))
                        }}</span>
                        <span
                            v-if="clip.looped"
                            class="rounded bg-purple-500/20 px-2 py-0.5 font-semibold text-purple-200"
                            >Loop</span
                        >
                        <span
                            v-for="(range, i) in clip.ranges"
                            :key="i"
                            class="rounded border border-white/10 px-2 py-0.5 text-gray-400"
                        >
                            <span v-if="clip.ranges.length > 1" class="mr-1 text-gray-300">
                                {{ ROLE_LABELS[range.role] }}
                            </span>
                            <span class="font-mono">
                                {{ clock(range.start) }}–{{ clock(range.end) }}</span
                            >
                        </span>
                    </div>

                    <dl
                        v-if="clip.ratings"
                        class="mb-4 grid grid-cols-5 gap-2 text-center text-[11px] text-gray-500"
                    >
                        <div v-for="[key, label] in RATING_LABELS" :key="key">
                            <dt>{{ label }}</dt>
                            <dd class="font-semibold text-gray-200">{{ clip.ratings[key] }}</dd>
                        </div>
                        <div v-if="clip.looped">
                            <dt>Loop</dt>
                            <dd class="font-semibold text-gray-200">
                                {{ clip.ratings.loopContinuity }}
                            </dd>
                        </div>
                    </dl>

                    <div class="mt-auto flex flex-wrap items-center gap-2">
                        <button
                            type="button"
                            class="rounded-lg bg-white/10 px-3 py-1.5 text-xs font-medium text-white hover:bg-white/20 disabled:opacity-40"
                            :disabled="!hasMediaFile"
                            :data-testid="`clip-preview-${clip.id}`"
                            @click="preview(clip)"
                        >
                            {{ previewing?.id === clip.id ? 'Stop' : 'Preview' }}
                        </button>

                        <div
                            class="flex items-center gap-1 text-xs text-gray-400"
                            role="group"
                            aria-label="Trim start"
                        >
                            Start
                            <button
                                type="button"
                                class="rounded bg-white/5 px-1.5 py-1 hover:bg-white/15"
                                aria-label="Start one word earlier"
                                :data-testid="`clip-start-earlier-${clip.id}`"
                                @click="trim(clip, 'start', -1)"
                            >
                                ◀
                            </button>
                            <button
                                type="button"
                                class="rounded bg-white/5 px-1.5 py-1 hover:bg-white/15"
                                aria-label="Start one word later"
                                :data-testid="`clip-start-later-${clip.id}`"
                                @click="trim(clip, 'start', 1)"
                            >
                                ▶
                            </button>
                        </div>
                        <div
                            class="flex items-center gap-1 text-xs text-gray-400"
                            role="group"
                            aria-label="Trim end"
                        >
                            End
                            <button
                                type="button"
                                class="rounded bg-white/5 px-1.5 py-1 hover:bg-white/15"
                                aria-label="End one word earlier"
                                @click="trim(clip, 'end', -1)"
                            >
                                ◀
                            </button>
                            <button
                                type="button"
                                class="rounded bg-white/5 px-1.5 py-1 hover:bg-white/15"
                                aria-label="End one word later"
                                @click="trim(clip, 'end', 1)"
                            >
                                ▶
                            </button>
                        </div>

                        <label class="ml-auto flex items-center gap-1.5 text-xs text-gray-300">
                            <input
                                type="checkbox"
                                :checked="clip.selected"
                                :data-testid="`clip-select-${clip.id}`"
                                class="rounded border-white/20 bg-white/10 text-pink-500 focus:ring-pink-500/50"
                                @change="
                                    setSelected(clip, ($event.target as HTMLInputElement).checked)
                                "
                            />
                            Include
                        </label>
                        <button
                            type="button"
                            class="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-gray-200 hover:bg-white/10 disabled:opacity-40"
                            :disabled="isProcessing || busy || !hasMediaFile"
                            :data-testid="`clip-export-${clip.id}`"
                            @click="exportClips([clip])"
                        >
                            Export
                        </button>
                    </div>
                </li>
            </ul>

            <div class="flex gap-4 mt-6">
                <button
                    @click="exportClips(selectedClips)"
                    :disabled="isProcessing || busy || !hasMediaFile || selectedClips.length === 0"
                    data-testid="clips-export-selected"
                    class="flex-1 bg-gray-700 hover:bg-gray-600 text-white font-bold py-4 px-6 rounded-2xl border border-gray-600 hover:border-gray-500 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
                >
                    Export {{ selectedClips.length }} selected
                </button>
                <button
                    v-if="lastExportPath"
                    @click="openExportFolder"
                    class="px-6 bg-gray-800 hover:bg-gray-700 text-white font-bold rounded-2xl border border-gray-700 transition-all"
                    title="Open folder"
                    aria-label="Open export folder"
                >
                    <FolderOpenIcon class="h-6 w-6" />
                </button>
            </div>
        </div>
    </div>
</template>

<style scoped>
.vertical-caption {
    top: 68%;
    transform: translateY(-50%);
    font-family: Poppins, ui-sans-serif, system-ui, sans-serif;
    font-size: 23px;
    font-weight: 800;
    line-height: 1.15;
    /* A dark outline and soft shadow, like the burned-in captions. */
    text-shadow:
        0 0 3px #000,
        0 0 3px #000,
        0 0 3px #000,
        1px 1px 2px rgb(0 0 0 / 0.6);
}
</style>
