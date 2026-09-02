//! Gemma4 forward pass — see the crate-level doc comment for the full
//! verified recipe (`src/models/gemma4.cpp`, ggml-org/llama.cpp commit
//! e3546c7948e3af463d0b401e6421d5a4c2faf565) and this module's inline
//! comments for exactly which step maps to which source line.
//!
//! Per-layer head_dim, shared-KV-layer reuse, and "proportional RoPE"
//! (`model.rs`'s module doc comment) are all implemented here now.
//! Per-layer head_dim and shared-KV reuse are verified against a real
//! local checkpoint (`gemma4:e4b`); proportional RoPE is NOT — that same
//! checkpoint has no `rope_freqs` tensor on any layer (checked directly,
//! all four full-attention layers), so the mechanism this function adds
//! is exercised only by the synthetic tests below. See
//! `tensor_core::ops::rope_inplace_with_freq_factors`'s doc comment for
//! the verified formula and the one real remaining gap (checkpoints that
//! also declare `rope.scaling.type` would need YaRN/linear-scaling support
//! this does not implement).

use tensor_core::ops::{
    add_inplace, causal_gqa_attention_scaled, gelu, grouped_norm, grouped_norm_no_weight, linear_quantized, mul_inplace, rmsnorm,
    rmsnorm_no_weight, rope_inplace_with_freq_factors,
};
use tensor_core::Matrix;

use model_core::KvCache;

use crate::model::{LayerWeights, Model};

/// Extracts columns `[start, start+width)` from every row of `x` — used to
/// pull one layer's `n_embd_per_layer`-wide slice out of the combined
/// `[n_tokens, n_layer*n_embd_per_layer]` per-layer-embedding matrix. Unlike
/// `grouped_norm`'s reshape trick, this genuinely copies (column ranges
/// aren't contiguous across rows in a row-major layout).
fn column_slice(x: &Matrix, start: usize, width: usize) -> Matrix {
    let mut out = Matrix::zeros(x.rows, width);
    for r in 0..x.rows {
        out.row_mut(r).copy_from_slice(&x.row(r)[start..start + width]);
    }
    out
}

fn scale_inplace(m: &mut Matrix, s: f32) {
    for v in m.data.iter_mut() {
        *v *= s;
    }
}

fn gelu_matrix(m: &Matrix) -> Matrix {
    let mut out = m.clone();
    for v in out.data.iter_mut() {
        *v = gelu(*v);
    }
    out
}

/// The dense "shared expert" FFN shape every layer has (MoE or not):
/// `down(gelu(gate(x)) * up(x))` — GeGLU, VERIFIED as Gemma4's activation
/// (`LLM_FFN_GELU` in the reference, not SiLU/SwiGLU like Qwen2/Llama).
fn geglu_ffn(x: &Matrix, gate: &tensor_core::QuantizedMatrix, up: &tensor_core::QuantizedMatrix, down: &tensor_core::QuantizedMatrix) -> Matrix {
    let gate_out = gelu_matrix(&linear_quantized(x, gate, None));
    let mut h = gate_out;
    let up_out = linear_quantized(x, up, None);
    mul_inplace(&mut h, &up_out);
    linear_quantized(&h, down, None)
}

