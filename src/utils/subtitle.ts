import { parseTime } from '../composables/useTimeFormat';
import type { TranscriptSegment } from '../types';

export function formatTime(seconds: number, separator: string = ','): string {
    const totalMillis = Math.round(Math.max(0, seconds) * 1000);
    const totalSeconds = Math.floor(totalMillis / 1000);
    const hours = Math.floor(totalSeconds / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);
    const secs = totalSeconds % 60;
    const millis = totalMillis % 1000;

    return `${hours.toString().padStart(2, '0')}:${minutes
        .toString()
        .padStart(2, '0')}:${secs.toString().padStart(2, '0')}${separator}${millis
        .toString()
        .padStart(3, '0')}`;
}

export function formatSubtitleTimestamp(time: string | number, separator: string = ','): string {
    return formatTime(parseTime(time), separator);
}

export function formatSubtitleTimeRange(
    start: string | number,
    end: string | number,
    separator: string = ',',
): { start: string; end: string } {
    const startSeconds = parseTime(start);
    const endSeconds = Math.max(startSeconds, parseTime(end));

    return {
        start: formatTime(startSeconds, separator),
        end: formatTime(endSeconds, separator),
    };
}

export function generateSubtitleContent(segments: TranscriptSegment[], format: 'srt' | 'vtt' | 'txt'): string {
    if (format === 'srt') {
        return segments.map((s, i) => {
            const { start, end } = formatSubtitleTimeRange(s.start, s.end, ',');
            return `${i + 1}\n${start} --> ${end}\n${s.speaker}: ${s.text}\n`;
        }).join('\n');
    } else if (format === 'vtt') {
        return "WEBVTT\n\n" + segments.map((s) => {
            const { start, end } = formatSubtitleTimeRange(s.start, s.end, '.');
            return `${start} --> ${end}\n<v ${s.speaker}>${s.text}`;
        }).join('\n\n');
    } else {
        return segments.map(s => `[${s.start} - ${s.end}] ${s.speaker}: ${s.text}`).join('\n');
    }
}
