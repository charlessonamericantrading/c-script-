//! The Qwen2 decoder-only forward pass, KV-cache aware.
//!
//! `forward_step` processes only the *newly seen* tokens on each call — the
//! whole prompt on the first ("prefill") call, one token per subsequent
//! ("decode") call — appending each layer's new K/V projections to
//! `KvCache` and attending over the full cached history. This replaced
//! Fase 1's "recompute the entire sequence from scratch every step"
//! design: that version was simpler to get right first (no cache
//! invalidation, no off-by-one position bugs to chase), but cost O(seq²)
//! total work across a generation instead of O(seq) — the single biggest
//! reason the engine was ~130x slower than Ollama before this change (see
//! the Fase 2 writeup). `tensor_core::ops::causal_gqa_attention` has a test
//! (`cached_attention_matches_full_recompute`) pinning down that the two
//! approaches produce bit-identical output — this is a performance change,
//! not a behavior change.
//!
//! Per-layer wiring (verified against Fase 0's tensor dump — no separate
//! `output.weight`, Q/K/V all carry a bias, `attn_output`/FFN projections do
//! not):
//!   h  = embedding(new_tokens)                     [dequant_row per token]
//!   for each layer:
//!     a  = rmsnorm(h, attn_norm)
//!     q,k_new,v_new = linear_q(a, Wq,bq), linear_q(a, Wk,bk), linear_q(a, Wv,bv)
//!     q,k_new = rope(q, abs. positions), rope(k_new, abs. positions)   [NEOX-style]
//!     cache.k/v.push_row(k_new/v_new)               [append this step's rows]
//!     o  = linear_q(causal_gqa_attention(q, cache.k, cache.v, kv_offset), Wo)
//!     h += o
//!     f  = rmsnorm(h, ffn_norm)
//!     h += linear_q(silu(linear_q(f,Wgate)) * linear_q(f,Wup), Wdown)
//!   h = rmsnorm(h, output_norm)
//!   logits = linear_q(h, output_weight.unwrap_or(token_embd))   [last position only]
//!
//! The final projection uses `output_weight` when the checkpoint has one
//! (untied embeddings, e.g. Qwen2.5-Coder-7B) and falls back to
//! `token_embd` only when it doesn't (tied embeddings, e.g. Qwen2.5-0.5B).
//! Assuming "always tied" here was a real bug caught scaling this engine up
//! to the 7B model — see the doc comment on `model::Model::output_weight`.

use tensor_core::ops::{add_inplace, causal_gqa_attention, linear_quantized, linear_quantized_dual_into, linear_quantized_into, mul_inplace, rmsnorm, rmsnorm_into, rope_inplace, silu_inplace};
use tensor_core::Matrix;

use model_core::KvCache;

use crate::model::Model;

fn debug_stats(label: &str, m: &Matrix) {
    if std::env::var("DEBUG_FORWARD").is_err() {
        return;
    }
    let row = m.last_row();
    let max_abs = row.iter().fold(0f32, |acc, &v| acc.max(v.abs()));
    let norm = row.iter().map(|v| v * v).sum::<f32>().sqrt();
    let nan_count = row.iter().filter(|v| v.is_nan()).count();
    eprintln!("  [{label}] last-row: max_abs={max_abs:.4} l2_norm={norm:.4} nan_count={nan_count}");
}

