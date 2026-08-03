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
import os
import sys
import traceback

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


def hallucination_module_available() -> bool:
    """Whether `crisperwhisper.hallucination` can be imported at all.

    crisperwhisper 2.0.1 imports `ctranslate2` at the top of that module, so on
    a PyTorch-only install it raises ModuleNotFoundError. Two *separate*
    decoding paths import it lazily, neither reachable at load time:

    * `generate_with_repair_and_attention` — gated by `hallucination_mitigation`
    * `decode_with_coverage_fallback` — gated by `temperature_fallback`, and
      only entered when a chunk trips the mel coverage pre-filter, which makes
      the crash look intermittent

    Both flags therefore have to be turned off together, and it has to be
    decided up front rather than discovered after minutes of inference.
    """
    try:
        import crisperwhisper.hallucination  # noqa: F401
    except Exception:
        return False
    return True


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

    info["hallucinationMitigation"] = hallucination_module_available()

    return info


def resolve_runtime(request: dict, env: dict) -> tuple[str, str, str]:
    """Pick (backend, device, compute_type), filling in "auto" sensibly.

    The package defaults to float16, which is unusably slow (and partly
    unimplemented) on CPU, so an "auto" compute type resolves to float32
    unless a CUDA device is actually going to be used.
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

    if device == "auto":
        device = "cuda" if env.get("cuda") else "cpu"

    if compute_type == "auto":
        compute_type = "float16" if device == "cuda" else "float32"

    return backend, device, compute_type


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

    backend, device, compute_type = resolve_runtime(request, env)
    model_name = (request.get("model") or "large").strip() or "large"

    try:
        from crisperwhisper import CrisperWhisperModel
    except Exception as error:
        fail(
            "Failed to import CrisperWhisper.",
            kind="missing_package",
            detail=f"{type(error).__name__}: {error}",
        )

    progress(
        f"Loading CrisperWhisper '{model_name}' "
        f"({backend} backend, {device}, {compute_type})..."
    )

    model_kwargs = {
        "backend": backend,
        "device": device,
        "compute_type": compute_type,
    }
    cache_dir = request.get("cacheDir")
    if cache_dir:
        model_kwargs["cache_dir"] = cache_dir

    try:
        model = CrisperWhisperModel(model_name, **model_kwargs)
    except Exception as error:
        fail(
            f"Failed to load CrisperWhisper model '{model_name}'.",
            kind="model_load",
            detail=f"{type(error).__name__}: {error}",
        )

    progress(
        f"Transcribing in {mode} mode ({language})... "
        "this runs locally and can take a while."
    )

    transcribe_kwargs = {
        "language": language,
        "mode": mode,
        "word_timestamps": bool(request.get("wordTimestamps", True)),
    }

    # Guard the known crisperwhisper 2.0.1 packaging bug: both the repair path
    # and the temperature-fallback path import `ctranslate2` even on the
    # PyTorch backend. Transcribing without them is far better than failing
    # after minutes of inference.
    if not hallucination_module_available():
        transcribe_kwargs["hallucination_mitigation"] = False
        transcribe_kwargs["temperature_fallback"] = False
        progress(
            "Note: hallucination mitigation and temperature fallback are "
            "unavailable in this environment (crisperwhisper imports "
            "ctranslate2 for both); continuing without them."
        )
    hotwords = request.get("hotwords") or []
    if hotwords:
        # Honoured by Pro models only; standard models warn and ignore it.
        transcribe_kwargs["hotwords"] = list(hotwords)

    try:
        result = model.transcribe(audio_path, **transcribe_kwargs)
    except Exception as error:
        fail(
            "CrisperWhisper transcription failed.",
            kind="inference",
            detail=f"{type(error).__name__}: {error}",
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
