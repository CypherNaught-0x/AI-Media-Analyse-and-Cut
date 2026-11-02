<script setup lang="ts">
import { computed, nextTick, onActivated, onBeforeUnmount, onMounted, ref, watch } from 'vue';
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
    applyAutoCuts,
    clipDuration,
    clipWords,
    editRequest,
    editWord,
    fromCandidates,
    captionWords,
    type ClipWord,
    playbackStep,
    playedRanges,
    ratingTone,
    setCutWords,
    setFaceOverride,
    speakerNames,
    speakerTurns,
    toExportSegments,
    trimClip,
    wordBoundaries,
} from '../utils/shortClips';
import { beginRun, isRunCancelled } from '../composables/useRunCancellation';
import { formatTime, parseTime } from '../composables/useTimeFormat';

import {
    Box,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Clapperboard,
    Download,
    FileWarning,
    FolderOpen,
    Heart,
    Lightbulb,
    Loader2,
    Play,
    Repeat,
    ScanFace,
    Sparkles,
    Square,
    TextQuote,
    Zap,
} from '@lucide/vue';
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
import ClipText from './ClipText.vue';
import TransitionPicker from './TransitionPicker.vue';
import PageHeader from './ui/PageHeader.vue';
import EmptyState from './ui/EmptyState.vue';
import SectionCard from './ui/SectionCard.vue';
import UiSwitch from './ui/UiSwitch.vue';
import UiSegmented, { type SegmentedOption } from './ui/UiSegmented.vue';

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
    /** The transcript with a word corrected from a clip's text. */
    'update:segments': [segments: TranscriptSegment[]];
}>();

const { settings } = useSettings();

const isProcessing = ref(false);
/** What this workspace is running, for specific busy labels. */
type Job = 'find' | 'preview' | 'faces' | 'autoEdit' | 'export';
const job = ref<Job | null>(null);
/** The clip whose preview is being prepared. */
const preparingClip = ref<string | null>(null);
const silenceIntervalsCache = ref<{ path: string; intervals: SilenceInterval[] } | null>(null);
const activeRunId = ref<number | null>(null);

function invalidateRun() {
    activeRunId.value = null;
    isProcessing.value = false;
    job.value = null;
    preparingClip.value = null;
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
    {
        key: 'autoEdit',
        title: 'Auto edit',
        text: 'An LLM cuts false starts, repetitions and asides from new clips; review them under Text.',
    },
] as const;

type ToggleKey = (typeof TOGGLES)[number]['key'];

function toggles(...keys: ToggleKey[]) {
    return TOGGLES.filter((toggle) => (keys as string[]).includes(toggle.key));
}

/** Shape of the clips. */
const STYLE_TOGGLES = toggles('looped', 'autoEdit');
/** Rarely changed. */
const ADVANCED_TOGGLES = toggles('allowSplicing', 'trimBoundarySilence');
const VERTICAL_TOGGLE = toggles('vertical')[0];
const CAPTIONS_TOGGLE = toggles('captions')[0];

function setFlag(key: ToggleKey, value: boolean) {
    updateState({ [key]: value });
}

/** Min longer than max can't be met. */
const durationError = computed(() =>
    Number(clipMinDuration.value) > Number(clipMaxDuration.value)
        ? 'Min seconds is longer than max seconds.'
        : '',
);

const INTENSITIES: { value: Intensity; label: string; text: string }[] = [
    { value: 'off', label: 'Off', text: 'Keep every pause and filler.' },
    { value: 'chill', label: 'Chill', text: 'Pauses up to 350 ms, no fillers.' },
    { value: 'punchy', label: 'Punchy', text: 'Pauses up to 180 ms, no fillers or stutters.' },
    { value: 'hyper', label: 'Hyper', text: 'Pauses up to 90 ms: rapid-fire.' },
];
const INTENSITY_OPTIONS: SegmentedOption<Intensity>[] = INTENSITIES.map((option) => ({
    value: option.value,
    label: option.label,
    title: option.text,
    testId: `clips-intensity-${option.value}`,
}));
const intensity = setting('intensity');
const transitions = setting('transitions');
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
        transitions: props.state.transitions,
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

function selectAll(selected: boolean) {
    updateState({ clips: clips.value.map((clip) => ({ ...clip, selected })) });
}

const allSelected = computed(
    () => clips.value.length > 0 && selectedClips.value.length === clips.value.length,
);

/** Clips whose full reason is shown (it's clamped to two lines otherwise). */
const expandedReasons = ref(new Set<string>());

function toggleReason(clip: ShortClip) {
    const next = new Set(expandedReasons.value);
    if (!next.delete(clip.id)) next.add(clip.id);
    expandedReasons.value = next;
}

/** Clips whose reason doesn't fit in two lines, so they get a Show more toggle. */
const overflowingReasons = ref(new Set<string>());
const reasonElements = new Map<string, HTMLElement>();
const reasonObserver =
    typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver((entries) => {
              for (const entry of entries) {
                  const id = (entry.target as HTMLElement).dataset.reason;
                  if (id) checkReason(id);
              }
          });

/** Whether the clamped reason cuts text off (an expanded one keeps its toggle). */
function checkReason(id: string) {
    const el = reasonElements.get(id);
    if (!el || expandedReasons.value.has(id)) return;
    const overflows = el.scrollHeight > el.clientHeight + 1;
    if (overflows === overflowingReasons.value.has(id)) return;
    const next = new Set(overflowingReasons.value);
    if (overflows) next.add(id);
    else next.delete(id);
    overflowingReasons.value = next;
}

function setReasonElement(id: string, el: Element | null) {
    const previous = reasonElements.get(id);
    if (previous === el) return;
    if (previous) reasonObserver?.unobserve(previous);
    if (el instanceof HTMLElement) {
        reasonElements.set(id, el);
        reasonObserver?.observe(el);
        void nextTick(() => checkReason(id));
    } else {
        reasonElements.delete(id);
    }
}

// The text can change without the box resizing (a new search reuses ids).
watch(
    () => clips.value.map((clip) => `${clip.id}:${clip.reason}`).join('|'),
    () => void nextTick(() => reasonElements.forEach((_, id) => checkReason(id))),
);

onBeforeUnmount(() => reasonObserver?.disconnect());

const SCORE_TITLE =
    'Ranking score, 0-100: mostly the ratings below (hook 35%, standalone 25%, emotion and info 20% each), plus small bonuses for back-and-forth, laughter and a lively pace.';

const ROLE_LABELS: Record<ClipRole, string> = {
    hook: 'Hook',
    body: 'Body',
    payoff: 'Payoff',
    loop_opener: 'Loop opener',
    closing: 'Closing',
};