impl Model {
    /// Feeds `new_tokens` through the model — the whole prompt on the first
    /// call, one token at a time after — updating `cache` in place, and
    /// returns the logits (length `vocab_size`) for the *last* new
    /// position, which is all a greedy/sampled next-token step needs.
    pub fn forward_step(&self, cache: &mut KvCache, new_tokens: &[u32]) -> Vec<f32> {
        let seq_new = new_tokens.len();
        let c = &self.config;
        let kv_offset = cache.len();

        let mut hidden = Matrix::zeros(seq_new, c.embed_dim);
        for (s, &id) in new_tokens.iter().enumerate() {
            hidden.row_mut(s).copy_from_slice(&self.token_embd.dequant_row(id as usize));
        }

        let positions: Vec<usize> = (kv_offset..kv_offset + seq_new).collect();
        debug_stats("embed", &hidden);

        // Fase 7 del roadmap de optimizacion: buffers reutilizados por las
        // 24 capas en vez de asignados frescos en cada una de las ~9
        // llamadas/capa (linear_quantized x7 + rmsnorm x2) -- reduce ~216
        // allocaciones grandes por token decodeado a un puñado. Valido
        // porque las 24 capas comparten exactamente las mismas dimensiones
        // (mismo n_heads/embed_dim/ffn_dim en cada una) -- un solo juego de
        // buffers dimensionado para `seq_new` sirve para las 24. `attn` (la
        // salida de causal_gqa_attention) queda sin optimizar en esta
        // pasada -- es una sola allocacion por capa, no siete, y cambiar su
        // firma tocaria mas codigo compartido; follow-up separable.
        let mut normed = Matrix::zeros(seq_new, c.embed_dim);
        let mut q = Matrix::zeros(seq_new, c.n_heads * c.head_dim);
        let mut k_new = Matrix::zeros(seq_new, c.n_kv_heads * c.head_dim);
        let mut v_new = Matrix::zeros(seq_new, c.n_kv_heads * c.head_dim);
        let mut attn_proj = Matrix::zeros(seq_new, c.embed_dim);
        let mut normed2 = Matrix::zeros(seq_new, c.embed_dim);
        let mut gate = Matrix::zeros(seq_new, c.ffn_dim);
        let mut up = Matrix::zeros(seq_new, c.ffn_dim);
        let mut down = Matrix::zeros(seq_new, c.embed_dim);

        for (li, layer) in self.layers.iter().enumerate() {
            rmsnorm_into(&hidden, &layer.attn_norm, c.rms_eps, &mut normed);

            linear_quantized_into(&normed, &layer.wq, Some(&layer.bq), &mut q);
            linear_quantized_into(&normed, &layer.wk, Some(&layer.bk), &mut k_new);
            linear_quantized_into(&normed, &layer.wv, Some(&layer.bv), &mut v_new);

            rope_inplace(&mut q, c.n_heads, c.head_dim, c.rope_freq_base, &positions);
            rope_inplace(&mut k_new, c.n_kv_heads, c.head_dim, c.rope_freq_base, &positions);

            let layer_cache = &mut cache.layers[li];
            for s in 0..seq_new {
                layer_cache.k.push_row(k_new.row(s));
                layer_cache.v.push_row(v_new.row(s));
            }

            let attn = causal_gqa_attention(&q, &layer_cache.k, &layer_cache.v, c.n_heads, c.n_kv_heads, c.head_dim, kv_offset);
            linear_quantized_into(&attn, &layer.wo, None, &mut attn_proj);
            add_inplace(&mut hidden, &attn_proj);
            debug_stats(&format!("layer {li} post-attn"), &hidden);

            rmsnorm_into(&hidden, &layer.ffn_norm, c.rms_eps, &mut normed2);
            // Fase 21: gate+up fused into one dispatch -- both read the
            // same `normed2`, so `x` is quantized once instead of twice
            // and the thread pool is dispatched once instead of twice.
            linear_quantized_dual_into(&normed2, &layer.w_gate, &layer.w_up, None, None, &mut gate, &mut up);
            silu_inplace(&mut gate);
            mul_inplace(&mut gate, &up);
            linear_quantized_into(&gate, &layer.w_down, None, &mut down);
            add_inplace(&mut hidden, &down);
            debug_stats(&format!("layer {li} post-ffn"), &hidden);
        }

        let final_hidden = rmsnorm(&hidden, &self.output_norm, c.rms_eps);
        debug_stats("final_norm", &final_hidden);
        let lm_head = self.output_weight.as_ref().unwrap_or(&self.token_embd);
        let logits = linear_quantized(&final_hidden, lm_head, None);
        debug_stats("logits", &logits);
        logits.last_row().to_vec()
    }

