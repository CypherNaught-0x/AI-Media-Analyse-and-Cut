/**
 * What the setup wizard recommends for a machine: which speech engine, and
 * how well each one runs here.
 */
import type { AppleSpeechStatus, NemotronStatus } from '../bindings';
import type { LocalEngine } from '../types';

export type EngineRating = 'great' | 'ok' | 'slow' | 'unavailable' | 'needs-key';

export interface EngineAssessment {
    engine: LocalEngine;
    title: string;
    rating: EngineRating;
    /** One line on why, or what it costs (download size, a key, ...). */
    note: string;
    /** Runs on this machine (nothing is uploaded). */
    onDevice: boolean;
}

/** The engines the wizard offers, best first. CrisperWhisper stays in Settings. */
export function assessEngines(
    nemotron: NemotronStatus | null,
    appleSpeech: AppleSpeechStatus | null,
    euOnly: boolean,
): EngineAssessment[] {
    const fit = nemotron?.recommendation;
    const variant = fit?.variant
        ? nemotron?.variants.find((entry) => entry.variant === fit.variant)
        : undefined;
    const size = variant ? `${(variant.downloadBytes / 1024 ** 3).toFixed(0)} GB download` : '';

    const nemotronRating: EngineRating = !fit
        ? 'unavailable'
        : fit.rating === 'good'
          ? 'great'
          : fit.rating === 'tight'
            ? 'ok'
            : fit.rating === 'slow'
              ? 'slow'
              : 'unavailable';

    const assessments: EngineAssessment[] = [
        {
            engine: 'nemotron',
            title: 'Nemotron 3.5 Transcribe (on this device)',
            rating: nemotronRating,
            note: fit
                ? [
                      fit.message,
                      size,
                      fit.rating === 'unsupported' ? '' : '~2x real time on an M3 Pro',
                  ]
                      .filter(Boolean)
                      .join(' · ')
                : 'Checking what this machine can run…',
            onDevice: true,
        },
        {
            engine: 'cloud',
            title: euOnly
                ? 'Cloud via OpenRouter EU (Voxtral)'
                : 'Nemotron 3.5 Transcribe in the cloud (OpenRouter)',
            rating: 'needs-key',
            note: euOnly
                ? 'Processed in Europe only. Nemotron is not served in the EU region, so this uses Mistral Voxtral.'
                : 'Same model, no download; about $0.12 per hour of audio. Needs an OpenRouter key.',
            onDevice: false,
        },
        {
            engine: 'parakeet',
            title: 'Parakeet (on this device)',
            rating: 'great',
            note: 'Light and fast on any machine, 25 European languages · 1.2 GB download.',
            onDevice: true,
        },
    ];
    if (appleSpeech?.supportedPlatform) {
        assessments.push({
            engine: 'apple-speech',
            title: 'Apple Speech (built into macOS)',
            rating: appleSpeech.available ? 'great' : 'unavailable',
            note: appleSpeech.available
                ? 'Very fast, ~45 languages, no download. Drops fillers like “um”.'
                : (appleSpeech.message ?? 'Not available on this Mac.'),
            onDevice: true,
        });
    }
    return assessments;
}

/**
 * Nemotron when this machine runs it on a GPU, otherwise the same model in
 * the cloud (Voxtral in the EU region), which is far faster than Nemotron on
 * a CPU.
 */
export function recommendedEngine(nemotron: NemotronStatus | null): LocalEngine {
    const rating = nemotron?.recommendation.rating;
    return rating === 'good' || rating === 'tight' ? 'nemotron' : 'cloud';
}

/** A short "Apple M3 Pro · 36 GB memory · 75 GB free" line. */
export function describeDevice(nemotron: NemotronStatus | null): string {
    const device = nemotron?.device;
    if (!device) return '';
    const parts: string[] = [];
    if (device.cpu) parts.push(device.cpu);
    parts.push(
        `${Math.round(device.totalMemoryGb)} GB ${device.appleSilicon ? 'unified memory' : 'RAM'}`,
    );
    for (const gpu of device.gpus) {
        if (gpu.vendor === 'nvidia') {
            parts.push(gpu.memoryGb ? `${gpu.name} (${Math.round(gpu.memoryGb)} GB)` : gpu.name);
        }
    }
    if (device.freeDiskGb != null) parts.push(`${Math.round(device.freeDiskGb)} GB free disk`);
    return parts.join(' · ');
}

/** Whether the recommended Nemotron download fits on the disk (1 GB spare). */
export function hasRoomForNemotron(nemotron: NemotronStatus | null): boolean {
    const free = nemotron?.device.freeDiskGb;
    const variant = nemotron?.recommendation.variant;
    if (free == null || !variant) return true;
    const info = nemotron.variants.find((entry) => entry.variant === variant);
    if (!info || info.installed) return true;
    return free >= info.downloadBytes / 1024 ** 3 + 1;
}
