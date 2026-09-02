//! PhiMoE forward pass — see `model.rs`'s module doc comment for the full
//! verified recipe and exact source citations. Structurally close to
//! `phi3::forward` (same LongRoPE mechanism, same causal GQA attention),
//! with three genuine differences: every norm carries a learned additive
//! bias (`rmsnorm_biased` below — the first crate in this workspace that
//! needs RMSNorm-then-bias, since it's not shared with any other
//! architecture yet), Q/K/V/output projections all have a required bias
//! (vs. phi3's optional fused-QKV bias), and the FFN block is
//! unconditionally the MoE path (no dense fallback — `qwen3::forward`'s
//! routing pattern, reused structurally, not by dependency).

use tensor_core::ops::{add_inplace, causal_gqa_attention, linear_quantized, mul_inplace, moe_route, rmsnorm, rope_inplace_with_freq_factors_and_scale, silu_inplace};
use tensor_core::Matrix;

use model_core::KvCache;

use crate::model::{LayerWeights, Model};

/// `rmsnorm` (normalize, scale by `weight`) followed by an elementwise
/// `bias` add — llama.cpp's own `build_norm(..., LLM_NORM_RMS, ...)` with
/// a bias tensor present, NOT true LayerNorm (see `model.rs`'s module doc
/// comment for the verified divergence from Microsoft's own reference
/// math, matched deliberately).
fn rmsnorm_biased(x: &Matrix, weight: &[f32], bias: &[f32], eps: f32) -> Matrix {
    let mut out = rmsnorm(x, weight, eps);
    for r in 0..out.rows {
        let row = out.row_mut(r);
        for (v, b) in row.iter_mut().zip(bias) {
            *v += *b;
        }
    }
    out
}

impl Model {
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

        let final_hidden = rmsnorm_biased(&hidden, &self.output_norm, &self.output_norm_bias, c.rms_eps);
        // Always the dedicated output projection -- no tied-embedding
        // fallback for this architecture, see model.rs's module doc comment.
        let logits = linear_quantized(&final_hidden, &self.output_weight, Some(&self.output_bias));
        logits.last_row().to_vec()
    }

    fn forward_layer(&self, layer: &LayerWeights, li: usize, hidden: &Matrix, cache: &mut KvCache, kv_offset: usize, positions: &[usize]) -> Matrix {
        let c = &self.config;

        // --- attention ---
        let a = rmsnorm_biased(hidden, &layer.attn_norm, &layer.attn_norm_bias, c.rms_eps);

        let mut q = linear_quantized(&a, &layer.wq, Some(&layer.bq));
        let mut k_new = linear_quantized(&a, &layer.wk, Some(&layer.bk));
        let v_new = linear_quantized(&a, &layer.wv, Some(&layer.bv));

        let freq_factors = c.active_rope_factors.as_deref();
        rope_inplace_with_freq_factors_and_scale(&mut q, c.n_heads, c.head_dim, c.n_rot, c.rope_freq_base, positions, freq_factors, c.rope_attn_factor);
        rope_inplace_with_freq_factors_and_scale(&mut k_new, c.n_kv_heads, c.head_dim, c.n_rot, c.rope_freq_base, positions, freq_factors, c.rope_attn_factor);

        let layer_cache = &mut cache.layers[li];
        for s in 0..q.rows {
            layer_cache.k.push_row(k_new.row(s));
            layer_cache.v.push_row(v_new.row(s));
        }

        let attn = causal_gqa_attention(&q, &layer_cache.k, &layer_cache.v, c.n_heads, c.n_kv_heads, c.head_dim, kv_offset);
        let attn_proj = linear_quantized(&attn, &layer.wo, Some(&layer.bo));
        let mut attn_out = hidden.clone();
        add_inplace(&mut attn_out, &attn_proj);

        // --- feed-forward: unconditionally MoE, every layer (module doc
        // comment) -- same routing shape as qwen3::forward, no shared/dense
        // expert running alongside the routed ones. ---
        let normed2 = rmsnorm_biased(&attn_out, &layer.ffn_norm, &layer.ffn_norm_bias, c.rms_eps);
        let logits = linear_quantized(&normed2, &layer.moe.gate_inp, None);
        let routed = moe_route(&logits, c.n_expert_used);

        let mut moe_out = Matrix::zeros(normed2.rows, c.embed_dim);
        for (t, picks) in routed.iter().enumerate() {
            let token_in = Matrix::from_vec(1, c.embed_dim, normed2.row(t).to_vec());
            for &(expert_idx, weight) in picks {
                let mut gate = linear_quantized(&token_in, &layer.moe.gate_exps[expert_idx], None);
                silu_inplace(&mut gate);
                let up = linear_quantized(&token_in, &layer.moe.up_exps[expert_idx], None);
                mul_inplace(&mut gate, &up);
                let expert_out = linear_quantized(&gate, &layer.moe.down_exps[expert_idx], None);
                let dst = moe_out.row_mut(t);
                for (i, v) in expert_out.row(0).iter().enumerate() {
                    dst[i] += v * weight;
                }
            }
        }

        let mut out = attn_out;
        add_inplace(&mut out, &moe_out);
        out
    }
}

#[cfg(test)]
mod tests {
    //! Same standing constraint as every other unverified-in-anger crate
    //! here: no real PhiMoE GGUF exists to check against, so this builds a
    //! tiny synthetic `Model` by hand and checks `forward_step` runs to
    //! completion producing correctly-shaped, finite output, plus targeted
    //! "does changing X actually change the output" checks for the
    //! genuinely new mechanisms (biased norm, MoE routing, the required
    //! untied output head). NOT a correctness check against a reference.
    //! GQA (n_heads=2, n_kv_heads=1) is exercised throughout.

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
        let n_ff_exp = 4;
        let n_expert = 3;
        let n_expert_used = 2;
        let vocab_size = 3;

