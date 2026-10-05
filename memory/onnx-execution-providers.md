---
name: onnx-execution-providers
description: CoreML is ~5x slower than CPU for Parakeet/Sortformer ONNX models; CPU with tuned threads is the measured best
metadata:
  node_type: memory
  type: project
  originSessionId: 88193af7-dc69-48f0-b721-1dbe4ca1e48b
  modified: 2026-10-05T17:35:06.360Z
---

Measured 2026-10-05 (M-series Mac, 74 s test recording, release build, `parakeet::profiling` ignored test): Parakeet TDT int8 runs ~37x realtime on CPU with 6–8 intra-op threads vs ~31x at parakeet-rs' default 4; the CoreML execution provider ran at ~6.6x (5x slower) and slightly changed the output. Cause: dynamic input shapes prevent CoreML from building ANE/GPU plans (parakeet-rs documents the same).

**Why:** the review plan assumed "enable CoreML EP" would speed up local ASR; it doesn't for these models. Parakeet is already fast — slowness users report on Apple Silicon comes from CrisperWhisper (PyTorch, deliberately CPU because MPS measured slower for word timings).

**How to apply:** keep `onnx_execution_config()` (local_asr.rs) on CPU; re-measure with `cargo test --release --features profile-coreml --lib profiling -- --ignored --nocapture` before trying EPs again, and measure any new vision models (YuNet etc. have static shapes and may behave differently). See [[measure-before-optimizing]].
