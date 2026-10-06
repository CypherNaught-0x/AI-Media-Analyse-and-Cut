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
    setFaceOverride,
    speakerNames,
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
    type DetectedFace,
    type FaceOverride,
    type FaceSpeaker,
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

function verticalRequest(clipSegments: ClipSegment[], clips: ShortClip[]): VerticalRequest {
    return {
        inputPath: props.inputPath,
        segments: clipSegments,
        turns: speakerTurns(props.segments),
        words: wordsAround(clips.flatMap((clip) => clip.ranges)),
        intensity: props.state.intensity,
        captions: props.state.captions,
        looped: clips.map((clip) => clip.looped),
        faces: props.state.faces,
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
    const framing = props.state.vertical ? `9:16|${JSON.stringify(props.state.faces)}` : 'source';
    return `${framing}|${props.state.intensity}|${ranges}`;
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
                verticalRequest([clipSegment(clip)], [clip]),
            );
            entry = { signature, ranges: piecesAsRanges(plan), plan };
        } else {
            const [tightened] = await commands.tightenClips(
                runId,
                props.inputPath,
                [clipSegment(clip)],
                wordsAround(clip.ranges),
                props.state.intensity,
                [clip.looped],
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

// ---- Faces: what 9:16 framing found, for the user to rule out (a face on
// a TV) or name. Overrides live in the workspace state and go with every
// plan and export. ----
const detectedFaces = ref<DetectedFace[]>([]);
const speakers = computed(() => speakerNames(props.segments));

watch(
    () => props.inputPath,
    () => {
        detectedFaces.value = [];
    },
);

function faceOverride(face: DetectedFace): FaceOverride | null {
    return face.applied === null ? null : (props.state.faces[face.applied] ?? null);
}

function faceIgnored(face: DetectedFace): boolean {
    return faceOverride(face)?.ignored ?? false;
}

/** The speaker select's value: `auto`, `nobody` or `named:<name>`. */
function faceSpeakerValue(face: DetectedFace): string {
    const speaker = faceOverride(face)?.speaker ?? { kind: 'auto' };
    return speaker.kind === 'named' ? `named:${speaker.name}` : speaker.kind;
}

function autoLabel(face: DetectedFace): string {
    const pinned = faceOverride(face)?.speaker.kind;
    if (pinned && pinned !== 'auto') return 'Auto';
    if (!face.speaker) return 'Auto (no speaker found)';
    return `Auto (${face.speaker}${face.confident ? '' : ', unsure'})`;
}

function changeFace(index: number, change: Partial<Pick<FaceOverride, 'ignored' | 'speaker'>>) {
    const face = detectedFaces.value[index];
    if (!face) return;
    const faces = setFaceOverride(props.state.faces, face, change);
    const applied = face.applied ?? faces.length - 1;
    detectedFaces.value = detectedFaces.value.map((f, i) => (i === index ? { ...f, applied } : f));
    updateState({ faces });
}

function setFaceSpeaker(index: number, value: string) {
    const speaker: FaceSpeaker = value.startsWith('named:')
        ? { kind: 'named', name: value.slice('named:'.length) }
        : value === 'nobody'
          ? { kind: 'nobody' }
          : { kind: 'auto' };
    changeFace(index, { speaker });
}

async function detectFaces() {
    const toScan = selectedClips.value.length ? selectedClips.value : clips.value;
    if (props.busy || isProcessing.value || toScan.length === 0 || !props.hasMediaFile) return;

    const runId = await beginRun();
    activeRunId.value = runId;
    isProcessing.value = true;
    emit('update:processing', true);
    emit('update:status', 'Finding faces...');
    stopPreview();
    try {
        const faces = await commands.detectVerticalFaces(
            runId,
            verticalRequest(toScan.map(clipSegment), toScan),
        );
        assertActiveRun(runId);
        detectedFaces.value = faces;
        emit(
            'update:status',
            `Found ${faces.length} face${faces.length === 1 ? '' : 's'} in ${toScan.length} clip${toScan.length === 1 ? '' : 's'}.`,
        );
    } catch (e) {
        if (isRunCancelled(e)) {
            emit('update:status', 'Run cancelled.');
            return;
        }
        emit('update:status', `Error finding faces: ${errorMessage(e)}`);
    } finally {
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isProcessing.value = false;
            emit('update:processing', false);
        }
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
                verticalRequest(clipSegments, toExport),
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
                    toExport.map((clip) => clip.looped),
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

            <!-- Faces the 9:16 framing works with -->
            <section
                v-if="state.vertical && hasMediaFile"
                class="mb-6 rounded-2xl border border-white/10 bg-black/20 p-5"
                data-testid="clips-faces"
            >
                <div class="mb-3 flex flex-wrap items-center gap-3">
                    <h3 class="text-sm font-bold uppercase tracking-wider text-gray-300">Faces</h3>
                    <span class="flex-1 text-xs text-gray-500">
                        The camera follows these in 9:16. Turn off faces that aren't people in the
                        room (a TV, a poster); name a face if the wrong one gets followed.
                    </span>
                    <button
                        type="button"
                        class="rounded-lg bg-white/10 px-3 py-1.5 text-xs font-medium text-white hover:bg-white/20 disabled:opacity-40"
                        :disabled="isProcessing || busy"
                        data-testid="clips-detect-faces"
                        @click="detectFaces"
                    >
                        {{ detectedFaces.length ? 'Refresh faces' : 'Find faces' }}
                    </button>
                </div>
                <p v-if="!detectedFaces.length && state.faces.length" class="text-xs text-gray-500">
                    {{ state.faces.length }} face setting{{ state.faces.length === 1 ? '' : 's' }}
                    saved; find faces to review them.
                </p>
                <ul
                    v-if="detectedFaces.length"
                    class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4"
                >
                    <template v-for="(face, index) in detectedFaces" :key="index">
                        <!-- One group per camera setup: the same person has a face in each. -->
                        <li
                            v-if="index === 0 || detectedFaces[index - 1].setup !== face.setup"
                            class="col-span-full mt-1 text-[11px] font-semibold uppercase tracking-wider text-gray-500"
                        >
                            Camera setup {{ face.setup }}
                        </li>
                        <li
                            class="flex flex-col gap-2 rounded-xl border border-white/10 bg-black/30 p-3"
                            :data-testid="`clips-face-${index}`"
                        >
                            <div class="flex items-center gap-3">
                                <img
                                    v-if="face.thumbnail"
                                    :src="face.thumbnail"
                                    alt=""
                                    class="h-16 w-16 rounded-lg object-cover transition"
                                    :class="faceIgnored(face) ? 'opacity-30 grayscale' : ''"
                                />
                                <div
                                    v-else
                                    class="h-16 w-16 rounded-lg bg-white/5"
                                    :class="faceIgnored(face) ? 'opacity-30' : ''"
                                />
                                <label class="flex flex-col gap-1 text-xs text-gray-300">
                                    <span class="flex items-center gap-1.5">
                                        <input
                                            type="checkbox"
                                            role="switch"
                                            :checked="!faceIgnored(face)"
                                            :data-testid="`clips-face-enabled-${index}`"
                                            class="rounded border-white/20 bg-white/10 text-pink-500 focus:ring-pink-500/50"
                                            @change="
                                                changeFace(index, {
                                                    ignored: !($event.target as HTMLInputElement)
                                                        .checked,
                                                })
                                            "
                                        />
                                        Follow
                                    </span>
                                    <span class="text-gray-500"
                                        >{{ Math.round(face.seconds) }} s on screen</span
                                    >
                                </label>
                            </div>
                            <select
                                :value="faceSpeakerValue(face)"
                                :disabled="faceIgnored(face)"
                                :aria-label="`Speaker of face ${index + 1}`"
                                :data-testid="`clips-face-speaker-${index}`"
                                class="w-full rounded-lg border border-white/10 bg-black/40 px-2 py-1.5 text-xs text-white disabled:opacity-40"
                                @change="
                                    setFaceSpeaker(
                                        index,
                                        ($event.target as HTMLSelectElement).value,
                                    )
                                "
                            >
                                <option value="auto">{{ autoLabel(face) }}</option>
                                <option
                                    v-for="name in speakers"
                                    :key="name"
                                    :value="`named:${name}`"
                                >
                                    {{ name }}
                                </option>
                                <option value="nobody">Not a speaker</option>
                            </select>
                        </li>
                    </template>
                </ul>
            </section>

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
