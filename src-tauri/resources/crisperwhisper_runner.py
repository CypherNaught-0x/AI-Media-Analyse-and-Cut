#!/usr/bin/env python3
"""CrisperWhisper 2.0 bridge for AI Media Cutter.

The `crisperwhisper` package only ships PyTorch/CTranslate2 runtimes (there is
no ONNX or GGML export), so the model is driven out-of-process through this
script instead of natively in Rust.

Protocol: a single JSON request object on stdin, newline-delimited JSON
objects on stdout:

    {"type": "progress", "message": "..."}
    {"type": "result", ...}
    {"type": "error", "message": "...", "kind": "..."}

Anything the ML stack prints (download bars, warnings) is forced to stderr so
it can never corrupt the protocol stream.
"""

from __future__ import annotations

import json
import math
import os
import sys
import time
import traceback
import types
import wave

# A few ops lack Metal kernels in some torch releases; run those on the CPU
# rather than failing a GPU transcription. Must be set before torch loads.
os.environ.setdefault("PYTORCH_ENABLE_MPS_FALLBACK", "1")

# Claim the real stdout for the protocol before any heavy import can print to
# it, then point sys.stdout at stderr so library chatter is harmless.
_PROTOCOL_OUT = os.fdopen(os.dup(1), "w", encoding="utf-8", newline="\n")
os.dup2(2, 1)
sys.stdout = sys.stderr

# Filler words and vocal events are separate concepts to the caller, so the two
# lists stay separate. These are the bracketed tokens in the model's
# `added_tokens.json`; matching is case-insensitive because the model card and
# the vocabulary disagree on case ("[um]" vs "[UM]").
FILLER_TOKENS = frozenset({"[uh]", "[um]"})
VOCAL_EVENT_TOKENS = frozenset(
    {
        "[breath]",
        "[cough]",
        "[crying]",
        "[fart]",
        "[laughter]",
        "[lipsmack]",
        "[noise]",
        "[scream]",
        "[sigh]",
        "[sneeze]",
        "[sniff]",
        "[throatclearing]",
        "[yawn]",
    }
)
# Prompt-control tokens. They should never be decoded into output, but if the
# model emits one it must not reach the transcript.
CONTROL_TOKENS = frozenset(
    {"<ctx>", "<ectx>", "<ehtx>", "<evtx>", "<htx>", "<vtx>"}
)
CONTROL_TOKEN_PREFIXES = ("[verbatim_", "[intended_")

SUPPORTED_LANGUAGES = ("en", "de")

# crisperwhisper declares `requires_python = ">=3.10"`.
MINIMUM_PYTHON = (3, 10)

# The ct2 backend needs nyrahealth's CTranslate2 *fork*
# (`ctranslate2-crisperwhisper`), not upstream `ctranslate2`. Both import as
# `ctranslate2`, so importability alone would happily select a build that then
# fails at model load. These are the fork-only APIs crisperwhisper requires.
CT2_FORK_APIS = (
    "prefill",
    "forward_step",
    "set_alignment_heads",
    "generate_greedy_with_attention",
)


def ct2_fork_status() -> tuple[bool, str | None]:
    """Return (fork_usable, note). `note` explains an unusable install."""
    try:
        import ctranslate2
    except Exception:
        return False, None

    version = getattr(ctranslate2, "__version__", "unknown")
    try:
        whisper_class = ctranslate2.models.Whisper
    except Exception:
        return False, f"ctranslate2 {version} exposes no Whisper model."

    missing = [api for api in CT2_FORK_APIS if not hasattr(whisper_class, api)]
    if missing:
        return False, (
            f"ctranslate2 {version} is installed but is not the CrisperWhisper "
            f"fork (missing {', '.join(missing)}); the ct2 backend is unavailable."
        )

    return True, None