    /// Fase 23, Milestone 1: `forward_step` for N sequences' decode step
    /// (exactly one new token each, i.e. NOT prefill -- every `cache` in
    /// `caches` must already hold its own prompt) in one call, sharing a
    /// single weight read per layer instead of N independent ones.
    ///
    /// Only the matmul-heavy parts of a layer are batched -- Q/K/V/O and
    /// gate/up/down all take an `x` with `caches.len()` rows instead of 1,
    /// reusing `linear_quantized_into`/`linear_quantized_dual_into`
    /// unmodified (they already generalize to arbitrary row counts).
    /// Attention is NOT batched: each sequence has its own `KvCache` with
    /// its own length, so causal_gqa_attention runs once per sequence,
    /// against that sequence's own cache and `kv_offset`, same as
    /// `forward_step` does -- unbatched attention is the right call here,
    /// not a shortcut: per-operation profiling earlier in this roadmap
    /// measured attention at ~0.2% of decode time, so batching it would
    /// add real complexity (ragged/masked attention across sequences of
    /// different lengths) for a vanishingly small share of the total cost.
    /// The FFN/projection matmuls this DOES batch are ~63-77% of decode
    /// time, per that same profiling pass.
    ///
    /// Correctness: this must produce output bit-identical to calling
    /// `forward_step(caches[i], &last_tokens[i..i+1])` once per sequence
    /// (same test discipline as every fusion in this file) -- no float op
    /// is reordered relative to that, only how many rows share one
    /// `linear_quantized_into`/`linear_quantized_dual_into` call changes.
    pub fn forward_decode_step_batch(&self, caches: &mut [&mut KvCache], last_tokens: &[u32]) -> Vec<Vec<f32>> {
        assert_eq!(caches.len(), last_tokens.len(), "forward_decode_step_batch: one token per cache");
        let n = caches.len();
        let c = &self.config;

        // Each sequence's position for this new token is its own cache's
        // current length -- captured BEFORE any push below, same as
        // `forward_step`'s single-sequence `kv_offset`.
        let kv_offsets: Vec<usize> = caches.iter().map(|cache| cache.len()).collect();
        // `rope_inplace` takes one position per row with no consecutiveness
        // requirement (`ops.rs`: `assert_eq!(x.rows, positions.len())`,
        // indexes `positions[row]` independently) -- exactly what a batch
        // of sequences at different lengths needs.
        let positions = kv_offsets.clone();

        let mut hidden = Matrix::zeros(n, c.embed_dim);
        for (s, &id) in last_tokens.iter().enumerate() {
            hidden.row_mut(s).copy_from_slice(&self.token_embd.dequant_row(id as usize));
        }

        let mut normed = Matrix::zeros(n, c.embed_dim);
        let mut q = Matrix::zeros(n, c.n_heads * c.head_dim);
        let mut k_new = Matrix::zeros(n, c.n_kv_heads * c.head_dim);
        let mut v_new = Matrix::zeros(n, c.n_kv_heads * c.head_dim);
        let mut attn = Matrix::zeros(n, c.n_heads * c.head_dim);
        let mut attn_proj = Matrix::zeros(n, c.embed_dim);
        let mut normed2 = Matrix::zeros(n, c.embed_dim);
        let mut gate = Matrix::zeros(n, c.ffn_dim);
        let mut up = Matrix::zeros(n, c.ffn_dim);
        let mut down = Matrix::zeros(n, c.embed_dim);

        for (li, layer) in self.layers.iter().enumerate() {
            rmsnorm_into(&hidden, &layer.attn_norm, c.rms_eps, &mut normed);

            linear_quantized_into(&normed, &layer.wq, Some(&layer.bq), &mut q);
            linear_quantized_into(&normed, &layer.wk, Some(&layer.bk), &mut k_new);
            linear_quantized_into(&normed, &layer.wv, Some(&layer.bv), &mut v_new);

            rope_inplace(&mut q, c.n_heads, c.head_dim, c.rope_freq_base, &positions);
            rope_inplace(&mut k_new, c.n_kv_heads, c.head_dim, c.rope_freq_base, &positions);

            // Per-sequence: push this step's K/V row into THAT sequence's
            // own cache, then attend against that cache alone. Each
            // `causal_gqa_attention` call here is exactly what
            // `forward_step` already does for one sequence -- unchanged,
            // just invoked N times instead of once.
            for i in 0..n {
                let layer_cache = &mut caches[i].layers[li];
                layer_cache.k.push_row(k_new.row(i));
                layer_cache.v.push_row(v_new.row(i));

                let q_i = Matrix::from_vec(1, q.cols, q.row(i).to_vec());
                let attn_i = causal_gqa_attention(&q_i, &layer_cache.k, &layer_cache.v, c.n_heads, c.n_kv_heads, c.head_dim, kv_offsets[i]);
                attn.row_mut(i).copy_from_slice(attn_i.row(0));
            }

            linear_quantized_into(&attn, &layer.wo, None, &mut attn_proj);
            add_inplace(&mut hidden, &attn_proj);

            rmsnorm_into(&hidden, &layer.ffn_norm, c.rms_eps, &mut normed2);
            linear_quantized_dual_into(&normed2, &layer.w_gate, &layer.w_up, None, None, &mut gate, &mut up);
            silu_inplace(&mut gate);
            mul_inplace(&mut gate, &up);
            linear_quantized_into(&gate, &layer.w_down, None, &mut down);
            add_inplace(&mut hidden, &down);
        }

        let final_hidden = rmsnorm(&hidden, &self.output_norm, c.rms_eps);
        let lm_head = self.output_weight.as_ref().unwrap_or(&self.token_embd);
        let logits = linear_quantized(&final_hidden, lm_head, None);
        (0..n).map(|i| logits.row(i).to_vec()).collect()
    }
}

