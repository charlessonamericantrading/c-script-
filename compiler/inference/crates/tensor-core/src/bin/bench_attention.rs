//! Fase 26 — isolated cost of `causal_gqa_attention` during PREFILL.
//!
//! The prefill benchmark (roadmap §14.5) showed prefill cost per token grows
//! with prompt length (~31 ms/tok at 128 tokens, ~58 at 896), which points at
//! causal attention's O(N^2) term — every query row attends over every earlier
//! key. But that was inferred from the shape of a curve measured through the
//! whole forward pass. This measures the term directly.
//!
//! No checkpoint is loaded: attention cost depends only on shapes
//! (seq_len, n_q_heads, n_kv_heads, head_dim), never on weight values, so
//! synthetic matrices of the right dimensions measure exactly the same work as
//! the real thing — and skip 4.7GB of I/O to do it.
//!
//! Defaults are qwen2.5:0.5b's real geometry, read from the checkpoint with
//! `gguf-inspect`: 24 layers, embedding_length 896, head_count 14,
//! head_count_kv 2 (so head_dim = 896/14 = 64).
//!
//! Usage: bench-attention [seq_lens=128,512,896] [rounds=3]
//!                        [n_q_heads=14] [n_kv_heads=2] [head_dim=64] [layers=24]
//!
//! Reports per-layer time and the whole-forward extrapolation (per-layer x
//! layers), which is the number to compare against the total prefill time from
//! `qwen2-bench-prefill`.

use std::env;
use std::process::ExitCode;
use std::time::Instant;

use tensor_core::ops::causal_gqa_attention;
use tensor_core::Matrix;

/// Deterministic pseudo-random fill. Values are bounded and varied so softmax
/// sees a realistic spread — a matrix of zeros would make every score equal,
/// which is still the same amount of arithmetic but makes the numbers look
/// suspiciously tidy if anyone reruns this by hand.
fn filled(rows: usize, cols: usize, seed: usize) -> Matrix {
    let data = (0..rows * cols)
        .map(|i| {
            let x = (i * 2654435761 + seed * 40503) % 2048;
            (x as f32 / 1024.0) - 1.0
        })
        .collect();
    Matrix::from_vec(rows, cols, data)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let seq_lens: Vec<usize> = args
        .get(1)
        .map(|s| s.split(',').filter_map(|p| p.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![128, 512, 896]);
    let rounds: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
    let n_q_heads: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(14);
    let n_kv_heads: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(2);
    let head_dim: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(64);
    let layers: usize = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(24);

    if seq_lens.is_empty() {
        eprintln!("error: no valid sequence lengths parsed");
        return ExitCode::FAILURE;
    }

    eprintln!(
        "geometry: n_q_heads={n_q_heads} n_kv_heads={n_kv_heads} head_dim={head_dim} layers={layers}"
    );

    println!("round,direction,seq_len,per_layer_ms,all_layers_ms,ms_per_token,quadratic_index");
    for r in 0..rounds {
        let ascending = r % 2 == 0;
        let mut sweep = seq_lens.clone();
        if !ascending {
            sweep.reverse();
        }
        let direction = if ascending { "asc" } else { "desc" };

        for &n in &sweep {
            // Prefill shape: q covers the whole prompt, k/v cover exactly the
            // same rows, kv_offset = 0. This is the call qwen2::forward_step
            // makes once per layer when handed a full prompt.
            let q = filled(n, n_q_heads * head_dim, 1);
            let k = filled(n, n_kv_heads * head_dim, 2);
            let v = filled(n, n_kv_heads * head_dim, 3);

            let start = Instant::now();
            let out = causal_gqa_attention(&q, &k, &v, n_q_heads, n_kv_heads, head_dim, 0);
            let per_layer_us = start.elapsed().as_micros();

            // Consume the result so the call cannot be optimized away.
            std::hint::black_box(&out);

            let per_layer_ms = per_layer_us as f64 / 1000.0;
            let all_layers_ms = per_layer_ms * layers as f64;
            let ms_per_token = all_layers_ms / n as f64;
            // Work per query row grows linearly with n, so total work grows as
            // n^2. Dividing by n^2 gives a constant IF the cost really is
            // quadratic — a flat column here confirms the O(N^2) claim, a
            // drifting one refutes it.
            let quadratic_index = all_layers_ms / (n * n) as f64 * 1_000_000.0;

            println!(
                "{r},{direction},{n},{per_layer_ms:.2},{all_layers_ms:.1},{ms_per_token:.3},{quadratic_index:.3}"
            );
        }
    }

    ExitCode::SUCCESS
}
