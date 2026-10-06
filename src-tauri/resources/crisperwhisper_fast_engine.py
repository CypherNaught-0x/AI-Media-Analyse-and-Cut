"""Lean decode loop for CrisperWhisper's PyTorch backend.

The stock `TransformersEngine` decodes through HuggingFace `generate()`, which
on this model spends most of its time on work the transcript never uses:

* every `generate()` call re-runs the 32-layer encoder on the same 30 s window
  (the repair, early-EOT and fallback paths call it several times per window),
  and Whisper's `generate()` additionally runs language detection -- another
  encoder pass -- although the prompt already forces the language;
* word timings need cross-attention from ~10 alignment heads, but asking HF for
  attention forces eager attention in *every* layer and returns all 32 layers x
  20 heads x 1500 frames per token, which are then stacked and copied.

`FastTransformersEngine` keeps the stock engine's tokenizer, feature extraction
and public methods, and replaces only the decoding primitives with a small
Whisper decoder written against the model's own weights: one encoder pass per
window, a static KV cache, SDPA attention, and attention weights computed only
for the alignment heads. Decoding semantics (forced prompt, suppression,
first-step bans, EOT handling, max_new_tokens) mirror the stock methods.

The loop depends on the Whisper module layout (`q_proj`, `encoder_attn`, ...),
so `self_check()` compares its logits against HuggingFace's own decoder at load
and the runner falls back to the stock engine if they disagree.
"""

from __future__ import annotations

import logging

import numpy as np

logger = logging.getLogger(__name__)


