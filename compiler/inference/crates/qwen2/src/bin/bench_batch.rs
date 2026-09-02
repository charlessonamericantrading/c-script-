//! Fase 23, Milestone 1 — aggregate-throughput A/B benchmark for
//! `forward_decode_step_batch` vs. N independent `forward_step` calls.
//!
//! Deliberately not a criterion-style microbenchmark: same minimal
//! spawnSync-free, no-new-deps approach as `diff-snapshot.mjs` and every
//! other benchmark in this roadmap. Both paths run inside ONE process (the
//! model is loaded once, shared by reference for both), so there's no
//! process-spawn or model-load noise between the two arms being compared —
//! only `std::time::Instant` around the timed segment. Round order
//! alternates (normal: sequential-then-batched; reversed: batched-then-
//! sequential) per the Fase 18 lesson: a "consistent" win that flips sign
//! under reversal is thermal/system bias, not a real effect.
//!
//! Usage: qwen2-bench-batch <model.gguf> [n_sequences] [decode_steps] [rounds]
//!
//! Reports AGGREGATE tokens/s (n_sequences * decode_steps / wall_time) for
//! each arm, per round — never mixed with single-sequence latency, per the
//! roadmap's own explicit rule (section 4).

use std::env;
use std::fs;
use std::process::ExitCode;
use std::time::Instant;

use model_core::KvCache;
use qwen2::Model;

fn build_primed_caches(model: &Model, prompt: &[u32], n: usize) -> Vec<KvCache> {
    (0..n)
        .map(|_| {
            let mut cache = KvCache::new(&model.config.cache_shape());
            model.forward_step(&mut cache, prompt);
            cache
        })
        .collect()
}

/// Distinct, deterministic token per (sequence, step) — matches the
/// correctness test in `forward.rs`, not a broadcast constant, so this
/// exercises the same "no row-mixing" shape of workload it validated.
fn decode_token(seq: usize, step: usize, vocab_size: usize) -> u32 {
    ((seq * 7 + step * 3 + 1) % vocab_size) as u32
}

fn time_sequential(model: &Model, caches: &mut [KvCache], steps: usize) -> u128 {
    let vocab = model.config.vocab_size;
    let start = Instant::now();
    for step in 0..steps {
        for (i, cache) in caches.iter_mut().enumerate() {
            model.forward_step(cache, &[decode_token(i, step, vocab)]);
        }
    }
    start.elapsed().as_millis()
}

fn time_batched(model: &Model, caches: &mut [KvCache], steps: usize) -> u128 {
    let vocab = model.config.vocab_size;
    let n = caches.len();
    let start = Instant::now();
    for step in 0..steps {
        let tokens: Vec<u32> = (0..n).map(|i| decode_token(i, step, vocab)).collect();
        let mut refs: Vec<&mut KvCache> = caches.iter_mut().collect();
        model.forward_decode_step_batch(&mut refs, &tokens);
    }
    start.elapsed().as_millis()
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: qwen2-bench-batch <model.gguf> [n_sequences=8] [decode_steps=40] [rounds=8]");
        return ExitCode::FAILURE;
    }
    let model_path = &args[1];
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    let steps: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(40);
    let rounds: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(8);

    let bytes = match fs::read(model_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: could not read {model_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let model = match Model::load(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: failed to load model: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Short fixed prompt, valid for any vocab — content doesn't matter,
    // only that prefill leaves every cache at the same starting kv_offset
    // before the timed decode segment begins.
    let prompt: Vec<u32> = vec![1, 2, 3, 4, 5];

    println!("round,order,n,steps,sequential_ms,batched_ms,sequential_tok_s,batched_tok_s");
    for r in 0..rounds {
        let normal_order = r % 2 == 0;
        let (seq_ms, batch_ms) = if normal_order {
            let mut c1 = build_primed_caches(&model, &prompt, n);
            let s = time_sequential(&model, &mut c1, steps);
            let mut c2 = build_primed_caches(&model, &prompt, n);
            let b = time_batched(&model, &mut c2, steps);
            (s, b)
        } else {
            let mut c2 = build_primed_caches(&model, &prompt, n);
            let b = time_batched(&model, &mut c2, steps);
            let mut c1 = build_primed_caches(&model, &prompt, n);
            let s = time_sequential(&model, &mut c1, steps);
            (s, b)
        };
        let total_tokens = (n * steps) as f64;
        let seq_tok_s = total_tokens / (seq_ms as f64 / 1000.0);
        let batch_tok_s = total_tokens / (batch_ms as f64 / 1000.0);
        let order = if normal_order { "normal" } else { "reversed" };
        println!("{r},{order},{n},{steps},{seq_ms},{batch_ms},{seq_tok_s:.1},{batch_tok_s:.1}");
    }

    ExitCode::SUCCESS
}
