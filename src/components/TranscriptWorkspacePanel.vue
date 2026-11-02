<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import {
    AudioLines,
    Check,
    ChevronDown,
    Eye,
    EyeOff,
    Film,
    Info,
    Languages,
    Loader2,
    TriangleAlert,
} from '@lucide/vue';
import { mediaUrl } from '../utils/mediaUrl';
import Editor from './Editor.vue';
import SubtitleExport from './SubtitleExport.vue';
import type { TranscriptSegment } from '../types';
import { parseTime } from '../composables/useTimeFormat';
import { detectTranscriptBlacklistMatches } from '../utils/transcriptBlacklist';
import { SUPPORTED_TRANSCRIPT_LANGUAGES } from '../utils/transcriptLanguages';
import { speakerColor } from '../utils/speakerColors';
import PageHeader from './ui/PageHeader.vue';
import SectionCard from './ui/SectionCard.vue';
import UiSwitch from './ui/UiSwitch.vue';

const props = defineProps<{
    inputPath: string;
    hasMediaFile: boolean;
    extractedAudioPath?: string;
    displaySegments: TranscriptSegment[];
    originalSegments: TranscriptSegment[];
    translations: Record<string, TranscriptSegment[]>;
    currentLanguage: string;
    targetLanguage: string;
    isTranslating: boolean;
    uniqueSpeakers: string[];
    isProcessing: boolean;
}>();

const emit = defineEmits<{
    (e: 'update:currentLanguage', value: string): void;
    (e: 'update:targetLanguage', value: string): void;
    (e: 'translate'): void;
    (e: 'export-video'): void;
    (
        e: 'rename-speaker',
        payload: { oldName: string; newName: string; inputElement: HTMLInputElement },
    ): void;
    (e: 'update:segments', value: TranscriptSegment[]): void;
}>();

const showLanguageDropdown = ref(false);
const videoRef = ref<HTMLVideoElement | null>(null);
const audioRef = ref<HTMLAudioElement | null>(null);
const previewIndex = ref<number | null>(null);
let previewEndTime: number | null = null;
const videoPreviewIndex = ref<number | null>(null);
let videoPreviewEndTime: number | null = null;
let lastPlayheadSource: 'audio' | 'video' | null = null;
const showOnlyReviewSegments = ref(false);
const reviewThresholdPercent = ref(85);
const hideBlacklistFromReviewFilter = ref(false);
const speakerVisibility = ref<Record<string, boolean>>({});
// Last reported media position (either player), for the editor's playing-row highlight.
const playhead = ref<number | null>(null);
const languageMenuRoot = ref<HTMLElement | null>(null);
const languageTrigger = ref<HTMLButtonElement | null>(null);
const languageMenu = ref<HTMLElement | null>(null);
const uid = Math.random().toString(36).slice(2, 8);

const syncSpeakerVisibility = (speakers: string[]) => {
    const nextVisibility: Record<string, boolean> = {};

    for (const speaker of speakers) {
        nextVisibility[speaker] = speakerVisibility.value[speaker] ?? true;
    }

    speakerVisibility.value = nextVisibility;
};

watch(
    () => props.uniqueSpeakers,
    (speakers) => {
        syncSpeakerVisibility(speakers);
    },
    { immediate: true },
);

const isSpeakerVisible = (speaker: string): boolean => speakerVisibility.value[speaker] ?? true;

const toggleSpeakerVisibility = (speaker: string, event: MouseEvent) => {
    if (event.shiftKey) {
        const visibleSpeakerCount = props.uniqueSpeakers.filter((name) =>
            isSpeakerVisible(name),
        ).length;
        const isAlreadySolo = isSpeakerVisible(speaker) && visibleSpeakerCount === 1;

        speakerVisibility.value = Object.fromEntries(
            props.uniqueSpeakers.map((name) => [name, isAlreadySolo ? true : name === speaker]),
        );
        return;
    }

    speakerVisibility.value = {
        ...speakerVisibility.value,
        [speaker]: !isSpeakerVisible(speaker),
    };
};

const showAllSpeakers = () => {
    speakerVisibility.value = Object.fromEntries(props.uniqueSpeakers.map((name) => [name, true]));
};

const speakerSegmentCounts = computed(() => {
    const counts: Record<string, number> = {};
    for (const segment of props.displaySegments) {
        counts[segment.speaker] = (counts[segment.speaker] ?? 0) + 1;
    }
    return counts;
});

