import type { TranscriptSegment } from '../types';

export interface SubtitleValidationError {
  segmentIndex: number;
  field: string;
  message: string;
  severity: 'warning' | 'error';
}

export interface SubtitleSplitOptions {
  maxCharsPerLine: number;
  maxLines: number;
  minDurationMs: number;
  maxDurationMs: number;
  minCps: number;
  maxCps: number;
}

export const DEFAULT_SUBTITLE_OPTIONS: SubtitleSplitOptions = {
  maxCharsPerLine: 42,
  maxLines: 2,
  minDurationMs: 500,
  maxDurationMs: 7000,
  minCps: 8,
  maxCps: 25,
};

/**
 * Split text into chunks that fit within maxCharsPerLine
 * Respects word boundaries when possible, but will force-split very long words
 */
export function splitTextIntoChunks(text: string, maxChars: number): string[] {
  if (text.length <= maxChars) return [text];
  
  const chunks: string[] = [];
  const sentences = text.split(/(?<=[.!?])\s+/);
  let currentChunk = '';
  
  for (const sentence of sentences) {
    if (sentence.length > maxChars) {
      // Sentence is too long, split at word boundaries
      if (currentChunk) {
        chunks.push(currentChunk.trim());
        currentChunk = '';
      }
      
      const words = sentence.split(' ');
      for (const word of words) {
        if (word.length > maxChars) {
          // Word is too long, force split it
          if (currentChunk) {
            chunks.push(currentChunk.trim());
            currentChunk = '';
          }
          
          for (let i = 0; i < word.length; i += maxChars) {
            const chunk = word.slice(i, i + maxChars);
            if (i + maxChars >= word.length) {
              currentChunk = chunk;
            } else {
              chunks.push(chunk);
            }
          }
        } else if ((currentChunk + ' ' + word).trim().length <= maxChars) {
          currentChunk = currentChunk ? currentChunk + ' ' + word : word;
        } else {
          if (currentChunk) chunks.push(currentChunk.trim());
          currentChunk = word;
        }
      }
    } else if ((currentChunk + ' ' + sentence).trim().length <= maxChars) {
      currentChunk = currentChunk ? currentChunk + ' ' + sentence : sentence;
    } else {
      if (currentChunk) chunks.push(currentChunk.trim());
      currentChunk = sentence;
    }
  }
  
  if (currentChunk) chunks.push(currentChunk.trim());
  return chunks;
}

/**
 * Split a long subtitle into multiple comfortable display subtitles
 * Respects the maxLines constraint
 */
export function splitSubtitleEntry(
  segment: TranscriptSegment,
  options: Partial<SubtitleSplitOptions> = {}
): TranscriptSegment[] {
  const opts = { ...DEFAULT_SUBTITLE_OPTIONS, ...options };
  const maxChars = opts.maxCharsPerLine * opts.maxLines;
  
  if (segment.text.length <= maxChars) {
    return [segment];
  }
  
  const chunks = splitTextIntoChunks(segment.text, opts.maxCharsPerLine);
  const groupedChunks: string[] = [];
  for (let i = 0; i < chunks.length; i += opts.maxLines) {
    groupedChunks.push(chunks.slice(i, i + opts.maxLines).join('\n'));
  }

  if (groupedChunks.length <= 1) {
    return [{ ...segment, text: groupedChunks[0] ?? segment.text }];
  }
  
  const startMs = timeToMs(segment.start);
  const endMs = timeToMs(segment.end);
  const totalDuration = endMs - startMs;

  // If the source cue has no usable duration, keep it as a single cue.
  if (totalDuration <= 0) {
    return [segment];
  }

  const weights = groupedChunks.map((chunk) => Math.max(chunk.replace(/\n/g, ' ').length, 1));
  const totalWeight = weights.reduce((sum, weight) => sum + weight, 0);
  const splitSegments: TranscriptSegment[] = [];
  let consumedWeight = 0;

  for (let i = 0; i < groupedChunks.length; i++) {
    const chunkText = groupedChunks[i];
    const chunkStartMs =
      i === 0
        ? startMs
        : startMs + Math.round((totalDuration * consumedWeight) / totalWeight);
    consumedWeight += weights[i];
    const chunkEndMs =
      i === groupedChunks.length - 1
        ? endMs
        : startMs + Math.round((totalDuration * consumedWeight) / totalWeight);

    splitSegments.push({
      start: msToTime(chunkStartMs),
      end: msToTime(Math.max(chunkStartMs, chunkEndMs)),
      text: chunkText,
      speaker: segment.speaker,
    });
  }

  return splitSegments;
}

/**
 * Convert time string (MM:SS or HH:MM:SS) to milliseconds
 */
export function timeToMs(time: string): number {
  const [base, fractional = '0'] = time.split(/[.,]/);
  const parts = base.split(':').map(Number);
  const milliseconds = Number(fractional.padEnd(3, '0').slice(0, 3));
  if (parts.length === 2) {
    return (parts[0] * 60 + parts[1]) * 1000 + milliseconds;
  } else if (parts.length === 3) {
    return (parts[0] * 3600 + parts[1] * 60 + parts[2]) * 1000 + milliseconds;
  }
  return 0;
}

