//! GGUF weight loading, tokenizer, and forward pass for every checkpoint
//! that shares `general.architecture = "llama"` — genuinely more than just
//! Llama 3.x. `model.rs`/`forward.rs` need no per-family changes (same
//! tensor names, same metadata convention); `tokenizer.rs` dispatches on
//! TWO independent axes since that's where the real per-family
//! differences live:
//!   - `tokenizer.ggml.model`: `"gpt2"` (byte-level BPE) vs `"llama"`
//!     (genuine SentencePiece — a different algorithm, not a pretokenizer
//!     variant).
//!   - `tokenizer.ggml.pre` (BPE only): `"llama3"`/`"llama-bpe"` (Llama
//!     3.x) vs `"tekken"` (modern Mistral NeMo/Small/Devstral).
//!
//! Together this covers Llama 3.x, classic Llama 1/2 (SPM), modern
//! Tekken-tokenizer Mistral (BPE), and classic pre-Tekken Mistral (SPM,
//! tokenizes/runs correctly but gets Llama-2's chat template rather than
//! Mistral's own — a real, flagged gap, see `chat_template.rs`). VERIFIED
//! real, not hypothetical, that Mistral of any vintage commonly reports
//! this same `general.architecture` string: `conversion/mistral.py`'s
//! `MistralModel.__init__` only switches to its own `"mistral3"` arch when
//! the source HF config has `llama_4_scaling`, a very recent mechanism
//! most Mistral releases don't have. Fase 1 of the multi-architecture plan
//! — see `model-core`'s crate doc comment for the trait boundary this
//! implements.
//!
//! Confidence tiers (be precise about what's been checked vs. assumed —
//! this crate has NOT been run against a real Llama or Mistral GGUF file,
//! per the standing "no model downloads" constraint; see each module's doc
//! comment for what backs it):
//!   - `model.rs`/`forward.rs` — HIGH: reuses the exact `tensor_core::ops`
//!     primitives `qwen2::forward` already exercises (all covered by
//!     tensor-core's own passing tests), same GGUF tensor-naming convention
//!     llama.cpp shares across architectures. The two real differences (no
//!     QKV bias; `llama.*` metadata prefix) are stable, well-established
//!     GGUF/llama.cpp convention, not per-checkpoint variables.
//!   - `tokenizer.rs`'s three tokenizer variants (two BPE pretokenizers,
//!     one full SPM engine) and `chat_template.rs`'s two template families
//!     — VERIFIED against Meta's/llama.cpp's/gguf-py's own source this
//!     session (see those files' doc comments for exact commit/URL), not
//!     recalled from memory.
//!   - What NONE of the above covers: an actual real-weights forward pass
//!     has never run end-to-end. Structural/unit tests (this crate's own
//!     `#[cfg(test)]` modules, all synthetic data) are the current ceiling.
//!     Treat this crate as unverified-in-anger until it's run against a
//!     real Llama 3.x or Mistral GGUF.

pub mod api;
pub mod chat_template;
pub mod error;
pub mod forward;
pub mod model;
pub mod tokenizer;

pub use chat_template::render_prompt_ids;
pub use error::LoadError;
pub use model::Model;
pub use tokenizer::Tokenizer;