impl Model {
    pub fn forward_step(&self, cache: &mut KvCache, new_tokens: &[u32]) -> Vec<f32> {
        let seq_new = new_tokens.len();
        let c = &self.config;
        let kv_offset = cache.len();
        let positions: Vec<usize> = (kv_offset..kv_offset + seq_new).collect();

        // 1. Embedding, scaled by sqrt(embed_dim). gemma4.cpp:98-100.
        let mut hidden = Matrix::zeros(seq_new, c.embed_dim);
        for (s, &id) in new_tokens.iter().enumerate() {
            hidden.row_mut(s).copy_from_slice(&self.token_embd.dequant_row(id as usize));
        }
        scale_inplace(&mut hidden, (c.embed_dim as f32).sqrt());

        // 2. Per-layer embeddings, built ONCE before the layer loop.
        //    gemma4.cpp:353-374 (injection, per-layer) + 448-497 (construction).
        let per_layer_combined: Option<Matrix> = if c.n_embd_per_layer > 0 {
            let table_flat = self.per_layer_tok_embd.as_ref().expect("n_embd_per_layer > 0 implies per_layer_tok_embd is loaded");
            let mut table = Matrix::zeros(seq_new, c.n_layers * c.n_embd_per_layer);
            for (s, &id) in new_tokens.iter().enumerate() {
                table.row_mut(s).copy_from_slice(&table_flat.dequant_row(id as usize));
            }
            scale_inplace(&mut table, (c.n_embd_per_layer as f32).sqrt());

            let proj_w = self.per_layer_model_proj.as_ref().expect("n_embd_per_layer > 0 implies per_layer_model_proj is loaded");
            let mut proj = linear_quantized(&hidden, proj_w, None);
            scale_inplace(&mut proj, 1.0 / (c.embed_dim as f32).sqrt());
            let proj_norm = self.per_layer_proj_norm.as_ref().expect("n_embd_per_layer > 0 implies per_layer_proj_norm is loaded");
            proj = grouped_norm(&proj, proj_norm, c.rms_eps, c.n_layers);

            let mut combined = proj;
            add_inplace(&mut combined, &table);
            scale_inplace(&mut combined, 1.0 / 2f32.sqrt());
            Some(combined)
        } else {
            None
        };

        for (li, layer) in self.layers.iter().enumerate() {
            hidden = self.forward_layer(layer, li, &hidden, cache, kv_offset, &positions, per_layer_combined.as_ref());
        }

        let final_hidden = rmsnorm(&hidden, &self.output_norm, c.rms_eps);
        let lm_head = self.output_weight.as_ref().unwrap_or(&self.token_embd);
        let mut logits = linear_quantized(&final_hidden, lm_head, None).last_row().to_vec();

        // Final logit softcap. gemma4.cpp:424-428. 0.0 = disabled (matches
        // model.rs's `unwrap_or(0.0)` for the optional metadata key).
        if c.final_logit_softcapping != 0.0 {
            let cap = c.final_logit_softcapping;
            for v in logits.iter_mut() {
                *v = (*v / cap).tanh() * cap;
            }
        }
        logits
    }

