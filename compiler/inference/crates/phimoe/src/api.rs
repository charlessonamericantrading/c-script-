//! Implements the `model_core` trait boundary for this crate's `Model`.
//! `Tokenizer`'s `ModelTokenizer` impl comes for free from `phi3` (see
//! `lib.rs`'s doc comment on why the tokenizer/chat template are reused
//! rather than reimplemented) — nothing to add here for it.

use model_core::{CacheShape, KvCache, LanguageModel};

use crate::model::Model;

impl LanguageModel for Model {
    fn forward_step(&self, cache: &mut KvCache, new_tokens: &[u32]) -> Vec<f32> {
        Model::forward_step(self, cache, new_tokens)
    }

    fn cache_shape(&self) -> CacheShape {
        self.config.cache_shape()
    }

    fn vocab_size(&self) -> usize {
        self.config.vocab_size
    }
}
