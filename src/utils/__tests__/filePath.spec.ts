import { describe, it, expect } from 'vitest';
import { appendFileNameSuffix } from '../filePath';

describe('appendFileNameSuffix', () => {
  it('inserts the suffix before the file extension', () => {
    expect(appendFileNameSuffix('/videos/talk.mp4', '_cut')).toBe('/videos/talk_cut.mp4');
  });

  it('does not overwrite the source path', () => {
    const input = '/videos/talk.mp4';
    expect(appendFileNameSuffix(input, '_cut')).not.toBe(input);
  });

  it('handles multi-character extensions', () => {
    expect(appendFileNameSuffix('/a/b/clip.webm', '_cut')).toBe('/a/b/clip_cut.webm');
  });

  it('ignores dots in parent directory names', () => {
    expect(appendFileNameSuffix('/my.folder/video.mov', '_cut')).toBe('/my.folder/video_cut.mov');
  });

  it('handles Windows-style backslash paths', () => {
    expect(appendFileNameSuffix('C:\\Users\\me\\video.mkv', '_cut')).toBe('C:\\Users\\me\\video_cut.mkv');
  });

  it('appends the suffix when there is no extension', () => {
    expect(appendFileNameSuffix('/videos/talk', '_cut')).toBe('/videos/talk_cut');
  });
});
