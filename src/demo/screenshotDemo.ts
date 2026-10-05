// Screenshot demo mode: mocks the Tauri IPC layer and seeds demo state so the
// app renders meaningful content in a plain browser. Only ever loaded when
// VITE_SCREENSHOT_DEMO is set (see main.ts) — never part of a production build.
import { mockConvertFileSrc, mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { createDefaultEditSession, createDefaultLastAnalyzedSettings } from '../utils/editSession';
import { SESSION_STORAGE_KEY } from '../composables/useHomeSessionPersistence';
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

type Scenario = 'home' | 'transcript';

function currentScenario(): Scenario {
    const demo = new URLSearchParams(window.location.search).get('demo');
    return demo === 'transcript' ? 'transcript' : 'home';
}

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
            case 'has_api_key':
                return true;
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