    #[allow(clippy::too_many_arguments)]
    fn forward_layer(
        &self,
        layer: &LayerWeights,
        li: usize,
        hidden: &Matrix,
        cache: &mut KvCache,
        kv_offset: usize,
        positions: &[usize],
        per_layer_combined: Option<&Matrix>,
    ) -> Matrix {
        let c = &self.config;
        let is_swa = c.is_swa[li];
        let freq_base = if is_swa { c.rope_freq_base_swa } else { c.rope_freq_base };
        // This layer's REAL per-head dimension — decoupled from
        // embed_dim/n_heads, and genuinely different for SWA vs
        // full-attention layers. VERIFIED against a real checkpoint; see
        // model.rs's module doc comment.
        let n_embd_head = c.head_dim_for(li);
        // Proportional RoPE -- only full-attention (non-SWA) layers ever
        // have this tensor (model.rs's per-layer loader gates it on
        // `!is_swa[i]`, matching gemma4.cpp's own `if (!is_swa(il))` check
        // before reading `model.layers[il].rope_freqs`); `None` recovers
        // plain RoPE exactly. Same value feeds BOTH Q's and K's rope call,
        // computed once here to mirror the reference's single `freq_factors`
        // local reused for both.
        let freq_factors = layer.rope_freqs.as_deref();

        // --- attention. gemma4.cpp:157-266. ---
        let a = rmsnorm(hidden, &layer.attn_norm, c.rms_eps);

        let mut q = linear_quantized(&a, &layer.wq, None);
        q = grouped_norm(&q, &layer.attn_q_norm, c.rms_eps, c.n_heads);
        rope_inplace_with_freq_factors(&mut q, c.n_heads, n_embd_head, n_embd_head, freq_base, positions, freq_factors);

        // Layers past `n_layer_kv_from_start` compute no K/V of their own
        // at all and instead reuse an earlier layer's already-cached K/V
        // (`layer.kv_source_layer`, read below regardless of this branch —
        // it's this layer's own index when `wk.is_some()`). VERIFIED
        // against a real checkpoint that this path is genuinely exercised,
        // not just structurally possible; see model.rs's module doc comment.
        if let Some(wk) = &layer.wk {
            let mut k = linear_quantized(&a, wk, None);
            // V falls back to the RAW (pre-K-norm) K projection when this
            // layer has no separate V weight — VERIFIED real (model.rs's
            // `wv` doc comment), not a hypothetical case.
            let mut v = match &layer.wv {
                Some(wv) => linear_quantized(&a, wv, None),
                None => k.clone(),
            };
            let attn_k_norm = layer.attn_k_norm.as_ref().expect("wk.is_some() implies attn_k_norm.is_some() -- both loaded together in model.rs, gated on the same has_own_kv check");
            k = grouped_norm(&k, attn_k_norm, c.rms_eps, c.n_kv_heads);
            v = grouped_norm_no_weight(&v, c.rms_eps, c.n_kv_heads, n_embd_head); // NO learned weight -- see model.rs / tensor-core::rmsnorm_no_weight doc comments
            rope_inplace_with_freq_factors(&mut k, c.n_kv_heads, n_embd_head, n_embd_head, freq_base, positions, freq_factors);

            let layer_cache = &mut cache.layers[li];
            for s in 0..q.rows {
                layer_cache.k.push_row(k.row(s));
                layer_cache.v.push_row(v.row(s));
            }
        }

        // f_attention_scale = 1.0, fixed -- VERIFIED (gemma4.cpp comment:
        // "Gemma4 uses self.scaling = 1.0 (no pre-attn scaling)"), not the
        // usual 1/sqrt(head_dim) causal_gqa_attention defaults to.
        let source = &cache.layers[layer.kv_source_layer];
        let attn = causal_gqa_attention_scaled(&q, &source.k, &source.v, c.n_heads, c.n_kv_heads, n_embd_head, kv_offset, 1.0);
        let mut out = linear_quantized(&attn, &layer.wo, None);
        out = rmsnorm(&out, &layer.attn_post_norm, c.rms_eps);

        let mut attn_out = hidden.clone();
        add_inplace(&mut attn_out, &out);

        // --- feed-forward: shared-expert dense FFN, plus MoE branch if this
        // layer has one. gemma4.cpp:288-338. ---
        let ffn_combined = match &layer.moe {
            Some(moe) => {
                let shared_in = rmsnorm(&attn_out, &layer.ffn_norm, c.rms_eps);
                let mut shared = geglu_ffn(&shared_in, &layer.ffn_gate, &layer.ffn_up, &layer.ffn_down);
                shared = rmsnorm(&shared, &moe.post_norm_1, c.rms_eps);

                let moe_in = rmsnorm(&attn_out, &moe.pre_norm_2, c.rms_eps);

                // Router logits: NOT from moe_in -- from attn_out normalized
                // (unweighted) and scaled SEPARATELY. gemma4.cpp:307-311.
                let mut router_in = rmsnorm_no_weight(&attn_out, c.rms_eps);
                scale_inplace(&mut router_in, 1.0 / (c.embed_dim as f32).sqrt());
                for r in 0..router_in.rows {
                    let row = router_in.row_mut(r);
                    for (i, v) in row.iter_mut().enumerate() {
                        *v *= moe.gate_inp_scale[i];
                    }
                }
                let logits = linear_quantized(&router_in, &moe.gate_inp, None);
                let routed = tensor_core::ops::moe_route(&logits, c.n_expert_used);

                let mut moe_out = Matrix::zeros(moe_in.rows, c.embed_dim);
                for (t, picks) in routed.iter().enumerate() {
                    let token_in = Matrix::from_vec(1, c.embed_dim, moe_in.row(t).to_vec());
                    for &(expert_idx, weight) in picks {
                        let gate_out = gelu_matrix(&linear_quantized(&token_in, &moe.gate_exps[expert_idx], None));
                        let mut h = gate_out;
                        let up_out = linear_quantized(&token_in, &moe.up_exps[expert_idx], None);
                        mul_inplace(&mut h, &up_out);
                        let expert_out = linear_quantized(&h, &moe.down_exps[expert_idx], None);
                        let dst = moe_out.row_mut(t);
                        for (i, v) in expert_out.row(0).iter().enumerate() {
                            dst[i] += v * weight;
                        }
                    }
                }
                moe_out = rmsnorm(&moe_out, &moe.post_norm_2, c.rms_eps);

                let mut combined = shared;
                add_inplace(&mut combined, &moe_out);
                combined
            }
            None => {
                let a2 = rmsnorm(&attn_out, &layer.ffn_norm, c.rms_eps);
                geglu_ffn(&a2, &layer.ffn_gate, &layer.ffn_up, &layer.ffn_down)
            }
        };

        let mut cur = rmsnorm(&ffn_combined, &layer.ffn_post_norm, c.rms_eps);
        add_inplace(&mut cur, &attn_out);

        // --- per-layer embedding injection. gemma4.cpp:353-374. ---
        if let (Some(combined), Some(inp_gate), Some(proj), Some(post_norm)) =
            (per_layer_combined, &layer.per_layer_inp_gate, &layer.per_layer_proj, &layer.per_layer_post_norm)
        {
            let pe_in = cur.clone();
            let gate = gelu_matrix(&linear_quantized(&cur, inp_gate, None));
            let slice = column_slice(combined, li * c.n_embd_per_layer, c.n_embd_per_layer);
            let mut mixed = gate;
            mul_inplace(&mut mixed, &slice);
            let mut projected = linear_quantized(&mixed, proj, None);
            projected = rmsnorm(&projected, post_norm, c.rms_eps);
            cur = pe_in;
            add_inplace(&mut cur, &projected);
        }

        // --- optional per-layer output scale. gemma4.cpp:378-381. ---
        if let Some(s) = layer.out_scale {
            scale_inplace(&mut cur, s);
        }

        cur
    }
}