const speakerVisibleSegments = computed(() =>
    props.displaySegments
        .map((segment, originalIndex) => ({ segment, originalIndex }))
        .filter(({ segment }) => isSpeakerVisible(segment.speaker)),
);

const visibleSegmentCount = computed(() => speakerVisibleSegments.value.length);
const hasSpeakerVisibilityFilter = computed(() =>
    props.uniqueSpeakers.some((speaker) => !isSpeakerVisible(speaker)),
);

const segmentNeedsReview = (segment: TranscriptSegment): boolean => {
    if (segment.reviewResolved) return false;

    if (segment.similarityScore != null) {
        return segment.similarityScore < reviewThresholdPercent.value / 100;
    }

    return segment.mergeStatus === 'missing_google' || segment.mergeStatus === 'missing_local';
};

const blacklistWarnings = computed(() =>
    detectTranscriptBlacklistMatches(props.displaySegments, props.currentLanguage),
);

const visibleBlacklistWarnings = computed(() => {
    const visibleIndices = new Set(
        speakerVisibleSegments.value.map(({ originalIndex }) => originalIndex),
    );

    return {
        ...blacklistWarnings.value,
        matches: blacklistWarnings.value.matches.filter((match) =>
            visibleIndices.has(match.segmentIndex),
        ),
        matchesBySegment: Object.fromEntries(
            Object.entries(blacklistWarnings.value.matchesBySegment).filter(([index]) =>
                visibleIndices.has(Number(index)),
            ),
        ),
        uniqueWords: blacklistWarnings.value.uniqueWords.filter((word) =>
            blacklistWarnings.value.matches.some(
                (match) => visibleIndices.has(match.segmentIndex) && match.matchedText === word,
            ),
        ),
    };
});

const hasBlacklistWarnings = computed(() => visibleBlacklistWarnings.value.matches.length > 0);
const blacklistSegmentCount = computed(
    () => Object.keys(visibleBlacklistWarnings.value.matchesBySegment).length,
);
const previewBlacklistWords = computed(() =>
    visibleBlacklistWarnings.value.uniqueWords.slice(0, 8),
);
const remainingBlacklistWordCount = computed(() =>
    Math.max(
        visibleBlacklistWarnings.value.uniqueWords.length - previewBlacklistWords.value.length,
        0,
    ),
);

const segmentNeedsAttention = (segment: TranscriptSegment, index: number): boolean => {
    if (segment.reviewResolved) return false;
    if (segmentNeedsReview(segment)) return true;
    if (hideBlacklistFromReviewFilter.value) return false;

    return (blacklistWarnings.value.matchesBySegment[index]?.length ?? 0) > 0;
};

const hasReviewMetadata = computed(() =>
    speakerVisibleSegments.value.some(
        ({ segment, originalIndex }) =>
            segmentNeedsAttention(segment, originalIndex) ||
            (segment.alternatives?.length ?? 0) > 0,
    ),
);

const reviewSegmentCount = computed(
    () =>
        speakerVisibleSegments.value.filter(({ segment, originalIndex }) =>
            segmentNeedsAttention(segment, originalIndex),
        ).length,
);

function selectLanguage(langName: string) {
    emit('update:targetLanguage', langName);
    showLanguageDropdown.value = false;
    languageTrigger.value?.focus();
    if (props.translations[langName]) {
        emit('update:currentLanguage', langName);
    }
}

function languageOptions(): HTMLButtonElement[] {
    return Array.from(
        languageMenu.value?.querySelectorAll<HTMLButtonElement>('[role="option"]') ?? [],
    );
}

async function toggleLanguageDropdown() {
    showLanguageDropdown.value = !showLanguageDropdown.value;
    if (!showLanguageDropdown.value) return;
    await nextTick();
    const options = languageOptions();
    (options.find((o) => o.getAttribute('aria-selected') === 'true') ?? options[0])?.focus();
}

function closeLanguageDropdown(refocus = false) {
    if (!showLanguageDropdown.value) return;
    showLanguageDropdown.value = false;
    if (refocus) languageTrigger.value?.focus();
}

