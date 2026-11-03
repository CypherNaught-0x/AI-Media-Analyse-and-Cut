// Screenshot demo mode: mocks the Tauri IPC layer and seeds demo state so the
// app renders meaningful content in a plain browser. Only ever loaded when
// VITE_SCREENSHOT_DEMO is set (see main.ts) — never part of a production build.
import { mockConvertFileSrc, mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { createDefaultEditSession, createDefaultLastAnalyzedSettings } from '../utils/editSession';
import { SESSION_STORAGE_KEY } from '../composables/useHomeSessionPersistence';
import { SETUP_WIZARD_VERSION } from '../utils/setupWizardVersion';
import type { ClipCandidate, TranscriptSegment } from '../types';

const DEMO_MEDIA_PATH = '/Users/demo/Videos/ai-deep-dive.mp4';

const DEMO_SEGMENTS: TranscriptSegment[] = [
    [
        '00:00',
        '00:04',
        'Speaker 1',
        "Okay, picture this. You've got an incredibly brilliant piece of software, right?",
    ],
    [
        '00:04',
        '00:09',
        'Speaker 1',
        "A large language model that's, you know, absorbed petabytes of text.",
    ],
    [
        '00:09',
        '00:13',
        'Speaker 1',
        "It can write code, generate creative text, answer really complex questions. It's amazing.",
    ],
    ['00:13', '00:15', 'Speaker 2', 'Yeah, they really are powerful.'],
    [
        '00:15',
        '00:21',
        'Speaker 2',
        'But the moment you point one at raw video, things get interesting fast.',
    ],
    [
        '00:21',
        '00:27',
        'Speaker 1',
        'Exactly. Hours of footage, filler words everywhere, and somewhere in there is the good stuff.',
    ],
    [
        '00:27',
        '00:33',
        'Speaker 2',
        'So the idea is simple: transcribe everything first, then edit the video by editing the text.',
    ],
    [
        '00:33',
        '00:39',
        'Speaker 1',
        'Delete a sentence from the transcript and the cut happens automatically. No timeline scrubbing.',
    ],
    [
        '00:39',
        '00:45',
        'Speaker 2',
        'And the same transcript powers subtitles, translations, even short viral clips.',
    ],
    ['00:45', '00:50', 'Speaker 1', 'Which means the boring part of editing basically disappears.'],
    [
        '00:50',
        '00:56',
        'Speaker 2',
        'Right. You focus on the story, and the tooling handles the scissors.',
    ],
    ['00:56', '01:01', 'Speaker 1', "Let's walk through how that works under the hood."],
].map(([start, end, speaker, text]) => ({ start, end, speaker, text }));

const DEMO_CLIPS: ClipCandidate[] = [
    {
        title: 'Edit video by editing text',
        hookLine: 'Delete a sentence from the transcript and the cut happens automatically.',
        reason: 'One clear, surprising idea with a concrete payoff.',
        ranges: [{ start: 27, end: 50, role: 'body', firstSegment: 6, lastSegment: 9 }],
        ratings: { hook: 9, standalone: 8, emotion: 6, info: 8, loopContinuity: 0 },
        signals: { speakerChanges: 3, laughs: 0, wordsPerSecond: 2.6 },
        score: 86.5,
        duration: 23,
        looped: false,
    },
    {
        title: 'The boring part disappears',
        hookLine: 'Which means the boring part of editing basically disappears.',
        reason: 'Opens on the conclusion; the last line leads straight back into it.',
        ranges: [
            { start: 45, end: 50, role: 'loop_opener', firstSegment: 9, lastSegment: 9 },
            { start: 15, end: 27, role: 'body', firstSegment: 4, lastSegment: 5 },
            { start: 39, end: 45, role: 'closing', firstSegment: 8, lastSegment: 8 },
        ],
        ratings: { hook: 8, standalone: 7, emotion: 6, info: 7, loopContinuity: 9 },
        signals: { speakerChanges: 3, laughs: 0, wordsPerSecond: 2.4 },
        score: 80.2,
        duration: 23,
        looped: true,
    },
];

type Scenario = 'home' | 'transcript' | 'setup';

function currentScenario(): Scenario {
    const demo = new URLSearchParams(window.location.search).get('demo');
    return demo === 'transcript' || demo === 'setup' ? demo : 'home';
}

/** A 36 GB M3 Pro: Nemotron fits the GPU with the recommended build. */
const DEMO_NEMOTRON_STATUS = {
    platformSupported: true,
    runtimeInstalled: false,
    runtimeDownloadBytes: 7_239_335,
    encoderInstalled: false,
    variants: [
        ['Q8_0', 'Q8_0 (closest to the original)', 36_675_053_312, 35.7],
        ['Q5_0_XL', 'Q5_0_XL', 30_927_808_512, 30.3],
        ['IQ4_NL_L', 'IQ4_NL_L', 24_222_689_280, 24.1],
        ['IQ4_NL', 'IQ4_NL (recommended)', 21_349_066_752, 21.4],
        ['Q2_0_L', 'Q2_0_L', 17_218_234_368, 17.5],
        ['Q2_0_M', 'Q2_0_M (for 16 GB GPUs)', 15_242_618_880, 15.7],
    ].map(([variant, label, downloadBytes, requiredMemoryGb]) => ({
        variant,
        label,
        downloadBytes,
        requiredMemoryGb,
        installed: false,
    })),
    recommendation: {
        rating: 'good',
        variant: 'IQ4_NL',
        device: 'metal',
        message: 'Runs on the GPU with the recommended IQ4_NL build.',
    },
    device: {
        os: 'macos',
        arch: 'aarch64',
        cpu: 'Apple M3 Pro',
        cpuCores: 12,
        totalMemoryGb: 36,
        appleSilicon: true,
        gpus: [{ name: 'Apple M3 Pro GPU', vendor: 'apple', memoryGb: null }],
        freeDiskGb: 75,
    },
    installing: false,
};

function seedStorage(scenario: Scenario) {
    localStorage.clear();
    // Seed a hybrid pipeline so the preview shows both selection rows: the
    // pipeline and the local engine it runs on top of.
    localStorage.setItem(
        'llm-settings',
        JSON.stringify({
            model: 'gemini-2.5-flash',
            transcriptionBackend: 'hybrid',
            localEngine: 'parakeet',
            // The README screenshots show the workspace, not the first-run wizard.
            setupWizardVersion: scenario === 'setup' ? 0 : SETUP_WIZARD_VERSION,
        }),
    );

    if (scenario === 'transcript') {
        const session = createDefaultEditSession();
        session.savedAt = new Date().toISOString();
        session.transcriptWorkspace.inputPath = DEMO_MEDIA_PATH;
        session.transcriptWorkspace.segments = DEMO_SEGMENTS;
        session.transcriptWorkspace.speakerOrder = ['Speaker 1', 'Speaker 2'];
        session.transcriptWorkspace.lastAnalyzedSettings = createDefaultLastAnalyzedSettings();
        localStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(session));
    }
}

