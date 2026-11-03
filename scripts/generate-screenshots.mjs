// Regenerates the README preview screenshots (dev-resources/preview_*.png)
// from the live UI. Starts a Vite dev server in screenshot-demo mode (mocked
// Tauri IPC + seeded demo data, see src/demo/screenshotDemo.ts) and captures
// the documented views with Playwright. Run via `just screenshots`.
import { execFileSync, spawn } from 'node:child_process';
import { rmSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';
import { chromium } from 'playwright';

const PORT = 5199;
const BASE_URL = `http://localhost:${PORT}`;
const VIEWPORT = { width: 1456, height: 900 };

const status = (page, text) => page.getByTestId('status-text').getByText(text);

const SHOTS = [
    {
        file: 'dev-resources/preview_setup.png',
        url: `${BASE_URL}/?demo=setup`,
        waitFor: (page) => page.getByRole('dialog').getByText('Quick setup'),
    },
    {
        file: 'dev-resources/preview_1.png',
        url: `${BASE_URL}/?demo=home`,
        // Tall enough for the whole Source page, analysis options included.
        viewport: { width: 1456, height: 1180 },
        waitFor: (page) => status(page, 'FFmpeg is already installed.'),
    },
    {
        file: 'dev-resources/preview_2.png',
        url: `${BASE_URL}/?demo=transcript`,
        before: (page) => page.getByText('Transcript', { exact: true }).first().click(),
        waitFor: (page) => page.getByRole('textbox', { name: 'Search transcript' }),
    },
    {
        file: 'dev-resources/preview_clips.png',
        url: `${BASE_URL}/?demo=transcript`,
        before: async (page) => {
            await page.getByText('Viral Clips', { exact: true }).click();
            await page.getByRole('switch', { name: 'Vertical 9:16' }).click();
            await page.getByRole('button', { name: 'Find clips' }).click();
        },
        waitFor: (page) => status(page, 'faces in'),
    },
];

async function waitForServer(url, timeoutMs = 30_000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
        try {
            const res = await fetch(url);
            if (res.ok) return;
        } catch {
            // server not up yet
        }
        await sleep(250);
    }
    throw new Error(`Dev server did not become ready at ${url}`);
}

const repoRoot = new URL('..', import.meta.url).pathname;
const demoMedia = [`${repoRoot}public/__demo-media__.mp4`, `${repoRoot}public/__demo-media__.m4a`];

// Placeholder media so the transcript view's players render a real, settled
// state instead of an endless loading spinner (the Tauri asset protocol can't
// resolve in a plain browser). Generated into public/ (served by Vite at /)
// and removed again in the finally block below.
function generateDemoMedia() {
    const common = ['-hide_banner', '-loglevel', 'error', '-y'];
    execFileSync('ffmpeg', [
        ...common,
        '-f',
        'lavfi',
        '-i',
        'color=black:s=1280x720:d=61',
        '-f',
        'lavfi',
        '-i',
        'anullsrc=r=44100:cl=stereo',
        '-shortest',
        '-pix_fmt',
        'yuv420p',
        demoMedia[0],
    ]);
    execFileSync('ffmpeg', [
        ...common,
        '-f',
        'lavfi',
        '-i',
        'anullsrc=r=44100:cl=stereo',
        '-t',
        '61',
        '-c:a',
        'aac',
        demoMedia[1],
    ]);
}

generateDemoMedia();

// detached => own process group, so the vite child spawned by pnpm can be
// killed reliably via the negative pid.
const server = spawn('pnpm', ['exec', 'vite', '--port', String(PORT), '--strictPort'], {
    cwd: new URL('..', import.meta.url).pathname,
    env: { ...process.env, VITE_SCREENSHOT_DEMO: '1' },
    stdio: ['ignore', 'pipe', 'inherit'],
    detached: true,
});

let browser;
try {
    await waitForServer(BASE_URL);
    browser = await chromium.launch();
    const page = await browser.newPage({ viewport: VIEWPORT, deviceScaleFactor: 1 });

    for (const shot of SHOTS) {
        await page.setViewportSize(shot.viewport ?? VIEWPORT);
        await page.goto(shot.url);
        await shot.before?.(page);
        await shot.waitFor(page).waitFor({ state: 'visible', timeout: 15_000 });
        // Wait for <video>/<audio> elements to finish loading or fail (the demo
        // asset URL never resolves in a browser), so no loading spinner is caught.
        await page
            .waitForFunction(
                () =>
                    [...document.querySelectorAll('video, audio')].every(
                        (m) => m.readyState >= 2 || m.error,
                    ),
                { timeout: 10_000 },
            )
            .catch(() => {});
        // No focus ring on whatever a dialog focused first.
        await page.evaluate(() => document.activeElement?.blur());
        // Let transitions/layout settle before capturing.
        await sleep(1500);
        await page.screenshot({ path: shot.file });
        console.log(`captured ${shot.file}`);
    }
} finally {
    await browser?.close();
    try {
        process.kill(-server.pid, 'SIGTERM');
    } catch {
        server.kill();
    }
    for (const file of demoMedia) rmSync(file, { force: true });
}