        let layer = |seed: f32| LayerWeights {
            attn_norm: f32_vec(embed_dim, seed),
            attn_norm_bias: f32_vec(embed_dim, seed + 0.05),
            wq: f32_qmatrix(n_heads * head_dim, embed_dim, seed + 0.1),
            bq: f32_vec(n_heads * head_dim, seed + 0.12),
            wk: f32_qmatrix(n_kv_heads * head_dim, embed_dim, seed + 0.2),
            bk: f32_vec(n_kv_heads * head_dim, seed + 0.22),
            wv: f32_qmatrix(n_kv_heads * head_dim, embed_dim, seed + 0.3),
            bv: f32_vec(n_kv_heads * head_dim, seed + 0.32),
            wo: f32_qmatrix(embed_dim, n_heads * head_dim, seed + 0.4),
            bo: f32_vec(embed_dim, seed + 0.42),
            ffn_norm: f32_vec(embed_dim, seed + 0.5),
            ffn_norm_bias: f32_vec(embed_dim, seed + 0.52),
            moe: MoeWeights {
                gate_inp: f32_qmatrix(n_expert, embed_dim, seed + 0.6),
                gate_exps: (0..n_expert).map(|e| f32_qmatrix(n_ff_exp, embed_dim, seed + 0.7 + e as f32 * 0.1)).collect(),
                up_exps: (0..n_expert).map(|e| f32_qmatrix(n_ff_exp, embed_dim, seed + 1.0 + e as f32 * 0.1)).collect(),
                down_exps: (0..n_expert).map(|e| f32_qmatrix(embed_dim, n_ff_exp, seed + 1.3 + e as f32 * 0.1)).collect(),
            },
        };

        Model {
            config: Config {
                n_layers: 2,
                embed_dim,
                n_heads,
                n_kv_heads,
                head_dim,
                n_rot: head_dim,
                vocab_size,
                rope_freq_base: 10000.0,
                rms_eps: 1e-5,
                context_length: 8,
                active_rope_factors: None,
                rope_attn_factor: 1.0,
                n_expert_used,
            },
            token_embd: f32_qmatrix(vocab_size, embed_dim, 0.0),
            output_weight: f32_qmatrix(vocab_size, embed_dim, 5.0),
            output_bias: f32_vec(vocab_size, 5.1),
            output_norm: f32_vec(embed_dim, 4.0),
            output_norm_bias: f32_vec(embed_dim, 4.05),
            layers: vec![layer(0.1), layer(1.2)],
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
        // No real PhiMoE GGUF to check logits against (see module doc) —
        // this instead pins down determinism, which the optimization
        // roadmap's later phases (persistent thread pool, buffer reuse,
        // MoE clone removal) can silently break even while "looking fine":
        // same weights + same token sequence must produce bit-identical
        // logits every time, over a run long enough (16 forward_step calls,
        // cache grows past its context_length=8 capacity hint) to exercise
        // cache growth AND route tokens through both MoE layers repeatedly,
        // not just the first couple of steps.
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
    fn norm_bias_actually_changes_the_output() {
        // Proves rmsnorm_biased's bias add is genuinely wired in, not a
        // dead field: changing ONLY layer 0's attn_norm_bias must change
        // the final logits.
        let baseline = tiny_model();
        let mut changed = tiny_model();
        changed.layers[0].attn_norm_bias = f32_vec(4, 99.0);

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&changed.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = changed.forward_step(&mut cache_b, &[0, 1, 2]);

        assert!(logits_a.iter().all(|v| v.is_finite()));
        assert!(logits_b.iter().all(|v| v.is_finite()));
        assert_ne!(logits_a, logits_b, "changing attn_norm_bias should change the output, but logits were identical");
    }

    #[test]
    fn moe_expert_weights_actually_affect_the_output() {
        // Same "change every expert" robustness reasoning as qwen3's
        // equivalent test: with n_expert_used=2 of n_expert=3, changing
        // only one expert risks it not being among the routed pair for
        // any of the 3 test tokens (routing depends on router logits, not
        // hand-verifiable by inspection). Changing all of them guarantees
        // the change is visible regardless of which subset gets selected.
        let baseline = tiny_model();
        let mut changed = tiny_model();
        for layer in &mut changed.layers {
            for e in 0..layer.moe.down_exps.len() {
                layer.moe.down_exps[e] = f32_qmatrix(4, 4, 77.0 + e as f32);
            }
        }

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&changed.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = changed.forward_step(&mut cache_b, &[0, 1, 2]);

        assert!(logits_a.iter().all(|v| v.is_finite()));
        assert!(logits_b.iter().all(|v| v.is_finite()));
        assert_ne!(logits_a, logits_b, "changing every expert's weights should change the output, but logits were identical");
    }

    #[test]
    fn output_bias_actually_changes_the_output() {
        // Proves the required (non-tied) output head's bias is genuinely
        // read and applied.
        let baseline = tiny_model();
        let mut changed = tiny_model();
        changed.output_bias = f32_vec(3, 88.0);

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&changed.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = changed.forward_step(&mut cache_b, &[0, 1, 2]);

        assert_ne!(logits_a, logits_b, "changing output_bias should change the output, but logits were identical");
    }
}
