import { describe, expect, it } from 'vitest';
import { createDefaultLastAnalyzedSettings } from '../editSession';
import { buildTranscriptSidecar, parseTranscriptSidecar } from '../transcriptSidecar';
import type { TranscriptWorkspaceState } from '../../types';

describe('transcriptSidecar', () => {
  it('parses legacy array-only transcript files', () => {
    const parsed = parseTranscriptSidecar(
      JSON.stringify([{ start: '00:00', end: '00:02', speaker: 'Host', text: 'Hello' }]),
      createDefaultLastAnalyzedSettings(),
    );

    expect(parsed).toEqual({
      segments: [{ start: '00:00', end: '00:02', speaker: 'Host', text: 'Hello' }],
    });
  });

  it('parses rich sidecar data and preserves last analyzed defaults', () => {
    const parsed = parseTranscriptSidecar(
      JSON.stringify({
        segments: [{ start: '00:00', end: '00:02', speaker: 'Host', text: 'Hello' }],
        context: 'ctx',
        glossary: 'AI',
        lastAnalyzedSettings: { context: 'ctx', glossary: 'AI' },
      }),
      createDefaultLastAnalyzedSettings(),
    );

    expect(parsed?.context).toBe('ctx');
    expect(parsed?.glossary).toBe('AI');
    expect(parsed?.lastAnalyzedSettings?.transcriptionBackend).toBe('llm');
  });

  it('builds a serializable sidecar shape from transcript workspace state', () => {
    const workspace: TranscriptWorkspaceState = {
      inputPath: '/tmp/audio.mp3',
      segments: [{ start: '00:00', end: '00:02', speaker: 'Host', text: 'Hello' }],
      translations: {},
      currentLanguage: 'Original',
      targetLanguage: '',
      context: 'ctx',
      speakerCount: 1,
      removeFillerWords: false,
      trimSilence: true,
      useAdvancedAlignment: false,
      speakerOrder: ['Host'],
      lastAnalyzedSettings: createDefaultLastAnalyzedSettings(),
      rawParakeetSegments: [],
      parakeetCacheKey: '',
      settingsSnapshot: {
        glossary: 'AI',
        transcriptionBackend: 'llm',
        parakeetModelPath: '',
        sortformerModelPath: '',
      },
    };

    const sidecar = buildTranscriptSidecar(workspace);

    expect(sidecar.glossary).toBe('AI');
    expect(sidecar.speakerOrder).toEqual(['Host']);
    expect(sidecar.segments).toHaveLength(1);
  });

  it('round-trips the cached raw Parakeet transcript and its cache key', () => {
    const rawParakeetSegments = [
      { start: '00:00', end: '00:02', speaker: 'Speaker 1', text: 'raw parakeet' },
    ];
    const parakeetCacheKey = JSON.stringify({
      inputPath: '/tmp/audio.mp3',
      trimSilence: true,
      parakeetModelPath: '',
      sortformerModelPath: '',
    });
    const workspace: TranscriptWorkspaceState = {
      inputPath: '/tmp/audio.mp3',
      segments: [{ start: '00:00', end: '00:02', speaker: 'Speaker 1', text: 'cleaned' }],
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
      rawParakeetSegments,
      parakeetCacheKey,
      settingsSnapshot: {
        glossary: '',
        transcriptionBackend: 'hybrid',
        parakeetModelPath: '',
        sortformerModelPath: '',
      },
    };

    const serialized = JSON.stringify(buildTranscriptSidecar(workspace));
    const parsed = parseTranscriptSidecar(serialized, createDefaultLastAnalyzedSettings());

    expect(parsed?.rawParakeetSegments).toEqual(rawParakeetSegments);
    expect(parsed?.parakeetCacheKey).toBe(parakeetCacheKey);
  });
});
