use std::env;
use std::fs;
use std::io::Write;
use std::process::ExitCode;
use std::time::Instant;

use gemma4::{Model, Tokenizer};
use gguf::GgufFile;
use model_core::{argmax, KvCache};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: gemma4-generate <path-to-model.gguf> <prompt> [max_new_tokens]");
        return ExitCode::FAILURE;
    }
    let model_path = &args[1];
    let prompt = &args[2];
    let max_new_tokens: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20);

    let bytes = match fs::read(model_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: could not read {model_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let t_load = Instant::now();
    let gguf = match GgufFile::parse(&bytes) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: failed to parse gguf: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tokenizer = match Tokenizer::from_gguf(&gguf) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: failed to load tokenizer: {e}");
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
    eprintln!("[loaded model + tokenizer in {:.2}s]", t_load.elapsed().as_secs_f64());

    let prompt_ids = tokenizer.encode(prompt);
    eprintln!("[prompt tokens: {prompt_ids:?}]");
    if env::var("DEBUG_FORWARD").is_ok() {
        eprintln!("[round trip: {:?}]", tokenizer.decode(&prompt_ids));
        eprintln!("[config: {:?}]", model.config);
        eprintln!("[tied embeddings: {}]", model.output_weight.is_none());
    }

    let mut cache = KvCache::new(&model.config.cache_shape());

    if env::var("TOP_K").is_ok() {
        let logits = model.forward_step(&mut cache, &prompt_ids);
        let mut indexed: Vec<(usize, f32)> = logits.iter().copied().enumerate().collect();
        indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        eprintln!("[top-10 next-token candidates]");
        for &(id, logit) in indexed.iter().take(10) {
            eprintln!("  id={id:<7} logit={logit:.4}  text={:?}", tokenizer.decode(&[id as u32]));
        }
        return ExitCode::SUCCESS;
    }

    // Prefill: feed the whole prompt in one forward_step call, populating
    // the KV-cache for every prompt position at once. Timed separately from
    // decode: prefill is a one-off batched cost, decode is what's directly
    // comparable to Ollama's per-token ms/token figures.
    let t_prefill = Instant::now();
    let mut logits = model.forward_step(&mut cache, &prompt_ids);
    let prefill_secs = t_prefill.elapsed().as_secs_f64();

    let mut generated = Vec::new();
    let mut decode_secs = 0.0f64;
    for step in 0..max_new_tokens {
        let next = argmax(&logits);
        if Some(next) == tokenizer.eos_token_id {
            eprintln!("[eos at step {step}]");
            break;
        }
        generated.push(next);
        eprint!(".");
        std::io::stderr().flush().ok();

        if step + 1 == max_new_tokens {
            break;
        }
        let t_decode_step = Instant::now();
        logits = model.forward_step(&mut cache, &[next]);
        decode_secs += t_decode_step.elapsed().as_secs_f64();
    }
    eprintln!();
    // decode_tokens = generated.len() - 1: the first generated token's logits
    // came from prefill, not from a decode call.
    let decode_tokens = generated.len().saturating_sub(1);
    eprintln!(
        "[prefill: {} tokens in {:.3}s | decode: {decode_tokens} tokens in {:.3}s ({:.2} ms/tok, {:.2} tok/s)]",
        prompt_ids.len(),
        prefill_secs,
        decode_secs,
        if decode_tokens > 0 { decode_secs * 1000.0 / decode_tokens as f64 } else { 0.0 },
        if decode_secs > 0.0 { decode_tokens as f64 / decode_secs } else { 0.0 },
    );

    println!("{}", tokenizer.decode(&generated));
    println!();
    println!("generated_ids={generated:?}");

    ExitCode::SUCCESS
}
