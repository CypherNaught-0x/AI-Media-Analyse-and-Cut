import { describe, expect, it } from 'vitest';
import type { AppleSpeechStatus, NemotronStatus } from '../../bindings';
import {
    assessEngines,
    describeDevice,
    hasRoomForNemotron,
    recommendedEngine,
} from '../setupRecommendation';

function status(
    rating: NemotronStatus['recommendation']['rating'],
    freeDiskGb = 100,
): NemotronStatus {
    return {
        platformSupported: true,
        runtimeInstalled: false,
        runtimeDownloadBytes: 7_239_335,
        encoderInstalled: false,
        variants: [
            {
                variant: 'IQ4_NL',
                label: 'IQ4_NL (recommended)',
                downloadBytes: 21_349_066_752,
                requiredMemoryGb: 21.4,
                installed: false,
            },
        ],
        recommendation: {
            rating,
            variant: rating === 'unsupported' ? null : 'IQ4_NL',
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
            freeDiskGb,
        },
        installing: false,
    };
}

const appleSpeech: AppleSpeechStatus = {
    supportedPlatform: true,
    available: true,
    supportedLocales: ['en-US'],
    installedLocales: ['en-US'],
    message: null,
};

describe('setup recommendation', () => {
    it('recommends Nemotron on device when the GPU holds it, else in the cloud', () => {
        expect(recommendedEngine(status('good'))).toBe('nemotron');
        expect(recommendedEngine(status('tight'))).toBe('nemotron');
        expect(recommendedEngine(status('slow'))).toBe('cloud');
        expect(recommendedEngine(status('unsupported'))).toBe('cloud');
        expect(recommendedEngine(null)).toBe('cloud');
    });

    it('rates every offered engine for this machine', () => {
        const engines = assessEngines(status('good'), appleSpeech, false);
        expect(engines.map((entry) => entry.engine)).toEqual([
            'nemotron',
            'cloud',
            'parakeet',
            'apple-speech',
        ]);
        expect(engines[0].rating).toBe('great');
        expect(engines[0].note).toContain('20 GB download');
        expect(engines[1].rating).toBe('needs-key');
        expect(engines[1].onDevice).toBe(false);
    });

    it('explains the EU region serves Voxtral instead of Nemotron', () => {
        const cloud = assessEngines(status('good'), null, true).find(
            (entry) => entry.engine === 'cloud',
        )!;
        expect(cloud.title).toContain('EU');
        expect(cloud.note).toContain('Voxtral');
    });

    it('leaves Apple Speech out off macOS', () => {
        const engines = assessEngines(
            status('good'),
            { ...appleSpeech, supportedPlatform: false },
            false,
        );
        expect(engines.some((entry) => entry.engine === 'apple-speech')).toBe(false);
    });

    it('describes the machine and checks the disk', () => {
        expect(describeDevice(status('good'))).toBe(
            'Apple M3 Pro · 36 GB unified memory · 100 GB free disk',
        );
        expect(hasRoomForNemotron(status('good', 100))).toBe(true);
        expect(hasRoomForNemotron(status('good', 15))).toBe(false);
    });
});
