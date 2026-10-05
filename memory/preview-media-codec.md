---
name: preview-media-codec
description: Silent source previews came from asset:// capping range responses at 1000 KiB, not from codecs; previews use the media:// protocol; Opus/Ogg still can't be seeked, so audio previews are AAC/m4a
metadata:
  type: project
---

**Silent `<video>` previews (root cause found 2026-10-05).** Tauri's built-in `asset://` protocol cuts every range response to 1000 KiB (`MAX_LEN` in `tauri/src/protocol/asset.rs`). WebKit's MP4 demuxer requests each box of the `moov` index with one explicit range and drops a track when the answer comes back short. Long recordings have a large `moov` (the user's 777 MB / ~1 h H.264+AAC file: 3.9 MB, already at the front), so the source played with no audio. The earlier explanation, "WKWebView often can't decode the source's audio", was wrong for normal AAC sources.

**Fix:** all players load through the app's own `media://` protocol (`src-tauri/src/media_protocol.rs`, frontend `utils/mediaUrl.ts` → `convertFileSrc(path, 'media')`). It answers explicit ranges in full (up to 64 MiB) and open-ended ones in 4 MiB chunks. It only serves folders granted via `allow_media_access` plus the media cache. Don't switch players back to `asset://`.

**Still true:** WKWebView can't reliably *seek* Opus/Ogg (bogus duration, mis-seeks), so the audio scrubber plays the transcoded AAC preview `analysis_preview.m4a` from `prepare_preview_audio`. It now lives in the media cache (see `media_cache.rs`; `cached_preview_audio` finds it on load). The Ogg/Opus analysis + upload pipeline (`gemini.rs`, `upload.rs`, `chunking.rs`, `silence.rs`) is deliberately unchanged; don't change it without testing the Gemini upload.

**How to apply:** when a preview is silent or a track is missing, check the container's box layout and `moov` size before blaming codecs (a short Python box walker or `ffprobe` works), and make sure playback goes through `media://`. The offset math is separate and correct; see [[silence-offset-flow]].
