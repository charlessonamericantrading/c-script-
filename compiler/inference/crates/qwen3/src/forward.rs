//! Qwen3/Qwen3MoE forward pass — see `model.rs`'s module doc comment for
//! the full verified recipe and exact source citations. Structurally very
//! close to `qwen2::forward` (same GQA attention, same SwiGLU dense FFN,
//! same tied-embedding fallback for the LM head), with three genuine
//! differences: QK-norm (per-head RMSNorm on Q/K, applied after
//! reshape-into-heads, before RoPE — via the shared
//! `tensor_core::ops::grouped_norm`, originally built for Gemma4's
//! identical mechanism), no QKV bias, and a per-layer dense-vs-MoE FFN
//! dispatch (`model.rs`'s `LayerWeights::moe`).

use tensor_core::ops::{add_inplace, causal_gqa_attention, grouped_norm, linear_quantized, mul_inplace, moe_route, rmsnorm, rope_inplace, silu_inplace};
use tensor_core::Matrix;

use model_core::KvCache;

use crate::model::{LayerWeights, Model};

impl Model {
    /// Feeds `new_tokens` through the model — the whole prompt on the
    /// first ("prefill") call, one token per subsequent ("decode") call —
    /// returning logits (length `vocab_size`) for the *last* new position.
    pub fn forward_step(&self, cache: &mut KvCache, new_tokens: &[u32]) -> Vec<f32> {
        let seq_new = new_tokens.len();
        let c = &self.config;
        let kv_offset = cache.len();
        let positions: Vec<usize> = (kv_offset..kv_offset + seq_new).collect();

        let mut hidden = Matrix::zeros(seq_new, c.embed_dim);
        for (s, &id) in new_tokens.iter().enumerate() {
            hidden.row_mut(s).copy_from_slice(&self.token_embd.dequant_row(id as usize));
        }

        for (li, layer) in self.layers.iter().enumerate() {
            hidden = self.forward_layer(layer, li, &hidden, cache, kv_offset, &positions);
        }

        let final_hidden = rmsnorm(&hidden, &self.output_norm, c.rms_eps);
        let lm_head = self.output_weight.as_ref().unwrap_or(&self.token_embd);
        linear_quantized(&final_hidden, lm_head, None).last_row().to_vec()
    }

    fn forward_layer(&self, layer: &LayerWeights, li: usize, hidden: &Matrix, cache: &mut KvCache, kv_offset: usize, positions: &[usize]) -> Matrix {
        let c = &self.config;

        // --- attention ---
        let a = rmsnorm(hidden, &layer.attn_norm, c.rms_eps);

        // No QKV bias -- see model.rs's module doc comment (a real removal
        // from Qwen2, not an oversight).
        let mut q = linear_quantized(&a, &layer.wq, None);
        let mut k_new = linear_quantized(&a, &layer.wk, None);
        let v_new = linear_quantized(&a, &layer.wv, None);

        // QK-Norm: per-head RMSNorm, AFTER reshape into heads (grouped_norm's
        // own reshape trick), BEFORE RoPE -- verified order, model.rs's
        // module doc comment.
        q = grouped_norm(&q, &layer.attn_q_norm, c.rms_eps, c.n_heads);
        k_new = grouped_norm(&k_new, &layer.attn_k_norm, c.rms_eps, c.n_kv_heads);

        // Full rotary (n_rot == head_dim, no partial-rotary factor -- see
        // model.rs's module doc comment), so the plain rope_inplace
        // suffices (unlike Gemma4/Phi-3, no freq_factors/attn_factor needed).
        rope_inplace(&mut q, c.n_heads, c.head_dim, c.rope_freq_base, positions);
        rope_inplace(&mut k_new, c.n_kv_heads, c.head_dim, c.rope_freq_base, positions);

        let layer_cache = &mut cache.layers[li];
        for s in 0..q.rows {
            layer_cache.k.push_row(k_new.row(s));
            layer_cache.v.push_row(v_new.row(s));
        }

        let attn = causal_gqa_attention(&q, &layer_cache.k, &layer_cache.v, c.n_heads, c.n_kv_heads, c.head_dim, kv_offset);
        let attn_proj = linear_quantized(&attn, &layer.wo, None);
        let mut attn_out = hidden.clone();
        add_inplace(&mut attn_out, &attn_proj);

        // --- feed-forward: dense SwiGLU, or routed MoE -- never both, see
        // model.rs's module doc comment (unlike Gemma4, Qwen3MoE has no
        // shared/dense expert running alongside the routed ones). ---
        let normed2 = rmsnorm(&attn_out, &layer.ffn_norm, c.rms_eps);
        let ffn_out = match &layer.moe {
            Some(moe) => {
                let logits = linear_quantized(&normed2, &moe.gate_inp, None);
                let routed = moe_route(&logits, c.n_expert_used);

                let mut moe_out = Matrix::zeros(normed2.rows, c.embed_dim);
                for (t, picks) in routed.iter().enumerate() {
                    let token_in = Matrix::from_vec(1, c.embed_dim, normed2.row(t).to_vec());
                    for &(expert_idx, weight) in picks {
                        let mut gate = linear_quantized(&token_in, &moe.gate_exps[expert_idx], None);
                        silu_inplace(&mut gate);
                        let up = linear_quantized(&token_in, &moe.up_exps[expert_idx], None);
                        mul_inplace(&mut gate, &up);
                        let expert_out = linear_quantized(&gate, &moe.down_exps[expert_idx], None);
                        let dst = moe_out.row_mut(t);
                        for (i, v) in expert_out.row(0).iter().enumerate() {
                            dst[i] += v * weight;
                        }
                    }
                }
                moe_out
            }
            None => {
                let gate_w = layer.ffn_gate.as_ref().expect("moe.is_none() implies dense ffn_gate is loaded -- both set together in model.rs");
                let up_w = layer.ffn_up.as_ref().expect("moe.is_none() implies dense ffn_up is loaded");
                let down_w = layer.ffn_down.as_ref().expect("moe.is_none() implies dense ffn_down is loaded");
                let mut gate = linear_quantized(&normed2, gate_w, None);
                silu_inplace(&mut gate);
                let up = linear_quantized(&normed2, up_w, None);
                mul_inplace(&mut gate, &up);
                linear_quantized(&gate, down_w, None)
            }
        };

        let mut out = attn_out;
        add_inplace(&mut out, &ffn_out);
        out
    }
}