def build_engine_class():
    """Return the engine class; imports torch lazily like the stock engine."""
    import torch
    import torch.nn.functional as F
    from crisperwhisper.transformers_engine import TransformersEngine

    class FastTransformersEngine(TransformersEngine):
        """Drop-in `TransformersEngine` with a single-pass decoding loop."""

        def __init__(
            self,
            model_name_or_path,
            device: str = "auto",
            device_index: int = 0,
            compute_type: str = "float16",
        ):
            int8 = str(compute_type).startswith("int8")
            super().__init__(
                model_name_or_path,
                device=device,
                device_index=device_index,
                # The stock engine maps int8* to float16, which is very slow on
                # a CPU; there the int8 weights run with float32 activations.
                compute_type="float32" if int8 and device == "cpu" else compute_type,
            )
            model = self.model
            # Our own loop computes alignment-head attention itself, so the HF
            # modules (only the encoder still runs through them) can use SDPA.
            _use_sdpa(model)

            decoder = model.model.decoder
            config = model.config
            self._layers = list(decoder.layers)
            self._embed_tokens = decoder.embed_tokens
            self._embed_positions = decoder.embed_positions.weight
            self._final_norm = decoder.layer_norm
            self._proj_out = model.proj_out
            self._n_heads = int(config.decoder_attention_heads)
            self._d_model = int(config.d_model)
            self._head_dim = self._d_model // self._n_heads
            self._max_positions = int(config.max_target_positions)

            # Q/K/V of each self-attention fused into one matmul. The HF
            # projections are re-pointed at slices of the fused weights, so the
            # fusion costs no extra memory and the HF decoder (self_check, beam
            # search fallback) still computes with the very same numbers.
            # (no_grad, not inference_mode: parameters can't hold inference
            # tensors.)
            with torch.no_grad():
                self._self_qkv = []
                for layer in self._layers:
                    attention = layer.self_attn
                    projections = (attention.q_proj, attention.k_proj, attention.v_proj)
                    weight = torch.cat([projection.weight for projection in projections])
                    bias = torch.cat([
                        attention.q_proj.bias,
                        torch.zeros_like(attention.v_proj.bias),
                        attention.v_proj.bias,
                    ])
                    for index, projection in enumerate(projections):
                        rows = slice(index * self._d_model, (index + 1) * self._d_model)
                        projection.weight.data = weight[rows]
                        if projection.bias is not None:
                            projection.bias.data = bias[rows]
                    self._self_qkv.append((weight, bias))

            # "int8*" precision means int8 weights with float compute (as on
            # CTranslate2): the decoder's matmuls read a half (GPU, float16) or
            # a quarter (CPU, float32) of the bytes, and memory bandwidth is
            # what bounds per-token decoding. The encoder and the logits
            # projection keep their float weights.
            self.quantized = int8 and hasattr(torch, "_weight_int8pack_mm")
            with torch.no_grad():
                self._projections = []
                for index, layer in enumerate(self._layers):
                    project = lambda weight, bias: _Projection(weight, bias, self.quantized)
                    self._projections.append({
                        "qkv": project(*self._self_qkv[index]),
                        "self_out": project(layer.self_attn.out_proj.weight, layer.self_attn.out_proj.bias),
                        "cross_q": project(layer.encoder_attn.q_proj.weight, layer.encoder_attn.q_proj.bias),
                        "cross_out": project(layer.encoder_attn.out_proj.weight, layer.encoder_attn.out_proj.bias),
                        "fc1": project(layer.fc1.weight, layer.fc1.bias),
                        "fc2": project(layer.fc2.weight, layer.fc2.bias),
                    })

            shape = (1, self._n_heads, self._max_positions, self._head_dim)
            dtype = next(model.parameters()).dtype
            self._self_k = [
                torch.zeros(shape, dtype=dtype, device=self.device)
                for _ in self._layers
            ]
            self._self_v = [
                torch.zeros(shape, dtype=dtype, device=self.device)
                for _ in self._layers
            ]

            # Encoder output + per-layer cross-attention K/V for the current
            # window, keyed by the features tensor itself (held, so the
            # identity check cannot be fooled by a recycled id).
            self._cached_features = None
            self._cached_cross: list | None = None
            self._cached_align: dict = {}
            self._head_indices: dict = {}

        # --------------------------------------------------------------
        # Encoder (one pass per window) and the decoder forward.
        # --------------------------------------------------------------

        def _cross_kv(self, features):
            if self._cached_features is features and self._cached_cross is not None:
                return self._cached_cross

            with torch.inference_mode():
                encoded = self.model.model.encoder(features).last_hidden_state
                cross = []
                for layer in self._layers:
                    attention = layer.encoder_attn
                    k = attention.k_proj(encoded)
                    v = attention.v_proj(encoded)
                    cross.append((self._split_heads(k), self._split_heads(v)))

            self._cached_features = features
            self._cached_cross = cross
            self._cached_align = {}
            return cross

        def _head_index(self, heads):
            """Device index tensor for `heads`; indexing a device tensor with a
            Python list would copy the list to the device (and sync) per call."""
            key = tuple(heads)
            index = self._head_indices.get(key)
            if index is None:
                index = torch.tensor(key, dtype=torch.long, device=self.device)
                self._head_indices[key] = index
            return index

        def _alignment_keys(self, plan, cross):
            """Float32 keys of the alignment heads, transposed for q @ k."""
            key = tuple(sorted((layer, tuple(heads)) for layer, heads in plan.items()))
            cached = self._cached_align.get(key)
            if cached is None:
                cached = {
                    layer: (
                        self._head_index(heads),
                        cross[layer][0][0]
                        .index_select(0, self._head_index(heads))
                        .float()
                        .transpose(-1, -2)
                        .contiguous(),
                    )
                    for layer, heads in plan.items()
                }
                self._cached_align[key] = cached
            return cached

        def _split_heads(self, x):
            batch, length, _ = x.shape
            return (
                x.view(batch, length, self._n_heads, self._head_dim)
                .transpose(1, 2)
                .contiguous()
            )

        def _merge_heads(self, x):
            batch, _, length, _ = x.shape
            return x.transpose(1, 2).reshape(batch, length, self._d_model)

        def _forward(
            self, tokens, position, cross, capture=None, all_logits=False, quantized=True,
        ):
            """Run the decoder over `tokens` starting at `position`.

            Prefill/teacher-forcing always starts at position 0, so the only
            causal mask ever needed is the plain lower-triangular one.

            `capture` maps layer -> (head index, float32 alignment keys), see
            `_alignment_keys`; the head-summed attention rows
            ([len(tokens), frames], float32) of those layers are returned
            alongside the logits. `quantized=False` uses the float weights even
            when int8 ones exist (for the self-check).
            """
            length = tokens.shape[1]
            end = position + length
            causal = length > 1
            if causal and position != 0:
                raise ValueError("multi-token decode must start at position 0")

            hidden = self._embed_tokens(tokens) + self._embed_positions[position:end]
            captured = None
            heads_shape = (1, length, 3, self._n_heads, self._head_dim)

            for index, layer in enumerate(self._layers):
                project = self._projections[index]

                normed = layer.self_attn_layer_norm(hidden)
                qkv = project["qkv"](normed, quantized).view(heads_shape).permute(2, 0, 3, 1, 4)
                self._self_k[index][:, :, position:end] = qkv[1]
                self._self_v[index][:, :, position:end] = qkv[2]
                # HF scales the query before attention; Whisper's scaling is
                # head_dim**-0.5 = 2**-3, so this is exact in any precision.
                out = F.scaled_dot_product_attention(
                    qkv[0] * layer.self_attn.scaling,
                    self._self_k[index][:, :, :end],
                    self._self_v[index][:, :, :end],
                    is_causal=causal,
                    scale=1.0,
                )
                hidden = hidden + project["self_out"](self._merge_heads(out), quantized)

                normed = layer.encoder_attn_layer_norm(hidden)
                q = (project["cross_q"](normed, quantized) * layer.encoder_attn.scaling).view(
                    1, length, self._n_heads, self._head_dim
                ).transpose(1, 2)
                cross_k, cross_v = cross[index]
                out = F.scaled_dot_product_attention(q, cross_k, cross_v, scale=1.0)
                hidden = hidden + project["cross_out"](self._merge_heads(out), quantized)

                if capture is not None and index in capture:
                    heads, keys = capture[index]
                    queries = q[0].index_select(0, heads).float()
                    rows = torch.softmax(queries @ keys, dim=-1).sum(dim=0)
                    captured = rows if captured is None else captured + rows

                normed = layer.final_layer_norm(hidden)
                hidden = hidden + project["fc2"](
                    layer.activation_fn(project["fc1"](normed, quantized)), quantized
                )

            hidden = self._final_norm(hidden)
            if not all_logits:
                hidden = hidden[:, -1:]
            return self._proj_out(hidden), captured

        def _capture_plan(self, cross):
            """Layer -> alignment keys for `_forward`, and the head count."""
            heads = self._resolved_alignment_heads()
            plan: dict[int, list[int]] = {}
            for layer, head in heads:
                plan.setdefault(int(layer), []).append(int(head))
            return self._alignment_keys(plan, cross), len(heads)

        # --------------------------------------------------------------
        # The decode loop all generation entry points share.
        # --------------------------------------------------------------

        def _decode(
            self,
            features,
            prefix: list[int],
            max_new: int,
            *,
            suppress_tokens: list[int] | None = None,
            ban_first: set[int] | None = None,
            eot_banned_steps: int = 0,
            sample_temperature: float | None = None,
            sample_top_k: int = 0,
            capture: bool = False,
            report_stop: bool = False,
            keep_eot: bool = False,
        ):
            """Greedy (or sampled) decode forcing `prefix`.

            Returns `(tokens, attention_rows, stop_prob)`. `tokens` excludes the
            EOT, like HF's Whisper `generate()`, unless `keep_eot` (HF keeps it
            when asked for attention, `return_dict_in_generate=True`);
            `attention_rows` is `[len(tokens), frames]` (row k predicted token
            k) when `capture`;
            `stop_prob` (with `report_stop`) is P(EOT) at the step that stopped,
            or None when the decode ran out of steps.
            """
            prefix = [int(t) for t in prefix]
            steps = min(int(max_new), self._max_positions - len(prefix))
            if steps <= 0:
                return [], np.zeros((0, 0), dtype=np.float32), None

            cross = self._cross_kv(features)
            plan, head_count = self._capture_plan(cross) if capture else (None, 0)
            suppress = self._resolve_suppress(suppress_tokens)
            suppress_index = (
                torch.tensor(list(suppress), dtype=torch.long, device=self.device)
                if suppress
                else None
            )
            ban_index = (
                torch.tensor(sorted(ban_first), dtype=torch.long, device=self.device)
                if ban_first
                else None
            )
            eot = self.eot_id
            sampling = sample_temperature is not None
            report_stop = report_stop and not sampling and eot is not None
            # Reading a token back to Python waits for the device. On a GPU,
            # checking for EOT only every few steps lets the CPU queue the next
            # steps while the device is still busy; steps decoded past the EOT
            # are discarded. On the CPU everything is synchronous anyway.
            check_every = 1 if self.device.type == "cpu" else 8

            picked: list = []
            rows: list = []
            stop_probs: list = []
            values: list[int] = []
            position = 0
            tokens = torch.tensor([prefix], dtype=torch.long, device=self.device)

            with torch.inference_mode():
                for step in range(steps):
                    logits, captured = self._forward(tokens, position, cross, plan)
                    position += tokens.shape[1]

                    scores = logits[0, -1].float()
                    if suppress_index is not None:
                        scores.index_fill_(0, suppress_index, float("-inf"))
                    if step == 0 and ban_index is not None:
                        scores.index_fill_(0, ban_index, float("-inf"))
                    if eot is not None and step < eot_banned_steps:
                        scores[eot] = float("-inf")

                    if sampling:
                        token = _sample(scores, sample_temperature, sample_top_k)
                    else:
                        token = torch.argmax(scores)
                    if report_stop:
                        stop_probs.append(torch.softmax(scores, dim=-1)[eot])
                    picked.append(token)
                    if capture:
                        rows.append(captured[-1])

                    if (step + 1) % check_every == 0 or step == steps - 1:
                        values.extend(torch.stack(picked[len(values):]).tolist())
                        if eot is not None and eot in values:
                            break
                    tokens = token.view(1, 1)

                if len(values) < len(picked):
                    values.extend(torch.stack(picked[len(values):]).tolist())

                stop_prob = None
                generated = values
                if eot is not None and eot in values:
                    stop = values.index(eot)
                    generated = values[: stop + 1] if keep_eot else values[:stop]
                    if report_stop:
                        stop_prob = float(stop_probs[stop])

                if capture and generated:
                    attention = torch.stack(rows[: len(generated)]) / head_count
                    attention = attention.cpu().numpy().astype(np.float32)
                else:
                    attention = np.zeros((0, 0), dtype=np.float32)
            return generated, attention, stop_prob

        # --------------------------------------------------------------
        # Stock entry points, re-routed.
        # --------------------------------------------------------------

        def _run_generate(
            self,
            features,
            prefix,
            max_new,
            *,
            ban_first=None,
            suppress_tokens=None,
            num_beams=1,
            do_sample=False,
            temperature=1.0,
            top_k=0,
        ):
            if int(num_beams) != 1:
                return super()._run_generate(
                    features, prefix, max_new,
                    ban_first=ban_first, suppress_tokens=suppress_tokens,
                    num_beams=num_beams, do_sample=do_sample,
                    temperature=temperature, top_k=top_k,
                )
            tokens, _, _ = self._decode(
                features, prefix, max_new,
                suppress_tokens=suppress_tokens,
                ban_first=ban_first,
                sample_temperature=float(temperature) if do_sample else None,
                sample_top_k=int(top_k),
            )
            return tokens

        def _run_generate_with_attention(
            self, features, prefix, max_new, *, ban_first=None, suppress_tokens=None,
        ):
            tokens, attention, _ = self._decode(
                features, prefix, max_new,
                suppress_tokens=suppress_tokens,
                ban_first=ban_first,
                capture=True,
                keep_eot=True,
            )
            return tokens, attention

        def greedy_stops_and_decode(
            self, features, prompt_tokens, *, max_length=256,
            suppress_tokens=None, min_new_tokens=0,
        ):
            tokens, _, stop_prob = self._decode(
                features, prompt_tokens, max_length,
                suppress_tokens=suppress_tokens,
                eot_banned_steps=int(min_new_tokens),
                report_stop=True,
            )
            return tokens, stop_prob

        def eot_probability(self, features, prompt_tokens, gen_ids, *, suppress_tokens=None):
            if self.eot_id is None:
                return None
            content = list(gen_ids)
            while content and content[-1] == self.eot_id:
                content.pop()
            if not content:
                return None
            sequence = list(prompt_tokens) + content
            if len(sequence) > self._max_positions:
                return super().eot_probability(
                    features, prompt_tokens, gen_ids, suppress_tokens=suppress_tokens,
                )
            cross = self._cross_kv(features)
            tokens = torch.tensor([sequence], dtype=torch.long, device=self.device)
            with torch.inference_mode():
                logits, _ = self._forward(tokens, 0, cross)
                scores = logits[0, -1].float()
                suppress = self._resolve_suppress(suppress_tokens)
                if suppress:
                    scores[torch.tensor(list(suppress), device=scores.device)] = float("-inf")
                return float(torch.softmax(scores, dim=-1)[self.eot_id])

        def _cross_attention_rows(self, features, prompt_tokens, gen_ids):
            if not gen_ids:
                return np.zeros((0, 0), dtype=np.float32)
            sequence = list(prompt_tokens) + list(gen_ids)
            if len(sequence) > self._max_positions:
                return super()._cross_attention_rows(features, prompt_tokens, gen_ids)
            cross = self._cross_kv(features)
            plan, head_count = self._capture_plan(cross)
            tokens = torch.tensor([sequence], dtype=torch.long, device=self.device)
            with torch.inference_mode():
                _, captured = self._forward(tokens, 0, cross, plan, all_logits=True)
            start = len(prompt_tokens) - 1
            rows = captured[start:start + len(gen_ids)] / head_count
            return rows.cpu().numpy().astype(np.float32)

        # --------------------------------------------------------------
        # Load-time guard.
        # --------------------------------------------------------------

        def self_check(self) -> float:
            """Max |logit difference| vs HuggingFace's decoder on a toy input,
            relative to the largest reference logit (at least 1).

            Uses a short random "encoder output" so it costs a few decoder
            steps, not an encoder pass.
            """
            generator = torch.Generator().manual_seed(0)
            dtype = next(self.model.parameters()).dtype
            encoded = torch.randn(1, 24, self._d_model, generator=generator)
            encoded = encoded.to(self.device, dtype)
            prompt = self.get_decoder_prefix("en")
            tokens = torch.tensor([prompt], dtype=torch.long, device=self.device)

            with torch.inference_mode():
                reference = self.model.model.decoder(
                    input_ids=tokens, encoder_hidden_states=encoded, use_cache=False,
                ).last_hidden_state
                reference = self._proj_out(reference)

                cross = []
                for layer in self._layers:
                    attention = layer.encoder_attn
                    cross.append((
                        self._split_heads(attention.k_proj(encoded)),
                        self._split_heads(attention.v_proj(encoded)),
                    ))
                # Prefill all but the last token, then one cached step, so the
                # incremental path is checked too.
                # The float path: this checks the module layout, not the int8
                # rounding (measured separately on real audio).
                full, _ = self._forward(tokens, 0, cross, all_logits=True, quantized=False)
                head, _ = self._forward(tokens[:, :-1], 0, cross, all_logits=True, quantized=False)
                step, _ = self._forward(
                    tokens[:, -1:], tokens.shape[1] - 1, cross, quantized=False,
                )

            error = float((full - reference).abs().max())
            error = max(error, float((head - reference[:, :-1]).abs().max()))
            error = max(error, float((step[:, -1] - reference[:, -1]).abs().max()))
            error /= max(1.0, float(reference.abs().max()))
            return error

    return FastTransformersEngine


