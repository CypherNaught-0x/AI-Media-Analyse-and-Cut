import { convertFileSrc } from '@tauri-apps/api/core';

/**
 * URL for playing a local file in <video>/<audio>. Uses the app's media://
 * protocol instead of asset://, which caps range responses at 1000 KiB: for
 * long recordings WebKit then can't read the MP4 index and drops the audio
 * track (see src-tauri/src/media_protocol.rs).
 */
export function mediaUrl(path: string): string {
    return convertFileSrc(path, 'media');
}
