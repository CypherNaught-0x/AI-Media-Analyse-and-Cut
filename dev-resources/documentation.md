# **Documentation**

## **FFmpeg Sidecar**

[FFmpeg Sidecar Documentation](https://docs.rs/ffmpeg-sidecar/2.3.0/ffmpeg_sidecar/)

## **Transcript Blacklists**

Transcript blacklist resources are stored in `src/assets/transcript-blacklists/` and loaded automatically by the frontend.

Guidelines:

*   Use one file per language, named by language code, such as `de.txt`.
*   Use one term per line.
*   Matching is word-level only. Substring matches do not trigger warnings.
*   Multi-word lines are ignored by the current matcher.

Warnings appear in the transcript workspace summary and on each affected segment, and blacklist hits are included in the existing review filter.