const RATINGS = [
    {
        key: 'hook',
        label: 'Hook',
        icon: Zap,
        text: 'Do the first seconds grab attention?',
    },
    {
        key: 'standalone',
        label: 'Standalone',
        icon: Box,
        text: 'Does it work without context?',
    },
    {
        key: 'emotion',
        label: 'Emotion',
        icon: Heart,
        text: 'Humour, surprise, conflict, passion',
    },
    { key: 'info', label: 'Info', icon: Lightbulb, text: 'Is it useful or insightful?' },
] as const;

/** Bar and value colours per rating tone: high values stand out, low ones fade. */
const TONE_CLASSES: Record<ReturnType<typeof ratingTone>, { bar: string; value: string }> = {
    top: { bar: 'bg-success', value: 'text-success' },
    high: { bar: 'bg-success/75', value: 'text-success' },
    good: { bar: 'bg-success/45', value: 'text-fg' },
    mid: { bar: 'bg-fg-subtle', value: 'text-fg-muted' },
    low: { bar: 'bg-fg-faint', value: 'text-fg-subtle' },
};

/** The ratings a clip shows: the four, plus loop continuity for looped clips. */
function clipRatings(clip: ShortClip) {
    if (!clip.ratings) return [];
    const ratings = clip.ratings;
    const rows = RATINGS.map((rating) => ({
        key: rating.key as string,
        label: rating.label as string,
        icon: rating.icon,
        text: rating.text as string,
        value: ratings[rating.key],
    }));
    if (clip.looped) {
        rows.push({
            key: 'loop',
            label: 'Loop',
            icon: Repeat,
            text: 'How seamlessly the ending leads back into the opening',
            value: ratings.loopContinuity,
        });
    }
    return rows;
}

function clock(seconds: number): string {
    // Milliseconds are noise on a card; show whole seconds.
    return formatTime(Math.round(seconds)).replace(/\.000$/, '');
}

// ---- Preview: plays what the export will contain (tightened ranges and,
// in 9:16 mode, the planned framing) in the player above. ----
/** CSS size of the 9:16 preview frame. */
const VERTICAL_FRAME = { width: 270, height: 480 };
/** The 9:16 frame is drawn smaller on the stage, so it fits beside the clip list. */
const DEFAULT_STAGE_SCALE = 0.68;
/** The stage well, sized to the window; the 9:16 frame scales to fit it. */
const stageWell = ref<HTMLElement | null>(null);
const stageWellHeight = ref(0);
const stageScale = computed(() =>
    stageWellHeight.value > 0
        ? Math.min(1, Math.max(0.3, (stageWellHeight.value - 16) / VERTICAL_FRAME.height))
        : DEFAULT_STAGE_SCALE,
);
const stageObserver =
    typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver(([entry]) => {
              stageWellHeight.value = entry?.contentRect.height ?? 0;
          });
watch(stageWell, (el, previous) => {
    if (previous) stageObserver?.unobserve(previous);
    if (el) {
        stageWellHeight.value = el.clientHeight;
        stageObserver?.observe(el);
    }
});
onBeforeUnmount(() => stageObserver?.disconnect());

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
/** Player time while a clip previews, for its live text. */
const playhead = ref<number | null>(null);

function previewSignature(clip: ShortClip): string {
    const ranges = playedRanges(clip)
        .map((range) => `${range.start}-${range.end}`)
        .join(',');
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
    if (!video || !previewing.value) return;
    playhead.value = video.currentTime;
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
        return { signature, ranges: playedRanges(clip), plan: null };
    }
    if (props.busy || isProcessing.value) return null;

    const runId = await beginRun();
    activeRunId.value = runId;
    isProcessing.value = true;
    job.value = 'preview';
    preparingClip.value = clip.id;
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
            entry = { signature, ranges: ranges.length ? ranges : playedRanges(clip), plan: null };
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
        if (preparingClip.value === clip.id) preparingClip.value = null;
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isProcessing.value = false;
            job.value = null;
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
    playhead.value = null;
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

// ---- Faces: the people 9:16 framing found (one face, recognised across
// camera angles), for the user to rule out (a face on a TV) or name. The
// last search and the overrides live in the workspace state (saved with the
// recording); overrides go with every plan and export. ----
const detectedFaces = computed(() => props.state.faceSearch?.faces ?? []);
const speakers = computed(() => speakerNames(props.segments));

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
    const search = props.state.faceSearch;
    updateState({
        faces,
        faceSearch: search && {
            ...search,
            faces: search.faces.map((f, i) => (i === index ? { ...f, applied } : f)),
        },
    });
}

function setFaceSpeaker(index: number, value: string) {
    const speaker: FaceSpeaker = value.startsWith('named:')
        ? { kind: 'named', name: value.slice('named:'.length) }
        : value === 'nobody'
          ? { kind: 'nobody' }
          : { kind: 'auto' };
    changeFace(index, { speaker });
}

/** The clips a face search looks at: the included ones, else all. */
const clipsToScan = computed(() =>
    selectedClips.value.length ? selectedClips.value : clips.value,
);

/** What a face search depends on; empty when there's nothing to search. */
const faceScanSignature = computed(() => {
    if (!props.state.vertical || !props.hasMediaFile || clipsToScan.value.length === 0) return '';
    const ranges = clipsToScan.value.map((clip) =>
        clip.ranges.map((range) => `${range.start}-${range.end}`).join(','),
    );
    return `${props.inputPath}|${ranges.join(';')}`;
});

/** The signature the last search started with (so a failed one doesn't loop). */
let scannedSignature = '';
let scanTimer: ReturnType<typeof setTimeout> | null = null;
/** Quiet time before an automatic search, so trimming word by word doesn't start one per click. */
const FACE_SCAN_DELAY_MS = 800;

// In 9:16 mode faces are searched for on their own whenever the clips
// change, once nothing else runs and no preview plays.
watch(
    [faceScanSignature, () => props.busy, isProcessing, previewing],
    () => {
        if (scanTimer !== null) clearTimeout(scanTimer);
        scanTimer = null;
        const signature = faceScanSignature.value;
        // A search saved with the recording counts as done.
        if (
            !signature ||
            signature === scannedSignature ||
            signature === props.state.faceSearch?.signature
        )
            return;
        scanTimer = setTimeout(() => {
            scanTimer = null;
            if (props.busy || isProcessing.value || previewing.value) return;
            void detectFaces('Clips changed; finding faces...');
        }, FACE_SCAN_DELAY_MS);
    },
    { immediate: true },
);

onBeforeUnmount(() => {
    if (scanTimer !== null) clearTimeout(scanTimer);
});

