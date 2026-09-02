//! Phi-3 decoder-only forward pass, KV-cache aware. Structurally the same
//! skeleton as `llama::forward` (RMSNorm, NEOX-style RoPE, SiLU-gated FFN,
//! causal GQA attention) — `model.rs` already pre-splits Phi-3's fused
//! on-disk tensors (`attn_qkv`, `ffn_up`) into the same `wq`/`wk`/`wv`/
//! `ffn_gate`/`ffn_up` shape every other crate here uses, so this file's
//! real deltas from `llama::forward` are:
//!   - `rope_inplace_with_freq_factors_and_scale` instead of plain
//!     `rope_inplace` — carries Phi-3's optional partial rotary dimension
//!     (`n_rot <= head_dim`) AND its optional LongRoPE scaling
//!     (`config.active_rope_factors`/`rope_attn_factor`, both already
//!     resolved to a no-op — `None`/`1.0` — at load time for checkpoints
//!     without LongRoPE; see `model.rs`'s doc comment).
//!   - Q/K/V bias is `Option<&[f32]>` (`layer.bq.as_deref()` etc.) instead
//!     of always-`None` (llama) or always-`Some` (qwen2) — see `model.rs`'s
//!     doc comment for why it's optional here.
//!
//! h  = embedding(new_tokens)
//! for each layer:
//!   a  = rmsnorm(h, attn_norm)
//!   q,k_new,v_new = linear_q(a, Wq, bq), linear_q(a, Wk, bk), linear_q(a, Wv, bv)
//!   q,k_new = rope(q, n_rot, freq_factors, attn_factor, abs. positions), rope(k_new, ...)
//!   cache.k/v.push_row(k_new/v_new)
//!   o  = linear_q(causal_gqa_attention(q, cache.k, cache.v, kv_offset), Wo)
//!   h += o
//!   f  = rmsnorm(h, ffn_norm)
//!   h += linear_q(silu(linear_q(f,Wgate)) * linear_q(f,Wup), Wdown)
//! h = rmsnorm(h, output_norm)
//! logits = linear_q(h, output_weight.unwrap_or(token_embd))   [last position only]

