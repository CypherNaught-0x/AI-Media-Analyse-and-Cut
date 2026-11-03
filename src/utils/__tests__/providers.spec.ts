import { describe, expect, it } from 'vitest';
import {
    EU_CLOUD_ASR_MODEL,
    NEMOTRON_CLOUD_MODEL,
    defaultCloudAsrModel,
    formatBytes,
    isEuOpenRouterUrl,
    isOpenRouterUrl,
    openRouterBaseUrl,
    providerForBaseUrl,
} from '../providers';

describe('providers', () => {
    it('recognises OpenRouter by host, including the EU region', () => {
        expect(isOpenRouterUrl('https://openrouter.ai/api')).toBe(true);
        expect(isOpenRouterUrl('https://eu.openrouter.ai/api')).toBe(true);
        expect(isOpenRouterUrl('https://openrouter.ai.example.com')).toBe(false);
        expect(isEuOpenRouterUrl('https://eu.openrouter.ai/api')).toBe(true);
        expect(isEuOpenRouterUrl('https://openrouter.ai/api')).toBe(false);
    });

    it('maps stored base URLs to provider presets', () => {
        expect(providerForBaseUrl('https://generativelanguage.googleapis.com')).toBe('gemini');
        expect(providerForBaseUrl(openRouterBaseUrl(true))).toBe('openrouter');
        expect(providerForBaseUrl('http://localhost:4000')).toBe('custom');
    });

    it('defaults to Nemotron, and to Voxtral where Nemotron is not served (EU)', () => {
        expect(defaultCloudAsrModel(false)).toBe(NEMOTRON_CLOUD_MODEL);
        expect(defaultCloudAsrModel(true)).toBe(EU_CLOUD_ASR_MODEL);
        expect(openRouterBaseUrl(true)).toBe('https://eu.openrouter.ai/api');
    });

    it('formats download sizes', () => {
        expect(formatBytes(21_349_066_752)).toBe('19.9 GB');
        expect(formatBytes(7_239_335)).toBe('7 MB');
    });
});
