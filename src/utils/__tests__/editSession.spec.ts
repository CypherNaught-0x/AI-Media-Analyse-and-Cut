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
});
