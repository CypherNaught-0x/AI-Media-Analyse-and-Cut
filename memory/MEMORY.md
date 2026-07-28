# Memory index

- [Measure before optimizing](measure-before-optimizing.md) — profile each phase to find hotspots before changing code
- [Hybrid alignment perf](hybrid-alignment-perf.md) — where transcript-merge time goes; build phase is the next lever
- [Silence offset flow](silence-offset-flow.md) — how trim-silence timestamp offsets are recalculated per mode (verified correct)
- [Subtitle export timelines](subtitle-export-timelines.md) — source vs `_cut` timeline; what "subtitles are N minutes late" actually means
- [Preview media codec](preview-media-codec.md) — preview players must use the original file, not the extracted Opus/Ogg (WKWebView can't seek it)