#[cfg(test)]
mod batch_tests {
    //! Fase 23, Milestone 1: `forward_decode_step_batch` must be bit-
    //! identical to calling `forward_step` once per sequence — same
    //! discipline as every fusion in this file (Fase 21's
    //! `linear_quantized_dual_into`), just proven with a tiny synthetic
    //! model (see `llama::forward`'s test module doc comment for why a
    //! synthetic model is the right tool here: no real GGUF needed, since
    //! the property under test — "does batching rows change the float
    //! result" — doesn't depend on the weights being semantically real).

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

    /// 2 layers (exercises the layer loop more than once), GQA
    /// (n_kv_heads < n_heads), tied embeddings.
    fn tiny_model() -> Model {
        let embed_dim = 4;
        let n_heads = 2;
        let n_kv_heads = 1;
        let head_dim = 2;
        let ffn_dim = 4;
        let vocab_size = 5;

        let make_layer = |seed: f32| LayerWeights {
            attn_norm: f32_vec(embed_dim, seed + 0.1),
            wq: f32_qmatrix(n_heads * head_dim, embed_dim, seed + 0.2),
            bq: f32_vec(n_heads * head_dim, seed + 0.21),
            wk: f32_qmatrix(n_kv_heads * head_dim, embed_dim, seed + 0.3),
            bk: f32_vec(n_kv_heads * head_dim, seed + 0.31),
            wv: f32_qmatrix(n_kv_heads * head_dim, embed_dim, seed + 0.4),
            bv: f32_vec(n_kv_heads * head_dim, seed + 0.41),
            wo: f32_qmatrix(embed_dim, n_heads * head_dim, seed + 0.5),
            ffn_norm: f32_vec(embed_dim, seed + 0.6),
            w_gate: f32_qmatrix(ffn_dim, embed_dim, seed + 0.7),
            w_up: f32_qmatrix(ffn_dim, embed_dim, seed + 0.8),
            w_down: f32_qmatrix(embed_dim, ffn_dim, seed + 0.9),
        };

        Model {
            config: Config {
                n_layers: 2,
                embed_dim,
                ffn_dim,
                n_heads,
                n_kv_heads,
                head_dim,
                vocab_size,
                rope_freq_base: 10000.0,
                rms_eps: 1e-5,
                context_length: 16,
            },
            token_embd: f32_qmatrix(vocab_size, embed_dim, 0.0),
            output_weight: None,
            layers: vec![make_layer(0.0), make_layer(1.0)],
            output_norm: f32_vec(embed_dim, 2.0),
        }
    }

