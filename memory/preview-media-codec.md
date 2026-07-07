---
name: preview-media-codec
description: The audio preview must be a transcoded seekable AAC/m4a — WKWebView can't seek Opus/Ogg and often can't decode the source audio track
metadata:
  type: project
---

The transcript workspace audio scrubber (`src/components/TranscriptWorkspacePanel.vue`) must play a **transcoded, seekable AAC/m4a preview**, produced by the `prepare_preview_audio` Tauri command (`src-tauri/src/lib.rs`).

Why neither obvious source works, on macOS (Tauri = WKWebView):
- The **source file** (`inputPath`): WKWebView often cannot decode its audio track at all → both the `<video>` and an `<audio src=inputPath>` play silently. (This is the whole reason the app extracts audio.)
- The **extracted analysis audio** (`prepare_audio_for_ai` → Opus in Ogg, `-c:a libopus`): WKWebView *plays* it but cannot reliably *seek* it — it reports a bogus duration (observed ~14h for a ~1h file) and mis-seeks, so segment previews land at the wrong spot.

Fix (2026-07): `prepare_preview_audio` transcodes the already-extracted `.ogg` (fast; same original timeline, untrimmed) to `<stem>_preview.m4a` with `-c:a aac -movflags +faststart` (moov atom at front → immediate duration + smooth seeking). It caches by mtime. `Home.vue` calls it after `prepare_audio_for_ai` and sets `extractedAudioPath` to the m4a; `refreshExtractedAudioPath` looks for the sibling `<stem>_preview.m4a` on load. The Ogg/Opus analysis+upload pipeline (`gemini.rs`, `upload.rs`, `chunking.rs`, `silence.rs`, all `audio/ogg`) is deliberately left untouched — do NOT change it without testing the Gemini upload.

Known remaining limitation: the `<video>` preview still plays the source file, so if the source's audio codec is undecodable in WKWebView the video is silent; the separate audio scrubber (m4a) is the reliable audio source. The offset math itself is correct — see [[silence-offset-flow]].