def load_hallucination_module() -> bool:
    """Import `crisperwhisper.hallucination`, also on a PyTorch-only install.

    crisperwhisper 2.0.1 imports `ctranslate2` at the top of that module, so on
    a PyTorch-only install it raises ModuleNotFoundError. Two *separate*
    decoding paths import it lazily, neither reachable at load time:

    * `generate_with_repair_and_attention` — gated by `hallucination_mitigation`
    * `decode_with_coverage_fallback` — gated by `temperature_fallback`, and
      only entered when a chunk trips the mel coverage pre-filter, which makes
      the crash look intermittent

    The PyTorch backend only uses the module's pure-Python loop detection
    (`find_token_loop`, `DEFAULT_REPAIR_THRESHOLDS`); `ctranslate2` appears
    there only in annotations (lazy under `from __future__ import annotations`)
    and in helpers the CT2 engine calls. So the module is imported against a
    placeholder `ctranslate2` that raises if anything does touch it, and the
    placeholder is removed again straight away so backend detection
    (`importlib.util.find_spec("ctranslate2")`) still sees no CT2.

    Returns False only if the module cannot be imported even so; both flags
    then have to be turned off together, decided up front rather than
    discovered after minutes of inference.
    """
    if "crisperwhisper.hallucination" in sys.modules:
        return True
    try:
        import crisperwhisper.hallucination  # noqa: F401
        return True
    except Exception:
        sys.modules.pop("crisperwhisper.hallucination", None)

    if "ctranslate2" in sys.modules:
        # A real (if unusable) ctranslate2 is already loaded; don't shadow it.
        return False

    placeholder = types.ModuleType("ctranslate2")

    def missing(name: str):
        raise ImportError(
            f"ctranslate2.{name} is unavailable: the CTranslate2 backend is not installed."
        )

    placeholder.__getattr__ = missing  # type: ignore[attr-defined]
    sys.modules["ctranslate2"] = placeholder
    try:
        import crisperwhisper.hallucination  # noqa: F401
        return True
    except Exception:
        sys.modules.pop("crisperwhisper.hallucination", None)
        return False
    finally:
        if sys.modules.get("ctranslate2") is placeholder:
            del sys.modules["ctranslate2"]


def emit(payload: dict) -> None:
    _PROTOCOL_OUT.write(json.dumps(payload, ensure_ascii=False) + "\n")
    _PROTOCOL_OUT.flush()


def progress(message: str) -> None:
    emit({"type": "progress", "message": message})


def fail(message: str, kind: str = "runtime", detail: str | None = None) -> None:
    payload = {"type": "error", "message": message, "kind": kind}
    if detail:
        payload["detail"] = detail
    emit(payload)
    sys.exit(1)


def describe_environment() -> dict:
    """Report what is installed without loading any model weights."""
    info: dict = {
        "python": sys.version.split()[0],
        "pythonPath": sys.executable,
        "pythonSupported": sys.version_info >= MINIMUM_PYTHON,
        "minimumPython": ".".join(str(part) for part in MINIMUM_PYTHON),
        "crisperwhisperVersion": None,
        "backends": [],
        "torchVersion": None,
        "cuda": False,
        "mps": False,
        "installed": False,
    }

    try:
        import crisperwhisper  # noqa: F401

        info["installed"] = True
        info["crisperwhisperVersion"] = getattr(
            crisperwhisper, "__version__", "unknown"
        )
    except Exception as error:  # pragma: no cover - depends on env
        info["importError"] = f"{type(error).__name__}: {error}"
        return info

    try:
        import torch

        info["torchVersion"] = torch.__version__
        info["cuda"] = bool(torch.cuda.is_available())
        info["mps"] = bool(
            getattr(torch.backends, "mps", None)
            and torch.backends.mps.is_available()
        )
        info["backends"].append("transformers")
    except Exception:
        pass

    fork_usable, fork_note = ct2_fork_status()
    if fork_usable:
        info["backends"].append("ct2")
    elif fork_note:
        info["ct2Note"] = fork_note

    info["hallucinationMitigation"] = load_hallucination_module()

    return info


# crisperwhisper releases the lean decoding engine has been validated against.
# It reuses the stock engine's tokenizer, features and private helpers, so an
# unknown release runs the stock engine instead.
FAST_ENGINE_VERSIONS = ("2.0.",)


def fast_engine_wanted(request: dict, env: dict, backend: str) -> bool:
    if backend != "transformers" or request.get("fastDecoding") is False:
        return False
    version = env.get("crisperwhisperVersion") or ""
    if not version.startswith(FAST_ENGINE_VERSIONS):
        progress(
            f"Fast decoding is not validated for crisperwhisper {version}; "
            "using the stock engine."
        )
        return False
    return True


