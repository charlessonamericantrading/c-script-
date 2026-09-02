//! Fase 25 — isolated PREFILL benchmark (roadmap §12's open step 2).
//!
//! Every performance measurement in this roadmap so far (Fases 9-21) timed the
//! DECODE loop — `forward_step` with one token at a time. §12 then found that
//! for JARVIS's real workload the prefill of a ~900-token system prompt
//! dominates the total cost (323.5s measured server-side in §13), and decode is
//! a rounding error next to it. But that number came through the HTTP server,
//! so it bundles transport, scheduling and prefix-cache logic together with the
//! actual compute.
//!
//! This binary separates them: no HTTP, no server, no prefix cache — just
//! `forward_step` with a whole prompt against a fresh `KvCache`, timed. Two
//! questions it answers, both of which decide whether Bloque C (GPU) reopens:
//!
//!   1. **Is prefill cost linear in prompt length?** If ms/token stays flat as
//!      the prompt grows, we're looking at the genuine per-token compute floor
//!      of this CPU (the roofline conclusion §12 reached by estimation), and no
//!      amount of transport-side tuning will help. If it curves upward, there's
//!      something super-linear in the implementation worth hunting.
//!   2. **How does prefill-per-token compare to decode-per-token?** Decode is
//!      bandwidth-bound (established across this whole roadmap). Prefill of a
//!      long prompt is a dense matmul over many rows — compute-bound. If
//!      prefill-per-token lands far above decode-per-token, that asymmetry is
//!      the concrete argument for a compute-heavy accelerator, and it's a
//!      genuinely different argument from the one that closed the GPU question
//!      for decode (shared DRAM, no extra bandwidth channel).
//!
//! Usage: qwen2-bench-prefill <model.gguf> [lens=64,128,256,512,896] [rounds=3]
//!
//! Prefill and decode figures are reported as SEPARATE columns and never fused
//! into one "faster" number, per the roadmap's own rule (section 4).
//!
//! Round order alternates the direction in which prompt lengths are swept
//! (ascending on even rounds, descending on odd) — the Fase 18 lesson applied
//! to a scaling sweep rather than an A/B: if the ms/token curve only looks
//! linear when measured in one direction, that's thermal drift across the
//! round, not the model's real scaling behaviour.

use std::env;
use std::fs;
use std::process::ExitCode;
use std::time::Instant;

use model_core::KvCache;
use qwen2::Model;

/// Deterministic synthetic prompt. Content is irrelevant to prefill cost —
/// the work is the same dense matmuls regardless of which token ids they run
/// on — but it must stay inside the vocab, and it must be reproducible so two
/// runs of this binary time the identical workload.
fn synthetic_prompt(len: usize, vocab_size: usize) -> Vec<u32> {
    (0..len).map(|i| ((i * 7919 + 13) % vocab_size) as u32).collect()
}

/// Time a cold prefill: fresh cache, whole prompt in one `forward_step`.
/// Returns (elapsed_ms, logit_checksum).
///
/// The checksum exists solely to consume the returned logits so the call
/// can't be optimized away as dead code — it is not a correctness check
/// (correctness lives in the golden-diff harness, not here).
fn time_prefill(model: &Model, prompt: &[u32]) -> (u128, f32) {
    let mut cache = KvCache::new(&model.config.cache_shape());
    let start = Instant::now();
    let logits = model.forward_step(&mut cache, prompt);
    let ms = start.elapsed().as_millis();
    (ms, logits.iter().take(8).sum())
}

/// Time a CHUNKED prefill: same prompt, same fresh cache, but fed to
/// `forward_step` in slices of `chunk` tokens instead of all at once.
///
/// Mathematically identical to the one-shot version — each chunk attends over
/// the cache the previous chunks filled, which is exactly what `kv_offset` is
/// for — so any time difference is pure memory behaviour, not less work.
///
/// The reason to try it: the intermediate buffers `forward_step` allocates are
/// sized `seq_new x dim`, so they grow with the prompt. For qwen2.5:0.5b the
/// FFN buffers (`gate`/`up`, 4864 wide) are ~2.5MB at 128 tokens but ~17.4MB at
/// 896 — and this machine's L3 is 12MB. If prefill's measured super-linear cost
/// comes from spilling out of cache, slicing the prompt into chunks that fit
/// should be FASTER despite doing identical arithmetic. If the timings come out
/// flat instead, the cache-pressure hypothesis is wrong and the super-linearity
/// is somewhere else.
fn time_prefill_chunked(model: &Model, prompt: &[u32], chunk: usize) -> (u128, f32) {
    let mut cache = KvCache::new(&model.config.cache_shape());
    let mut checksum = 0f32;
    let start = Instant::now();
    for slice in prompt.chunks(chunk) {
        let logits = model.forward_step(&mut cache, slice);
        checksum = logits.iter().take(8).sum();
    }
    (start.elapsed().as_millis(), checksum)
}