async function detectFaces(status = 'Finding faces...') {
    const toScan = clipsToScan.value;
    if (props.busy || isProcessing.value || toScan.length === 0 || !props.hasMediaFile) return;
    const signature = faceScanSignature.value;
    scannedSignature = signature;

    const runId = await beginRun();
    activeRunId.value = runId;
    isProcessing.value = true;
    job.value = 'faces';
    emit('update:processing', true);
    emit('update:status', status);
    stopPreview();
    try {
        const faces = await commands.detectVerticalFaces(
            runId,
            verticalRequest(toScan.map(clipSegment), toScan),
        );
        assertActiveRun(runId);
        updateState({ faceSearch: { signature, faces } });
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
            job.value = null;
            emit('update:processing', false);
        }
    }
}

// ---- Clip text: what a clip says, with the spoken word highlighted while
// it previews. Words are edited in the transcript itself, so captions,
// subtitles and every other clip get the correction. ----
const openTexts = ref(new Set<string>());

function toggleText(clip: ShortClip) {
    const next = new Set(openTexts.value);
    if (!next.delete(clip.id)) next.add(clip.id);
    openTexts.value = next;
}

/** What the clip's prepared preview plays, if prepared for its current settings. */
function keptRanges(clip: ShortClip): PlaybackRange[] | null {
    const entry = prepared.value.get(clip.id);
    return entry && entry.signature === previewSignature(clip) ? entry.ranges : null;
}

function cutClipWords(clip: ShortClip, words: ClipWord[], cut: boolean) {
    updateClip(clip.id, (current) =>
        setCutWords(current, clipWords(props.segments, current.ranges), words, cut),
    );
}

function editClipWord(word: ClipWord, text: string) {
    emit('update:segments', editWord(props.segments, word, text));
    // Prepared previews show the correction right away.
    const next = new Map(prepared.value);
    for (const [id, entry] of next) {
        if (!entry.plan) continue;
        const captions = entry.plan.captions.map((chunks) =>
            chunks.map((chunk) =>
                chunk.map((w) =>
                    w.start === word.start && w.end === word.end ? { ...w, text } : w,
                ),
            ),
        );
        next.set(id, { ...entry, plan: { ...entry.plan, captions } });
    }
    prepared.value = next;
}

// ---- Auto editing: an LLM proposes cuts for a better flow; they land as
// auto cuts the user reviews in the clip's text. ----
const autoEditingClip = ref<string | null>(null);

/**
 * Auto-edit `targets` within run `runId`, updating them in `all` (the
 * clips as they are now); says what it cut.
 */
async function autoEdit(
    runId: number,
    targets: ShortClip[],
    all: ShortClip[] = props.state.clips,
): Promise<string> {
    const cuts = await commands.suggestClipCuts(
        runId,
        { baseUrl: settings.value.baseUrl, model: settings.value.model },
        targets.map((clip) => editRequest(props.segments, clip)),
    );
    assertActiveRun(runId);
    let words = 0;
    const edited = new Map(
        targets.map((clip, i) => {
            const result = applyAutoCuts(props.segments, clip, cuts[i] ?? []);
            words += (result.cutWords ?? []).filter((cut) => cut.auto).length;
            return [clip.id, result];
        }),
    );
    updateState({ clips: all.map((clip) => edited.get(clip.id) ?? clip) });
    return words === 0
        ? 'auto editing found nothing to cut'
        : `auto editing cut ${words} word${words === 1 ? '' : 's'}`;
}

async function autoEditClip(clip: ShortClip) {
    if (props.busy || isProcessing.value) return;
    const runId = await beginRun();
    activeRunId.value = runId;
    isProcessing.value = true;
    job.value = 'autoEdit';
    autoEditingClip.value = clip.id;
    emit('update:processing', true);
    emit('update:status', 'Auto-editing the clip...');
    stopPreview();
    try {
        const result = await autoEdit(runId, [clip]);
        emit('update:status', `${result.charAt(0).toUpperCase()}${result.slice(1)}.`);
    } catch (e) {
        if (isRunCancelled(e)) {
            emit('update:status', 'Run cancelled.');
            return;
        }
        emit('update:status', `Error auto-editing the clip: ${errorMessage(e)}`);
    } finally {
        autoEditingClip.value = null;
        if (activeRunId.value === runId) {
            activeRunId.value = null;
            isProcessing.value = false;
            job.value = null;
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
    job.value = 'find';
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
        const found = fromCandidates(candidates);
        updateState({ clips: found });
        const summary = `Found ${candidates.length} clip${candidates.length === 1 ? '' : 's'}`;
        emit('update:status', `${summary}.`);
        if (props.state.autoEdit && found.length > 0) {
            // The clips stay even if editing them fails.
            try {
                emit('update:status', `${summary}; auto-editing them...`);
                const edited = await autoEdit(runId, found, found);
                emit('update:status', `${summary}; ${edited}.`);
            } catch (e) {
                if (isRunCancelled(e)) throw e;
                emit('update:status', `${summary}, but auto editing failed: ${errorMessage(e)}`);
            }
        }
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
            job.value = null;
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
    job.value = 'export';
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
            job.value = null;
            emit('update:processing', false);
        }
    }
}

const BUSY_LABELS: Record<Job, string> = {
    find: 'Finding clips…',
    preview: 'Preparing the preview…',
    faces: 'Finding faces…',
    autoEdit: 'Auto editing…',
    export: 'Exporting…',
};

const generateBlocked = computed(() => {
    // Finding clips shows on the button itself.
    if (job.value === 'find') return '';
    if (isProcessing.value) {
        return `${job.value ? BUSY_LABELS[job.value] : 'Working…'} Wait for it to finish.`;
    }
    if (props.busy) return 'Wait for the running job to finish.';
    if (props.segments.length === 0) return 'There is no transcript to pick clips from.';
    return '';
});

const exportBlocked = computed(() => {
    if (!props.hasMediaFile) return 'The media file is missing; relink it on the Source step.';
    if (isProcessing.value || props.busy) return 'Wait for the running job to finish.';
    if (selectedClips.value.length === 0) return 'Include at least one clip to export.';
    return '';
});

const previewingClip = computed(() =>
    previewing.value
        ? (clips.value.find((clip) => clip.id === previewing.value?.id) ?? null)
        : null,
);

// ---- Setup inspector: on wide layouts it sticks beside the clips and is
// capped to what's visible of the page (above the status bar), so Find clips
// at its bottom is always in reach. ----
const inspector = ref<HTMLElement | null>(null);
const inspectorMaxHeight = ref<string | undefined>(undefined);
/** Space kept free under the inspector. */
const INSPECTOR_GAP = 12;
let scrollRoot: HTMLElement | null = null;
let fitFrame: number | null = null;