def resolve_runtime(request: dict, env: dict) -> tuple[str, str, str, bool]:
    """Pick (backend, device, compute_type, fast_engine), filling in "auto".

    The package defaults to float16, which is unusably slow (and partly
    unimplemented) on CPU, so an "auto" compute type resolves to float32
    unless a GPU is actually going to be used.

    "auto" picks Apple's GPU (MPS) only with the fast engine: the stock engine
    needs eager attention for word timings and measured slower on MPS than on
    the CPU, while the fast engine on MPS float16 is ~2x faster than on the CPU
    with identical output (74 s English test clip and a 10 min German panel).
    """
    backend = (request.get("backend") or "auto").strip() or "auto"
    device = (request.get("device") or "auto").strip() or "auto"
    compute_type = (request.get("computeType") or "auto").strip() or "auto"

    available = env.get("backends") or []
    if backend == "auto":
        backend = "ct2" if "ct2" in available else "transformers"
    if backend not in available and available:
        progress(
            f"Backend '{backend}' is not installed; falling back to '{available[0]}'."
        )
        backend = available[0]

    fast = fast_engine_wanted(request, env, backend)

    if device == "auto":
        if env.get("cuda"):
            device = "cuda"
        elif fast and env.get("mps"):
            device = "mps"
        else:
            device = "cpu"

    if compute_type == "auto":
        compute_type = "float16" if device in ("cuda", "mps") else "float32"

    return backend, device, compute_type, fast


def is_filler(token: str) -> bool:
    return token.strip().lower() in FILLER_TOKENS


def is_vocal_event(token: str) -> bool:
    return token.strip().lower() in VOCAL_EVENT_TOKENS


def is_control_token(token: str) -> bool:
    lowered = token.strip().lower()
    return lowered in CONTROL_TOKENS or lowered.startswith(CONTROL_TOKEN_PREFIXES)


def collect_words(result) -> list[dict]:
    """Normalise `result.words` into JSON-safe dicts, tagging each word.

    Timings are preserved for every word, including fillers, so the caller can
    cut them out of the video rather than only out of the text.
    """
    words = getattr(result, "words", None) or []
    collected: list[dict] = []

    for word in words:
        text = (getattr(word, "word", None) or "").strip()
        if not text or is_control_token(text):
            continue

        start = getattr(word, "start", None)
        end = getattr(word, "end", None)
        if start is None or end is None:
            continue

        collected.append(
            {
                "text": text,
                "start": float(start),
                "end": float(end),
                "filler": is_filler(text),
                "vocalEvent": is_vocal_event(text),
            }
        )

    return collected


def load_model(model_name, backend, device, compute_type, cache_dir, engine_class=None):
    """Load the model, optionally with `engine_class` in place of the stock
    `TransformersEngine` (which `CrisperWhisperModel` looks up at load time).

    Failing to load the stock engine ends the run; failing with `engine_class`
    raises, so the caller can fall back to the stock engine."""
    from crisperwhisper import CrisperWhisperModel

    progress(
        f"Loading CrisperWhisper '{model_name}' "
        f"({backend} backend, {device}, {compute_type})..."
    )
    model_kwargs = {"backend": backend, "device": device, "compute_type": compute_type}
    if cache_dir:
        model_kwargs["cache_dir"] = cache_dir

    stock = None
    if engine_class is not None:
        import crisperwhisper.transformers_engine as engines

        stock = engines.TransformersEngine
        engines.TransformersEngine = engine_class
    try:
        return CrisperWhisperModel(model_name, **model_kwargs)
    except Exception as error:
        if engine_class is not None:
            raise
        fail(
            f"Failed to load CrisperWhisper model '{model_name}'.",
            kind="model_load",
            detail=f"{type(error).__name__}: {error}",
        )
    finally:
        if stock is not None:
            engines.TransformersEngine = stock


# Relative logit error above which the fast engine is not trusted. Real layout
# mismatches are O(1); float16 rounding measured ~1e-3.
FAST_ENGINE_TOLERANCE = 1e-2


