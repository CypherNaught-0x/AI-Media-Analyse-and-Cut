import type {
  ClipWorkspaceState,
  EditSessionV1,
  LastAnalyzedSettings,
  PodcastWorkspaceState,
  TranscriptWorkspaceState,
  ViralClipsWorkspaceState,
} from '../types';

export const EDIT_SESSION_VERSION = 1 as const;

export function createDefaultLastAnalyzedSettings(): LastAnalyzedSettings {
  return {
    context: '',
    glossary: '',
    speakerCount: null,
    removeFillerWords: false,
    trimSilence: true,
    transcriptionBackend: 'llm',
    parakeetModelPath: '',
    sortformerModelPath: '',
  };
}

export function createDefaultClipWorkspaceState(): ClipWorkspaceState {
  return {
    count: 3,
    minDuration: 10,
    maxDuration: 120,
    topic: '',
    allowSplicing: false,
    clips: [],
    lastExportPath: '',
    includeSubtitles: true,
    fastMode: true,
    trimBoundarySilence: false,
    selectedClipIndices: [],
  };
}

export function createDefaultViralClipsWorkspaceState(): ViralClipsWorkspaceState {
  return {
    count: 3,
    minDuration: 10,
    maxDuration: 120,
    topic: '',
    allowSplicing: false,
    clips: [],
    lastExportPath: '',
    trimBoundarySilence: false,
  };
}

export function createDefaultPodcastWorkspaceState(): PodcastWorkspaceState {
  return {
    minDurationMinutes: 10,
    maxDurationMinutes: 15,
    startPadding: 0.5,
    endPadding: 0.5,
    introPath: '',
    outroPath: '',
    podcastScript: null,
    lastExportPath: '',
  };
}

export function createDefaultTranscriptWorkspaceState(): TranscriptWorkspaceState {
  return {
    inputPath: '',
    segments: [],
    translations: {},
    currentLanguage: 'Original',
    targetLanguage: '',
    context: '',
    speakerCount: null,
    removeFillerWords: false,
    trimSilence: true,
    useAdvancedAlignment: false,
    speakerOrder: [],
    lastAnalyzedSettings: createDefaultLastAnalyzedSettings(),
    settingsSnapshot: {
      glossary: '',
      transcriptionBackend: 'llm',
      parakeetModelPath: '',
      sortformerModelPath: '',
    },
  };
}

export function createDefaultEditSession(): EditSessionV1 {
  return {
    version: EDIT_SESSION_VERSION,
    savedAt: new Date(0).toISOString(),
    transcriptWorkspace: createDefaultTranscriptWorkspaceState(),
    clipWorkspace: createDefaultClipWorkspaceState(),
    viralClipsWorkspace: createDefaultViralClipsWorkspaceState(),
    podcastWorkspace: createDefaultPodcastWorkspaceState(),
  };
}

export function normalizeEditSession(candidate: unknown): EditSessionV1 | null {
  if (!candidate || typeof candidate !== 'object') {
    return null;
  }

  const raw = candidate as Partial<EditSessionV1>;
  if (raw.version !== EDIT_SESSION_VERSION) {
    return null;
  }

  const defaults = createDefaultEditSession();
  return {
    version: EDIT_SESSION_VERSION,
    savedAt: typeof raw.savedAt === 'string' ? raw.savedAt : new Date().toISOString(),
    transcriptWorkspace: {
      ...defaults.transcriptWorkspace,
      ...(raw.transcriptWorkspace ?? {}),
      lastAnalyzedSettings: {
        ...defaults.transcriptWorkspace.lastAnalyzedSettings,
        ...(raw.transcriptWorkspace?.lastAnalyzedSettings ?? {}),
      },
      settingsSnapshot: {
        ...defaults.transcriptWorkspace.settingsSnapshot,
        ...(raw.transcriptWorkspace?.settingsSnapshot ?? {}),
      },
    },
    clipWorkspace: {
      ...defaults.clipWorkspace,
      ...(raw.clipWorkspace ?? {}),
    },
    viralClipsWorkspace: {
      ...defaults.viralClipsWorkspace,
      ...(raw.viralClipsWorkspace ?? {}),
    },
    podcastWorkspace: {
      ...defaults.podcastWorkspace,
      ...(raw.podcastWorkspace ?? {}),
    },
  };
}