function fitInspector() {
    fitFrame = null;
    const el = inspector.value;
    if (!el || getComputedStyle(el).position !== 'sticky') {
        inspectorMaxHeight.value = undefined;
        return;
    }
    const root = scrollRoot ?? document.documentElement;
    const rootRect = root.getBoundingClientRect();
    const visibleTop = scrollRoot ? rootRect.top : 0;
    const visibleBottom = scrollRoot ? rootRect.bottom : window.innerHeight;
    // The status bar sticks to the bottom of the scroll container.
    const statusBar = root.querySelector<HTMLElement>('[data-testid="status-bar"]');
    const covered = statusBar ? statusBar.getBoundingClientRect().height : 0;
    const top = Math.max(el.getBoundingClientRect().top, visibleTop);
    inspectorMaxHeight.value = `${Math.max(240, visibleBottom - covered - INSPECTOR_GAP - top)}px`;
}

function scheduleFit() {
    if (fitFrame === null) fitFrame = requestAnimationFrame(fitInspector);
}

const inspectorObserver =
    typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(scheduleFit);

onMounted(() => {
    scrollRoot = document.getElementById('app-main');
    (scrollRoot ?? window).addEventListener('scroll', scheduleFit, { passive: true });
    window.addEventListener('resize', scheduleFit);
    // The page above can change height (the header gains its actions).
    if (scrollRoot) inspectorObserver?.observe(scrollRoot);
    if (inspector.value?.parentElement) inspectorObserver?.observe(inspector.value.parentElement);
    scheduleFit();
});

// Home is kept alive; the page may have scrolled or resized meanwhile.
onActivated(scheduleFit);

onBeforeUnmount(() => {
    (scrollRoot ?? window).removeEventListener('scroll', scheduleFit);
    window.removeEventListener('resize', scheduleFit);
    inspectorObserver?.disconnect();
    if (fitFrame !== null) cancelAnimationFrame(fitFrame);
});

async function openExportFolder() {
    if (lastExportPath.value) {
        await commands.openFolder(lastExportPath.value);
    }
}
</script>

