---
name: hybrid-alignment-perf
description: Where hybrid transcript-alignment time goes and what has/hasn't been optimized
metadata:
  type: project
---

`src-tauri/src/transcript_merge.rs` hybrid alignment (Parakeet ↔ remote). Pipeline phases and their cost characteristics after the 2026-06 optimization pass:

- **detect_alignment_anchors** — was the #1 cost; fixed with precomputed `NormalizedText` (chars + sorted tokens) and a token-overlap pre-prune (combined score ≤ `0.75 + 0.25·token`, so pairs that can't reach the 0.94 anchor threshold skip Levenshtein). ~20x faster.
- **rebalance_adjacent_boundaries** — was the #2 cost; fixed with incremental prefix/suffix normalization, a provably-exact "skip search when original boundary scores ≥1.97" early-out, and a per-side similarity-floor prune in the ±8 search. ~8x faster.
- **gap DP (`compute_alignment_window`)** — small once anchors split the work; uses a precomputed group table + banded Levenshtein (`bounded_levenshtein`).
- **build (`build_matched_segments`/`merge_words`)** — NOT yet optimized; clones every word/text. It is the dominant linear cost at very large transcripts (≈195ms at 4000 segments). The next lever is to make `materialize_alignment` consume the owned input instead of cloning (avoid `merge_words` word clones).

All optimizations preserve byte-identical output (54 lib unit tests pass + new exactness tests vs naive Levenshtein/Jaccard). End-to-end ≈10x at 1000 segments, ≈7x at 4000 (build dominates there). See [[measure-before-optimizing]].
