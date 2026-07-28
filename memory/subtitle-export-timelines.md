---
name: subtitle-export-timelines
description: Two incompatible timelines exist for exports — source media vs the _cut file — and this is what "subtitles are N minutes late" really means
metadata:
  type: project
---

Exports live on **two different timelines**, and a report of "subtitles off by N minutes (late)" almost always means the wrong one was used, not a broken offset table:

- **Source timeline** — every transcript timestamp, including after silence trimming (remapped back by [[silence-offset-flow]]). Matches the file the user selected.
- **Cut timeline** — `cut_video` (`src-tauri/src/video.rs`) concatenates *only* the transcript segment ranges, dropping a silent intro, the tail, and every inter-segment gap. The first cue therefore sits at 00:00 in `<name>_cut.mp4`.

**Why:** diagnosed 2026-07-28 on `~/Downloads/2026-07-23 15-56-55.mp4` (a 4517 s recording with a silent 3:17 pre-show). The offset table was verified *correct* against the user's own files — silencedetect found silence up to 197.05 s, raw Parakeet 00:00.400 mapped to 03:17.449 — but the exported SRT was played against `_cut.mp4` (4265.8 s ≈ source − all silence), where the same speech is at 00:00. Hence a constant ~3:17 lateness.

**How to apply:** when subtitle timing is reported as shifted, first ask/check which file the subtitles were played against, and compare `ffprobe` durations of the source vs `_cut` vs `_nosilence.ogg` — a constant shift equal to the leading silence points at the cut export, a *growing* shift points at the offset table. The fix shipped: a **Timeline** selector in `src/components/SubtitleExport.vue` (`source` | `cut`) with `src/utils/subtitleTimeline.ts` doing the piecewise remap, writing `<name>_cut.srt` so players auto-load it beside the cut video.