#[cfg(test)]
mod tests {
    //! Same standing constraint as every other unverified-in-anger crate
    //! here (`llama`, and now this one — see `model.rs`'s module doc
    //! comment): no real Qwen3/Qwen3MoE GGUF exists to check against, so
    //! this builds a tiny synthetic `Model` by hand (deterministic
    //! weights, not from any file) and checks `forward_step` runs to
    //! completion producing correctly-shaped, finite output, plus a few
    //! targeted "does changing X actually change the output" checks for
    //! the two genuinely new mechanisms (QK-norm, MoE routing). NOT a
    //! correctness check against a reference — confirms the wiring,
    //! nothing more. Two layers deliberately exercise both FFN paths:
    //! layer 0 is dense SwiGLU, layer 1 is MoE — between them, every
    //! conditional branch in `forward_layer` runs at least once. GQA
    //! (n_heads=2, n_kv_heads=1) is exercised throughout, matching
    //! Gemma4's own tiny-model convention.

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
        let head_dim = 4;
        let ffn_dim = 4;
        let n_ff_exp = 4;
        let n_expert = 3;
        let n_expert_used = 2;
        let vocab_size = 3;

        let layer0 = LayerWeights {
            attn_norm: f32_vec(embed_dim, 0.1),
            wq: f32_qmatrix(n_heads * head_dim, embed_dim, 0.2),
            wk: f32_qmatrix(n_kv_heads * head_dim, embed_dim, 0.3),
            wv: f32_qmatrix(n_kv_heads * head_dim, embed_dim, 0.35),
            wo: f32_qmatrix(embed_dim, n_heads * head_dim, 0.4),
            attn_q_norm: f32_vec(head_dim, 0.5),
            attn_k_norm: f32_vec(head_dim, 0.55),
            ffn_norm: f32_vec(embed_dim, 0.7),
            ffn_gate: Some(f32_qmatrix(ffn_dim, embed_dim, 0.8)),
            ffn_up: Some(f32_qmatrix(ffn_dim, embed_dim, 0.85)),
            ffn_down: Some(f32_qmatrix(embed_dim, ffn_dim, 0.9)),
            moe: None,
        };

        let layer1 = LayerWeights {
            attn_norm: f32_vec(embed_dim, 1.2),
            wq: f32_qmatrix(n_heads * head_dim, embed_dim, 1.3),
            wk: f32_qmatrix(n_kv_heads * head_dim, embed_dim, 1.4),
            wv: f32_qmatrix(n_kv_heads * head_dim, embed_dim, 1.45),
            wo: f32_qmatrix(embed_dim, n_heads * head_dim, 1.5),
            attn_q_norm: f32_vec(head_dim, 1.6),
            attn_k_norm: f32_vec(head_dim, 1.65),
            ffn_norm: f32_vec(embed_dim, 1.8),
            ffn_gate: None,
            ffn_up: None,
            ffn_down: None,
            moe: Some(MoeWeights {
                gate_inp: f32_qmatrix(n_expert, embed_dim, 2.1),
                gate_exps: (0..n_expert).map(|e| f32_qmatrix(n_ff_exp, embed_dim, 2.2 + e as f32 * 0.1)).collect(),
                up_exps: (0..n_expert).map(|e| f32_qmatrix(n_ff_exp, embed_dim, 2.5 + e as f32 * 0.1)).collect(),
                down_exps: (0..n_expert).map(|e| f32_qmatrix(embed_dim, n_ff_exp, 2.8 + e as f32 * 0.1)).collect(),
            }),
        };

