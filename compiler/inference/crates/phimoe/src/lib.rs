//! PhiMoE (Microsoft's "Phi-3.5-MoE") model graph — GGUF weight loading +
//! forward pass. See `model.rs`'s module doc comment for the full
//! architectural recipe, exact source citations, and the standing
//! "unverified-in-anger, no real checkpoint available locally" caveat
//! this crate shares with `llama`'s Mistral/classic-Llama support and
//! `qwen3`.
//!
//! Tokenizer and chat template are NOT reimplemented here — `Tokenizer`
//! and `render_prompt_ids` are re-exported directly from `phi3`. Verified,
//! not assumed: PhiMoE's HF conversion class (`PhiMoeModel`) inherits
//! `Phi3MiniModel.set_vocab()` unmodified (same SentencePiece tokenizer),
//! and llama.cpp has no separate chat-template entry for `"phimoe"` — the
//! real `microsoft/Phi-3.5-MoE-instruct` checkpoint's own
//! `tokenizer_config.json` uses the identical `<|role|>`/`<|end|>` skeleton
//! and special-token IDs as dense Phi-3.
//!
//! Two of this crate's structural choices are deliberate matches to
//! llama.cpp's own (verified, not accidental) simplifications relative to
//! Microsoft's reference implementation — see `model.rs`'s module doc
//! comment for the full reasoning on both:
//!   - Norms are RMSNorm-plus-bias, not the true mean-centering LayerNorm
//!     Microsoft's reference math specifies.
//!   - MoE routing is plain softmax + top-k + renormalize
//!     (`tensor_core::ops::moe_route`), not Microsoft's more elaborate
//!     "SparseMixer" dual-masked-softmax scheme.
//!
//! Both are matched because any real PhiMoE GGUF file was produced by (and
//! any Ollama-compatible consumer expects) llama.cpp's actual conversion
//! and inference behavior, not a from-scratch reimplementation of
//! Microsoft's bit-exact training-time reference.

pub mod api;
pub mod error;
pub mod forward;
pub mod model;

pub use error::LoadError;
pub use model::Model;
pub use phi3::render_prompt_ids;
pub use phi3::Tokenizer;