class _Projection:
    """One decoder matmul: the float weights, plus int8 ones when quantized
    (symmetric, per output channel)."""

    def __init__(self, weight, bias, quantize: bool):
        import torch

        self.weight = weight
        self.bias = bias
        self.int8 = None
        if quantize:
            scale = weight.float().abs().amax(dim=1).clamp(min=1e-8) / 127.0
            values = torch.round(weight.float() / scale[:, None]).clamp(-127, 127)
            self.int8 = (values.to(torch.int8).contiguous(), scale.to(weight.dtype))

    def __call__(self, x, quantized: bool = True):
        import torch
        import torch.nn.functional as F

        if self.int8 is None or not quantized:
            return F.linear(x, self.weight, self.bias)
        shape = x.shape
        out = torch._weight_int8pack_mm(x.reshape(-1, shape[-1]), *self.int8)
        if self.bias is not None:
            out = out + self.bias
        return out.view(*shape[:-1], out.shape[-1])


def _use_sdpa(model) -> None:
    try:
        model.set_attn_implementation("sdpa")
        return
    except Exception:
        pass
    try:
        model.config._attn_implementation = "sdpa"
    except Exception:
        logger.debug("Could not switch the encoder to SDPA attention", exc_info=True)


def _sample(scores, temperature: float, top_k: int):
    """Match HF's sampling: temperature, optional top-k, multinomial."""
    import torch

    scores = scores / max(float(temperature), 1e-6)
    if top_k and top_k > 0:
        threshold = torch.topk(scores, min(int(top_k), scores.numel())).values[-1]
        scores = scores.masked_fill(scores < threshold, float("-inf"))
    probabilities = torch.softmax(scores, dim=-1)
    return torch.multinomial(probabilities, num_samples=1)[0]
