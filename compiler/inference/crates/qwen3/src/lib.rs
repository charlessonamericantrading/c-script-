//! Qwen3 (dense) and Qwen3MoE (mixture-of-experts) model graph — GGUF
//! weight loading + forward pass. See `model.rs`'s module doc comment for
//! the full architectural recipe, exact source citations, and the
//! standing "unverified-in-anger, no real checkpoint available locally"
//! caveat that applies to this crate the same way it does to `llama`'s
//! Mistral/classic-Llama support.
//!
//! Tokenizer and chat template are NOT reimplemented here — `Tokenizer`
//! and `render_prompt_ids` are re-exported directly from `qwen2`. This
//! isn't a shortcut: Qwen3 genuinely uses the exact same tokenizer as
//! Qwen2/Qwen2.5 (same `tokenizer.ggml.model`="gpt2"/`tokenizer.ggml.pre`=
//! "qwen2" byte-level BPE vocab+merges convention, confirmed against
//! llama.cpp's own checksum-based pre-tokenizer detector, which maps a
//! Qwen3 checkpoint's vocab to `res = "qwen2"`) and the same ChatML chat
//! template skeleton (`<|im_start|>{role}\n{content}<|im_end|>\n`,
//! confirmed against Qwen3-8B's own `tokenizer_config.json`). Qwen3's
//! `<think>`/`</think>` "thinking mode" tokens are ordinary vocabulary
//! entries (not special/control tokens), and the mode itself is a pure
//! prompt-formatting convention (optionally pre-seeding
//! `<think>\n\n</think>\n\n` after the assistant role tag to force-skip
//! reasoning) — deliberately NOT implemented here, the same kind of
//! flagged-but-deferred gap as Mistral's own distinct chat template in
//! `llama::chat_template`. Reusing `qwen2::Tokenizer` unmodified means
//! this deferred feature is the only chat-template difference from Qwen2
//! a caller could ever notice.

pub mod api;
pub mod error;
pub mod forward;
pub mod model;

pub use error::LoadError;
pub use model::Model;
pub use qwen2::render_prompt_ids;
pub use qwen2::Tokenizer;
