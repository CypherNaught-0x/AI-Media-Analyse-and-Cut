---
name: silence-offset-flow
description: How silence-trim timestamp offsets are recalculated per transcription mode (and why it's correct)
metadata:
  type: project
---

When trim-silence is on, transcription runs on the *trimmed* audio, so all timestamps come back in trimmed time and must be remapped to the original timeline. The remap is applied in TWO different places depending on mode — this split is non-obvious and easy to misread as a missing adjust:

- **LLM-only** (`llm` backend): remapped *inside* `analyzeWithLlmTranscript` via the `silenceAdjuster`/`buildChunkAdjuster` per chunk (`src/views/Home.vue`). It does NOT hit the end-of-branch adjust.
- **parakeet / hybrid / hybrid-merge**: Parakeet and the remote reference are BOTH left in trimmed time; `merge_transcript_hypotheses` preserves that timeline; the merged result is remapped exactly ONCE at the end of the non-LLM branch via `adjustSegmentsWithOffsets` (`Home.vue`, ~line 1091). So the reference deliberately is not remapped before the merge — both inputs share the trimmed timeline and the single post-merge adjust covers everything.

Both paths therefore adjust exactly once. Verified correct 2026-07; the offset helpers were extracted to `src/utils/transcriptOffsets.ts` with tests in `src/utils/__tests__/transcriptOffsets.spec.ts` (`adjustTimestamp` lives in `src/composables/useTimeFormat.ts`).

Offset table (`remove_silence` in `src-tauri/src/silence.rs`): entries sorted by trimmed-time `min_time`; `offset` is removed-silence to add back. `adjustTimestamp` picks the last entry with `min_time <= t`, so a timestamp exactly on a seam maps forward into post-silence time (a segment spanning a cut spans the gap in original time) — intended, and identical across all modes. The keep segments feed both the ffmpeg filtergraph and the table, so trimmed audio and mapping cannot disagree; `merge_silence_intervals`/`plan_keep_segments` drop sub-`MIN_KEEP_SECONDS` slivers (silencedetect splits one silence into two ~100µs apart, which used to add a bogus first entry). See [[subtitle-export-timelines]] before believing a report that trim-silence subtitles are shifted — the usual cause is the cut export, not this table. Also [[hybrid-alignment-perf]].
