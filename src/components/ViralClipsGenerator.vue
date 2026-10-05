<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { mediaUrl } from '../utils/mediaUrl';
import type {
    ClipRole,
    ShortClip,
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
import { formatTime } from '../composables/useTimeFormat';

import FolderOpenIcon from '../assets/icons/folder-open.svg?component';
import { commands, type VerticalPreview } from '../bindings';
import { videoBox, type VideoBox } from '../utils/verticalFraming';
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

const boundaries = computed(() => wordBoundaries(props.segments));

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

// ---- Preview: plays a clip's ranges back to back in the player above. ----
const player = ref<HTMLVideoElement | null>(null);
const previewing = ref<{ id: string; rangeIndex: number } | null>(null);
const mediaSrc = computed(() => (props.hasMediaFile ? mediaUrl(props.inputPath) : ''));

// ---- 9:16 preview: the planned framing applied to the source player. ----
/** CSS size of the 9:16 preview frame. */
const VERTICAL_FRAME = { width: 270, height: 480 };
/** Planned framing per clip id, valid for the ranges it was planned for. */
const verticalPlans = ref(new Map<string, { signature: string; plan: VerticalPreview }>());
const verticalBox = ref<VideoBox | null>(null);
let frameRequest: number | null = null;

function rangeSignature(clip: ShortClip): string {
    return clip.ranges.map((range) => `${range.start}-${range.end}`).join(',');
}

const verticalPlan = computed(() => {
    const current = previewing.value;
    if (!props.state.vertical || !current) return null;
    const clip = clips.value.find((candidate) => candidate.id === current.id);
    const entry = verticalPlans.value.get(current.id);
    return clip && entry?.signature === rangeSignature(clip) ? entry.plan : null;
});

function updateVerticalBox() {
    const video = player.value;
    const plan = verticalPlan.value;
    verticalBox.value =
        video && plan ? videoBox(plan, plan.clips[0], video.currentTime, VERTICAL_FRAME) : null;
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

/** Plan the clip's vertical framing unless a plan for its ranges exists. */
async function ensureVerticalPlan(clip: ShortClip): Promise<boolean> {
    const signature = rangeSignature(clip);
    if (verticalPlans.value.get(clip.id)?.signature === signature) return true;
    if (props.busy || isProcessing.value) return false;

    const runId = await beginRun();
    activeRunId.value = runId;
    isProcessing.value = true;
    emit('update:processing', true);
    emit('update:status', 'Planning the 9:16 framing...');
    try {
        const plan = await commands.planVerticalClips(
            runId,
            props.inputPath,
            [{ segments: toExportSegments(clip), label: clip.title, reason: clip.reason }],
            speakerTurns(props.segments),
        );
        assertActiveRun(runId);
        const plans = new Map(verticalPlans.value);
        plans.set(clip.id, { signature, plan });
        verticalPlans.value = plans;
        emit('update:status', 'Previewing the 9:16 framing.');
        return true;
    } catch (e) {
        if (isRunCancelled(e)) {
            emit('update:status', 'Run cancelled.');
        } else {
            emit('update:status', `Error planning the 9:16 framing: ${errorMessage(e)}`);
        }
        return false;
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
    if (props.state.vertical && !(await ensureVerticalPlan(clip))) return;
    const video = player.value;
    if (!video) return;
    previewing.value = { id: clip.id, rangeIndex: 0 };
    video.currentTime = clip.ranges[0].start;
    void video.play();
    stopFollowingFrames();
    followFrames();
}

function stopPreview() {
    previewing.value = null;
    player.value?.pause();
    stopFollowingFrames();
    verticalBox.value = null;
}

function onTimeUpdate() {
    const video = player.value;
    const current = previewing.value;
    if (!video || !current) return;
    const clip = clips.value.find((candidate) => candidate.id === current.id);
    if (!clip) {
        stopPreview();
        return;
    }
    const step = playbackStep(video.currentTime, clip.ranges, current.rangeIndex);
    if (step.action === 'seek') {
        previewing.value = { id: clip.id, rangeIndex: step.rangeIndex };
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
        let clipSegments = toExport.map((clip) => ({
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

        emit('update:status', `Exporting to ${outputDir}...`);
        if (props.state.vertical) {
            await commands.exportVerticalClips(
                runId,
                props.inputPath,
                clipSegments,
                speakerTurns(props.segments),
                outputDir,
                settings.value.exportQuality,
                props.state.captions
                    ? captionWords(
                          props.segments,
                          toExport.flatMap((clip) => clip.ranges),
                      )
                    : null,
            );
        } else {
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
