---
name: measure-before-optimizing
description: User expects profiling/measurement to locate hotspots before optimizing, not assumption-driven changes
metadata:
  type: feedback
---

When optimizing performance, measure *where* the time is actually spent (profile each phase) before changing code — don't optimize based on assumptions.

**Why:** On the hybrid transcript alignment work (2026-06), I first optimized the gap-DP based on a guess; it only bought ~15% because the real hotspots were elsewhere (anchor detection + boundary rebalancing, each ~half the time). A per-phase profiling harness revealed the true cost was a ~3µs-per-call similarity function invoked ~130k times.

**How to apply:** Add a phase-level timing harness (e.g. an `#[ignore]`d profiling test that times each pipeline stage and the per-call cost of the inner primitive) and read the numbers before and after each change. There is one at `transcript_merge::tests::profile_phases` (run with `--ignored --nocapture`) and a criterion bench at `benches/transcript_merge.rs`. See [[hybrid-alignment-perf]].