/// Time `steps` single-token decode steps against a cache primed with a FIXED
/// short context, independent of the prompt length being swept.
///
/// The fixed context is the whole point. An earlier version of this benchmark
/// primed the cache with the same prompt whose prefill it was measuring, so the
/// decode column silently carried a second variable: attention cost grows with
/// context length, meaning the 896-token row's decode was doing strictly more
/// work than the 64-token row's. That makes the rows incomparable and the
/// prefill/decode ratio meaningless. Decode is a per-token BASELINE here — it
/// must measure the same thing on every row.
///
/// The priming prefill stays outside the timed region either way.
fn time_decode(model: &Model, steps: usize) -> (u128, f32) {
    const BASELINE_CONTEXT: usize = 16;
    let vocab = model.config.vocab_size;
    let prompt = synthetic_prompt(BASELINE_CONTEXT, vocab);
    let mut cache = KvCache::new(&model.config.cache_shape());
    model.forward_step(&mut cache, &prompt);

    let mut checksum = 0f32;
    let start = Instant::now();
    for step in 0..steps {
        let token = ((step * 31 + 5) % vocab) as u32;
        let logits = model.forward_step(&mut cache, &[token]);
        checksum += logits.iter().take(8).sum::<f32>();
    }
    (start.elapsed().as_millis(), checksum)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: qwen2-bench-prefill <model.gguf> [lens=64,128,256,512,896] [rounds=3]");
        eprintln!();
        eprintln!("  lens    comma-separated prompt lengths in tokens");
        eprintln!("  rounds  sweep repetitions; direction alternates per round");
        return ExitCode::FAILURE;
    }
    let model_path = &args[1];
    let lens: Vec<usize> = args
        .get(2)
        .map(|s| s.split(',').filter_map(|p| p.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![64, 128, 256, 512, 896]);
    let rounds: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);

    if lens.is_empty() {
        eprintln!("error: no valid prompt lengths parsed");
        return ExitCode::FAILURE;
    }

    let bytes = match fs::read(model_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: could not read {model_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Model load is timed and reported separately: it's ~4.7GB for coder:7b and
    // would otherwise be silently folded into whichever measurement ran first,
    // which is exactly the confound §9 had to correct for with a two-point
    // estimate when measuring ms/tok.
    let load_start = Instant::now();
    let model = match Model::load(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: failed to load model: {e}");
            return ExitCode::FAILURE;
        }
    };
    let load_ms = load_start.elapsed().as_millis();
    eprintln!("model loaded in {load_ms} ms ({} bytes)", bytes.len());

    let vocab = model.config.vocab_size;
    const DECODE_STEPS: usize = 8;

    println!("round,direction,prompt_len,call_order,prefill_ms,prefill_ms_per_tok,decode_ms_per_tok,prefill_vs_decode,chunk128_ms,chunk256_ms,best_chunk_speedup");
    // Counts every (round, len) row across the whole run, independent of the
    // outer sweep direction — used below to flip which of the three prefill
    // variants runs first/last, on its own cadence from the length-sweep
    // alternation.
    let mut call_index: usize = 0;
    for r in 0..rounds {
        let ascending = r % 2 == 0;
        let mut sweep = lens.clone();
        if !ascending {
            sweep.reverse();
        }
        let direction = if ascending { "asc" } else { "desc" };

        for &len in &sweep {
            let prompt = synthetic_prompt(len, vocab);
            let (decode_ms, _) = time_decode(&model, DECODE_STEPS);

            // The one-shot vs. chunked comparison is itself an A/B test and
            // needs its own order guard, separate from the outer length-sweep
            // direction above. The original version always ran one-shot ->
            // chunk128 -> chunk256 in that fixed order every single row — so
            // whichever variant ran last always did so on the hottest CPU
            // state. That is exactly the confound Fase 18 found for a plain
            // two-arm A/B, just one level up: within a single row, run order
            // was never varied at all. Flipping it row-by-row (not tied to
            // the direction flag above, so the two axes vary independently)
            // is what actually tests whether "chunking wins" survives this,
            // rather than just re-confirming Fase 18's lesson a second time.
            let reverse_order = call_index % 2 == 1;
            call_index += 1;

            let run_one_shot = |m: &Model| time_prefill(m, &prompt).0;
            let run_chunk128 = |m: &Model| if len > 128 { time_prefill_chunked(m, &prompt, 128).0 } else { run_one_shot(m) };
            let run_chunk256 = |m: &Model| if len > 256 { time_prefill_chunked(m, &prompt, 256).0 } else { run_one_shot(m) };

            let (prefill_ms, chunk128_ms, chunk256_ms, call_order) = if !reverse_order {
                let a = run_one_shot(&model);
                let b = run_chunk128(&model);
                let c = run_chunk256(&model);
                (a, b, c, "fwd")
            } else {
                let c = run_chunk256(&model);
                let b = run_chunk128(&model);
                let a = run_one_shot(&model);
                (a, b, c, "rev")
            };

            let prefill_per_tok = prefill_ms as f64 / len as f64;
            let decode_per_tok = decode_ms as f64 / DECODE_STEPS as f64;
            // Cost of a prompt token relative to a generated token at a fixed
            // short context. A value near 1.0 is the finding that matters: the
            // weight blocks are read ONCE for all `len` prompt tokens (verified
            // in `ops::linear_quantized_range` — the loop over input rows sits
            // inside the loop over weight blocks), so if a prompt token still
            // costs about as much as a decode token, prefill cannot be
            // bandwidth-bound. It is compute-bound, which is a different
            // regime from decode and admits different hardware answers.
            let ratio = if decode_per_tok > 0.0 { prefill_per_tok / decode_per_tok } else { f64::NAN };

            let best_chunk = chunk128_ms.min(chunk256_ms);
            let chunk_speedup = if best_chunk > 0 { prefill_ms as f64 / best_chunk as f64 } else { f64::NAN };

            println!(
                "{r},{direction},{len},{call_order},{prefill_ms},{prefill_per_tok:.2},{decode_per_tok:.2},{ratio:.2},{chunk128_ms},{chunk256_ms},{chunk_speedup:.2}"
            );
        }
    }

    ExitCode::SUCCESS
}