        Model {
            config: Config {
                n_layers: 2,
                embed_dim,
                head_dim,
                n_heads,
                n_kv_heads,
                vocab_size,
                rope_freq_base: 1_000_000.0,
                rms_eps: 1e-5,
                context_length: 8,
                n_expert_used,
            },
            token_embd: f32_qmatrix(vocab_size, embed_dim, 0.0),
            output_weight: None, // tied embeddings
            output_norm: f32_vec(embed_dim, 4.0),
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
    fn forward_step_is_deterministic_across_repeated_runs() {
        // No real Qwen3 GGUF to check logits against (see module doc) —
        // this instead pins down determinism, which the optimization
        // roadmap's later phases (persistent thread pool, buffer reuse,
        // MoE clone removal) can silently break even while "looking fine":
        // same weights + same token sequence must produce bit-identical
        // logits every time, over a run long enough (16 forward_step calls,
        // cache grows past its context_length=8 capacity hint) to exercise
        // cache growth AND route tokens through the MoE layer (layer 1)
        // repeatedly, not just the first couple of steps.
        fn run() -> Vec<Vec<f32>> {
            let model = tiny_model();
            let mut cache = KvCache::new(&model.config.cache_shape());
            let mut steps = vec![model.forward_step(&mut cache, &[0, 1, 2])]; // prefill
            for &tok in [1u32, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0, 1, 2, 0].iter() {
                steps.push(model.forward_step(&mut cache, &[tok]));
            }
            steps
        }

        let run1 = run();
        let run2 = run();
        assert_eq!(run1.len(), run2.len());
        assert!(run1.len() >= 15, "expected >=15 forward_step calls, got {}", run1.len());
        for (step, (a, b)) in run1.iter().zip(run2.iter()).enumerate() {
            assert!(a.iter().all(|v| v.is_finite()), "run1 step {step} has NaN/Inf: {a:?}");
            assert!(b.iter().all(|v| v.is_finite()), "run2 step {step} has NaN/Inf: {b:?}");
            assert_eq!(a, b, "step {step}: non-deterministic output between identical runs");
        }
    }

    #[test]
    fn qk_norm_actually_changes_the_output() {
        // Proves QK-norm is genuinely wired in (not silently skipped):
        // changing ONLY layer 0's attn_q_norm weight must change the
        // final logits.
        let baseline = tiny_model();
        let mut changed = tiny_model();
        changed.layers[0].attn_q_norm = f32_vec(4, 99.0);

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&changed.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = changed.forward_step(&mut cache_b, &[0, 1, 2]);

        assert!(logits_a.iter().all(|v| v.is_finite()));
        assert!(logits_b.iter().all(|v| v.is_finite()));
        assert_ne!(logits_a, logits_b, "changing attn_q_norm should change the output, but logits were identical");
    }

    #[test]
    fn moe_expert_weights_actually_affect_the_output() {
        // Proves the MoE branch actually reads the expert matrices (not,
        // say, silently falling through to a zeroed or ignored path).
        // Changes EVERY expert's down-projection, not just one: with
        // n_expert_used=2 of n_expert=3, changing only a single expert
        // risks it not being among the 2 actually routed-to for any of
        // the 3 test tokens (routing depends on the router logits, not
        // hand-verifiable by inspection) -- changing all of them
        // guarantees the change is visible regardless of which subset
        // gets selected.
        let baseline = tiny_model();
        let mut changed = tiny_model();
        let moe = changed.layers[1].moe.as_mut().unwrap();
        for e in 0..moe.down_exps.len() {
            moe.down_exps[e] = f32_qmatrix(4, 4, 77.0 + e as f32);
        }

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&changed.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = changed.forward_step(&mut cache_b, &[0, 1, 2]);

        assert!(logits_a.iter().all(|v| v.is_finite()));
        assert!(logits_b.iter().all(|v| v.is_finite()));
        assert_ne!(logits_a, logits_b, "changing every expert's weights should change the output, but logits were identical");
    }
}
