export interface TranscriptLanguageOption {
  code: string;
  name: string;
  country: string;
}

export const SUPPORTED_TRANSCRIPT_LANGUAGES: TranscriptLanguageOption[] = [
  { code: 'en', name: 'English', country: 'us' },
  { code: 'es', name: 'Spanish', country: 'es' },
  { code: 'fr', name: 'French', country: 'fr' },
  { code: 'de', name: 'German', country: 'de' },
  { code: 'it', name: 'Italian', country: 'it' },
  { code: 'pt', name: 'Portuguese', country: 'pt' },
  { code: 'nl', name: 'Dutch', country: 'nl' },
  { code: 'ru', name: 'Russian', country: 'ru' },
  { code: 'ja', name: 'Japanese', country: 'jp' },
  { code: 'zh', name: 'Chinese', country: 'cn' },
  { code: 'ko', name: 'Korean', country: 'kr' },
  { code: 'hi', name: 'Hindi', country: 'in' },
  { code: 'ar', name: 'Arabic', country: 'sa' },
  { code: 'tr', name: 'Turkish', country: 'tr' },
  { code: 'pl', name: 'Polish', country: 'pl' },
];

const LANGUAGE_NAME_TO_CODE = new Map(
  SUPPORTED_TRANSCRIPT_LANGUAGES.map((language) => [language.name, language.code]),
);

const LANGUAGE_CODE_TO_NAME = new Map(
  SUPPORTED_TRANSCRIPT_LANGUAGES.map((language) => [language.code, language.name]),
);

export function getTranscriptLanguageCode(languageName: string): string | null {
  return LANGUAGE_NAME_TO_CODE.get(languageName.trim()) ?? null;
}

export function getTranscriptLanguageName(languageCode: string): string | null {
  return LANGUAGE_CODE_TO_NAME.get(languageCode.trim()) ?? null;
}