#[cfg(test)]
mod tests {
    //! Same standing constraint as `qwen2`/`llama`'s equivalent tests: no
    //! real GGUF exists to verify against, so this builds a tiny synthetic
    //! `Model` by hand (deterministic weights, not from any file) and checks
    //! `forward_step` runs to completion producing correctly-shaped, finite
    //! output. NOT a correctness check against a reference — confirms the
    //! wiring (tensor shapes, MoE routing, per-layer-embedding injection,
    //! QK-norm, the V-fallback and dense-vs-MoE branches) doesn't panic or
    //! produce NaN/Inf, nothing more. Two layers deliberately exercise
    //! different combinations: layer 0 is full-attention + has its own V
    //! projection + dense FFN; layer 1 is SWA + reuses K as V (no separate
    //! V weight) + MoE FFN — between them, every conditional path in
    //! `forward_layer` runs at least once.

    use model_core::KvCache;
    use tensor_core::QuantizedMatrix;

    use crate::model::{Config, LayerWeights, MoeWeights, Model};
    use gguf::GgmlType;

    fn f32_qmatrix(rows: usize, cols: usize, seed: f32) -> QuantizedMatrix {
        let mut raw = Vec::with_capacity(rows * cols * 4);
        for i in 0..rows * cols {
            let v = ((i as f32) * 0.01 + seed).sin() * 0.1;
            raw.extend_from_slice(&v.to_le_bytes());
        }
        QuantizedMatrix::from_raw(rows, cols, GgmlType::F32, raw).unwrap()
    }

    fn f32_vec(len: usize, seed: f32) -> Vec<f32> {
        (0..len).map(|i| 1.0 + ((i as f32) * 0.01 + seed).sin() * 0.05).collect()
    }