export function setupDemoMode() {
    const scenario = currentScenario();
    seedStorage(scenario);
    mockWindows('main');
    mockConvertFileSrc('macos');
    // Serve the placeholder media generated by scripts/generate-screenshots.mjs
    // instead of the real asset protocol, which cannot resolve in a plain
    // browser and leaves media elements stuck on a loading spinner.
    (
        window as unknown as { __TAURI_INTERNALS__: { convertFileSrc: (path: string) => string } }
    ).__TAURI_INTERNALS__.convertFileSrc = (path: string) =>
        path.endsWith('.m4a') ? '/__demo-media__.m4a' : '/__demo-media__.mp4';

    const statusMessage =
        scenario === 'transcript'
            ? `Analysis complete. Found ${DEMO_SEGMENTS.length} segments (2 removed).`
            : 'FFmpeg is already installed.';

    mockIPC((cmd, args) => {
        switch (cmd) {
            case 'init_ffmpeg':
                return statusMessage;
            case 'path_exists':
                return true;
            case 'begin_run':
                return 1;
            case 'select_clips':
                return DEMO_CLIPS;
            case 'plan_vertical_clips': {
                // One followed framing per requested clip, slightly right of centre.
                const { request } = args as { request: { segments: unknown[] } };
                const clips = request.segments.map(() => [
                    {
                        start: 0,
                        end: 1e9,
                        fit: false,
                        keys: [{ time: 0, centerX: 760, centerY: 330, height: 520 }],
                        zoom: 1,
                    },
                ]);
                return {
                    sourceWidth: 1280,
                    sourceHeight: 720,
                    outputWidth: 1080,
                    outputHeight: 1920,
                    clips,
                    captions: clips.map(() => [
                        [
                            { start: 0, end: 0.5, text: 'Edit' },
                            { start: 0.5, end: 1.0, text: 'video' },
                        ],
                    ]),
                };
            }
            case 'detect_vertical_faces': {
                // Two speakers in a wide shot and a face on a screen behind
                // them, as plain silhouettes.
                const silhouette = (colour: string) =>
                    'data:image/svg+xml,' +
                    encodeURIComponent(
                        `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96">` +
                            `<rect width="96" height="96" fill="#1f2937"/>` +
                            `<circle cx="48" cy="40" r="20" fill="${colour}"/>` +
                            `<ellipse cx="48" cy="96" rx="34" ry="28" fill="${colour}"/></svg>`,
                    );
                const face = (x: number, colour: string, speaker: string | null, views = 1) => ({
                    anchors: [{ time: 30, x, y: 0.4 }],
                    thumbnail: silhouette(colour),
                    speaker,
                    confident: speaker !== null,
                    views,
                    seconds: 23,
                    applied: null,
                });
                return [
                    face(0.25, '#f472b6', 'Speaker 1', 2),
                    face(0.5, '#94a3b8', null),
                    face(0.75, '#60a5fa', 'Speaker 2'),
                ];
            }
            case 'suggest_clip_cuts': {
                // One aside per clip, a few words after its hook.
                const { clips } = args as { clips: { words: unknown[] }[] };
                return clips.map((clip) =>
                    clip.words.length > 12
                        ? [{ from: 8, to: 10, reason: 'Aside that repeats the point' }]
                        : [],
                );
            }
            case 'export_vertical_clips':
                return null;
            case 'tighten_clips':
                // No audio to measure in demo mode: clips stay as they are.
                return (args as { segments: unknown[] }).segments;
            case 'has_api_key':
                return true;
            case 'has_openrouter_api_key':
                return false;
            case 'nemotron_status':
                return DEMO_NEMOTRON_STATUS;
            case 'apple_speech_status':
                return {
                    supportedPlatform: true,
                    available: true,
                    supportedLocales: ['en-US', 'de-DE'],
                    installedLocales: ['en-US'],
                    message: null,
                };
            case 'list_transcription_models':
                return (args as { baseUrl: string }).baseUrl.includes('eu.')
                    ? [
                          {
                              id: 'mistralai/voxtral-small-transcribe-3-0',
                              name: 'Mistral: Voxtral Small Transcribe 3.0',
                              usdPerMinute: 0.0044,
                          },
                          {
                              id: 'mistralai/voxtral-mini-transcribe-3-0',
                              name: 'Mistral: Voxtral Mini Transcribe 3.0',
                              usdPerMinute: 0.00385,
                          },
                      ]
                    : [
                          {
                              id: 'nvidia/nemotron-3.5-transcribe',
                              name: 'NVIDIA: Nemotron 3.5 Transcribe',
                              usdPerMinute: 0.001998,
                          },
                          {
                              id: 'mistralai/voxtral-small-transcribe-3-0',
                              name: 'Mistral: Voxtral Small Transcribe 3.0',
                              usdPerMinute: 0.004,
                          },
                      ];
            case 'list_models':
                return { supported: true, models: ['gemini-2.5-flash', 'gemini-2.5-pro'] };
            case 'cached_preview_audio':
                // Any path: convertFileSrc maps .m4a to the demo audio.
                return '/demo/cache/analysis_preview.m4a';
            case 'write_text_file':
            case 'allow_media_access':
                return null;
            case 'read_text_file':
                // No transcript sidecar in demo mode; the session seed supplies segments.
                return Promise.reject('demo: no sidecar');
            case 'plugin:event|listen':
                return 1;
            case 'plugin:event|unlisten':
                return null;
            case 'plugin:updater|check':
                return null;
            default:
                console.warn('[screenshot-demo] unmocked command:', cmd, args);
                return null;
        }
    });
}