    #[test]
    fn forward_decode_step_batch_matches_sequential_forward_step() {
        let model = tiny_model();
        let vocab = model.config.vocab_size as u32;

        // Four sequences with DIFFERENT prompt lengths, so by the time
        // batched decode starts each cache sits at a different position
        // (kv_offset) -- exactly the case a real scheduler would produce,
        // and the case that would expose a per-sequence-position bug that
        // same-length prompts could hide.
        let prompts: [&[u32]; 4] = [&[0, 1], &[2, 0, 1], &[1], &[0, 1, 2, 0]];
        const N: usize = 4;
        const STEPS: usize = 5;
        // Distinct, deterministic token per (sequence, step) -- not the
        // same token broadcast to every sequence -- so a row-mixing bug
        // (sequence i reading sequence j's activations) can't hide behind
        // sequences coincidentally computing the same thing that step.
        let decode_token = |seq: usize, step: usize| -> u32 { ((seq * 7 + step * 3 + 1) % vocab as usize) as u32 };

        // -- Sequential reference: forward_step, once per sequence per step.
        let mut seq_caches: Vec<KvCache> = (0..N).map(|_| KvCache::new(&model.config.cache_shape())).collect();
        for (i, prompt) in prompts.iter().enumerate() {
            model.forward_step(&mut seq_caches[i], prompt);
        }
        let mut sequential_logits = vec![vec![Vec::new(); N]; STEPS];
        for step in 0..STEPS {
            for i in 0..N {
                sequential_logits[step][i] = model.forward_step(&mut seq_caches[i], &[decode_token(i, step)]);
            }
        }

        // -- Batched: forward_decode_step_batch, once per step, all N sequences together.
        // Prefill stays sequential/unbatched by design (Milestone 1 only
        // batches the decode phase).
        let mut batch_caches: Vec<KvCache> = (0..N).map(|_| KvCache::new(&model.config.cache_shape())).collect();
        for (i, prompt) in prompts.iter().enumerate() {
            model.forward_step(&mut batch_caches[i], prompt);
        }
        let mut batch_logits = vec![vec![Vec::new(); N]; STEPS];
        for step in 0..STEPS {
            let tokens: Vec<u32> = (0..N).map(|i| decode_token(i, step)).collect();
            let mut refs: Vec<&mut KvCache> = batch_caches.iter_mut().collect();
            let step_out = model.forward_decode_step_batch(&mut refs, &tokens);
            batch_logits[step] = step_out;
        }

        for step in 0..STEPS {
            for i in 0..N {
                let (seq_l, batch_l) = (&sequential_logits[step][i], &batch_logits[step][i]);
                assert!(seq_l.iter().all(|v| v.is_finite()), "step {step} seq {i}: sequential logits contain NaN/Inf: {seq_l:?}");
                assert_eq!(seq_l, batch_l, "step {step} seq {i}: batched decode diverged from sequential forward_step");
            }
        }
    }

    /// Feeding a prompt to `forward_step` in slices must give bit-identical
    /// logits to feeding it all at once, for every chunk size that evenly
    /// divides the prompt AND every one that doesn't (so the last, short
    /// chunk gets exercised too).
    ///
    /// This is the correctness prerequisite for "chunked prefill" (Fase 26,
    /// roadmap §14.6) — the candidate fix for prefill's measured
    /// super-linear cost. `causal_gqa_attention`'s own
    /// `cached_attention_matches_full_recompute` test already proves the
    /// underlying math is chunk-size-invariant (kv_offset makes each call
    /// attend over exactly the history the previous calls filled); this test
    /// pins that guarantee down at the `Model::forward_step` level, across
    /// two layers and RoPE's position-dependent rotation, not just in the
    /// attention kernel alone.
    #[test]
    fn chunked_prefill_matches_one_shot_prefill() {
        let model = tiny_model();
        let vocab = model.config.vocab_size as u32;
        let prompt: Vec<u32> = (0..11).map(|i| (i * 3 + 1) % vocab).collect();

        let one_shot_last_logits = {
            let mut cache = KvCache::new(&model.config.cache_shape());
            model.forward_step(&mut cache, &prompt)
        };

        // 4 evenly divides 11? no -- deliberately, so the final chunk (3
        // tokens) is short. 11 also gets a chunk size that DOES divide it
        // evenly (11 -> single chunk of 11, i.e. one_shot itself, skipped as
        // redundant) and one that's larger than the whole prompt (chunk
        // count collapses to 1, same as one-shot but through the chunked
        // code path).
        for chunk_size in [1usize, 3, 4, 5, 20] {
            let mut cache = KvCache::new(&model.config.cache_shape());
            let mut last_logits = Vec::new();
            for slice in prompt.chunks(chunk_size) {
                last_logits = model.forward_step(&mut cache, slice);
            }
            assert_eq!(
                last_logits, one_shot_last_logits,
                "chunk_size={chunk_size}: chunked prefill diverged from one-shot prefill"
            );
        }
    }
}