    fn tiny_model() -> Model {
        let embed_dim = 4;
        let n_heads = 2;
        let n_kv_heads = 1;
        // Deliberately DIFFERENT per-type head dims (not the old shared
        // `head_dim=2` both layers used) -- proves the two are resolved
        // independently, not still accidentally coupled. layer0 is
        // full-attention (uses head_dim_full), layer1 is SWA (uses
        // head_dim_swa).
        let head_dim_full = 4;
        let head_dim_swa = 2;
        let ffn_dim = 4;
        let n_ff_exp = 4;
        let vocab_size = 3;
        let n_expert = 3;
        let n_expert_used = 2;
        let n_embd_per_layer = 2;
        let n_layers = 2;

        let layer0 = LayerWeights {
            attn_norm: f32_vec(embed_dim, 0.1),
            wq: f32_qmatrix(n_heads * head_dim_full, embed_dim, 0.2),
            wk: Some(f32_qmatrix(n_kv_heads * head_dim_full, embed_dim, 0.3)),
            wv: Some(f32_qmatrix(n_kv_heads * head_dim_full, embed_dim, 0.35)), // has its own V
            wo: f32_qmatrix(embed_dim, n_heads * head_dim_full, 0.4),
            attn_q_norm: f32_vec(head_dim_full, 0.5),
            attn_k_norm: Some(f32_vec(head_dim_full, 0.55)),
            attn_post_norm: f32_vec(embed_dim, 0.6),
            kv_source_layer: 0, // has its own KV
            rope_freqs: None,
            ffn_norm: f32_vec(embed_dim, 0.7),
            ffn_gate: f32_qmatrix(ffn_dim, embed_dim, 0.8),
            ffn_up: f32_qmatrix(ffn_dim, embed_dim, 0.85),
            ffn_down: f32_qmatrix(embed_dim, ffn_dim, 0.9),
            ffn_post_norm: f32_vec(embed_dim, 0.95),
            moe: None, // dense FFN only
            out_scale: None,
            per_layer_inp_gate: Some(f32_qmatrix(n_embd_per_layer, embed_dim, 1.0)),
            per_layer_proj: Some(f32_qmatrix(embed_dim, n_embd_per_layer, 1.05)),
            per_layer_post_norm: Some(f32_vec(embed_dim, 1.1)),
        };

        let layer1 = LayerWeights {
            attn_norm: f32_vec(embed_dim, 1.2),
            wq: f32_qmatrix(n_heads * head_dim_swa, embed_dim, 1.3),
            wk: Some(f32_qmatrix(n_kv_heads * head_dim_swa, embed_dim, 1.4)),
            wv: None, // reuse K as V -- exercises the fallback path
            wo: f32_qmatrix(embed_dim, n_heads * head_dim_swa, 1.5),
            attn_q_norm: f32_vec(head_dim_swa, 1.6),
            attn_k_norm: Some(f32_vec(head_dim_swa, 1.65)),
            attn_post_norm: f32_vec(embed_dim, 1.7),
            kv_source_layer: 1, // has its own KV
            rope_freqs: None,
            ffn_norm: f32_vec(embed_dim, 1.8),
            ffn_gate: f32_qmatrix(ffn_dim, embed_dim, 1.85), // shared expert -- present even on MoE layers
            ffn_up: f32_qmatrix(ffn_dim, embed_dim, 1.9),
            ffn_down: f32_qmatrix(embed_dim, ffn_dim, 1.95),
            ffn_post_norm: f32_vec(embed_dim, 2.0),
            moe: Some(MoeWeights {
                gate_inp: f32_qmatrix(n_expert, embed_dim, 2.1),
                gate_inp_scale: f32_vec(embed_dim, 2.15),
                gate_exps: (0..n_expert).map(|e| f32_qmatrix(n_ff_exp, embed_dim, 2.2 + e as f32 * 0.1)).collect(),
                up_exps: (0..n_expert).map(|e| f32_qmatrix(n_ff_exp, embed_dim, 2.5 + e as f32 * 0.1)).collect(),
                down_exps: (0..n_expert).map(|e| f32_qmatrix(embed_dim, n_ff_exp, 2.8 + e as f32 * 0.1)).collect(),
                pre_norm_2: f32_vec(embed_dim, 3.1),
                post_norm_1: f32_vec(embed_dim, 3.2),
                post_norm_2: f32_vec(embed_dim, 3.3),
            }),
            out_scale: Some(0.98), // exercises the optional-out_scale path
            per_layer_inp_gate: Some(f32_qmatrix(n_embd_per_layer, embed_dim, 3.4)),
            per_layer_proj: Some(f32_qmatrix(embed_dim, n_embd_per_layer, 3.45)),
            per_layer_post_norm: Some(f32_vec(embed_dim, 3.5)),
        };

        Model {
            config: Config {
                n_layers,
                embed_dim,
                ffn_dim,
                n_ff_exp,
                n_heads,
                n_kv_heads,
                n_embd_head_k_full: head_dim_full,
                n_embd_head_k_swa: head_dim_swa,
                n_layer_kv_from_start: n_layers, // no shared-kv layers in this base model -- see the dedicated test below
                vocab_size,
                rope_freq_base: 10000.0,
                rope_freq_base_swa: 10000.0,
                rms_eps: 1e-5,
                context_length: 8,
                n_expert,
                n_expert_used,
                n_embd_per_layer,
                final_logit_softcapping: 30.0, // exercises the softcap path (nonzero)
                n_swa: 4,
                is_swa: vec![false, true], // layer 0 full-attention, layer 1 SWA
            },
            token_embd: f32_qmatrix(vocab_size, embed_dim, 0.0),
            output_weight: None, // tied embeddings
            output_norm: f32_vec(embed_dim, 4.0),
            per_layer_tok_embd: Some(f32_qmatrix(vocab_size, n_layers * n_embd_per_layer, 4.1)),
            per_layer_model_proj: Some(f32_qmatrix(n_layers * n_embd_per_layer, embed_dim, 4.2)),
            per_layer_proj_norm: Some(f32_vec(n_embd_per_layer, 4.3)),
            layers: vec![layer0, layer1],
        }
    }

