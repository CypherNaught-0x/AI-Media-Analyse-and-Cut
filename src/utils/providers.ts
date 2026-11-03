/**
 * AI provider presets and the OpenRouter endpoints shared by the LLM stages
 * and cloud speech-to-text.
 */

export type LlmProvider = 'gemini' | 'openrouter' | 'custom';

export const GEMINI_BASE_URL = 'https://generativelanguage.googleapis.com';
export const OPENROUTER_BASE_URL = 'https://openrouter.ai/api';
/**
 * EU in-region routing: requests are decrypted and processed in Europe only
 * and fail rather than leave it. Same key and model ids, fewer models.
 */
export const OPENROUTER_EU_BASE_URL = 'https://eu.openrouter.ai/api';

export function openRouterBaseUrl(euOnly: boolean): string {
    return euOnly ? OPENROUTER_EU_BASE_URL : OPENROUTER_BASE_URL;
}

/** The recommended speech-to-text model; not served in the EU region. */
export const NEMOTRON_CLOUD_MODEL = 'nvidia/nemotron-3.5-transcribe';
/** The EU region's best timed speech-to-text model (Mistral, served in the EU). */
export const EU_CLOUD_ASR_MODEL = 'mistralai/voxtral-small-transcribe-3-0';

export function defaultCloudAsrModel(euOnly: boolean): string {
    return euOnly ? EU_CLOUD_ASR_MODEL : NEMOTRON_CLOUD_MODEL;
}

/** Default LLM on OpenRouter; both accept audio, so LLM Only works too. */
export function defaultOpenRouterModel(euOnly: boolean): string {
    return euOnly ? 'google/gemini-3.5-flash-lite' : 'google/gemini-3.5-flash';
}

export const DEFAULT_GEMINI_MODEL = 'gemini-2.5-flash';

/** True for openrouter.ai and its regional hosts (eu.openrouter.ai, ...). */
export function isOpenRouterUrl(url: string): boolean {
    const host = url
        .trim()
        .replace(/^https?:\/\//i, '')
        .split(/[/:?]/)[0]
        .toLowerCase();
    return host === 'openrouter.ai' || host.endsWith('.openrouter.ai');
}

export function isEuOpenRouterUrl(url: string): boolean {
    return isOpenRouterUrl(url) && /^(https?:\/\/)?eu\.openrouter\.ai/i.test(url.trim());
}

/** The provider preset a stored base URL belongs to. */
export function providerForBaseUrl(url: string): LlmProvider {
    if (isOpenRouterUrl(url)) return 'openrouter';
    if (url.includes('generativelanguage.googleapis.com')) return 'gemini';
    return 'custom';
}

/** Transcript languages offered for Nemotron and cloud speech-to-text. */
export const ASR_LANGUAGES: { value: string; label: string }[] = [
    { value: '', label: 'Detect automatically' },
    { value: 'en', label: 'English' },
    { value: 'de', label: 'German' },
    { value: 'fr', label: 'French' },
    { value: 'es', label: 'Spanish' },
    { value: 'it', label: 'Italian' },
    { value: 'pt', label: 'Portuguese' },
    { value: 'nl', label: 'Dutch' },
    { value: 'pl', label: 'Polish' },
    { value: 'cs', label: 'Czech' },
    { value: 'sv', label: 'Swedish' },
    { value: 'da', label: 'Danish' },
    { value: 'fi', label: 'Finnish' },
    { value: 'no', label: 'Norwegian' },
    { value: 'ru', label: 'Russian' },
    { value: 'uk', label: 'Ukrainian' },
    { value: 'tr', label: 'Turkish' },
    { value: 'ar', label: 'Arabic' },
    { value: 'hi', label: 'Hindi' },
    { value: 'ja', label: 'Japanese' },
    { value: 'ko', label: 'Korean' },
    { value: 'zh', label: 'Chinese' },
];

export function formatBytes(bytes: number): string {
    const gb = bytes / 1024 ** 3;
    if (gb >= 1) return `${gb.toFixed(1)} GB`;
    return `${Math.max(1, Math.round(bytes / 1024 ** 2))} MB`;
}