def is_auto(value) -> bool:
    return (value or "auto").strip() in ("", "auto")


def load_fast_model(model_name, backend, device, compute_type, cache_dir):
    """Load with the fast decoding engine.

    Returns `(model, engine)` with engine "fast" or "stock", or `(None, "stock")`
    when the caller should load the stock engine itself."""
    try:
        import crisperwhisper_fast_engine

        engine_class = crisperwhisper_fast_engine.build_engine_class()
    except Exception as error:
        progress(f"Fast decoding unavailable ({type(error).__name__}: {error}); using the stock engine.")
        return None, "stock"

    try:
        model = load_model(model_name, backend, device, compute_type, cache_dir, engine_class)
    except Exception as error:
        progress(
            f"Fast decoding failed to load ({type(error).__name__}: {error}); "
            "using the stock engine."
        )
        return None, "stock"
    engine = getattr(model, "_engine", None)
    if not isinstance(engine, engine_class):
        # e.g. a legacy v1 checkpoint, which has its own pipeline.
        return model, "stock"
    try:
        error = engine.self_check()
    except Exception as failure:
        error = float("inf")
        progress(f"Fast decoding self-check failed ({type(failure).__name__}: {failure}).")
    if not error <= FAST_ENGINE_TOLERANCE:
        progress(
            f"Fast decoding disagrees with the reference decoder (error {error:.3g}); "
            "reloading with the stock engine."
        )
        del model, engine
        return None, "stock"
    return model, "fast"


def audio_duration(path: str) -> float:
    """Duration of the WAV the app hands over, without decoding it."""
    try:
        with wave.open(path, "rb") as handle:
            return handle.getnframes() / float(handle.getframerate())
    except Exception:
        return 0.0


def format_clock(seconds: float) -> str:
    seconds = int(round(seconds))
    hours, rest = divmod(seconds, 3600)
    minutes, seconds = divmod(rest, 60)
    if hours:
        return f"{hours}:{minutes:02d}:{seconds:02d}"
    return f"{minutes}:{seconds:02d}"


# The default continuation longform strategy: 30 s windows every 26 s.
LONGFORM_WINDOW_S = 30.0
LONGFORM_STRIDE_S = 26.0


def report_window_progress(model, duration: float, label: str) -> None:
    """Report progress per decoded 30 s window.

    Counts feature extractions, which happen once per window (a rare coverage
    fallback re-extracts, hence the clamp), and estimates the time left from
    the windows done so far.
    """
    engine = getattr(model, "_engine", None)
    extract = getattr(engine, "extract_features_with_mel", None)
    if extract is None or duration <= 0:
        progress(f"{label}... this runs locally and can take a while.")
        return

    total = 1 + max(0, math.ceil((duration - LONGFORM_WINDOW_S) / LONGFORM_STRIDE_S))
    started = time.monotonic()
    state = {"done": 0}

    def counting(*args, **kwargs):
        done = min(state["done"], total - 1)
        message = f"{label}: window {done + 1} of {total}"
        if done:
            remaining = (time.monotonic() - started) / done * (total - done)
            message += f", about {format_clock(remaining)} left"
        progress(message + "...")
        state["done"] += 1
        return extract(*args, **kwargs)

    engine.extract_features_with_mel = counting


