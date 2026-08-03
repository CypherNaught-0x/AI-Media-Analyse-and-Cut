import { describe, expect, it } from 'vitest';
import {
  createDefaultClipWorkspaceState,
  createDefaultEditSession,
  normalizeEditSession,
} from '../editSession';

describe('editSession', () => {
  it('creates a default session with version 1', () => {
    const session = createDefaultEditSession();

    expect(session.version).toBe(1);
    expect(session.clipWorkspace).toEqual(createDefaultClipWorkspaceState());
  });

  it('normalizes partial session payloads with defaults', () => {
    const normalized = normalizeEditSession({
      version: 1,
      savedAt: '2026-04-07T10:00:00.000Z',
      transcriptWorkspace: {
        inputPath: '/tmp/audio.mp3',
      },
    });

    expect(normalized).not.toBeNull();
    expect(normalized?.transcriptWorkspace.inputPath).toBe('/tmp/audio.mp3');
    expect(normalized?.clipWorkspace.count).toBe(3);
    expect(normalized?.podcastWorkspace.minDurationMinutes).toBe(10);
  });

  it('rejects unsupported session versions', () => {
    expect(normalizeEditSession({ version: 2 })).toBeNull();
  });

  it('accepts legacy unversioned session payloads', () => {
    const normalized = normalizeEditSession({
      savedAt: '2026-04-07T10:00:00.000Z',
      transcriptWorkspace: {
        inputPath: '/tmp/legacy.mp4',
      },
      clipWorkspace: {
        count: 5,
      },
    });

    expect(normalized).not.toBeNull();
    expect(normalized?.version).toBe(1);
    expect(normalized?.transcriptWorkspace.inputPath).toBe('/tmp/legacy.mp4');
    expect(normalized?.clipWorkspace.count).toBe(5);
    expect(normalized?.clipWorkspace.includeSubtitles).toBe(true);
    expect(normalized?.podcastWorkspace.minDurationMinutes).toBe(10);
  });

  it('wraps transcript-workspace-only payloads as legacy sessions', () => {
    const normalized = normalizeEditSession({
      inputPath: '/tmp/audio.mp3',
      segments: [{ start: '00:00', end: '00:02', speaker: 'Host', text: 'Hello' }],
      glossary: 'AI',
      transcriptionBackend: 'hybrid-merge',
    });

    expect(normalized).not.toBeNull();
    expect(normalized?.transcriptWorkspace.inputPath).toBe('/tmp/audio.mp3');
    expect(normalized?.transcriptWorkspace.segments).toHaveLength(1);
    expect(normalized?.transcriptWorkspace.settingsSnapshot.glossary).toBe('AI');
    expect(normalized?.transcriptWorkspace.settingsSnapshot.transcriptionBackend).toBe('hybrid-merge');
    expect(normalized?.clipWorkspace.includeSubtitles).toBe(true);
    expect(normalized?.podcastWorkspace.minDurationMinutes).toBe(10);
  });

  it('migrates legacy engine-as-pipeline values into the split representation', () => {
    // 'parakeet' and 'crisper' used to be pipelines in their own right.
    const parakeet = normalizeEditSession({
      inputPath: '/tmp/a.mp3',
      transcriptionBackend: 'parakeet',
    });
    expect(parakeet?.transcriptWorkspace.settingsSnapshot.transcriptionBackend).toBe('local');
    expect(parakeet?.transcriptWorkspace.settingsSnapshot.localEngine).toBe('parakeet');

    const crisper = normalizeEditSession({
      inputPath: '/tmp/b.mp3',
      transcriptionBackend: 'crisper',
    });
    expect(crisper?.transcriptWorkspace.settingsSnapshot.transcriptionBackend).toBe('local');
    expect(crisper?.transcriptWorkspace.settingsSnapshot.localEngine).toBe('crisper');

    // Hybrids implied Parakeet before the split; keep that behaviour.
    const hybrid = normalizeEditSession({
      inputPath: '/tmp/c.mp3',
      transcriptionBackend: 'hybrid',
    });
    expect(hybrid?.transcriptWorkspace.settingsSnapshot.transcriptionBackend).toBe('hybrid');
    expect(hybrid?.transcriptWorkspace.settingsSnapshot.localEngine).toBe('parakeet');
  });

  it('keeps an explicitly stored engine when both values are present', () => {
    const normalized = normalizeEditSession({
      inputPath: '/tmp/d.mp3',
      transcriptionBackend: 'hybrid-merge',
      localEngine: 'crisper',
    });

    expect(normalized?.transcriptWorkspace.settingsSnapshot.transcriptionBackend).toBe('hybrid-merge');
    expect(normalized?.transcriptWorkspace.settingsSnapshot.localEngine).toBe('crisper');
  });

  it('falls back to defaults for an unrecognised pipeline value', () => {
    const normalized = normalizeEditSession({
      inputPath: '/tmp/e.mp3',
      transcriptionBackend: 'nonsense',
    });

    expect(normalized?.transcriptWorkspace.settingsSnapshot.transcriptionBackend).toBe('llm');
    expect(normalized?.transcriptWorkspace.settingsSnapshot.localEngine).toBe('parakeet');
  });
});
