//! The Llama decoder-only forward pass, KV-cache aware. Same skeleton as
//! `qwen2::forward` (same `tensor_core::ops` primitives, same NEOX-style RoPE
//! convention — the base case, before Llama 3.1+'s extended-context RoPE
//! scaling, deliberately out of scope for this pass, see the crate doc
//! comment). The one wiring difference: Q/K/V linear projections take no
//! bias (`None` instead of `Some(&layer.bq)` etc.) — Llama has no attention
//! bias tensors at all.
//!
//! h  = embedding(new_tokens)
//! for each layer:
//!   a  = rmsnorm(h, attn_norm)
//!   q,k_new,v_new = linear_q(a, Wq), linear_q(a, Wk), linear_q(a, Wv)   [no bias]
//!   q,k_new = rope(q, abs. positions), rope(k_new, abs. positions)     [NEOX-style]
//!   cache.k/v.push_row(k_new/v_new)
//!   o  = linear_q(causal_gqa_attention(q, cache.k, cache.v, kv_offset), Wo)
//!   h += o
//!   f  = rmsnorm(h, ffn_norm)
//!   h += linear_q(silu(linear_q(f,Wgate)) * linear_q(f,Wup), Wdown)
//! h = rmsnorm(h, output_norm)
//! logits = linear_q(h, output_weight.unwrap_or(token_embd))   [last position only]

use tensor_core::ops::{add_inplace, causal_gqa_attention, linear_quantized, mul_inplace, rmsnorm, rope_inplace, silu_inplace};
use tensor_core::Matrix;

use model_core::KvCache;

use crate::model::Model;

impl Model {
    pub fn forward_step(&self, cache: &mut KvCache, new_tokens: &[u32]) -> Vec<f32> {
        let seq_new = new_tokens.len();
        let c = &self.config;
        let kv_offset = cache.len();

        let mut hidden = Matrix::zeros(seq_new, c.embed_dim);
        for (s, &id) in new_tokens.iter().enumerate() {
            hidden.row_mut(s).copy_from_slice(&self.token_embd.dequant_row(id as usize));
        }

        let positions: Vec<usize> = (kv_offset..kv_offset + seq_new).collect();

        for (li, layer) in self.layers.iter().enumerate() {
            let normed = rmsnorm(&hidden, &layer.attn_norm, c.rms_eps);

            let mut q = linear_quantized(&normed, &layer.wq, None);
            let mut k_new = linear_quantized(&normed, &layer.wk, None);
            let v_new = linear_quantized(&normed, &layer.wv, None);

            rope_inplace(&mut q, c.n_heads, c.head_dim, c.rope_freq_base, &positions);
            rope_inplace(&mut k_new, c.n_kv_heads, c.head_dim, c.rope_freq_base, &positions);

            let layer_cache = &mut cache.layers[li];
            for s in 0..seq_new {
                layer_cache.k.push_row(k_new.row(s));
                layer_cache.v.push_row(v_new.row(s));
            }

            let attn = causal_gqa_attention(&q, &layer_cache.k, &layer_cache.v, c.n_heads, c.n_kv_heads, c.head_dim, kv_offset);
            let attn_proj = linear_quantized(&attn, &layer.wo, None);
            add_inplace(&mut hidden, &attn_proj);

            let normed2 = rmsnorm(&hidden, &layer.ffn_norm, c.rms_eps);
            let mut gate = linear_quantized(&normed2, &layer.w_gate, None);
            let up = linear_quantized(&normed2, &layer.w_up, None);
            silu_inplace(&mut gate);
            mul_inplace(&mut gate, &up);
            let down = linear_quantized(&gate, &layer.w_down, None);
            add_inplace(&mut hidden, &down);
        }

        let final_hidden = rmsnorm(&hidden, &self.output_norm, c.rms_eps);
        let lm_head = self.output_weight.as_ref().unwrap_or(&self.token_embd);
        let logits = linear_quantized(&final_hidden, lm_head, None);
        logits.last_row().to_vec()
    }
}

