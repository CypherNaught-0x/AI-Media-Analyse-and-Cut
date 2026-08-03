import type {
  ClipWorkspaceState,
  EditSessionV1,
  LastAnalyzedSettings,
  LocalEngine,
  PodcastWorkspaceState,
  TranscriptWorkspaceState,
  ViralClipsWorkspaceState,
} from '../types';
import { migrateTranscriptionBackend } from '../types';

export const EDIT_SESSION_VERSION = 1 as const;
type SupportedEditSessionVersion = 0 | typeof EDIT_SESSION_VERSION;

export function createDefaultLastAnalyzedSettings(): LastAnalyzedSettings {
  return {
    context: '',
    glossary: '',
    speakerCount: null,
    removeFillerWords: false,
    trimSilence: true,
    transcriptionBackend: 'llm',
    localEngine: 'parakeet',
    parakeetModelPath: '',
    sortformerModelPath: '',
    crisperSignature: '',
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
    rawParakeetSegments: [],
    parakeetCacheKey: '',
    settingsSnapshot: {
      glossary: '',
      transcriptionBackend: 'llm',
      localEngine: 'parakeet',
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

const TRANSCRIPT_WORKSPACE_KEYS = [
  'inputPath',
  'segments',
  'translations',
  'currentLanguage',
  'targetLanguage',
  'context',
  'speakerCount',
  'removeFillerWords',
  'trimSilence',
  'useAdvancedAlignment',
  'speakerOrder',
  'lastAnalyzedSettings',
  'rawParakeetSegments',
  'parakeetCacheKey',
  'settingsSnapshot',
  'transcriptionBackend',
  'localEngine',
  'parakeetModelPath',
  'sortformerModelPath',
] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

function hasAnyKey(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return keys.some((key) => key in value);
}

/**
 * Resolve the pipeline and local engine from a stored workspace.
 *
 * Sessions written before the split stored the engine inside the pipeline
 * ('parakeet' / 'crisper'), and the value could live either at the top level or
 * inside `settingsSnapshot`.
 */
function migrateSnapshotBackend(
  raw: Record<string, unknown>,
  defaults: TranscriptWorkspaceState,
): Pick<TranscriptWorkspaceState['settingsSnapshot'], 'transcriptionBackend' | 'localEngine'> {
  const snapshot = isRecord(raw.settingsSnapshot) ? raw.settingsSnapshot : {};
  const storedBackend = raw.transcriptionBackend ?? snapshot.transcriptionBackend;
  const storedEngine = raw.localEngine ?? snapshot.localEngine;

  const migrated = migrateTranscriptionBackend(storedBackend);
  const localEngine =
    storedEngine === 'parakeet' || storedEngine === 'crisper'
      ? storedEngine
      : (migrated?.localEngine ?? defaults.settingsSnapshot.localEngine);

  return {
    transcriptionBackend: migrated?.backend ?? defaults.settingsSnapshot.transcriptionBackend,
    localEngine,
  };
}

function normalizeTranscriptWorkspace(
  candidate: unknown,
  defaults: TranscriptWorkspaceState,
): TranscriptWorkspaceState {
  if (!isRecord(candidate)) {
    return defaults;
  }

  const raw = candidate as Partial<TranscriptWorkspaceState> & {
    glossary?: string;
    transcriptionBackend?: LastAnalyzedSettings['transcriptionBackend'];
    localEngine?: LocalEngine;
    parakeetModelPath?: string;
    sortformerModelPath?: string;
  };

  return {
    ...defaults,
    ...raw,
    lastAnalyzedSettings: {
      ...defaults.lastAnalyzedSettings,
      ...(isRecord(raw.lastAnalyzedSettings) ? raw.lastAnalyzedSettings : {}),
    },
    settingsSnapshot: {
      ...defaults.settingsSnapshot,
      ...(isRecord(raw.settingsSnapshot) ? raw.settingsSnapshot : {}),
      glossary:
        typeof raw.glossary === 'string'
          ? raw.glossary
          : isRecord(raw.settingsSnapshot) && typeof raw.settingsSnapshot.glossary === 'string'
            ? raw.settingsSnapshot.glossary
            : defaults.settingsSnapshot.glossary,
      ...migrateSnapshotBackend(raw, defaults),
      parakeetModelPath:
        typeof raw.parakeetModelPath === 'string'
          ? raw.parakeetModelPath
          : isRecord(raw.settingsSnapshot) &&
              typeof raw.settingsSnapshot.parakeetModelPath === 'string'
            ? raw.settingsSnapshot.parakeetModelPath
            : defaults.settingsSnapshot.parakeetModelPath,
      sortformerModelPath:
        typeof raw.sortformerModelPath === 'string'
          ? raw.sortformerModelPath
          : isRecord(raw.settingsSnapshot) &&
              typeof raw.settingsSnapshot.sortformerModelPath === 'string'
            ? raw.settingsSnapshot.sortformerModelPath
            : defaults.settingsSnapshot.sortformerModelPath,
    },
  };
}

function inferLegacySession(candidate: Record<string, unknown>): Partial<EditSessionV1> | null {
  if ('transcriptWorkspace' in candidate) {
    return candidate as Partial<EditSessionV1>;
  }

  if (hasAnyKey(candidate, TRANSCRIPT_WORKSPACE_KEYS)) {
    return {
      transcriptWorkspace: candidate as unknown as TranscriptWorkspaceState,
    };
  }

  return null;
}

function inferEditSessionVersion(candidate: Record<string, unknown>): SupportedEditSessionVersion | null {
  if (typeof candidate.version === 'number' && Number.isInteger(candidate.version)) {
    return candidate.version === EDIT_SESSION_VERSION ? EDIT_SESSION_VERSION : null;
  }

  return inferLegacySession(candidate) ? 0 : null;
}

function migrateEditSessionV0ToV1(candidate: Record<string, unknown>): Partial<EditSessionV1> | null {
  const legacy = inferLegacySession(candidate);
  if (!legacy) {
    return null;
  }

  return {
    version: EDIT_SESSION_VERSION,
    savedAt: typeof candidate.savedAt === 'string' ? candidate.savedAt : new Date().toISOString(),
    transcriptWorkspace: legacy.transcriptWorkspace,
    clipWorkspace: legacy.clipWorkspace,
    viralClipsWorkspace: legacy.viralClipsWorkspace,
    podcastWorkspace: legacy.podcastWorkspace,
  };
}

function migrateEditSession(candidate: Record<string, unknown>): Partial<EditSessionV1> | null {
  let current: Record<string, unknown> = candidate;
  let version = inferEditSessionVersion(current);

  if (version === null) {
    return null;
  }

  while (version < EDIT_SESSION_VERSION) {
    switch (version) {
      case 0: {
        const migrated = migrateEditSessionV0ToV1(current);
        if (!migrated || !isRecord(migrated)) {
          return null;
        }
        current = migrated;
        version = inferEditSessionVersion(current);
        break;
      }
      default:
        return null;
    }

    if (version === null) {
      return null;
    }
  }

  return current as Partial<EditSessionV1>;
}

export function normalizeEditSession(candidate: unknown): EditSessionV1 | null {
  if (!isRecord(candidate)) {
    return null;
  }

  const raw = migrateEditSession(candidate);
  if (!raw) {
    return null;
  }

  const defaults = createDefaultEditSession();
  return {
    version: EDIT_SESSION_VERSION,
    savedAt: typeof raw.savedAt === 'string' ? raw.savedAt : new Date().toISOString(),
    transcriptWorkspace: normalizeTranscriptWorkspace(
      raw.transcriptWorkspace,
      defaults.transcriptWorkspace,
    ),
    clipWorkspace: {
      ...defaults.clipWorkspace,
      ...(isRecord(raw.clipWorkspace) ? raw.clipWorkspace : {}),
    },
    viralClipsWorkspace: {
      ...defaults.viralClipsWorkspace,
      ...(isRecord(raw.viralClipsWorkspace) ? raw.viralClipsWorkspace : {}),
    },
    podcastWorkspace: {
      ...defaults.podcastWorkspace,
      ...(isRecord(raw.podcastWorkspace) ? raw.podcastWorkspace : {}),
    },
  };
}