use tensor_core::ops::{add_inplace, causal_gqa_attention, linear_quantized, mul_inplace, rmsnorm, rope_inplace_with_freq_factors_and_scale, silu_inplace};
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

            let mut q = linear_quantized(&normed, &layer.wq, layer.bq.as_deref());
            let mut k_new = linear_quantized(&normed, &layer.wk, layer.bk.as_deref());
            let v_new = linear_quantized(&normed, &layer.wv, layer.bv.as_deref());

            let freq_factors = c.active_rope_factors.as_deref();
            rope_inplace_with_freq_factors_and_scale(&mut q, c.n_heads, c.head_dim, c.n_rot, c.rope_freq_base, &positions, freq_factors, c.rope_attn_factor);
            rope_inplace_with_freq_factors_and_scale(&mut k_new, c.n_kv_heads, c.head_dim, c.n_rot, c.rope_freq_base, &positions, freq_factors, c.rope_attn_factor);

            let layer_cache = &mut cache.layers[li];
            for s in 0..seq_new {
                layer_cache.k.push_row(k_new.row(s));
                layer_cache.v.push_row(v_new.row(s));
            }

            let attn = causal_gqa_attention(&q, &layer_cache.k, &layer_cache.v, c.n_heads, c.n_kv_heads, c.head_dim, kv_offset);
            let attn_proj = linear_quantized(&attn, &layer.wo, None);
            add_inplace(&mut hidden, &attn_proj);

            let normed2 = rmsnorm(&hidden, &layer.ffn_norm, c.rms_eps);
            let mut gate = linear_quantized(&normed2, &layer.ffn_gate, None);
            let up = linear_quantized(&normed2, &layer.ffn_up, None);
            silu_inplace(&mut gate);
            mul_inplace(&mut gate, &up);
            let down = linear_quantized(&gate, &layer.ffn_down, None);
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
    //! Same standing constraint as every other crate here: no real GGUF to
    //! verify against, so these build a tiny synthetic `Model` by hand and
    //! check `forward_step` runs to completion producing correctly-shaped,
    //! finite output — confirms the wiring, not numerical correctness
    //! against a reference. `tiny_model()` deliberately sets `n_rot <
    //! head_dim` so the partial-rotary path (this crate's one genuinely new
    //! piece of forward-pass logic beyond `llama::forward`) is actually
    //! exercised, not just defaulted to the full-rotation case.

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

    /// 1 layer, GQA (n_kv_heads < n_heads), head_dim=4 with n_rot=2 (partial
    /// rotary -- half of each head's dims pass through RoPE untouched), and
    /// a fused-bias present (`bq`/`bk`/`bv` all `Some`) to exercise that
    /// optional path too.
    fn tiny_model() -> Model {
        let embed_dim = 4;
        let n_heads = 2;
        let n_kv_heads = 1;
        let head_dim = 4;
        let n_rot = 2;
        let ffn_dim = 4;
        let vocab_size = 3;

        let layer = LayerWeights {
            attn_norm: f32_vec(embed_dim, 0.1),
            wq: f32_qmatrix(n_heads * head_dim, embed_dim, 0.2),
            wk: f32_qmatrix(n_kv_heads * head_dim, embed_dim, 0.3),
            wv: f32_qmatrix(n_kv_heads * head_dim, embed_dim, 0.4),
            bq: Some(f32_vec(n_heads * head_dim, 0.21)),
            bk: Some(f32_vec(n_kv_heads * head_dim, 0.31)),
            bv: Some(f32_vec(n_kv_heads * head_dim, 0.41)),
            wo: f32_qmatrix(embed_dim, n_heads * head_dim, 0.5),
            ffn_norm: f32_vec(embed_dim, 0.6),
            ffn_gate: f32_qmatrix(ffn_dim, embed_dim, 0.7),
            ffn_up: f32_qmatrix(ffn_dim, embed_dim, 0.8),
            ffn_down: f32_qmatrix(embed_dim, ffn_dim, 0.9),
        };

        Model {
            config: Config {
                n_layers: 1,
                embed_dim,
                ffn_dim,
                n_heads,
                n_kv_heads,
                head_dim,
                n_rot,
                vocab_size,
                rope_freq_base: 10000.0,
                rms_eps: 1e-5,
                context_length: 8,
                n_swa: 0,
                active_rope_factors: None,
                rope_attn_factor: 1.0,
            },
            token_embd: f32_qmatrix(vocab_size, embed_dim, 0.0),
            output_weight: None, // tied embeddings — the simpler, more common case
            output_norm: f32_vec(embed_dim, 1.0),
            layers: vec![layer],
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
        // No real Phi-3 GGUF to check logits against (see module doc) — this
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
    fn forward_step_without_qkv_bias_also_produces_finite_logits() {
        // bq/bk/bv all None -- the other half of the Option<&[f32]> branch.
        let mut model = tiny_model();
        model.layers[0].bq = None;
        model.layers[0].bk = None;
        model.layers[0].bv = None;
        let mut cache = KvCache::new(&model.config.cache_shape());
        let logits = model.forward_step(&mut cache, &[0, 1]);
        assert!(logits.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn partial_rotary_dimension_actually_changes_the_forward_pass_output() {
        // Proves n_rot is really wired into forward_step (not silently
        // ignored / hardcoded to head_dim): full rotation (n_rot=head_dim)
        // must produce DIFFERENT logits than partial rotation (n_rot=2) on
        // the same weights and input, since they rotate a different number
        // of dimensions per head.
        let mut full = tiny_model();
        full.config.n_rot = full.config.head_dim; // 4, i.e. no partial rotation
        let mut partial = tiny_model();
        partial.config.n_rot = 2;

        let mut cache_full = KvCache::new(&full.config.cache_shape());
        let mut cache_partial = KvCache::new(&partial.config.cache_shape());
        let logits_full = full.forward_step(&mut cache_full, &[0, 1]);
        let logits_partial = partial.forward_step(&mut cache_partial, &[0, 1]);

        assert_ne!(logits_full, logits_partial, "n_rot should change attention output, but logits were identical");
    }

    #[test]
    fn longrope_freq_factors_actually_changes_the_output() {
        // tiny_model() has n_rot=2 (half=1), so a single-entry table.
        let baseline = tiny_model();
        let mut with_factors = tiny_model();
        with_factors.config.active_rope_factors = Some(vec![2.0]);

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&with_factors.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = with_factors.forward_step(&mut cache_b, &[0, 1, 2]);

        assert!(logits_a.iter().all(|v| v.is_finite()));
        assert!(logits_b.iter().all(|v| v.is_finite()));
        assert_ne!(logits_a, logits_b, "setting active_rope_factors should change the output, but logits were identical");
    }

    #[test]
    fn longrope_attn_factor_actually_changes_the_output() {
        let baseline = tiny_model();
        let mut with_scale = tiny_model();
        with_scale.config.rope_attn_factor = 1.5;

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&with_scale.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = with_scale.forward_step(&mut cache_b, &[0, 1, 2]);

        assert_ne!(logits_a, logits_b, "a non-1.0 rope_attn_factor should change the output, but logits were identical");
    }

    #[test]
    fn longrope_trivial_settings_match_baseline() {
        // active_rope_factors=Some([1.0]) + rope_attn_factor=1.0 must be a
        // complete no-op, matching the None/1.0 baseline exactly -- the
        // same equivalence pinned at the tensor-core level, checked again
        // through the whole forward pass.
        let baseline = tiny_model();
        let mut trivial = tiny_model();
        trivial.config.active_rope_factors = Some(vec![1.0]);
        trivial.config.rope_attn_factor = 1.0;

        let mut cache_a = KvCache::new(&baseline.config.cache_shape());
        let mut cache_b = KvCache::new(&trivial.config.cache_shape());
        let logits_a = baseline.forward_step(&mut cache_a, &[0, 1, 2]);
        let logits_b = trivial.forward_step(&mut cache_b, &[0, 1, 2]);

        assert_eq!(logits_a, logits_b);
    }
}