function onLanguageMenuKeydown(event: KeyboardEvent) {
    const options = languageOptions();
    const index = options.indexOf(document.activeElement as HTMLButtonElement);
    let next: number | null = null;
    if (event.key === 'ArrowDown') next = Math.min(index + 1, options.length - 1);
    else if (event.key === 'ArrowUp') next = Math.max(index - 1, 0);
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = options.length - 1;
    else if (event.key === 'Escape' || event.key === 'Tab') {
        if (event.key === 'Escape') event.preventDefault();
        closeLanguageDropdown(event.key === 'Escape');
        return;
    }
    if (next === null) return;
    event.preventDefault();
    options[next]?.focus();
}

function onDocumentPointerDown(event: MouseEvent) {
    if (!showLanguageDropdown.value) return;
    const target = event.target;
    if (target instanceof Node && languageMenuRoot.value?.contains(target)) return;
    closeLanguageDropdown();
}

onMounted(() => document.addEventListener('mousedown', onDocumentPointerDown));
onBeforeUnmount(() => document.removeEventListener('mousedown', onDocumentPointerDown));

// Why the primary actions are unavailable, shown next to them and as tooltips.
const exportDisabledReason = computed(() => {
    if (!props.hasMediaFile)
        return 'The source media file is missing. Select it again on the Source step to export.';
    if (props.originalSegments.length === 0) return 'The transcript has no segments to export.';
    if (props.isProcessing) return 'Another job is running. Export is available when it finishes.';
    return '';
});

const translateDisabledReason = computed(() => {
    if (props.isTranslating) return 'Translating…';
    if (props.isProcessing) return 'Another job is running';
    if (!props.targetLanguage) return 'Pick a language to translate to first';
    if (props.translations[props.targetLanguage])
        return `Already translated to ${props.targetLanguage}. Pick it in the view menu.`;
    return '';
});

const hasExtractedAudio = computed(() => !!props.extractedAudioPath);

function markAudioActive() {
    lastPlayheadSource = 'audio';
}

function markVideoActive() {
    lastPlayheadSource = 'video';
}

// Report the current playback position so a segment split can snap to the
// exact moment the user is hearing. Prefers the player that is actually
// playing, then the one most recently used.
function getPlayhead(): number | null {
    const audio = audioRef.value;
    const video = videoRef.value;
    if (audio && !audio.paused) return audio.currentTime;
    if (video && !video.paused) return video.currentTime;
    if (lastPlayheadSource === 'audio' && audio) return audio.currentTime;
    if (lastPlayheadSource === 'video' && video) return video.currentTime;
    if (audio) return audio.currentTime;
    if (video) return video.currentTime;
    return null;
}

function clearPreviewState() {
    previewIndex.value = null;
    previewEndTime = null;
}

function previewSegment(payload: { start: string; end: string; index: number }) {
    const audio = audioRef.value;
    if (!audio) return;

    // Toggle off when the same segment is already previewing.
    if (previewIndex.value === payload.index && !audio.paused) {
        audio.pause();
        return;
    }

    // Avoid overlapping audio from the video player.
    if (videoRef.value && !videoRef.value.paused) {
        videoRef.value.pause();
    }

    const start = parseTime(payload.start);
    const end = parseTime(payload.end);
    previewEndTime = Number.isFinite(end) && end > start ? end : null;
    previewIndex.value = payload.index;
    audio.currentTime = start;
    void audio.play().catch((error) => {
        console.error('Failed to preview segment audio:', error);
        clearPreviewState();
    });
}

function onAudioTimeUpdate() {
    if (audioRef.value) playhead.value = audioRef.value.currentTime;
    if (previewEndTime === null || !audioRef.value) return;
    if (audioRef.value.currentTime >= previewEndTime) {
        audioRef.value.pause();
    }
}

function clearVideoPreviewState() {
    videoPreviewIndex.value = null;
    videoPreviewEndTime = null;
}

function previewVideoSegment(payload: { start: string; end: string; index: number }) {
    const video = videoRef.value;
    if (!video) return;

    // Toggle off when the same segment is already previewing.
    if (videoPreviewIndex.value === payload.index && !video.paused) {
        video.pause();
        return;
    }

    // Avoid overlapping audio from the extracted-audio preview.
    if (audioRef.value && !audioRef.value.paused) {
        audioRef.value.pause();
    }

    const start = parseTime(payload.start);
    const end = parseTime(payload.end);
    videoPreviewEndTime = Number.isFinite(end) && end > start ? end : null;
    videoPreviewIndex.value = payload.index;
    video.currentTime = start;
    void video.play().catch((error) => {
        console.error('Failed to preview segment video:', error);
        clearVideoPreviewState();
    });
}

