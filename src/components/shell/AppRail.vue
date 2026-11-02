<script setup lang="ts">
// The app's left rail: brand, the open project, the workspace steps and
// Settings. Collapses to icons; the choice is remembered.
import { computed, ref, watch } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import {
    AudioLines,
    Check,
    Clapperboard,
    FileVideo,
    Mic,
    PanelLeftClose,
    PanelLeftOpen,
    Settings as SettingsIcon,
    TextQuote,
    Loader2,
} from '@lucide/vue';
import { useWorkspaceNav, type WorkspaceStep } from '../../composables/useWorkspaceNav';

const COLLAPSE_KEY = 'rail-collapsed';

const route = useRoute();
const router = useRouter();
const { state, steps, mediaName, selectStep } = useWorkspaceNav();

const collapsed = ref(localStorage.getItem(COLLAPSE_KEY) === '1');
watch(collapsed, (value) => localStorage.setItem(COLLAPSE_KEY, value ? '1' : '0'));

const isMac = typeof navigator !== 'undefined' && /Mac/.test(navigator.userAgent);
const onSettings = computed(() => route.path.startsWith('/settings'));

const icons: Record<WorkspaceStep, typeof FileVideo> = {
    source: FileVideo,
    transcript: TextQuote,
    clips: Clapperboard,
    podcast: Mic,
};

const projectStatus = computed(() => {
    if (state.busy) return 'Working…';
    if (state.hasTranscript) return 'Transcript ready';
    if (state.mediaPath && !state.hasMediaFile) return 'Media file missing';
    if (state.mediaPath) return 'Not analysed yet';
    return 'Pick a file to begin';
});

async function open(step: WorkspaceStep) {
    if (steps.value.find((s) => s.id === step)?.disabled) return;
    if (onSettings.value) {
        // Settings may hold the user back (unsaved changes): only switch the
        // step once the navigation went through.
        const failure = await router.push('/');
        if (failure || route.path.startsWith('/settings')) return;
    }
    selectStep(step);
}

function onStepKeydown(event: KeyboardEvent, index: number) {
    if (!['ArrowDown', 'ArrowUp'].includes(event.key)) return;
    event.preventDefault();
    const list = (event.currentTarget as HTMLElement).parentElement?.parentElement;
    const buttons = Array.from(list?.querySelectorAll<HTMLButtonElement>('[data-step]') ?? []);
    const delta = event.key === 'ArrowDown' ? 1 : -1;
    for (let i = 1; i <= buttons.length; i++) {
        const next = buttons[(index + delta * i + buttons.length) % buttons.length];
        if (!next.disabled) {
            next.focus();
            break;
        }
    }
}
</script>