#[cfg(test)]
mod tests {
    //! No real GGUF file exists to verify against (standing "no model
    //! downloads" constraint) — these build a tiny synthetic `Model` by hand
    //! (deterministic small weights, not loaded from any file) to check
    //! `forward_step` runs to completion and produces correctly-shaped,
    //! finite output. This is NOT a correctness check against a reference —
    //! it can't tell you the logits are semantically right, only that the
    //! wiring (tensor shapes, GQA head-count handling, cache growth) doesn't
    //! panic or produce NaN/Inf. Real correctness needs a real Llama 3.x
    //! GGUF compared token-for-token against a trusted reference (see
    //! `qwen2::forward`'s doc comment for why that bar matters — this
    //! project already shipped one bug that ran cleanly and looked plausible).

    use model_core::KvCache;
    use tensor_core::QuantizedMatrix;

    use crate::model::{Config, LayerWeights, Model};
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

    /// 1 layer, tiny dims, GQA (n_kv_heads < n_heads to exercise the
    /// grouped-query path, not just the degenerate MHA case).
    fn tiny_model() -> Model {
        let embed_dim = 4;
        let n_heads = 2;
        let n_kv_heads = 1;
        let head_dim = 2;
        let ffn_dim = 4;
        let vocab_size = 3;

        let layer = LayerWeights {
            attn_norm: f32_vec(embed_dim, 0.1),
            wq: f32_qmatrix(n_heads * head_dim, embed_dim, 0.2),
            wk: f32_qmatrix(n_kv_heads * head_dim, embed_dim, 0.3),
            wv: f32_qmatrix(n_kv_heads * head_dim, embed_dim, 0.4),
            wo: f32_qmatrix(embed_dim, n_heads * head_dim, 0.5),
            ffn_norm: f32_vec(embed_dim, 0.6),
            w_gate: f32_qmatrix(ffn_dim, embed_dim, 0.7),
            w_up: f32_qmatrix(ffn_dim, embed_dim, 0.8),
            w_down: f32_qmatrix(embed_dim, ffn_dim, 0.9),
        };

        Model {
            config: Config {
                n_layers: 1,
                embed_dim,
                ffn_dim,
                n_heads,
                n_kv_heads,
                head_dim,
                vocab_size,
                rope_freq_base: 10000.0,
                rms_eps: 1e-5,
                context_length: 8,
            },
            token_embd: f32_qmatrix(vocab_size, embed_dim, 0.0),
            output_weight: None, // tied embeddings — the simpler, more common case
            layers: vec![layer],
            output_norm: f32_vec(embed_dim, 1.0),
        }
    }

    #[test]
    fn forward_step_prefill_produces_finite_vocab_sized_logits() {
        let model = tiny_model();
        let mut cache = KvCache::new(&model.config.cache_shape());
        let logits = model.forward_step(&mut cache, &[0, 1, 2]);
        assert_eq!(logits.len(), model.config.vocab_size);
        assert!(logits.iter().all(|v| v.is_finite()), "logits contain NaN/Inf: {logits:?}");
        assert_eq!(cache.len(), 3, "prefill of 3 tokens should leave 3 cached positions");
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
        assert_eq!(cache.len(), 3, "decode step should append exactly one cached position");
    }

    #[test]
    fn forward_step_is_deterministic_across_repeated_runs() {
        // No real Llama GGUF to check logits against (see module doc) — this
        // instead pins down determinism, which the optimization roadmap's
        // later phases (persistent thread pool, buffer reuse) can silently
        // break even while "looking fine": same weights + same token
        // sequence must produce bit-identical logits every time, over a run
        // long enough (16 forward_step calls, cache grows past its
        // context_length=8 capacity hint) to exercise cache growth beyond
        // its initial allocation, not just the first couple of steps.
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
    fn forward_step_with_tied_embeddings_uses_token_embd_as_lm_head() {
        // output_weight is None in tiny_model() — this only checks the
        // "doesn't panic and produces the right shape" property; the actual
        // tied/untied CHOICE is exercised structurally, not verified against
        // a reference (see module doc).
        let model = tiny_model();
        assert!(model.output_weight.is_none());
        let mut cache = KvCache::new(&model.config.cache_shape());
        let logits = model.forward_step(&mut cache, &[0]);
        assert_eq!(logits.len(), model.config.vocab_size);
    }
}
