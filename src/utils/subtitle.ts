import type { TranscriptSegment } from '../types';

export function formatTime(seconds: number, separator: string = ','): string {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = Math.floor(seconds % 60);
    const ms = Math.floor((seconds % 1) * 1000);

    const hStr = h.toString().padStart(2, '0');
    const mStr = m.toString().padStart(2, '0');
    const sStr = s.toString().padStart(2, '0');
    const msStr = ms.toString().padStart(3, '0');

    return `${hStr}:${mStr}:${sStr}${separator}${msStr}`;
}

export function generateSubtitleContent(segments: TranscriptSegment[], format: 'srt' | 'vtt' | 'txt'): string {
    if (format === 'srt') {
        return segments.map((s, i) => {
            const start = formatTime(parseTime(s.start), ',');
            const end = formatTime(parseTime(s.end), ',');
            return `${i + 1}\n${start} --> ${end}\n${s.speaker}: ${s.text}\n`;
        }).join('\n');
    } else if (format === 'vtt') {
        return "WEBVTT\n\n" + segments.map((s) => {
            const start = formatTime(parseTime(s.start), '.');
            const end = formatTime(parseTime(s.end), '.');
            return `${start} --> ${end}\n<v ${s.speaker}>${s.text}`;
        }).join('\n\n');
    } else {
        return segments.map(s => `[${s.start} - ${s.end}] ${s.speaker}: ${s.text}`).join('\n');
    }
}

// Helper to parse "HH:MM:SS.mmm" or similar back to seconds, 
// but wait, TranscriptSegment uses string for start/end.
// We need a consistent parseTime. 
// The one in Home.vue handles "MM:SS" etc.
// Let's duplicate it here or export it from somewhere.
// For now, I'll implement a robust one here.

function parseTime(time: string | number): number {
    if (typeof time === 'number') return time;
    const parts = time.split(':');
    if (parts.length === 3) {
        return parseFloat(parts[0]) * 3600 + parseFloat(parts[1]) * 60 + parseFloat(parts[2]);
    } else if (parts.length === 2) {
        return parseFloat(parts[0]) * 60 + parseFloat(parts[1]);
    }
    return parseFloat(time);
}
