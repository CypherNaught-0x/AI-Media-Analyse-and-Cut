import { computed, ref, type ComputedRef, type Ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import { normalizeEditSession } from '../utils/editSession';
import type {
  ClipWorkspaceState,
  EditSessionV1,
  PodcastWorkspaceState,
  TranscriptWorkspaceState,
  ViralClipsWorkspaceState,
} from '../types';

export const SESSION_STORAGE_KEY = 'home-edit-session-v1';
const SESSION_FILE_EXTENSION = 'aimc-session.json';

interface UseHomeSessionPersistenceOptions {
  autosaveDebounceMs: number;
  status: Ref<string>;
  inputPath: Ref<string>;
  inputPathExists: Ref<boolean>;
  transcriptWorkspaceState: ComputedRef<TranscriptWorkspaceState>;
  clipWorkspaceState: ComputedRef<ClipWorkspaceState>;
  viralClipsState: Ref<ViralClipsWorkspaceState>;
  podcastWorkspaceState: Ref<PodcastWorkspaceState>;
  updateInputPathExists: (path: string) => Promise<boolean>;
  saveTranscript: () => Promise<void>;
  loadTranscript: () => Promise<void>;
  resetTranscriptWorkspaceState: () => void;
  resetDerivedWorkspaceState: () => void;
  applyTranscriptWorkspace: (state: TranscriptWorkspaceState) => void;
  applyClipWorkspace: (state: ClipWorkspaceState) => void;
}

export function useHomeSessionPersistence(options: UseHomeSessionPersistenceOptions) {
  const isApplyingSession = ref(false);
  const isSwitchingInput = ref(false);
  let autosaveTimer: number | null = null;
  let transcriptSaveTimer: number | null = null;

  const buildSessionSnapshot = computed<EditSessionV1>(() => ({
    version: 1,
    savedAt: new Date().toISOString(),
    transcriptWorkspace: options.transcriptWorkspaceState.value,
    clipWorkspace: options.clipWorkspaceState.value,
    viralClipsWorkspace: options.viralClipsState.value,
    podcastWorkspace: options.podcastWorkspaceState.value,
  }));

  function clearAutosaveTimer() {
    if (autosaveTimer !== null) {
      clearTimeout(autosaveTimer);
      autosaveTimer = null;
    }
  }

  function clearTranscriptSaveTimer() {
    if (transcriptSaveTimer !== null) {
      clearTimeout(transcriptSaveTimer);
      transcriptSaveTimer = null;
    }
  }

  function scheduleAutosave() {
    if (isApplyingSession.value || isSwitchingInput.value) return;
    clearAutosaveTimer();
    autosaveTimer = window.setTimeout(() => {
      try {
        localStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(buildSessionSnapshot.value));
      } catch (error) {
        console.error('Failed to autosave session:', error);
      }
    }, options.autosaveDebounceMs);
  }

  function scheduleTranscriptSave() {
    if (isApplyingSession.value || isSwitchingInput.value || !options.inputPath.value) return;
    clearTranscriptSaveTimer();
    transcriptSaveTimer = window.setTimeout(async () => {
      await options.saveTranscript();
    }, options.autosaveDebounceMs);
  }

  async function applySessionSnapshot(session: EditSessionV1, reason: 'autosave' | 'manual-load') {
    isApplyingSession.value = true;
    clearAutosaveTimer();
    clearTranscriptSaveTimer();

    try {
      const mediaExists = await options.updateInputPathExists(session.transcriptWorkspace.inputPath);
      options.applyTranscriptWorkspace(session.transcriptWorkspace);
      options.applyClipWorkspace(session.clipWorkspace);
      options.viralClipsState.value = session.viralClipsWorkspace;
      options.podcastWorkspaceState.value = session.podcastWorkspace;

      if (session.transcriptWorkspace.inputPath && !mediaExists) {
        options.status.value = `Restored ${reason === 'autosave' ? 'autosaved' : 'saved'} session, but the media file is missing: ${session.transcriptWorkspace.inputPath}`;
      } else if (reason === 'autosave') {
        options.status.value = 'Restored previous session.';
      } else {
        options.status.value = 'Session loaded.';
      }
    } finally {
      isApplyingSession.value = false;
    }

    scheduleAutosave();
    scheduleTranscriptSave();
  }

  async function restoreAutosavedSession() {
    const storedSession = localStorage.getItem(SESSION_STORAGE_KEY);
    if (!storedSession) return;

    try {
      const parsed = normalizeEditSession(JSON.parse(storedSession));
      if (parsed) {
        await applySessionSnapshot(parsed, 'autosave');
      }
    } catch (error) {
      console.error('Failed to restore autosaved session:', error);
    }
  }

  async function handleInputPathChange(newPath: string, oldPath: string) {
    if (isApplyingSession.value) return;

    await options.updateInputPathExists(newPath);
    if (newPath === oldPath) return;

    isSwitchingInput.value = true;
    try {
      options.resetTranscriptWorkspaceState();
      options.resetDerivedWorkspaceState();
      if (newPath) {
        await options.loadTranscript();
      }
    } finally {
      isSwitchingInput.value = false;
    }

    scheduleAutosave();
    scheduleTranscriptSave();
  }

  async function saveSessionToFile() {
    const defaultPath = options.inputPath.value
      ? `${options.inputPath.value}.${SESSION_FILE_EXTENSION}`
      : `session.${SESSION_FILE_EXTENSION}`;
    const selectedPath = await save({
      defaultPath,
      filters: [{
        name: 'Session JSON',
        extensions: ['json'],
      }],
    });

    if (!selectedPath) return;

    await invoke('write_text_file', {
      path: selectedPath,
      content: JSON.stringify(buildSessionSnapshot.value, null, 2),
    });
    options.status.value = `Session saved to ${selectedPath}`;
  }

  async function loadSessionFromFile() {
    const selectedPath = await open({
      directory: false,
      multiple: false,
      filters: [{
        name: 'Session JSON',
        extensions: ['json'],
      }],
      title: 'Load Session',
    });

    if (typeof selectedPath !== 'string') return;

    const content = await invoke<string>('read_text_file', { path: selectedPath });
    const parsed = normalizeEditSession(JSON.parse(content));
    if (!parsed) {
      throw new Error('Unsupported or invalid session file.');
    }

    await applySessionSnapshot(parsed, 'manual-load');
  }

  async function handleSaveSession() {
    try {
      await saveSessionToFile();
    } catch (error) {
      console.error('Failed to save session:', error);
      options.status.value = `Failed to save session: ${error}`;
    }
  }

  async function handleLoadSession() {
    try {
      await loadSessionFromFile();
    } catch (error) {
      console.error('Failed to load session:', error);
      options.status.value = `Failed to load session: ${error}`;
    }
  }

  function dispose() {
    clearAutosaveTimer();
    clearTranscriptSaveTimer();
  }

  return {
    buildSessionSnapshot,
    isApplyingSession,
    scheduleAutosave,
    scheduleTranscriptSave,
    applySessionSnapshot,
    restoreAutosavedSession,
    handleInputPathChange,
    handleSaveSession,
    handleLoadSession,
    dispose,
  };
}