// "Start from here": seek and play onward, leaving the silence-skip logic active.
function jumpTo(time: number) {
    if (!videoRef.value) return;
    clearVideoPreviewState();
    if (audioRef.value && !audioRef.value.paused) {
        audioRef.value.pause();
    }
    videoRef.value.currentTime = time;
    void videoRef.value.play();
}

function onTimeUpdate() {
    if (!videoRef.value) return;
    playhead.value = videoRef.value.currentTime;

    // Bounded segment preview: stop once the segment end is reached.
    if (videoPreviewEndTime !== null) {
        if (videoRef.value.currentTime >= videoPreviewEndTime) {
            videoRef.value.pause();
        }
        return;
    }

    if (props.originalSegments.length === 0) return;

    const currentTime = videoRef.value.currentTime;
    let inside = false;
    let nextStart = -1;

    for (const seg of props.originalSegments) {
        const start = parseTime(seg.start);
        const end = parseTime(seg.end);

        if (currentTime >= start && currentTime < end) {
            inside = true;
            break;
        }
        if (start > currentTime) {
            nextStart = start;
            break;
        }
    }

    if (!inside && nextStart !== -1) {
        videoRef.value.currentTime = nextStart;
    } else if (!inside && nextStart === -1) {
        const lastEnd = parseTime(props.originalSegments[props.originalSegments.length - 1].end);
        if (currentTime > lastEnd) {
            videoRef.value.pause();
        }
    }
}
</script>

