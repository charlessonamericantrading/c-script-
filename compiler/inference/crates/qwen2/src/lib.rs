//! Qwen2 model graph: GGUF weight loading, tokenizer, and the forward pass.
//! `KvCache`/`CacheShape`/`ChatMessage`/`argmax` live in `model_core` now
//! (Fase 0 of multi-architecture support) — this crate re-implements
//! `model_core::{LanguageModel, ModelTokenizer}` for its `Model`/`Tokenizer`
//! in `api.rs` rather than defining its own copies.

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