<template>
    <aside
        class="relative flex h-full shrink-0 flex-col border-r border-line bg-rail transition-[width] duration-[var(--duration-slow)] ease-[var(--ease-out-expo)]"
        :class="collapsed ? 'w-[60px]' : 'w-[228px]'"
        aria-label="Application"
    >
        <!-- Brand / window drag area (traffic lights live here on macOS). -->
        <div
            data-tauri-drag-region
            class="flex shrink-0 items-center gap-2.5 px-4"
            :class="isMac ? 'h-[52px] pt-[22px]' : 'h-14'"
        >
            <img
                src="/logo.svg"
                alt=""
                class="h-6 w-6 shrink-0 rounded-md"
                data-tauri-drag-region
                :class="collapsed ? 'mx-auto' : ''"
            />
            <span
                v-if="!collapsed"
                data-tauri-drag-region
                class="truncate text-[13px] font-semibold tracking-tight text-fg"
            >
                Media Cutter
            </span>
        </div>

        <!-- Open project -->
        <div class="px-2.5 pt-3">
            <button
                type="button"
                class="group flex w-full items-center gap-2.5 rounded-lg border border-line bg-surface p-2 text-left transition-colors hover:border-line-strong hover:bg-surface-2"
                :class="collapsed ? 'justify-center' : ''"
                :title="state.mediaPath || 'No media selected'"
                @click="open('source')"
            >
                <span
                    class="relative flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-gradient-to-br from-accent/30 to-accent/5 text-accent-soft"
                >
                    <Loader2 v-if="state.busy" class="h-4 w-4 animate-spin" aria-hidden="true" />
                    <AudioLines v-else class="h-4 w-4" aria-hidden="true" />
                    <span
                        v-if="state.hasTranscript && !state.busy"
                        class="absolute -right-0.5 -bottom-0.5 h-2.5 w-2.5 rounded-full border-2 border-surface bg-success"
                    />
                </span>
                <span v-if="!collapsed" class="min-w-0 flex-1">
                    <span class="block truncate text-[12.5px] font-medium text-fg">
                        {{ mediaName || 'No media' }}
                    </span>
                    <span
                        class="block truncate text-[11px]"
                        :class="
                            state.mediaPath && !state.hasMediaFile
                                ? 'text-warning'
                                : 'text-fg-subtle'
                        "
                    >
                        {{ projectStatus }}
                    </span>
                </span>
            </button>
        </div>

        <!-- Workspace steps -->
        <nav class="mt-5 px-2.5" aria-label="Workspace">
            <p v-if="!collapsed" class="eyebrow mb-1.5 px-2">Workspace</p>
            <ol class="space-y-0.5">
                <li v-for="(step, index) in steps" :key="step.id">
                    <button
                        type="button"
                        data-step
                        :data-testid="`rail-step-${step.id}`"
                        :disabled="step.disabled"
                        :aria-current="
                            !onSettings && state.activeStep === step.id ? 'page' : undefined
                        "
                        :title="
                            collapsed
                                ? step.label +
                                  (step.disabledReason ? ` — ${step.disabledReason}` : '')
                                : step.disabledReason || undefined
                        "
                        class="group relative flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left transition-colors duration-150 disabled:cursor-not-allowed"
                        :class="[
                            collapsed ? 'justify-center' : '',
                            !onSettings && state.activeStep === step.id
                                ? 'bg-white/[0.07] text-fg'
                                : step.disabled
                                  ? 'text-fg-faint'
                                  : 'text-fg-muted hover:bg-white/[0.04] hover:text-fg',
                        ]"
                        @click="open(step.id)"
                        @keydown="onStepKeydown($event, index)"
                    >
                        <span
                            v-if="!onSettings && state.activeStep === step.id"
                            class="absolute top-1.5 bottom-1.5 left-0 w-[2px] rounded-full bg-accent"
                            aria-hidden="true"
                        />
                        <component
                            :is="icons[step.id]"
                            class="h-4 w-4 shrink-0"
                            aria-hidden="true"
                        />
                        <span
                            v-if="!collapsed"
                            class="min-w-0 flex-1 truncate text-[13px] font-medium"
                        >
                            {{ step.label }}
                        </span>
                        <span
                            v-if="!collapsed"
                            class="flex h-4 min-w-4 items-center justify-center rounded-full text-[10px] font-semibold"
                            :class="step.done ? 'bg-success/15 text-success' : 'text-fg-faint'"
                            aria-hidden="true"
                        >
                            <Check v-if="step.done" class="h-2.5 w-2.5" :stroke-width="3" />
                            <template v-else>{{ index + 1 }}</template>
                        </span>
                    </button>
                </li>
            </ol>
        </nav>

        <div class="flex-1" data-tauri-drag-region />

        <div class="space-y-0.5 border-t border-line px-2.5 py-2.5">
            <RouterLink
                to="/settings"
                data-testid="rail-settings"
                :aria-current="onSettings ? 'page' : undefined"
                :title="collapsed ? 'Settings' : undefined"
                class="relative flex items-center gap-2.5 rounded-md px-2 py-1.5 text-[13px] font-medium transition-colors"
                :class="[
                    collapsed ? 'justify-center' : '',
                    onSettings
                        ? 'bg-white/[0.07] text-fg'
                        : 'text-fg-muted hover:bg-white/[0.04] hover:text-fg',
                ]"
            >
                <span
                    v-if="onSettings"
                    class="absolute top-1.5 bottom-1.5 left-0 w-[2px] rounded-full bg-accent"
                    aria-hidden="true"
                />
                <SettingsIcon class="h-4 w-4 shrink-0" aria-hidden="true" />
                <span v-if="!collapsed">Settings</span>
            </RouterLink>
            <button
                type="button"
                class="flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-[13px] text-fg-subtle transition-colors hover:bg-white/[0.04] hover:text-fg"
                :class="collapsed ? 'justify-center' : ''"
                :aria-label="collapsed ? 'Expand sidebar' : 'Collapse sidebar'"
                :title="collapsed ? 'Expand sidebar' : undefined"
                @click="collapsed = !collapsed"
            >
                <PanelLeftOpen v-if="collapsed" class="h-4 w-4" aria-hidden="true" />
                <PanelLeftClose v-else class="h-4 w-4" aria-hidden="true" />
                <span v-if="!collapsed">Collapse</span>
            </button>
        </div>
    </aside>
</template>
