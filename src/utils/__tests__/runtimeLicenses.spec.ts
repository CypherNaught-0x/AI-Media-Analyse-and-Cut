import { describe, expect, it } from 'vitest';
import {
    runtimeDownload,
    runtimeLicenses,
    runtimeLicenseViolations,
    type RuntimeLicenseRegister,
} from '../runtimeLicenses';

function register(overrides: Partial<RuntimeLicenseRegister['policy']> = {}, downloads = []) {
    return {
        policy: { allowed: ['MIT'], requiresNotice: [], exceptions: [], ...overrides },
        downloads,
    } as RuntimeLicenseRegister;
}

const download = (id: string, license: string, notice?: string) => ({
    id,
    name: id,
    kind: 'model-weights' as const,
    usedBy: 'test',
    source: 'test',
    license,
    licenseUrl: 'https://example.com',
    notice,
});

describe('runtime licence register', () => {
    it('complies with its own policy', () => {
        expect(runtimeLicenseViolations(runtimeLicenses)).toEqual([]);
    });

    it('keeps the non-commercial CrisperWhisper weights as a noticed exception', () => {
        const weights = runtimeDownload('crisperwhisper-weights');
        expect(runtimeLicenses.policy.allowed).not.toContain(weights.license);
        expect(weights.notice).toMatch(/non-commercial/i);
    });
});

describe('runtimeLicenseViolations', () => {
    it('rejects licences that are neither allowed nor excepted', () => {
        expect(
            runtimeLicenseViolations(register({}, [download('a', 'Proprietary')] as never)),
        ).toEqual(['a: licence Proprietary is not allowed']);
    });

    it('requires a notice for attribution licences and exceptions', () => {
        const violations = runtimeLicenseViolations(
            register(
                {
                    allowed: ['CC-BY-4.0'],
                    requiresNotice: ['CC-BY-4.0'],
                    exceptions: [{ license: 'NC', appliesTo: ['b'], reason: 'optional' }],
                },
                [download('a', 'CC-BY-4.0'), download('b', 'NC', ' ')] as never,
            ),
        );
        expect(violations).toEqual(['a: CC-BY-4.0 requires a notice', 'b: NC requires a notice']);
    });

    it('limits an exception to the downloads it names', () => {
        const violations = runtimeLicenseViolations(
            register({ exceptions: [{ license: 'NC', appliesTo: ['b'], reason: 'optional' }] }, [
                download('b', 'NC', 'notice'),
                download('c', 'NC', 'notice'),
            ] as never),
        );
        expect(violations).toEqual(['c: NC is only excepted for b']);
    });
});
