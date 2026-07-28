import { describe, it, expect } from 'vitest';
import {
  buildCutTimeline,
  mapSourceTimeToCut,
  remapSegmentsToCutTimeline,
} from '../subtitleTimeline';
import type { TranscriptSegment } from '../../types';

function segment(start: string, end: string, text = 'text'): TranscriptSegment {
  return { start, end, text, speaker: 'Speaker 1' };
}

describe('subtitleTimeline', () => {
  it('builds cumulative ranges in concatenation order', () => {
    const ranges = buildCutTimeline([segment('01:00.000', '01:10.000'), segment('02:00.000', '02:05.000')]);

    expect(ranges).toEqual([
      { sourceStart: 60, sourceEnd: 70, cutStart: 0 },
      { sourceStart: 120, sourceEnd: 125, cutStart: 10 },
    ]);
  });

  it('drops the material the cut removes', () => {
    const ranges = buildCutTimeline([segment('01:00.000', '01:10.000'), segment('02:00.000', '02:05.000')]);

    // Before the first kept range: the cut starts there.
    expect(mapSourceTimeToCut(0, ranges)).toBe(0);
    expect(mapSourceTimeToCut(59.5, ranges)).toBe(0);
    // Inside a kept range.
    expect(mapSourceTimeToCut(65, ranges)).toBe(5);
    expect(mapSourceTimeToCut(120, ranges)).toBe(10);
    expect(mapSourceTimeToCut(123, ranges)).toBe(13);
    // Inside the dropped gap, and past the end.
    expect(mapSourceTimeToCut(90, ranges)).toBe(10);
    expect(mapSourceTimeToCut(600, ranges)).toBe(15);
  });

  it('leaves timestamps untouched when there is nothing to cut', () => {
    expect(mapSourceTimeToCut(42, [])).toBe(42);
  });

  it('re-times a silent intro away so the first cue starts at zero', () => {
    // The reported failure: a recording whose first 3:17 are silent. On the
    // source timeline the first cue is at 03:17.449; the cut export starts with
    // that cue, so exporting source timestamps runs 3:17 late against it.
    const segments = [
      segment('03:17.449', '03:21.289', 'first'),
      segment('03:21.289', '03:29.049', 'second'),
    ];

    const remapped = remapSegmentsToCutTimeline(segments, segments);

    expect(remapped[0].start).toBe('00:00.000');
    expect(remapped[0].end).toBe('00:03.840');
    expect(remapped[1].start).toBe('00:03.840');
    expect(remapped[1].end).toBe('00:11.600');
    expect(remapped[0].text).toBe('first');
  });

  it('collapses the gaps between segments, matching cut_video', () => {
    const segments = [segment('00:10.000', '00:20.000'), segment('01:00.000', '01:05.000')];

    const remapped = remapSegmentsToCutTimeline(segments, segments);

    expect(remapped.map((s) => [s.start, s.end])).toEqual([
      ['00:00.000', '00:10.000'],
      ['00:10.000', '00:15.000'],
    ]);
  });

  it('re-times translated cues through the cut segment list', () => {
    const cutSegments = [segment('00:10.000', '00:20.000'), segment('01:00.000', '01:10.000')];
    // A translation may merge cues; it still has to follow the cut timeline.
    const translated = [segment('00:10.000', '01:10.000', 'merged translation')];

    const remapped = remapSegmentsToCutTimeline(translated, cutSegments);

    expect(remapped[0].start).toBe('00:00.000');
    expect(remapped[0].end).toBe('00:20.000');
  });

  it('re-times word timings and keeps ends at or after starts', () => {
    const segments: TranscriptSegment[] = [
      {
        start: '01:00.000',
        end: '01:02.000',
        text: 'two words',
        speaker: 'Speaker 1',
        words: [
          { start: '01:00.000', end: '01:01.000', text: 'two' },
          { start: '01:01.000', end: '01:02.000', text: 'words' },
        ],
      },
    ];

    const remapped = remapSegmentsToCutTimeline(segments, segments);

    expect(remapped[0].words).toEqual([
      { start: '00:00.000', end: '00:01.000', text: 'two' },
      { start: '00:01.000', end: '00:02.000', text: 'words' },
    ]);
  });

  it('clamps inverted cues instead of emitting negative durations', () => {
    const cutSegments = [segment('00:10.000', '00:20.000')];
    const inverted = [segment('00:18.000', '00:12.000')];

    const remapped = remapSegmentsToCutTimeline(inverted, cutSegments);

    expect(remapped[0].start).toBe('00:08.000');
    expect(remapped[0].end).toBe('00:08.000');
  });
});
