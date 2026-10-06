<script setup lang="ts">
import { computed } from 'vue';
import type { Intensity, Transition } from '../bindings';

/** The splice transitions to choose from (the export renders them with ffmpeg). */
export type SpliceTransition = Exclude<Transition, 'cut' | 'morph'>;

const props = defineProps<{
    /** The chosen transitions, taken in turn; null: the intensity's own. */
    modelValue: Transition[] | null;
    intensity: Intensity;
    /** Only 9:16 exports use transitions. */
    inactive?: boolean;
}>();

const emit = defineEmits<{ 'update:modelValue': [value: Transition[] | null] }>();

const TRANSITIONS: { value: SpliceTransition; label: string; text: string }[] = [
    { value: 'whip', label: 'Whip', text: 'Fast slide with motion blur.' },
    { value: 'whip_up', label: 'Whip up', text: 'Slides up with vertical motion blur.' },
    { value: 'fade', label: 'Fade', text: 'Short crossfade.' },
    { value: 'blur_zoom', label: 'Blur zoom', text: 'Zooms into a blur and out of it.' },
    { value: 'blur_cut', label: 'Blur cut', text: 'Hard cut hidden in a blur burst.' },
    { value: 'wipe_cut', label: 'Wipe cut', text: 'Fast hard-edged wipe.' },
    { value: 'flash', label: 'Flash', text: 'Dips through white.' },
    { value: 'dip_to_black', label: 'Dip to black', text: 'Dips through black.' },
    { value: 'pixelate', label: 'Pixelate', text: 'Breaks into blocks and back.' },
    { value: 'iris', label: 'Iris', text: 'A circle opens onto the next shot.' },
];

/** What "Auto" uses, per intensity (as the backend's cut styles). */
const AUTO: Record<Intensity, Transition> = {
    off: 'cut',
    chill: 'fade',
    punchy: 'whip',
    hyper: 'whip',
};

const auto = computed(() => props.modelValue === null);
const chosen = computed(() => props.modelValue ?? [AUTO[props.intensity]]);

const summary = computed(() => {
    const labels = chosen.value
        .map((value) => TRANSITIONS.find((option) => option.value === value)?.label)
        .filter(Boolean);
    if (labels.length === 0) {
        return auto.value ? 'Auto: plain cuts at this tightening.' : 'None: plain cuts.';
    }
    const list = labels.join(', ');
    if (auto.value) {
        return `Auto: ${list}, from the tightening.`;
    }
    return labels.length > 1 ? `${list}, taken in turn.` : list;
});

function toggle(value: SpliceTransition) {
    // From Auto, a click picks just that one.
    const current = props.modelValue ?? [];
    emit(
        'update:modelValue',
        current.includes(value)
            ? current.filter((item) => item !== value)
            : TRANSITIONS.map((option) => option.value).filter(
                  (item) => item === value || current.includes(item),
              ),
    );
}
</script>

<template>
    <div class="mb-8" :class="{ 'opacity-60': inactive }" data-testid="clips-transitions">
        <div class="mb-3 flex flex-wrap items-center gap-3">
            <span class="text-xs font-bold uppercase tracking-wider text-gray-400"
                >Transitions</span
            >
            <button
                type="button"
                role="switch"
                :aria-checked="auto"
                data-testid="clips-transitions-auto"
                class="rounded-xl border border-white/10 px-3 py-1 text-xs transition-colors"
                :class="
                    auto
                        ? 'bg-pink-600/80 font-semibold text-white'
                        : 'bg-black/20 text-gray-300 hover:bg-white/10'
                "
                @click="emit('update:modelValue', auto ? chosen.filter((t) => t !== 'cut') : null)"
            >
                Auto
            </button>
            <span class="text-xs text-gray-500">{{ summary }}</span>
        </div>
        <p class="mb-3 text-xs text-gray-500">
            Where a 9:16 clip jumps to another moment.
            <span v-if="inactive">Turn on Vertical 9:16 to use them.</span>
        </p>
        <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
            <button
                v-for="option in TRANSITIONS"
                :key="option.value"
                type="button"
                role="checkbox"
                :aria-checked="chosen.includes(option.value)"
                :data-testid="`clips-transition-${option.value}`"
                class="flex items-center gap-3 rounded-xl border p-2 text-left transition-colors"
                :class="
                    chosen.includes(option.value)
                        ? auto
                            ? 'border-dashed border-pink-500/60 bg-pink-500/5'
                            : 'border-pink-500/70 bg-pink-500/10'
                        : 'border-white/5 bg-black/20 hover:bg-white/5'
                "
                @click="toggle(option.value)"
            >
                <span class="preview" :class="`t-${option.value}`" aria-hidden="true">
                    <span class="shot a" />
                    <span class="shot b" />
                    <span class="fx" />
                </span>
                <span class="min-w-0">
                    <span class="block text-sm font-semibold text-gray-200">{{
                        option.label
                    }}</span>
                    <span class="block text-xs leading-snug text-gray-500">{{ option.text }}</span>
                </span>
            </button>
        </div>
    </div>
</template>

<style scoped>
/*
 * A looping 9:16 thumbnail per transition: shot A, the transition, shot B.
 * The export renders them with ffmpeg; these approximate the look in CSS.
 */
.preview {
    --cycle: 2.4s;
    position: relative;
    flex: none;
    width: 36px;
    height: 64px;
    overflow: hidden;
    border-radius: 6px;
    background: #000;
}

.shot,
.fx {
    position: absolute;
    inset: 0;
    animation-duration: var(--cycle);
    animation-iteration-count: infinite;
    animation-timing-function: ease-in-out;
}

/* Two shots with some detail (a person against a backdrop), so blur and
   blocks read. */
