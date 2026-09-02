//! The actual GGUF-to-resident-model pipeline: read the whole file, parse
//! its header, dispatch on `general.architecture` to build the right
//! `LanguageModel`/`ModelTokenizer` pair. Used to live inline in
//! `bin/server.rs` and run once per `--model` at startup; now called by
//! `routes::ServerState`'s lazy-loading cache (see that module's doc
//! comment) on first request for a given model instead.

use std::fs;
use std::sync::Arc;
use std::time::Instant;

use gguf::GgufFile;
use model_core::{LanguageModel, ModelTokenizer};

use crate::routes::ModelEntry;

/// Registered architectures — one match arm per crate implementing
/// `model_core::{LanguageModel, ModelTokenizer}`. Adding an architecture
/// means a new crate + one arm here, nothing else in `server`/`routes`
/// changes.
pub fn load_model(name: &str, path: &str) -> Result<ModelEntry, String> {
    eprintln!("[loading {name} from {path} ...]");
    let t0 = Instant::now();
    let bytes = fs::read(path).map_err(|e| format!("could not read {path}: {e}"))?;
    let file_size = bytes.len() as u64;
    let gguf = GgufFile::parse(&bytes).map_err(|e| format!("failed to parse GGUF: {e}"))?;
    let arch = gguf.architecture().unwrap_or("").to_string();

    let (model, tokenizer): (Arc<dyn LanguageModel>, Arc<dyn ModelTokenizer>) = match arch.as_str() {
        "qwen2" => {
            let tokenizer =
                qwen2::Tokenizer::from_gguf(&gguf).map_err(|e| format!("failed to load tokenizer: {e}"))?;
            let model = qwen2::Model::load(&bytes).map_err(|e| format!("failed to load model: {e}"))?;
            eprintln!(
                "[loaded {name} in {:.2}s, tied_embeddings={}]",
                t0.elapsed().as_secs_f64(),
                model.output_weight.is_none()
            );
            (Arc::new(model), Arc::new(tokenizer))
        }
        "llama" => {
            let tokenizer =
                llama::Tokenizer::from_gguf(&gguf).map_err(|e| format!("failed to load tokenizer: {e}"))?;
            let model = llama::Model::load(&bytes).map_err(|e| format!("failed to load model: {e}"))?;
            eprintln!(
                "[loaded {name} in {:.2}s, tied_embeddings={}]",
                t0.elapsed().as_secs_f64(),
                model.output_weight.is_none()
            );
            (Arc::new(model), Arc::new(tokenizer))
        }
        "gemma4" => {
            let tokenizer =
                gemma4::Tokenizer::from_gguf(&gguf).map_err(|e| format!("failed to load tokenizer: {e}"))?;
            let model = gemma4::Model::load(&bytes).map_err(|e| format!("failed to load model: {e}"))?;
            eprintln!(
                "[loaded {name} in {:.2}s, tied_embeddings={}]",
                t0.elapsed().as_secs_f64(),
                model.output_weight.is_none()
            );
            (Arc::new(model), Arc::new(tokenizer))
        }
        "phi3" => {
            let tokenizer =
                phi3::Tokenizer::from_gguf(&gguf).map_err(|e| format!("failed to load tokenizer: {e}"))?;
            let model = phi3::Model::load(&bytes).map_err(|e| format!("failed to load model: {e}"))?;
            eprintln!(
                "[loaded {name} in {:.2}s, tied_embeddings={}]",
                t0.elapsed().as_secs_f64(),
                model.output_weight.is_none()
            );
            (Arc::new(model), Arc::new(tokenizer))
        }
        // Two distinct GGUF architecture strings, one crate -- qwen3's own
        // module doc comment explains why (they share every convention
        // except the FFN block, which the crate already dispatches on
        // per-layer).
        "qwen3" | "qwen3moe" => {
            let tokenizer =
                qwen3::Tokenizer::from_gguf(&gguf).map_err(|e| format!("failed to load tokenizer: {e}"))?;
            let model = qwen3::Model::load(&bytes).map_err(|e| format!("failed to load model: {e}"))?;
            eprintln!(
                "[loaded {name} in {:.2}s, tied_embeddings={}]",
                t0.elapsed().as_secs_f64(),
                model.output_weight.is_none()
            );
            (Arc::new(model), Arc::new(tokenizer))
        }
        "phimoe" => {
            let tokenizer =
                phimoe::Tokenizer::from_gguf(&gguf).map_err(|e| format!("failed to load tokenizer: {e}"))?;
            let model = phimoe::Model::load(&bytes).map_err(|e| format!("failed to load model: {e}"))?;
            eprintln!("[loaded {name} in {:.2}s]", t0.elapsed().as_secs_f64());
            (Arc::new(model), Arc::new(tokenizer))
        }
        other => {
            return Err(format!(
                "architecture '{other}' is not supported yet (only 'qwen2', 'llama', 'gemma4', 'phi3', 'qwen3', 'qwen3moe', 'phimoe' are registered) — model '{name}' at {path}"
            ))
        }
    };

    Ok(ModelEntry { name: name.to_string(), architecture: arch, model, tokenizer, file_size })
}