/**
 * Convert milliseconds to time string (HH:MM:SS)
 */
export function msToTime(ms: number): string {
  const safeMs = Math.max(0, Math.round(ms));
  const totalSeconds = Math.floor(safeMs / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const milliseconds = safeMs % 1000;
  
  if (hours > 0) {
    const base = `${hours}:${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`;
    return milliseconds > 0 ? `${base}.${milliseconds.toString().padStart(3, '0')}` : base;
  }
  const base = `${minutes}:${seconds.toString().padStart(2, '0')}`;
  return milliseconds > 0 ? `${base}.${milliseconds.toString().padStart(3, '0')}` : base;
}

/**
 * Validate subtitle segments for comfortable display
 */
export function validateSubtitles(
  segments: TranscriptSegment[],
  options: Partial<SubtitleSplitOptions> = {}
): SubtitleValidationError[] {
  const opts = { ...DEFAULT_SUBTITLE_OPTIONS, ...options };
  const errors: SubtitleValidationError[] = [];
  
  segments.forEach((segment, index) => {
    // Check text length
    const lines = segment.text.split('\n');
    lines.forEach((line, lineIndex) => {
      if (line.length > opts.maxCharsPerLine) {
        errors.push({
          segmentIndex: index,
          field: `text.line${lineIndex + 1}`,
          message: `Line ${lineIndex + 1} exceeds ${opts.maxCharsPerLine} characters (${line.length})`,
          severity: 'error',
        });
      }
    });
    
    if (lines.length > opts.maxLines) {
      errors.push({
        segmentIndex: index,
        field: 'text.lines',
        message: `Subtitle has ${lines.length} lines, max is ${opts.maxLines}`,
        severity: 'error',
      });
    }
    
    // Check duration
    const duration = timeToMs(segment.end) - timeToMs(segment.start);
    if (duration < 0) {
      errors.push({
        segmentIndex: index,
        field: 'duration',
        message: `End timestamp ${segment.end} is before start ${segment.start}`,
        severity: 'error',
      });
    } else if (duration < opts.minDurationMs) {
      errors.push({
        segmentIndex: index,
        field: 'duration',
        message: `Duration ${duration}ms is less than minimum ${opts.minDurationMs}ms`,
        severity: 'warning',
      });
    }
    
    if (duration > opts.maxDurationMs) {
      errors.push({
        segmentIndex: index,
        field: 'duration',
        message: `Duration ${duration}ms exceeds maximum ${opts.maxDurationMs}ms`,
        severity: 'warning',
      });
    }
    
    // Check characters per second
    const textLength = segment.text.replace('\n', ' ').length;
    const cps = duration > 0 ? textLength / (duration / 1000) : Infinity;
    if (cps > opts.maxCps) {
      errors.push({
        segmentIndex: index,
        field: 'cps',
        message: `Reading speed ${cps.toFixed(1)} chars/sec exceeds maximum ${opts.maxCps}`,
        severity: 'warning',
      });
    }
    
    if (cps < opts.minCps && textLength > 10) {
      errors.push({
        segmentIndex: index,
        field: 'cps',
        message: `Reading speed ${cps.toFixed(1)} chars/sec is below minimum ${opts.minCps}`,
        severity: 'warning',
      });
    }
    
    // Check for empty text
    if (!segment.text.trim()) {
      errors.push({
        segmentIndex: index,
        field: 'text',
        message: 'Subtitle text is empty',
        severity: 'error',
      });
    }
    
    // Check speaker
    if (!segment.speaker.trim()) {
      errors.push({
        segmentIndex: index,
        field: 'speaker',
        message: 'Speaker is not specified',
        severity: 'warning',
      });
    }
  });
  
  // Check for overlapping timestamps
  for (let i = 1; i < segments.length; i++) {
    const prevEnd = timeToMs(segments[i - 1].end);
    const currStart = timeToMs(segments[i].start);
    if (currStart < prevEnd) {
      errors.push({
        segmentIndex: i,
        field: 'timing',
        message: `Overlaps with previous subtitle (prev ends at ${segments[i - 1].end}, this starts at ${segments[i].start})`,
        severity: 'error',
      });
    }
  }
  
  return errors;
}

/**
 * Process all segments to ensure they meet comfortable display requirements
 * Splits long entries and validates the result
 */
export function processSubtitlesForDisplay(
  segments: TranscriptSegment[],
  options: Partial<SubtitleSplitOptions> = {}
): { segments: TranscriptSegment[]; errors: SubtitleValidationError[] } {
  // First pass: split long entries
  const processedSegments: TranscriptSegment[] = [];
  
  for (const segment of segments) {
    const split = splitSubtitleEntry(segment, options);
    processedSegments.push(...split);
  }
  
  // Second pass: validate
  const errors = validateSubtitles(processedSegments, options);
  
  return { segments: processedSegments, errors };
}
