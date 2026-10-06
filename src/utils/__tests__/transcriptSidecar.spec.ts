import { describe, expect, it } from 'vitest';
import { createDefaultEditSession, createDefaultLastAnalyzedSettings } from '../editSession';
import { fromCandidates } from '../shortClips';
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
            speakerOrder: ['Host'],
            lastAnalyzedSettings: createDefaultLastAnalyzedSettings(),
            rawParakeetSegments: [],
            parakeetCacheKey: '',
            settingsSnapshot: {
                glossary: 'AI',
                transcriptionBackend: 'llm',
                localEngine: 'parakeet',
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
            speakerOrder: [],
            lastAnalyzedSettings: createDefaultLastAnalyzedSettings(),
            rawParakeetSegments,
            parakeetCacheKey,
            settingsSnapshot: {
                glossary: '',
                transcriptionBackend: 'hybrid',
                localEngine: 'parakeet',
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

describe('saved viral clips', () => {
    it('round-trip with the recording and are left out when empty', () => {
        const workspace = createDefaultEditSession().transcriptWorkspace;
        expect(buildTranscriptSidecar(workspace)).not.toHaveProperty('viralClips');
        expect(
            buildTranscriptSidecar(workspace, { clips: [], faces: [], faceSearch: null }),
        ).not.toHaveProperty('viralClips');

        const [clip] = fromCandidates([
            {
                title: 'A clip',
                hookLine: 'Hook',
                reason: 'Reason',
                ranges: [{ start: 10, end: 30, role: 'body', firstSegment: 0, lastSegment: 1 }],
                ratings: { hook: 9, standalone: 8, emotion: 7, info: 6, loopContinuity: 0 },
                signals: { speakerChanges: 1, laughs: 0, wordsPerSecond: 2.5 },
                score: 80,
                duration: 20,
                looped: false,
            },
        ]);
        const saved = {
            clips: [{ ...clip, selected: false }],
            faces: [
                {
                    anchors: [{ time: 12, x: 0.4, y: 0.3 }],
                    ignored: true,
                    speaker: { kind: 'auto' as const },
                },
            ],
            faceSearch: {
                signature: '/m.mp4|10-30',
                faces: [
                    {
                        anchors: [{ time: 12, x: 0.4, y: 0.3 }],
                        thumbnail: 'data:image/png;base64,AA==',
                        speaker: null,
                        confident: false,
                        views: 2,
                        seconds: 20,
                        applied: 0,
                    },
                ],
            },
        };
        const parsed = parseTranscriptSidecar(
            JSON.stringify(buildTranscriptSidecar(workspace, saved)),
            createDefaultLastAnalyzedSettings(),
        );
        expect(parsed?.viralClips).toEqual(saved);
    });
});