.shot.a {
    background:
        radial-gradient(circle at 50% 38%, #fde68a 0 17%, transparent 18%),
        radial-gradient(ellipse at 50% 92%, #fbbf24 0 34%, transparent 35%),
        linear-gradient(160deg, #f43f5e, #7c2d12);
}

.shot.b {
    background:
        radial-gradient(circle at 42% 40%, #e0f2fe 0 15%, transparent 16%),
        radial-gradient(ellipse at 42% 92%, #38bdf8 0 32%, transparent 33%),
        linear-gradient(200deg, #6366f1, #0c4a6e);
    opacity: 0;
}

.fx {
    opacity: 0;
}

/* Under reduced motion, previews only play on hover. */
@media (prefers-reduced-motion: reduce) {
    .shot,
    .fx {
        animation-play-state: paused;
    }

    button:hover .shot,
    button:hover .fx {
        animation-play-state: running;
    }
}

/* Default for B: shown from the switch on. */
@keyframes show-b {
    0%,
    50% {
        opacity: 0;
    }
    50.01%,
    100% {
        opacity: 1;
    }
}

/* Whip: slide left with motion blur. */
.t-whip .a {
    animation-name: whip-a;
}
.t-whip .b {
    animation-name: whip-b;
}
@keyframes whip-a {
    0%,
    40% {
        transform: translateX(0);
        filter: blur(0);
    }
    60%,
    100% {
        transform: translateX(-100%);
        filter: blur(3px);
    }
}
@keyframes whip-b {
    0%,
    40% {
        opacity: 1;
        transform: translateX(100%);
        filter: blur(3px);
    }
    60%,
    100% {
        opacity: 1;
        transform: translateX(0);
        filter: blur(0);
    }
}

/* Whip up. */
.t-whip_up .a {
    animation-name: whip-up-a;
}
.t-whip_up .b {
    animation-name: whip-up-b;
}
@keyframes whip-up-a {
    0%,
    40% {
        transform: translateY(0);
        filter: blur(0);
    }
    60%,
    100% {
        transform: translateY(-100%);
        filter: blur(3px);
    }
}
@keyframes whip-up-b {
    0%,
    40% {
        opacity: 1;
        transform: translateY(100%);
        filter: blur(3px);
    }
    60%,
    100% {
        opacity: 1;
        transform: translateY(0);
        filter: blur(0);
    }
}

/* Fade. */
.t-fade .b {
    animation-name: fade-b;
}
@keyframes fade-b {
    0%,
    38% {
        opacity: 0;
    }
    62%,
    100% {
        opacity: 1;
    }
}

/* Blur zoom: A zooms into a blur, B comes out of it. */
.t-blur_zoom .a {
    animation-name: blur-zoom-a;
}
.t-blur_zoom .b {
    animation-name: blur-zoom-b;
}
@keyframes blur-zoom-a {
    0%,
    38% {
        transform: scale(1);
        filter: blur(0);
    }
    50% {
        transform: scale(1.7);
        filter: blur(4px);
    }
    100% {
        transform: scale(1.7);
        filter: blur(4px);
    }
}
@keyframes blur-zoom-b {
    0%,
    44% {
        opacity: 0;
        transform: scale(1.3);
        filter: blur(4px);
    }
    56% {
        opacity: 1;
    }
    64%,
    100% {
        opacity: 1;
        transform: scale(1);
        filter: blur(0);
    }
}

/* Blur cut: a hard cut inside a blur burst. */
.t-blur_cut .a {
    animation-name: blur-cut-a;
}
.t-blur_cut .b {
    animation-name: blur-cut-b;
}
@keyframes blur-cut-a {
    0%,
    42% {
        filter: blur(0);
    }
    50%,
    100% {
        filter: blur(5px);
    }
}
@keyframes blur-cut-b {
    0%,
    50% {
        opacity: 0;
        filter: blur(5px);
    }
    50.01% {
        opacity: 1;
        filter: blur(5px);
    }
    58%,
    100% {
        opacity: 1;
        filter: blur(0);
    }
}

/* Wipe cut: a fast hard-edged wipe from the right. */
.t-wipe_cut .b {
    animation-name: wipe-cut-b;
    animation-timing-function: linear;
}
@keyframes wipe-cut-b {
    0%,
    44% {
        opacity: 1;
        clip-path: inset(0 0 0 100%);
    }
    56%,
    100% {
        opacity: 1;
        clip-path: inset(0 0 0 0);
    }
}

/* Flash and dip to black: through a colour. */
.t-flash .b,
.t-dip_to_black .b,
.t-pixelate .b {
    animation-name: show-b;
}
.t-flash .fx {
    background: #fff;
    animation-name: dip;
}
.t-dip_to_black .fx {
    background: #000;
    animation-name: dip;
}
@keyframes dip {
    0%,
    36% {
        opacity: 0;
    }
    50% {
        opacity: 1;
    }
    64%,
    100% {
        opacity: 0;
    }
}

/* Pixelate: a mosaic over a blur, coarsest at the switch. */
.t-pixelate .fx {
    background:
        repeating-linear-gradient(90deg, rgb(0 0 0 / 0.25) 0 1px, transparent 1px 9px),
        repeating-linear-gradient(0deg, rgb(0 0 0 / 0.25) 0 1px, transparent 1px 9px);
    backdrop-filter: blur(4px) saturate(1.3);
    animation-name: dip;
    animation-timing-function: steps(4, end);
}

/* Iris: a circle opens onto B. */
.t-iris .b {
    animation-name: iris-b;
}
@keyframes iris-b {
    0%,
    38% {
        opacity: 1;
        clip-path: circle(0% at 50% 50%);
    }
    64%,
    100% {
        opacity: 1;
        clip-path: circle(75% at 50% 50%);
    }
}
</style>
