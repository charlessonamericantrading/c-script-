//! Implements the `model_core` trait boundary (Fase 0 of multi-architecture
//! support) for this crate's `Model`/`Tokenizer`. A thin adapter layer, kept
//! separate from the actual forward-pass/tokenizer logic so those stay easy
//! to read on their own — every method here just delegates to the existing
//! inherent method or field of the same name.

use model_core::{CacheShape, ChatMessage, KvCache, LanguageModel, ModelTokenizer};

use crate::chat_template;
use crate::model::Model;
use crate::tokenizer::Tokenizer;

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

    fn forward_step_batch(&self, caches: &mut [&mut KvCache], new_tokens: &[u32]) -> Vec<Vec<f32>> {
        Model::forward_decode_step_batch(self, caches, new_tokens)
    }
}

impl ModelTokenizer for Tokenizer {
    fn encode(&self, text: &str) -> Vec<u32> {
        Tokenizer::encode(self, text)
    }

    fn decode(&self, ids: &[u32]) -> String {
        Tokenizer::decode(self, ids)
    }

    fn decode_bytes(&self, ids: &[u32]) -> Vec<u8> {
        Tokenizer::decode_bytes(self, ids)
    }

    fn eos_token_id(&self) -> Option<u32> {
        self.eos_token_id
    }

    fn render_prompt_ids(&self, messages: &[ChatMessage]) -> Vec<u32> {
        chat_template::render_prompt_ids(self, messages)
    }
}