<template>
    <div class="@container mx-auto w-full max-w-[1400px] px-8 pt-8 pb-10">
        <PageHeader
            title="Viral clips"
            description="Find the strongest moments and export them as shorts."
        >
            <template v-if="clips.length > 0" #actions>
                <button
                    type="button"
                    class="btn-ghost btn-sm"
                    data-testid="clips-select-all"
                    @click="selectAll(!allSelected)"
                >
                    {{ allSelected ? 'Select none' : 'Select all' }}
                </button>
                <button
                    v-if="lastExportPath"
                    type="button"
                    class="btn-secondary btn-icon"
                    title="Open folder"
                    aria-label="Open export folder"
                    @click="openExportFolder"
                >
                    <FolderOpen aria-hidden="true" />
                </button>
                <button
                    type="button"
                    class="btn-primary"
                    :disabled="isProcessing || busy || !hasMediaFile || selectedClips.length === 0"
                    :title="exportBlocked || undefined"
                    data-testid="clips-export-selected"
                    @click="exportClips(selectedClips)"
                >
                    <Loader2 v-if="job === 'export'" class="animate-spin" aria-hidden="true" />
                    <Download v-else aria-hidden="true" />
                    Export {{ selectedClips.length }} selected
                </button>
            </template>
        </PageHeader>

        <div
            class="grid items-start gap-5 @3xl:grid-cols-[300px_minmax(0,1fr)] @5xl:grid-cols-[340px_minmax(0,1fr)]"
        >
            <!-- Setup inspector -->
            <aside
                ref="inspector"
                class="flex min-w-0 flex-col @3xl:sticky @3xl:top-4 @3xl:max-h-[calc(100vh-5.5rem)]"
                :style="inspectorMaxHeight ? { maxHeight: inspectorMaxHeight } : undefined"
                aria-label="Clip setup"
            >
                <div class="-mx-1 flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-1 pb-1">
                    <SectionCard title="Basics">
                        <div class="grid grid-cols-3 gap-2.5">
                            <div>
                                <label for="clips-count" class="field-label">Count</label>
                                <input
                                    id="clips-count"
                                    v-model.number="clipCount"
                                    type="number"
                                    min="1"
                                    max="10"
                                    class="input"
                                />
                            </div>
                            <div>
                                <label for="clips-min" class="field-label">Min length</label>
                                <div class="relative">
                                    <input
                                        id="clips-min"
                                        v-model.number="clipMinDuration"
                                        type="number"
                                        min="5"
                                        class="input pr-6"
                                        :aria-invalid="durationError ? 'true' : undefined"
                                        :aria-describedby="
                                            durationError ? 'clips-duration-error' : undefined
                                        "
                                    />
                                    <span class="unit" aria-hidden="true">s</span>
                                </div>
                            </div>
                            <div>
                                <label for="clips-max" class="field-label">Max length</label>
                                <div class="relative">
                                    <input
                                        id="clips-max"
                                        v-model.number="clipMaxDuration"
                                        type="number"
                                        min="10"
                                        class="input pr-6"
                                        :aria-invalid="durationError ? 'true' : undefined"
                                        :aria-describedby="
                                            durationError ? 'clips-duration-error' : undefined
                                        "
                                    />
                                    <span class="unit" aria-hidden="true">s</span>
                                </div>
                            </div>
                        </div>
                        <p
                            v-if="durationError"
                            id="clips-duration-error"
                            class="mt-1.5 text-[12px] text-danger"
                            role="alert"
                        >
                            {{ durationError }}
                        </p>
                        <label for="clips-topic" class="field-label mt-3">
                            Topic <span class="text-fg-subtle">(optional)</span>
                        </label>
                        <input
                            id="clips-topic"
                            v-model="clipTopic"
                            type="text"
                            class="input"
                            placeholder="Funny moments, rants, a technical explanation…"
                        />
                    </SectionCard>

                    <SectionCard title="Style">
                        <div class="flex flex-col gap-3">
                            <div
                                v-for="toggle in STYLE_TOGGLES"
                                :key="toggle.key"
                                class="flex items-start gap-3"
                            >
                                <div class="min-w-0 flex-1">
                                    <label
                                        :for="`clips-flag-${toggle.key}`"
                                        class="block cursor-pointer text-[13px] font-medium text-fg"
                                        >{{ toggle.title }}</label
                                    >
                                    <p class="hint mt-0.5">{{ toggle.text }}</p>
                                </div>
                                <UiSwitch
                                    :id="`clips-flag-${toggle.key}`"
                                    :model-value="state[toggle.key]"
                                    :data-testid="`clips-toggle-${toggle.key}`"
                                    @update:model-value="setFlag(toggle.key, $event)"
                                />
                            </div>
                            <div class="divider" />
                            <div>
                                <p id="clips-tightening-label" class="field-label">Tightening</p>
                                <UiSegmented
                                    v-model="intensity"
                                    :options="INTENSITY_OPTIONS"
                                    label="Tightening"
                                    size="sm"
                                    class="w-full [&>button]:flex-1 [&>button]:justify-center"
                                />
                                <p class="hint mt-1.5">{{ intensityText }}</p>
                            </div>
                        </div>
                    </SectionCard>

                    <!-- Everything only 9:16 uses, shown once it's on. -->
                    <section class="panel" data-testid="clips-vertical">
                        <div class="flex items-start gap-3 px-4 py-3.5">
                            <div class="min-w-0 flex-1">
                                <label
                                    :for="`clips-flag-${VERTICAL_TOGGLE.key}`"
                                    class="section-title block cursor-pointer"
                                    >{{ VERTICAL_TOGGLE.title }}</label
                                >
                                <p class="hint mt-0.5">{{ VERTICAL_TOGGLE.text }}</p>
                            </div>
                            <UiSwitch
                                :id="`clips-flag-${VERTICAL_TOGGLE.key}`"
                                :model-value="state.vertical"
                                :data-testid="`clips-toggle-${VERTICAL_TOGGLE.key}`"
                                @update:model-value="setFlag(VERTICAL_TOGGLE.key, $event)"
                            />
                        </div>
                        <div
                            v-show="state.vertical"
                            class="animate-rise-in border-t border-line px-4 pt-3.5 pb-4"
                        >
                            <div class="flex items-start gap-3">
                                <div class="min-w-0 flex-1">
                                    <label
                                        :for="`clips-flag-${CAPTIONS_TOGGLE.key}`"
                                        class="block cursor-pointer text-[13px] font-medium text-fg"
                                        >{{ CAPTIONS_TOGGLE.title }}</label
                                    >
                                    <p class="hint mt-0.5">{{ CAPTIONS_TOGGLE.text }}</p>
                                </div>
                                <UiSwitch
                                    :id="`clips-flag-${CAPTIONS_TOGGLE.key}`"
                                    :model-value="state.captions"
                                    :data-testid="`clips-toggle-${CAPTIONS_TOGGLE.key}`"
                                    @update:model-value="setFlag(CAPTIONS_TOGGLE.key, $event)"
                                />
                            </div>

                            <div class="divider my-3.5" />
                            <TransitionPicker
                                v-model="transitions"
                                :intensity="intensity"
                                :inactive="!state.vertical"
                            />

                            <div class="divider my-3.5" />
                            <!-- Faces the 9:16 framing works with -->
                            <section
                                v-if="clips.length > 0 && hasMediaFile"
                                data-testid="clips-faces"
                            >
                                <div class="flex items-center gap-3">
                                    <h3 class="min-w-0 flex-1 text-[13px] font-medium text-fg">
                                        Faces
                                    </h3>
                                    <button
                                        type="button"
                                        class="btn-secondary btn-sm"
                                        :disabled="isProcessing || busy"
                                        :title="
                                            isProcessing || busy
                                                ? 'Wait for the running job to finish.'
                                                : undefined
                                        "
                                        data-testid="clips-detect-faces"
                                        @click="detectFaces()"
                                    >
                                        <Loader2
                                            v-if="job === 'faces'"
                                            class="animate-spin"
                                            aria-hidden="true"
                                        />
                                        <ScanFace v-else aria-hidden="true" />
                                        {{ detectedFaces.length ? 'Refresh faces' : 'Find faces' }}
                                    </button>
                                </div>
                                <p class="hint mt-0.5">The 9:16 crop follows these faces.</p>
                                <details class="group mt-1">
                                    <summary
                                        class="hint inline-flex cursor-pointer list-none items-center gap-1 hover:text-fg-muted [&::-webkit-details-marker]:hidden"
                                    >
                                        <ChevronDown
                                            class="h-3 w-3 transition-transform group-open:rotate-180"
                                            aria-hidden="true"
                                        />
                                        When to change them
                                    </summary>
                                    <p class="hint mt-1">
                                        Turn off faces that aren't people in the room (a TV, a
                                        poster). Name a face if the wrong one gets followed.
                                    </p>
                                </details>
                                <p
                                    v-if="!detectedFaces.length && state.faces.length"
                                    class="hint mt-2"
                                >
                                    {{ state.faces.length }} face setting{{
                                        state.faces.length === 1 ? '' : 's'
                                    }}
                                    saved; find faces to review them.
                                </p>
                                <TransitionGroup
                                    v-if="detectedFaces.length"
                                    name="list"
                                    tag="ul"
                                    class="relative mt-3 flex flex-col gap-2"
                                >
                                    <li
                                        v-for="(face, index) in detectedFaces"
                                        :key="face.thumbnail ?? index"
                                        class="flex gap-3 rounded-md border border-line bg-surface-2 p-2.5"
                                        :data-testid="`clips-face-${index}`"
                                    >
                                        <img
                                            v-if="face.thumbnail"
                                            :src="face.thumbnail"
                                            alt=""
                                            class="h-[72px] w-[72px] shrink-0 rounded-md object-cover transition-[opacity,filter] duration-200"
                                            :class="faceIgnored(face) ? 'opacity-30 grayscale' : ''"
                                        />
                                        <div
                                            v-else
                                            class="flex h-[72px] w-[72px] shrink-0 items-center justify-center rounded-md bg-well text-fg-faint"
                                            :class="faceIgnored(face) ? 'opacity-30' : ''"
                                        >
                                            <ScanFace class="h-6 w-6" aria-hidden="true" />
                                        </div>
                                        <div class="flex min-w-0 flex-1 flex-col gap-1.5">
                                            <div class="flex items-center gap-2">
                                                <label
                                                    :for="`clips-face-follow-${index}`"
                                                    class="flex-1 cursor-pointer text-[12px] font-medium text-fg"
                                                    >Follow</label
                                                >
                                                <UiSwitch
                                                    :id="`clips-face-follow-${index}`"
                                                    size="sm"
                                                    :model-value="!faceIgnored(face)"
                                                    :data-testid="`clips-face-enabled-${index}`"
                                                    @update:model-value="
                                                        changeFace(index, { ignored: !$event })
                                                    "
                                                />
                                            </div>
                                            <select
                                                :value="faceSpeakerValue(face)"
                                                :disabled="faceIgnored(face)"
                                                :title="
                                                    faceIgnored(face)
                                                        ? 'Turn Follow on to name this face'
                                                        : undefined
                                                "
                                                :aria-label="`Speaker of face ${index + 1}`"
                                                :data-testid="`clips-face-speaker-${index}`"
                                                class="input input-sm"
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
                                            <p class="text-[11px] text-fg-subtle">
                                                {{ Math.round(face.seconds) }} s on screen<span
                                                    v-if="face.views > 1"
                                                    :data-testid="`clips-face-views-${index}`"
                                                >
                                                    · {{ face.views }} camera angles</span
                                                >
                                            </p>
                                        </div>
                                    </li>
                                </TransitionGroup>
                            </section>
                            <div v-else>
                                <h3 class="text-[13px] font-medium text-fg">Faces</h3>
                                <p class="hint mt-0.5">
                                    {{
                                        hasMediaFile
                                            ? 'Faces are found in your clips once you have some.'
                                            : 'Finding faces needs the media file.'
                                    }}
                                </p>
                            </div>
                        </div>
                    </section>

                    <SectionCard title="Advanced" collapsible :default-open="false">
                        <div class="flex flex-col gap-3">
                            <div
                                v-for="toggle in ADVANCED_TOGGLES"
                                :key="toggle.key"
                                class="flex items-start gap-3"
                            >
                                <div class="min-w-0 flex-1">
                                    <label
                                        :for="`clips-flag-${toggle.key}`"
                                        class="block cursor-pointer text-[13px] font-medium text-fg"
                                        >{{ toggle.title }}</label
                                    >
                                    <p class="hint mt-0.5">{{ toggle.text }}</p>
                                </div>
                                <UiSwitch
                                    :id="`clips-flag-${toggle.key}`"
                                    :model-value="state[toggle.key]"
                                    :data-testid="`clips-toggle-${toggle.key}`"
                                    @update:model-value="setFlag(toggle.key, $event)"
                                />
                            </div>
                        </div>
                    </SectionCard>
                </div>

                <div class="pt-3">
                    <button
                        type="button"
                        class="btn-primary btn-lg w-full"
                        :disabled="isProcessing || busy"
                        :title="generateBlocked || undefined"
                        :aria-busy="job === 'find' ? 'true' : undefined"
                        data-testid="clips-generate"
                        @click="generateClips"
                    >
                        <Loader2 v-if="job === 'find'" class="animate-spin" aria-hidden="true" />
                        <Sparkles v-else aria-hidden="true" />
                        {{
                            job === 'find'
                                ? BUSY_LABELS.find
                                : clips.length
                                  ? 'Find clips again'
                                  : 'Find clips'
                        }}
                    </button>
                    <p v-if="generateBlocked" class="hint mt-1.5 text-center">
                        {{ generateBlocked }}
                    </p>
                </div>
            </aside>

            <!-- Preview stage and clips -->
            <div class="flex min-w-0 flex-col gap-4">
                <template v-if="clips.length > 0">
                    <!-- Sticky only where the window is tall enough to keep the list in view. -->
                    <div
                        class="z-10 -mx-px -mt-4 bg-canvas px-px pt-4 pb-1 @3xl:[@media(min-height:760px)]:sticky @3xl:[@media(min-height:760px)]:top-0"
                        data-testid="clips-stage"
                    >
                        <div
                            v-if="hasMediaFile"
                            class="overflow-hidden rounded-lg border border-line bg-black"
                        >
                            <!-- Preview player; in 9:16 mode it shows the planned framing. -->
                            <div
                                ref="stageWell"
                                class="flex h-[clamp(200px,36vh,340px)] items-center justify-center"
                            >
                                <div
                                    :class="verticalBox ? 'relative shrink-0' : 'h-full w-full'"
                                    :style="
                                        verticalBox
                                            ? {
                                                  width: `${VERTICAL_FRAME.width * stageScale}px`,
                                                  height: `${VERTICAL_FRAME.height * stageScale}px`,
                                              }
                                            : undefined
                                    "
                                >
                                    <div
                                        :class="
                                            verticalBox
                                                ? 'absolute top-0 left-0 origin-top-left overflow-hidden rounded-md ring-1 ring-line-strong'
                                                : 'h-full w-full'
                                        "
                                        :style="
                                            verticalBox
                                                ? {
                                                      width: `${VERTICAL_FRAME.width}px`,
                                                      height: `${VERTICAL_FRAME.height}px`,
                                                      transform: `scale(${stageScale})`,
                                                  }
                                                : undefined
                                        "
                                        data-testid="clips-player-frame"
                                    >
                                        <video
                                            ref="player"
                                            :src="mediaSrc"
                                            :class="
                                                verticalBox
                                                    ? 'absolute max-w-none'
                                                    : 'h-full w-full object-contain'
                                            "
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
                                        <!-- Captions as the export burns them in (68% down, yellow highlight). -->
                                        <p
                                            v-if="verticalBox && verticalCaption"
                                            class="vertical-caption pointer-events-none absolute inset-x-3 text-center"
                                            data-testid="clips-player-caption"
                                        >
                                            <span
                                                v-for="(word, i) in verticalCaption"
                                                :key="i"
                                                :class="
                                                    word.active ? 'text-[#ffd60a]' : 'text-white'
                                                "
                                                >{{ word.text
                                                }}{{
                                                    i < verticalCaption.length - 1 ? ' ' : ''
                                                }}</span
                                            >
                                        </p>
                                    </div>
                                </div>
                            </div>
                            <div
                                class="flex items-center gap-3 border-t border-line bg-surface px-3 py-2"
                            >
                                <template v-if="previewingClip">
                                    <span class="chip-accent shrink-0">
                                        <Play aria-hidden="true" />
                                        Playing
                                    </span>
                                    <span class="min-w-0 flex-1 truncate text-[13px] text-fg">
                                        {{ previewingClip.title || 'Untitled clip' }}
                                    </span>
                                    <span v-if="state.vertical" class="hint shrink-0"
                                        >9:16 framing</span
                                    >
                                    <button
                                        type="button"
                                        class="btn-ghost btn-sm"
                                        data-testid="clips-stage-stop"
                                        @click="stopPreview"
                                    >
                                        <Square aria-hidden="true" />
                                        Stop
                                    </button>
                                </template>
                                <p v-else class="hint flex-1">
                                    Press Preview on a clip to play what its export will contain{{
                                        state.vertical ? ', framed for 9:16' : ''
                                    }}.
                                </p>
                            </div>
                        </div>
                        <EmptyState
                            v-else
                            compact
                            :icon="FileWarning"
                            title="Media file not found"
                            description="Preview and export need the source file. Relink it on the Source step."
                        />
                    </div>

                    <div class="flex items-baseline justify-between gap-3">
                        <h2 class="section-title">
                            {{ clips.length }} clip{{ clips.length === 1 ? '' : 's' }}
                            <span class="font-normal text-fg-subtle"
                                >· {{ selectedClips.length }} included</span
                            >
                        </h2>
                        <p class="hint">Ranked by score</p>
                    </div>

                    <!-- Clip cards -->
                    <TransitionGroup
                        name="list"
                        tag="ul"
                        class="relative grid grid-cols-1 gap-3 @6xl:grid-cols-2"
                        data-testid="clip-cards"
                    >
                        <li
                            v-for="(clip, rank) in clips"
                            :key="clip.id"
                            class="flex flex-col rounded-lg border p-4 transition-[border-color,background-color,box-shadow] duration-150"
                            :class="
                                previewing?.id === clip.id
                                    ? 'border-accent/60 bg-surface inset-ring inset-ring-accent/25'
                                    : clip.selected
                                      ? 'border-line bg-surface hover:border-line-strong'
                                      : 'border-dashed border-line-strong bg-transparent'
                            "
                            :data-testid="`clip-card-${clip.id}`"
                        >
                            <div class="flex items-start gap-3">
                                <input
                                    type="checkbox"
                                    :checked="clip.selected"
                                    :aria-label="`Include “${clip.title || 'Untitled clip'}” in the export`"
                                    title="Include in “Export selected”"
                                    :data-testid="`clip-select-${clip.id}`"
                                    class="mt-0.5 h-4 w-4 shrink-0 cursor-pointer"
                                    @change="
                                        setSelected(
                                            clip,
                                            ($event.target as HTMLInputElement).checked,
                                        )
                                    "
                                />
                                <div class="min-w-0 flex-1">
                                    <h3
                                        class="text-[14px] leading-snug font-semibold"
                                        :class="clip.selected ? 'text-fg' : 'text-fg-muted'"
                                    >
                                        <span
                                            class="mr-1 font-mono text-[12px] font-normal text-fg-subtle"
                                            >#{{ rank + 1 }}</span
                                        >
                                        {{ clip.title || 'Untitled clip' }}
                                    </h3>
                                    <p
                                        v-if="clip.hookLine"
                                        class="mt-1 text-[13px] italic"
                                        :class="clip.selected ? 'text-fg-muted' : 'text-fg-subtle'"
                                    >
                                        “{{ clip.hookLine }}”
                                    </p>
                                </div>
                                <button
                                    v-if="clip.score !== null"
                                    type="button"
                                    class="shrink-0 cursor-help font-mono"
                                    :class="clip.selected ? 'chip-accent' : 'chip'"
                                    :title="SCORE_TITLE"
                                    :data-testid="`clip-score-${clip.id}`"
                                >
                                    <span class="sr-only">Score </span>{{ clip.score
                                    }}<span class="sr-only"> of 100. {{ SCORE_TITLE }}</span>
                                </button>
                            </div>

                            <div class="mt-2 pl-7">
                                <p
                                    :id="`clip-reason-${clip.id}`"
                                    :ref="(el) => setReasonElement(clip.id, el as Element | null)"
                                    :data-reason="clip.id"
                                    class="text-[12px] leading-relaxed text-fg-subtle"
                                    :class="expandedReasons.has(clip.id) ? '' : 'line-clamp-2'"
                                    :data-testid="`clip-reason-${clip.id}`"
                                >
                                    {{ clip.reason }}
                                </p>
                                <button
                                    v-if="
                                        clip.reason &&
                                        (overflowingReasons.has(clip.id) ||
                                            expandedReasons.has(clip.id))
                                    "
                                    type="button"
                                    class="mt-0.5 text-[12px] text-fg-muted hover:text-fg"
                                    :aria-expanded="expandedReasons.has(clip.id) ? 'true' : 'false'"
                                    :aria-controls="`clip-reason-${clip.id}`"
                                    :data-testid="`clip-reason-toggle-${clip.id}`"
                                    @click="toggleReason(clip)"
                                >
                                    {{ expandedReasons.has(clip.id) ? 'Show less' : 'Show more' }}
                                </button>
                            </div>

                            <div class="mt-3 flex flex-wrap items-center gap-1.5">
                                <span class="chip font-mono text-fg" title="Duration">{{
                                    clock(clipDuration(clip))
                                }}</span>
                                <span v-if="clip.looped" class="chip-accent" title="Looped short">
                                    <Repeat aria-hidden="true" />
                                    Loop
                                </span>
                                <span v-for="(range, i) in clip.ranges" :key="i" class="chip">
                                    <span v-if="clip.ranges.length > 1" class="text-fg">{{
                                        ROLE_LABELS[range.role]
                                    }}</span>
                                    <span class="font-mono"
                                        >{{ clock(range.start) }}–{{ clock(range.end) }}</span
                                    >
                                </span>
                            </div>

                            <dl
                                v-if="clip.ratings"
                                class="mt-3 grid grid-cols-[repeat(auto-fill,minmax(13rem,1fr))] gap-x-5 gap-y-1.5"
                                :data-testid="`clip-ratings-${clip.id}`"
                            >
                                <div
                                    v-for="rating in clipRatings(clip)"
                                    :key="rating.key"
                                    class="flex items-center gap-2"
                                    :title="rating.text"
                                    :data-tone="ratingTone(rating.value)"
                                >
                                    <dt
                                        class="flex w-[84px] shrink-0 items-center gap-1.5 truncate text-[12px] text-fg-muted"
                                    >
                                        <component
                                            :is="rating.icon"
                                            class="h-3.5 w-3.5 text-fg-subtle"
                                            aria-hidden="true"
                                        />
                                        {{ rating.label }}
                                    </dt>
                                    <dd class="flex min-w-0 flex-1 items-center gap-2">
                                        <span
                                            class="h-1.5 flex-1 overflow-hidden rounded-full bg-surface-3"
                                            aria-hidden="true"
                                        >
                                            <span
                                                class="block h-full rounded-full"
                                                :class="TONE_CLASSES[ratingTone(rating.value)].bar"
                                                :style="{
                                                    width: `${Math.min(100, Math.max(0, rating.value * 10))}%`,
                                                }"
                                            />
                                        </span>
                                        <span
                                            class="w-4 text-right font-mono text-[12px] font-semibold"
                                            :class="TONE_CLASSES[ratingTone(rating.value)].value"
                                            >{{ rating.value }}</span
                                        >
                                    </dd>
                                </div>
                            </dl>

                            <ClipText
                                v-if="openTexts.has(clip.id)"
                                class="animate-rise-in mt-3"
                                :words="clipWords(segments, clip.ranges)"
                                :kept="keptRanges(clip)"
                                :playhead="previewing?.id === clip.id ? playhead : null"
                                :cut-words="clip.cutWords"
                                :auto-editing="autoEditingClip === clip.id"
                                :can-auto-edit="!isProcessing && !busy"
                                :speaker-order="speakers"
                                @edit="editClipWord"
                                @cut="(words, cut) => cutClipWords(clip, words, cut)"
                                @auto-edit="autoEditClip(clip)"
                            />

                            <!-- Actions sit at the bottom, so cards in a row line up. -->
                            <div class="mt-auto flex flex-wrap items-center gap-x-3 gap-y-2 pt-3">
                                <div class="flex items-center gap-1.5">
                                    <button
                                        type="button"
                                        class="btn-secondary btn-sm"
                                        :class="
                                            previewing?.id === clip.id
                                                ? 'border-accent/50 bg-accent/15 text-accent-soft'
                                                : ''
                                        "
                                        :disabled="!hasMediaFile"
                                        :title="
                                            hasMediaFile
                                                ? previewing?.id === clip.id
                                                    ? 'Stop the preview'
                                                    : 'Play what the export will contain'
                                                : 'The media file is missing'
                                        "
                                        :aria-pressed="
                                            previewing?.id === clip.id ? 'true' : 'false'
                                        "
                                        :data-testid="`clip-preview-${clip.id}`"
                                        @click="preview(clip)"
                                    >
                                        <Loader2
                                            v-if="preparingClip === clip.id"
                                            class="animate-spin"
                                            aria-hidden="true"
                                        />
                                        <Square
                                            v-else-if="previewing?.id === clip.id"
                                            aria-hidden="true"
                                        />
                                        <Play v-else aria-hidden="true" />
                                        {{
                                            preparingClip === clip.id
                                                ? 'Preparing'
                                                : previewing?.id === clip.id
                                                  ? 'Stop'
                                                  : 'Preview'
                                        }}
                                    </button>
                                    <button
                                        type="button"
                                        class="btn-ghost btn-sm"
                                        :aria-pressed="openTexts.has(clip.id) ? 'true' : 'false'"
                                        title="Show the clip's words to correct or cut them"
                                        :data-testid="`clip-text-toggle-${clip.id}`"
                                        @click="toggleText(clip)"
                                    >
                                        <TextQuote aria-hidden="true" />
                                        Text
                                    </button>
                                    <button
                                        type="button"
                                        class="btn-ghost btn-sm"
                                        :disabled="isProcessing || busy || !hasMediaFile"
                                        :title="
                                            !hasMediaFile
                                                ? 'The media file is missing'
                                                : isProcessing || busy
                                                  ? 'Wait for the running job to finish'
                                                  : 'Export just this clip'
                                        "
                                        :data-testid="`clip-export-${clip.id}`"
                                        @click="exportClips([clip])"
                                    >
                                        <Download aria-hidden="true" />
                                        Export
                                    </button>
                                </div>

                                <div
                                    class="ml-auto flex items-center gap-2 text-[12px] text-fg-subtle"
                                >
                                    <div class="trim-group" role="group" aria-label="Trim start">
                                        <span class="px-1.5">Start</span>
                                        <button
                                            type="button"
                                            class="btn-ghost btn-sm btn-icon"
                                            aria-label="Start one word earlier"
                                            title="Start one word earlier"
                                            :data-testid="`clip-start-earlier-${clip.id}`"
                                            @click="trim(clip, 'start', -1)"
                                        >
                                            <ChevronLeft aria-hidden="true" />
                                        </button>
                                        <button
                                            type="button"
                                            class="btn-ghost btn-sm btn-icon"
                                            aria-label="Start one word later"
                                            title="Start one word later"
                                            :data-testid="`clip-start-later-${clip.id}`"
                                            @click="trim(clip, 'start', 1)"
                                        >
                                            <ChevronRight aria-hidden="true" />
                                        </button>
                                    </div>
                                    <div class="trim-group" role="group" aria-label="Trim end">
                                        <span class="px-1.5">End</span>
                                        <button
                                            type="button"
                                            class="btn-ghost btn-sm btn-icon"
                                            aria-label="End one word earlier"
                                            title="End one word earlier"
                                            :data-testid="`clip-end-earlier-${clip.id}`"
                                            @click="trim(clip, 'end', -1)"
                                        >
                                            <ChevronLeft aria-hidden="true" />
                                        </button>
                                        <button
                                            type="button"
                                            class="btn-ghost btn-sm btn-icon"
                                            aria-label="End one word later"
                                            title="End one word later"
                                            :data-testid="`clip-end-later-${clip.id}`"
                                            @click="trim(clip, 'end', 1)"
                                        >
                                            <ChevronRight aria-hidden="true" />
                                        </button>
                                    </div>
                                </div>
                            </div>
                        </li>
                    </TransitionGroup>
                </template>

                <!-- While the first search runs: placeholders where the clips will land. -->
                <ul
                    v-else-if="job === 'find'"
                    class="grid grid-cols-1 gap-3 @6xl:grid-cols-2"
                    aria-hidden="true"
                >
                    <li
                        v-for="n in Math.min(Math.max(Number(clipCount) || 3, 1), 4)"
                        :key="n"
                        class="panel p-4"
                    >
                        <div class="shimmer h-4 w-2/3 rounded-sm bg-surface-3" />
                        <div class="shimmer mt-2.5 h-3 w-5/6 rounded-sm bg-surface-2" />
                        <div class="shimmer mt-1.5 h-3 w-1/2 rounded-sm bg-surface-2" />
                        <div class="mt-4 grid grid-cols-2 gap-2">
                            <div
                                v-for="m in 4"
                                :key="m"
                                class="shimmer h-2 rounded-full bg-surface-2"
                            />
                        </div>
                    </li>
                </ul>

                <EmptyState
                    v-else
                    :icon="Clapperboard"
                    title="No clips yet"
                    description="Set up the search on the left and press Find clips. The strongest moments show up here, ranked, with a hook line, ratings, a preview and export."
                    data-testid="clips-empty"
                />
            </div>
        </div>
    </div>
</template>

<style scoped>
.unit {
    position: absolute;
    top: 50%;
    right: 0.625rem;
    transform: translateY(-50%);
    font-size: 12px;
    color: var(--color-fg-subtle);
    pointer-events: none;
}

/* Hide the number spinners: the unit suffix sits there. */
input[type='number']::-webkit-inner-spin-button,
input[type='number']::-webkit-outer-spin-button {
    -webkit-appearance: none;
    margin: 0;
}

.trim-group {
    display: inline-flex;
    align-items: center;
    gap: 0.0625rem;
    padding: 0.125rem;
    border-radius: var(--radius-md);
    border: 1px solid var(--color-line);
    background: var(--color-well);
}

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