def transcribe(request: dict) -> None:
    audio_path = request.get("audioPath")
    if not audio_path or not os.path.isfile(audio_path):
        fail(f"Audio file not found: {audio_path}", kind="input")

    language = (request.get("language") or "en").strip().lower()
    if language not in SUPPORTED_LANGUAGES:
        fail(
            "CrisperWhisper 2.0 is published for English and German only; "
            f"got language '{language}'.",
            kind="input",
        )

    mode = (request.get("mode") or "verbatim").strip().lower()
    if mode not in ("verbatim", "intended"):
        fail(f"Unsupported mode '{mode}'; expected 'verbatim' or 'intended'.", kind="input")

    env = describe_environment()
    if not env.get("installed"):
        fail(
            "The 'crisperwhisper' package is not installed in this environment.",
            kind="missing_package",
            detail=env.get("importError"),
        )
    if not env.get("backends"):
        fail(
            "No inference backend is installed. Install the 'transformers' "
            "extra (portable) or the 'ct2' extra (Linux + NVIDIA).",
            kind="missing_backend",
        )

    backend, device, compute_type, fast = resolve_runtime(request, env)
    model_name = (request.get("model") or "large").strip() or "large"

    try:
        from crisperwhisper import CrisperWhisperModel  # noqa: F401
    except Exception as error:
        fail(
            "Failed to import CrisperWhisper.",
            kind="missing_package",
            detail=f"{type(error).__name__}: {error}",
        )

    cache_dir = request.get("cacheDir")
    model, engine = None, "stock"
    if fast:
        model, engine = load_fast_model(model_name, backend, device, compute_type, cache_dir)
    if model is None:
        if fast and device == "mps" and is_auto(request.get("device")):
            # Only the fast engine made MPS the automatic choice; the stock
            # engine is slower there than on the CPU.
            device = "cpu"
            if is_auto(request.get("computeType")):
                compute_type = "float32"
        model = load_model(model_name, backend, device, compute_type, cache_dir)

    transcribe_kwargs = {
        "language": language,
        "mode": mode,
        "word_timestamps": bool(request.get("wordTimestamps", True)),
    }

    # Normally importable even without ctranslate2 (see
    # `load_hallucination_module`); if it still is not, transcribing without
    # both safeguards beats failing after minutes of inference.
    if not load_hallucination_module():
        transcribe_kwargs["hallucination_mitigation"] = False
        transcribe_kwargs["temperature_fallback"] = False
        progress(
            "Note: crisperwhisper's hallucination module could not be "
            "imported; continuing without repetition repair and temperature "
            "fallback."
        )
    hotwords = request.get("hotwords") or []
    if hotwords:
        # Honoured by Pro models only; standard models warn and ignore it.
        transcribe_kwargs["hotwords"] = list(hotwords)

    duration = audio_duration(audio_path)
    report_window_progress(model, duration, f"Transcribing in {mode} mode ({language})")

    started = time.monotonic()
    try:
        result = model.transcribe(audio_path, **transcribe_kwargs)
    except Exception as error:
        fail(
            "CrisperWhisper transcription failed.",
            kind="inference",
            detail=f"{type(error).__name__}: {error}",
        )
    elapsed = time.monotonic() - started
    if duration and elapsed > 0:
        progress(
            f"Transcribed {format_clock(duration)} of audio in "
            f"{format_clock(elapsed)} ({duration / elapsed:.1f}x realtime, "
            f"{device} {compute_type})."
        )

    words = collect_words(result)
    if transcribe_kwargs["word_timestamps"] and not words:
        progress(
            "Warning: the model returned no word timings; "
            "segment timings will be coarse."
        )

    emit(
        {
            "type": "result",
            "text": getattr(result, "text", "") or "",
            "language": getattr(result, "language", language) or language,
            "mode": getattr(result, "mode", mode) or mode,
            "duration": float(getattr(result, "duration", 0.0) or 0.0),
            "processingTime": float(getattr(result, "processing_time", 0.0) or 0.0),
            "backend": backend,
            "device": device,
            "computeType": compute_type,
            "engine": engine,
            "hallucinationMitigation": transcribe_kwargs.get("hallucination_mitigation", True),
            "model": model_name,
            "words": words,
        }
    )


def main() -> None:
    raw = sys.stdin.read()
    try:
        request = json.loads(raw) if raw.strip() else {}
    except json.JSONDecodeError as error:
        fail(f"Invalid request JSON: {error}", kind="protocol")
        return

    action = (request.get("action") or "transcribe").strip().lower()

    if action == "probe":
        env = describe_environment()
        env["type"] = "result"
        emit(env)
        return

    if action == "transcribe":
        transcribe(request)
        return

    fail(f"Unknown action '{action}'.", kind="protocol")


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as error:  # pragma: no cover - last-resort guard
        emit(
            {
                "type": "error",
                "message": f"Unhandled error: {type(error).__name__}: {error}",
                "kind": "runtime",
                "detail": traceback.format_exc(),
            }
        )
        sys.exit(1)
