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
});