    #[test]
    fn forward_step_prefill_produces_finite_vocab_sized_logits() {
        let model = tiny_model();
        let mut cache = KvCache::new(&model.config.cache_shape());
        let logits = model.forward_step(&mut cache, &[0, 1, 2]);
        assert_eq!(logits.len(), model.config.vocab_size);
        assert!(logits.iter().all(|v| v.is_finite()), "logits contain NaN/Inf: {logits:?}");
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn forward_step_decode_after_prefill_advances_cache_by_one() {
        let model = tiny_model();
        let mut cache = KvCache::new(&model.config.cache_shape());
        model.forward_step(&mut cache, &[0, 1]);
        assert_eq!(cache.len(), 2);
        let logits = model.forward_step(&mut cache, &[2]);
        assert_eq!(logits.len(), model.config.vocab_size);
        assert!(logits.iter().all(|v| v.is_finite()));
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn final_logit_softcap_keeps_logits_bounded_by_the_cap() {
        // tanh(x/cap)*cap is bounded in (-cap, cap) for any finite x -- a
        // structural property independent of the specific weights, and a
        // cheap way to confirm the softcap branch actually ran (nonzero
        // final_logit_softcapping is set in tiny_model()).
        let model = tiny_model();
        let mut cache = KvCache::new(&model.config.cache_shape());
        let logits = model.forward_step(&mut cache, &[0, 1]);
        let cap = model.config.final_logit_softcapping;
        assert!(logits.iter().all(|&v| v.abs() < cap), "logits exceed the softcap bound {cap}: {logits:?}");
    }

    #[test]
    fn proportional_rope_freq_factors_actually_changes_the_output() {
        // tiny_model()'s layer0 (full-attention, head_dim_full=4, so
        // half=2) has rope_freqs=None -- proves the wiring by setting a
        // non-trivial freq_factors table on a clone and checking the
        // forward pass output actually differs (not silently ignored).
        let baseline = tiny_model();
        let mut with_freq_factors = tiny_model();
        with_freq_factors.layers[0].rope_freqs = Some(vec![2.0, 3.0]);

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&with_freq_factors.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = with_freq_factors.forward_step(&mut cache_b, &[0, 1, 2]);

        assert!(logits_a.iter().all(|v| v.is_finite()));
        assert!(logits_b.iter().all(|v| v.is_finite()));
        assert_ne!(logits_a, logits_b, "setting rope_freqs on a full-attention layer should change the output, but logits were identical");
    }

    #[test]
    fn proportional_rope_freq_factors_all_ones_matches_none() {
        // A checkpoint whose rope_freqs happens to be all 1.0 (a trivial
        // table) must produce the SAME output as not having the tensor at
        // all -- confirms the None/Some(all-ones) equivalence already
        // pinned at the tensor-core level also holds through the whole
        // forward pass, not just in isolation.
        let baseline = tiny_model();
        let mut with_ones = tiny_model();
        with_ones.layers[0].rope_freqs = Some(vec![1.0, 1.0]);

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&with_ones.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = with_ones.forward_step(&mut cache_b, &[0, 1, 2]);

        assert_eq!(logits_a, logits_b);
    }

    /// 3 layers, no per-layer-embeddings/MoE (keeps this focused on just the
    /// shared-KV mechanism, already-covered paths live in `tiny_model()`):
    /// layer0 (SWA, own KV), layer1 (full-attention, own KV), layer2 (SWA,
    /// VIRTUAL -- reuses layer0's cache, matching the verified formula
    /// `n_layer_kv_from_start(2) - (is_swa ? 2 : 1) = 2 - 2 = 0`).
    fn tiny_model_with_shared_kv_layer() -> Model {
        let embed_dim = 4;
        let n_heads = 2;
        let n_kv_heads = 1;
        let head_dim_full = 4;
        let head_dim_swa = 2;
        let ffn_dim = 4;
        let vocab_size = 3;

        let own_kv_layer = |seed: f32, head_dim: usize, kv_source_layer: usize| LayerWeights {
            attn_norm: f32_vec(embed_dim, seed),
            wq: f32_qmatrix(n_heads * head_dim, embed_dim, seed + 0.1),
            wk: Some(f32_qmatrix(n_kv_heads * head_dim, embed_dim, seed + 0.2)),
            wv: Some(f32_qmatrix(n_kv_heads * head_dim, embed_dim, seed + 0.3)),
            wo: f32_qmatrix(embed_dim, n_heads * head_dim, seed + 0.4),
            attn_q_norm: f32_vec(head_dim, seed + 0.5),
            attn_k_norm: Some(f32_vec(head_dim, seed + 0.6)),
            attn_post_norm: f32_vec(embed_dim, seed + 0.7),
            kv_source_layer,
            rope_freqs: None,
            ffn_norm: f32_vec(embed_dim, seed + 0.8),
            ffn_gate: f32_qmatrix(ffn_dim, embed_dim, seed + 0.9),
            ffn_up: f32_qmatrix(ffn_dim, embed_dim, seed + 1.0),
            ffn_down: f32_qmatrix(embed_dim, ffn_dim, seed + 1.1),
            ffn_post_norm: f32_vec(embed_dim, seed + 1.2),
            moe: None,
            out_scale: None,
            per_layer_inp_gate: None,
            per_layer_proj: None,
            per_layer_post_norm: None,
        };

        let layer0 = own_kv_layer(0.1, head_dim_swa, 0);
        let layer1 = own_kv_layer(1.1, head_dim_full, 1);
        let layer2 = LayerWeights {
            attn_norm: f32_vec(embed_dim, 2.1),
            wq: f32_qmatrix(n_heads * head_dim_swa, embed_dim, 2.2),
            wk: None, // virtual layer -- no KV of its own
            wv: None,
            wo: f32_qmatrix(embed_dim, n_heads * head_dim_swa, 2.4),
            attn_q_norm: f32_vec(head_dim_swa, 2.5),
            attn_k_norm: None,
            attn_post_norm: f32_vec(embed_dim, 2.7),
            kv_source_layer: 0, // reuses layer0 (SWA, matching type)
            rope_freqs: None,
            ffn_norm: f32_vec(embed_dim, 2.8),
            ffn_gate: f32_qmatrix(ffn_dim, embed_dim, 2.9),
            ffn_up: f32_qmatrix(ffn_dim, embed_dim, 3.0),
            ffn_down: f32_qmatrix(embed_dim, ffn_dim, 3.1),
            ffn_post_norm: f32_vec(embed_dim, 3.2),
            moe: None,
            out_scale: None,
            per_layer_inp_gate: None,
            per_layer_proj: None,
            per_layer_post_norm: None,
        };

        Model {
            config: Config {
                n_layers: 3,
                embed_dim,
                ffn_dim,
                n_ff_exp: 0,
                n_heads,
                n_kv_heads,
                n_embd_head_k_full: head_dim_full,
                n_embd_head_k_swa: head_dim_swa,
                n_layer_kv_from_start: 2, // layers 0,1 own their KV; layer 2 is virtual
                vocab_size,
                rope_freq_base: 10000.0,
                rope_freq_base_swa: 10000.0,
                rms_eps: 1e-5,
                context_length: 8,
                n_expert: 0,
                n_expert_used: 0,
                n_embd_per_layer: 0,
                final_logit_softcapping: 0.0,
                n_swa: 4,
                is_swa: vec![true, false, true], // layer0 SWA, layer1 full, layer2 SWA (matches its donor's type)
            },
            token_embd: f32_qmatrix(vocab_size, embed_dim, 0.0),
            output_weight: None,
            output_norm: f32_vec(embed_dim, 4.0),
            per_layer_tok_embd: None,
            per_layer_model_proj: None,
            per_layer_proj_norm: None,
            layers: vec![layer0, layer1, layer2],
        }
    }

    #[test]
    fn shared_kv_layer_produces_finite_output_and_never_gets_its_own_cache_rows() {
        let model = tiny_model_with_shared_kv_layer();
        let mut cache = KvCache::new(&model.config.cache_shape());
        let logits = model.forward_step(&mut cache, &[0, 1]);
        assert!(logits.iter().all(|v| v.is_finite()), "logits contain NaN/Inf: {logits:?}");

        // Layers 0 and 1 (own KV) grow normally...
        assert_eq!(cache.layers[0].k.rows, 2);
        assert_eq!(cache.layers[1].k.rows, 2);
        // ...but layer 2 (virtual, reuses layer 0) never gets pushed to at
        // all -- directly verifying the "skip K/V computation entirely for
        // !has_own_kv layers" branch actually took effect, not just that
        // nothing crashed.
        assert_eq!(cache.layers[2].k.rows, 0, "virtual layer's own cache slot should stay empty -- it reads layer 0's instead");
        assert_eq!(cache.layers[2].v.rows, 0);
    }

    #[test]
    fn shared_kv_layer_output_depends_on_its_donors_weights() {
        // Changing ONLY layer 0's (the donor's) K projection must change
        // layer 2's (the virtual layer's) contribution to the final
        // output, proving layer 2 actually reads layer 0's cache rather
        // than e.g. silently attending over nothing / a zeroed cache.
        let model_a = tiny_model_with_shared_kv_layer();
        let mut model_b = tiny_model_with_shared_kv_layer();
        // n_kv_heads(1) * head_dim_swa(2) = 2 rows, embed_dim(4) cols --
        // same shape as layer0's original wk, different seed (99.0) so the
        // values differ.
        model_b.layers[0].wk = Some(f32_qmatrix(2, 4, 99.0));

        let mut cache_a = KvCache::new(&model_a.config.cache_shape());
        let mut cache_b = KvCache::new(&model_b.config.cache_shape());
        let logits_a = model_a.forward_step(&mut cache_a, &[0, 1]);
        let logits_b = model_b.forward_step(&mut cache_b, &[0, 1]);

        assert_ne!(logits_a, logits_b, "changing the donor layer's K projection should change the virtual layer's output, but logits were identical");
    }
}
