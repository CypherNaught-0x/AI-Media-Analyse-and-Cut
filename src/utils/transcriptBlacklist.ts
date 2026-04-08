import type { TranscriptSegment } from '../types';
import { getTranscriptLanguageCode, getTranscriptLanguageName } from './transcriptLanguages';

export interface TranscriptBlacklistMatch {
  languageCode: string;
  matchedText: string;
  normalizedWord: string;
  segmentIndex: number;
  wordIndex?: number;
  speaker: string;
  start: string;
  end: string;
  segmentText: string;
}

export interface TranscriptBlacklistWarningResult {
  languageCode: string | null;
  languageLabel: string | null;
  matches: TranscriptBlacklistMatch[];
  matchesBySegment: Record<number, TranscriptBlacklistMatch[]>;
  uniqueWords: string[];
}

const blacklistFileContents = import.meta.glob('../assets/transcript-blacklists/*.txt', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const EDGE_PUNCTUATION_PATTERN = /^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu;

function normalizeWord(value: string): string {
  return value
    .normalize('NFKC')
    .trim()
    .replace(EDGE_PUNCTUATION_PATTERN, '')
    .toLocaleLowerCase();
}

function hasSingleToken(value: string): boolean {
  return (value.match(/\S+/gu) ?? []).length === 1;
}

function parseBlacklistWords(raw: string): Set<string> {
  const words = new Set<string>();

  for (const line of raw.split(/\r?\n/u)) {
    const candidate = line.trim();
    if (!candidate || candidate.startsWith('#') || !hasSingleToken(candidate)) {
      continue;
    }

    const normalized = normalizeWord(candidate);
    if (normalized) {
      words.add(normalized);
    }
  }

  return words;
}

const transcriptBlacklistByLanguage = Object.fromEntries(
  Object.entries(blacklistFileContents)
    .map(([path, raw]) => {
      const match = path.match(/\/([a-z0-9-]+)\.txt$/iu);
      if (!match) {
        return null;
      }

      return [match[1].toLocaleLowerCase(), parseBlacklistWords(raw)] as const;
    })
    .filter((entry): entry is readonly [string, Set<string>] => entry !== null),
);

function resolveBlacklistLanguageCode(currentLanguage: string): string | null {
  const trimmedLanguage = currentLanguage.trim();
  const explicitCode = getTranscriptLanguageCode(trimmedLanguage) ?? trimmedLanguage.toLocaleLowerCase();

  if (explicitCode in transcriptBlacklistByLanguage) {
    return explicitCode;
  }

  const availableCodes = Object.keys(transcriptBlacklistByLanguage);
  if (trimmedLanguage === 'Original' && availableCodes.length === 1) {
    return availableCodes[0];
  }

  return null;
}

function tokenizeSegment(segment: TranscriptSegment): Array<{
  text: string;
  start: string;
  end: string;
  wordIndex?: number;
}> {
  if ((segment.words?.length ?? 0) > 0) {
    return segment.words!.map((word, wordIndex) => ({
      text: word.text,
      start: word.start,
      end: word.end,
      wordIndex,
    }));
  }

  return (segment.text.match(/\S+/gu) ?? []).map((text) => ({
    text,
    start: segment.start,
    end: segment.end,
  }));
}

export function detectTranscriptBlacklistMatches(
  segments: TranscriptSegment[],
  currentLanguage: string,
): TranscriptBlacklistWarningResult {
  const languageCode = resolveBlacklistLanguageCode(currentLanguage);
  const blacklistWords =
    languageCode !== null ? transcriptBlacklistByLanguage[languageCode] : undefined;

  if (!languageCode || !blacklistWords) {
    return {
      languageCode: null,
      languageLabel: null,
      matches: [],
      matchesBySegment: {},
      uniqueWords: [],
    };
  }

  const matches: TranscriptBlacklistMatch[] = [];

  segments.forEach((segment, segmentIndex) => {
    tokenizeSegment(segment).forEach((token) => {
      const normalizedWord = normalizeWord(token.text);
      if (!normalizedWord || !blacklistWords.has(normalizedWord)) {
        return;
      }

      matches.push({
        languageCode,
        matchedText: token.text,
        normalizedWord,
        segmentIndex,
        wordIndex: token.wordIndex,
        speaker: segment.speaker,
        start: token.start,
        end: token.end,
        segmentText: segment.text,
      });
    });
  });

  const matchesBySegment = matches.reduce<Record<number, TranscriptBlacklistMatch[]>>(
    (accumulator, match) => {
      accumulator[match.segmentIndex] ??= [];
      accumulator[match.segmentIndex].push(match);
      return accumulator;
    },
    {},
  );

  const uniqueWords: string[] = [];
  const seenWords = new Set<string>();

  matches.forEach((match) => {
    if (!seenWords.has(match.normalizedWord)) {
      seenWords.add(match.normalizedWord);
      uniqueWords.push(match.matchedText);
    }
  });

  return {
    languageCode,
    languageLabel: getTranscriptLanguageName(languageCode),
    matches,
    matchesBySegment,
    uniqueWords,
  };
}