<template>
    <div class="@container mx-auto w-full max-w-[1400px] px-8 pt-8 pb-10">
        <PageHeader
            title="Transcript"
            description="Review and correct the transcript, then export the cut video or subtitles."
        >
            <template #actions>
                <div class="flex flex-col items-start gap-1.5 @5xl:items-end">
                    <div class="flex flex-wrap items-start gap-2">
                        <SubtitleExport
                            :segments="displaySegments"
                            :cutSegments="originalSegments"
                            :inputPath="inputPath"
                            :language="currentLanguage"
                            :disabled="!hasMediaFile"
                        />
                        <button
                            type="button"
                            data-testid="export-video"
                            class="btn-primary"
                            :disabled="
                                originalSegments.length === 0 || isProcessing || !hasMediaFile
                            "
                            :title="
                                exportDisabledReason ||
                                'Export the video with the current cuts applied'
                            "
                            @click="$emit('export-video')"
                        >
                            <Film aria-hidden="true" />
                            Export video
                        </button>
                    </div>
                    <!-- Directly under the actions it explains. -->
                    <p
                        v-if="exportDisabledReason"
                        data-testid="export-disabled-reason"
                        class="flex max-w-[34rem] items-start gap-1.5 text-[12px] leading-snug text-warning @5xl:text-right"
                    >
                        <TriangleAlert class="mt-px h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                        {{ exportDisabledReason }}
                    </p>
                </div>
            </template>
        </PageHeader>

        <div class="grid gap-5 @5xl:grid-cols-[minmax(0,380px)_minmax(0,1fr)] @5xl:items-start">
            <!-- Inspector: media, speakers and review state. Sticky beside the
           transcript on wide windows, stacked above it on narrow ones. -->
            <aside
                class="-m-0.5 flex min-w-0 flex-col gap-3 p-0.5 @5xl:sticky @5xl:top-4 @5xl:max-h-[calc(100vh-6rem)] @5xl:overflow-y-auto"
                aria-label="Media and speakers"
            >
                <div
                    v-if="hasMediaFile"
                    class="overflow-hidden rounded-lg border border-line bg-black"
                >
                    <video
                        ref="videoRef"
                        :src="mediaUrl(inputPath)"
                        class="mx-auto block max-h-[45vh] w-full [color-scheme:dark]"
                        controls
                        @timeupdate="onTimeUpdate"
                        @play="markVideoActive"
                        @pause="clearVideoPreviewState"
                        @ended="clearVideoPreviewState"
                    ></video>
                </div>
                <div
                    v-else
                    data-testid="missing-media-notice"
                    class="flex gap-2.5 rounded-lg border border-warning/25 bg-warning/[0.07] p-3"
                >
                    <TriangleAlert
                        class="mt-0.5 h-4 w-4 shrink-0 text-warning"
                        aria-hidden="true"
                    />
                    <div>
                        <p class="section-title">Media preview unavailable</p>
                        <p class="hint mt-0.5">
                            The saved file path no longer exists. Select the source file again to
                            re-enable playback and export actions.
                        </p>
                    </div>
                </div>

                <div v-if="hasExtractedAudio" class="panel px-3 pt-2.5 pb-3">
                    <p class="eyebrow mb-2 flex items-center gap-1.5">
                        <AudioLines class="h-3.5 w-3.5" aria-hidden="true" />
                        Audio
                    </p>
                    <!--
            `extractedAudioPath` is the seekable AAC/m4a preview (see
            prepare_preview_audio), on the transcript's original timeline. We use it
            rather than the source file because the webview (WKWebView on macOS)
            often can't decode the source's audio track, and it can't reliably seek
            the Opus/Ogg analysis audio (bogus duration + mis-seeking).
          -->
                    <audio
                        ref="audioRef"
                        :src="mediaUrl(extractedAudioPath!)"
                        data-testid="extracted-audio"
                        class="block h-9 w-full [color-scheme:dark]"
                        controls
                        preload="metadata"
                        @timeupdate="onAudioTimeUpdate"
                        @play="markAudioActive"
                        @pause="clearPreviewState"
                        @ended="clearPreviewState"
                    ></audio>
                    <p class="hint mt-1.5">
                        The speaker button on each segment previews the audio at that timestamp.
                    </p>
                </div>

                <SectionCard
                    v-if="uniqueSpeakers.length > 0"
                    title="Speakers"
                    description="Edit a name to rename. Hide with the eye; Shift-click to solo."
                >
                    <template v-if="hasSpeakerVisibilityFilter" #actions>
                        <button
                            type="button"
                            data-testid="speaker-show-all"
                            class="btn-ghost btn-sm"
                            @click="showAllSpeakers"
                        >
                            Show all
                        </button>
                    </template>
                    <ul class="space-y-1.5">
                        <li
                            v-for="speaker in uniqueSpeakers"
                            :key="speaker"
                            class="flex items-center gap-2"
                        >
                            <span
                                aria-hidden="true"
                                class="h-2.5 w-2.5 shrink-0 rounded-full transition-opacity duration-150"
                                :class="isSpeakerVisible(speaker) ? '' : 'opacity-30'"
                                :style="{ background: speakerColor(speaker, uniqueSpeakers) }"
                            ></span>
                            <input
                                :value="speaker"
                                :aria-label="`Rename ${speaker}`"
                                class="input input-sm min-w-0 flex-1"
                                :class="
                                    isSpeakerVisible(speaker) ? '' : 'text-fg-subtle line-through'
                                "
                                @change="
                                    $emit('rename-speaker', {
                                        oldName: speaker,
                                        newName: ($event.target as HTMLInputElement).value,
                                        inputElement: $event.target as HTMLInputElement,
                                    })
                                "
                            />
                            <span
                                class="w-7 shrink-0 text-right font-mono text-[11px] text-fg-subtle"
                                :title="`${speakerSegmentCounts[speaker] ?? 0} segments`"
                                >{{ speakerSegmentCounts[speaker] ?? 0 }}</span
                            >
                            <button
                                :data-testid="`speaker-visibility-toggle-${speaker}`"
                                type="button"
                                class="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md transition-colors duration-100 hover:bg-surface-hover"
                                :class="
                                    isSpeakerVisible(speaker)
                                        ? 'text-fg-muted hover:text-fg'
                                        : 'text-fg-faint hover:text-fg-muted'
                                "
                                :aria-pressed="isSpeakerVisible(speaker) ? 'true' : 'false'"
                                :aria-label="`Show ${speaker}`"
                                :title="
                                    (isSpeakerVisible(speaker)
                                        ? `Hide ${speaker}`
                                        : `Show ${speaker}`) + ' (Shift-click to solo)'
                                "
                                @click="toggleSpeakerVisibility(speaker, $event)"
                            >
                                <Eye
                                    v-if="isSpeakerVisible(speaker)"
                                    class="h-4 w-4"
                                    aria-hidden="true"
                                />
                                <EyeOff v-else class="h-4 w-4" aria-hidden="true" />
                            </button>
                        </li>
                    </ul>
                </SectionCard>

                <SectionCard
                    v-if="hasReviewMetadata"
                    title="Review"
                    :description="`Segments missing a counterpart, below ${reviewThresholdPercent}% similarity, or matching the blacklist.`"
                >
                    <template #actions>
                        <span class="chip-warning" data-testid="review-count"
                            >{{ reviewSegmentCount }} to review</span
                        >
                    </template>
                    <div class="space-y-3">
                        <div class="flex items-center justify-between gap-3">
                            <span :id="`${uid}-only-review`" class="text-[13px] text-fg-muted"
                                >Show only review items</span
                            >
                            <UiSwitch
                                v-model="showOnlyReviewSegments"
                                data-testid="review-filter-toggle"
                                size="sm"
                                :aria-labelledby="`${uid}-only-review`"
                            />
                        </div>
                        <div class="flex items-center justify-between gap-3">
                            <span :id="`${uid}-hide-blacklist`" class="text-[13px] text-fg-muted"
                                >Hide blacklist-only matches</span
                            >
                            <UiSwitch
                                v-model="hideBlacklistFromReviewFilter"
                                data-testid="review-filter-hide-blacklist-toggle"
                                size="sm"
                                :aria-labelledby="`${uid}-hide-blacklist`"
                            />
                        </div>
                        <div>
                            <div class="flex items-center justify-between">
                                <label :for="`${uid}-threshold`" class="field-label mb-0!"
                                    >Review below similarity</label
                                >
                                <span class="font-mono text-[12px] text-fg-muted"
                                    >{{ reviewThresholdPercent }}%</span
                                >
                            </div>
                            <input
                                :id="`${uid}-threshold`"
                                v-model.number="reviewThresholdPercent"
                                data-testid="review-filter-threshold"
                                type="range"
                                min="0"
                                max="100"
                                step="1"
                                class="mt-2 block w-full"
                            />
                        </div>
                    </div>
                </SectionCard>

                <SectionCard
                    v-if="hasBlacklistWarnings"
                    data-testid="blacklist-warnings"
                    title="Blacklist warnings"
                    :description="`${visibleBlacklistWarnings.matches.length} word-level matches across ${blacklistSegmentCount} segments using the ${visibleBlacklistWarnings.languageLabel ?? visibleBlacklistWarnings.languageCode} list.`"
                >
                    <div class="flex flex-wrap gap-1.5">
                        <span
                            v-for="word in previewBlacklistWords"
                            :key="word"
                            class="chip-warning"
                        >
                            {{ word }}
                        </span>
                        <span v-if="remainingBlacklistWordCount > 0" class="chip">
                            +{{ remainingBlacklistWordCount }} more
                        </span>
                    </div>
                </SectionCard>
            </aside>

            <section class="min-w-0" aria-label="Transcript editor">
                <p
                    v-if="currentLanguage !== 'Original'"
                    data-testid="translation-edit-hint"
                    class="mb-5 flex items-start gap-2 rounded-lg border border-info/25 bg-info/[0.08] px-3 py-2 text-[12px] leading-relaxed text-fg-muted"
                >
                    <Info class="mt-0.5 h-3.5 w-3.5 shrink-0 text-info" aria-hidden="true" />
                    <span>
                        Viewing the {{ currentLanguage }} translation: you can edit the wording
                        here. Switch to Original to cut, split, merge or retime segments.
                    </span>
                </p>

                <Editor
                    :segments="displaySegments"
                    :lockStructure="currentLanguage !== 'Original'"
                    :speakerVisibility="speakerVisibility"
                    :showOnlyReviewSegments="showOnlyReviewSegments"
                    :reviewThreshold="reviewThresholdPercent / 100"
                    :hideBlacklistFromReview="hideBlacklistFromReviewFilter"
                    :blacklistMatchesBySegment="blacklistWarnings.matchesBySegment"
                    :audioAvailable="hasExtractedAudio"
                    :previewIndex="previewIndex"
                    :videoAvailable="hasMediaFile"
                    :videoPreviewIndex="videoPreviewIndex"
                    :getPlayhead="getPlayhead"
                    :playhead="playhead"
                    :speakerOrder="uniqueSpeakers"
                    @jump-to="jumpTo"
                    @preview="previewSegment"
                    @preview-video="previewVideoSegment"
                    @update:segments="$emit('update:segments', $event)"
                >
                    <template #status>
                        <span
                            class="chip h-6 px-2.5 text-[12px] whitespace-nowrap"
                            data-testid="segment-count"
                            >{{ visibleSegmentCount
                            }}{{
                                hasSpeakerVisibilityFilter ? ` of ${displaySegments.length}` : ''
                            }}
                            {{ displaySegments.length === 1 ? 'segment' : 'segments' }}</span
                        >
                    </template>
                    <template #toolbar>
                        <label :for="`${uid}-view-language`" class="sr-only"
                            >Viewing language</label
                        >
                        <select
                            :id="`${uid}-view-language`"
                            :value="currentLanguage"
                            data-testid="view-language-select"
                            class="input input-sm h-7 w-auto min-w-[7.5rem]"
                            title="Which version of the transcript you see and edit"
                            @change="
                                $emit(
                                    'update:currentLanguage',
                                    ($event.target as HTMLSelectElement).value,
                                )
                            "
                        >
                            <option value="Original">Original</option>
                            <option v-for="(_, lang) in translations" :key="lang" :value="lang">
                                {{ lang }}
                            </option>
                        </select>

                        <div ref="languageMenuRoot" class="relative">
                            <button
                                ref="languageTrigger"
                                type="button"
                                data-testid="target-language-trigger"
                                class="btn-secondary btn-sm h-7 w-40 justify-between"
                                aria-haspopup="listbox"
                                :aria-expanded="showLanguageDropdown ? 'true' : 'false'"
                                title="Language to translate into"
                                @click="toggleLanguageDropdown"
                                @keydown.down.prevent="
                                    !showLanguageDropdown && toggleLanguageDropdown()
                                "
                            >
                                <span class="flex min-w-0 items-center gap-1.5">
                                    <Languages
                                        class="h-3.5 w-3.5 shrink-0 text-fg-subtle"
                                        aria-hidden="true"
                                    />
                                    <span
                                        class="truncate"
                                        :class="targetLanguage ? '' : 'text-fg-subtle'"
                                        >{{ targetLanguage || 'Translate to…' }}</span
                                    >
                                </span>
                                <ChevronDown
                                    class="h-3.5 w-3.5 shrink-0 text-fg-subtle transition-transform duration-150"
                                    :class="showLanguageDropdown ? 'rotate-180' : ''"
                                    aria-hidden="true"
                                />
                            </button>

                            <Transition name="pop">
                                <div
                                    v-if="showLanguageDropdown"
                                    ref="languageMenu"
                                    role="listbox"
                                    aria-label="Translate to"
                                    class="popover absolute top-full right-0 z-50 mt-1 max-h-72 w-56 origin-top-right overflow-y-auto p-1"
                                    @keydown="onLanguageMenuKeydown"
                                >
                                    <button
                                        v-for="lang in SUPPORTED_TRANSCRIPT_LANGUAGES"
                                        :key="lang.code"
                                        type="button"
                                        role="option"
                                        :aria-selected="
                                            targetLanguage === lang.name ? 'true' : 'false'
                                        "
                                        class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] outline-none transition-colors duration-100 hover:bg-surface-hover hover:text-fg focus-visible:bg-surface-3 focus-visible:text-fg"
                                        :class="
                                            targetLanguage === lang.name
                                                ? 'bg-accent/12 text-fg'
                                                : 'text-fg-muted'
                                        "
                                        @click="selectLanguage(lang.name)"
                                    >
                                        <span
                                            :class="`fi fi-${lang.country}`"
                                            class="rounded-[2px]"
                                        ></span>
                                        <span class="flex-1">{{ lang.name }}</span>
                                        <span
                                            v-if="translations[lang.name]"
                                            class="flex items-center gap-1 text-[11px] text-success"
                                        >
                                            <Check class="h-3.5 w-3.5" aria-hidden="true" />
                                            <span class="sr-only">translated</span>
                                        </span>
                                    </button>
                                </div>
                            </Transition>
                        </div>

                        <button
                            type="button"
                            data-testid="translate-button"
                            class="btn-subtle btn-sm h-7"
                            :disabled="
                                isTranslating ||
                                isProcessing ||
                                !targetLanguage ||
                                !!translations[targetLanguage]
                            "
                            :title="translateDisabledReason || 'Translate'"
                            @click="$emit('translate')"
                        >
                            <Loader2 v-if="isTranslating" class="animate-spin" aria-hidden="true" />
                            <Languages v-else aria-hidden="true" />
                            {{ isTranslating ? 'Translating…' : 'Translate' }}
                        </button>
                    </template>
                </Editor>
            </section>
        </div>
    </div>
</template>
