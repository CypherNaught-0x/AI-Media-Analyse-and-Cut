import { computed, reactive } from 'vue';

export type WorkspaceStep = 'source' | 'transcript' | 'clips' | 'podcast';

export interface WorkspaceStepInfo {
    id: WorkspaceStep;
    label: string;
    description: string;
    disabled: boolean;
    /** Why the step can't be opened yet, for tooltips. */
    disabledReason: string;
    done: boolean;
}

// One workspace per app window: Home owns the project and writes here; the
// shell rail reads it to draw navigation and the project summary.
const state = reactive({
    activeStep: 'source' as WorkspaceStep,
    hasTranscript: false,
    mediaPath: '',
    hasMediaFile: false,
    busy: false,
});

const steps = computed<WorkspaceStepInfo[]>(() => {
    const locked = !state.hasTranscript;
    const reason = locked ? 'Analyze a media file first' : '';
    return [
        {
            id: 'source',
            label: 'Source',
            description: 'Media & analysis',
            disabled: false,
            disabledReason: '',
            done: state.hasTranscript,
        },
        {
            id: 'transcript',
            label: 'Transcript',
            description: 'Edit, translate, export',
            disabled: locked,
            disabledReason: reason,
            done: false,
        },
        {
            id: 'clips',
            label: 'Viral Clips',
            description: 'Short-form highlights',
            disabled: locked,
            disabledReason: reason,
            done: false,
        },
        {
            id: 'podcast',
            label: 'Podcast',
            description: 'Condensed episode',
            disabled: locked,
            disabledReason: reason,
            done: false,
        },
    ];
});

const mediaName = computed(() => state.mediaPath.split(/[\\/]/).pop() ?? '');

function selectStep(step: WorkspaceStep): boolean {
    const info = steps.value.find((s) => s.id === step);
    if (!info || info.disabled) return false;
    state.activeStep = step;
    return true;
}

/** Back to a fresh workspace (tests; a new window). */
export function resetWorkspaceNav() {
    state.activeStep = 'source';
    state.hasTranscript = false;
    state.mediaPath = '';
    state.hasMediaFile = false;
    state.busy = false;
}

export function useWorkspaceNav() {
    return { state, steps, mediaName, selectStep };
}
