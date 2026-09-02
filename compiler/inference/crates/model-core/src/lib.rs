//! Shared types and the trait boundary between `server` (the Ollama-API-
//! compatible HTTP frontend) and any architecture-specific model crate
//! (`qwen2`, and future `llama`/`gemma`/...). `server` depends only on this
//! crate plus whichever architecture crates it wires in — never on an
//! architecture crate's internals directly. Fase 0 of the multi-architecture
//! plan: this crate didn't exist before — everything in it moved out of
//! `qwen2`, where it worked identically but was unreachable by any other
//! architecture crate.
//!
//! What's shared here vs. what stays per-architecture:
//!  - `KvCache`/`CacheShape`: the K/V cache SHAPE (layer count, kv-head
//!    count, head dim, context length) is identical across every dense
//!    (non-MoE) transformer architecture — it's just numbers, not behavior
//!    — so the cache itself is real shared code, not a trait.
//!  - `argmax`: only looks at a logits vector, never at how it was produced.
//!  - `forward_step` and chat-template rendering genuinely differ per
//!    architecture (attention/MLP/norm details; chat template tags), so
//!    those stay behind `LanguageModel`/`ModelTokenizer`.

pub mod cache;
pub mod paged_attention;
pub mod prefix_cache;

pub use cache::{CacheShape, KvCache, LayerKv};
pub use paged_attention::{BlockManager, BlockTable, PhysicalBlock};
pub use prefix_cache::PrefixCacheManager;

/// One message in a chat-style prompt. Architecture-agnostic; each
/// `ModelTokenizer` impl renders these into its own chat-template skeleton.
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Deterministic (greedy) next-token pick — same for every architecture,
/// since it only looks at the logits vector, never at how it was produced.
pub fn argmax(logits: &[f32]) -> u32 {
    let mut best_i = 0usize;
    let mut best_v = f32::NEG_INFINITY;
    for (i, &v) in logits.iter().enumerate() {
        if v > best_v {
            best_v = v;
            best_i = i;
        }
    }
    best_i as u32
}

/// A loaded, ready-to-run model. `forward_step` is the one piece every
/// architecture crate implements from scratch; everything else here is
/// bookkeeping `server`/`routes` needs regardless of architecture.
pub trait LanguageModel: Send + Sync {
    /// Feeds `new_tokens` through the model, updating `cache` in place, and
    /// returns logits (length `vocab_size`) for the last new position.
    fn forward_step(&self, cache: &mut KvCache, new_tokens: &[u32]) -> Vec<f32>;

    /// The K/V cache shape this model needs — used to build a fresh
    /// `KvCache` for a new request.
    fn cache_shape(&self) -> CacheShape;

    fn vocab_size(&self) -> usize;

    /// Fase 23, Milestone 2: the decode-step counterpart to `forward_step`
    /// for N sequences at once (exactly one new token each — NOT prefill,
    /// every `cache` must already hold its own prompt), letting a shared
    /// scheduler batch concurrent connections' decode steps into one
    /// weight read instead of N independent ones.
    ///
    /// Default implementation just loops `forward_step` once per sequence
    /// — behaviorally identical to today's per-connection calls, so the 5
    /// architectures that don't override this get correct output and zero
    /// behavior change, just routed through the scheduler. Only `qwen2`
    /// overrides it with a real batched implementation
    /// (`forward_decode_step_batch`, `crates/qwen2/src/forward.rs`) that
    /// shares one weight read across all N sequences' Q/K/V/O and gate/up/
    /// down projections.
    fn forward_step_batch(&self, caches: &mut [&mut KvCache], new_tokens: &[u32]) -> Vec<Vec<f32>> {
        assert_eq!(caches.len(), new_tokens.len(), "forward_step_batch: one token per cache");
        caches.iter_mut().zip(new_tokens.iter()).map(|(cache, &tok)| self.forward_step(cache, &[tok])).collect()
    }
}

/// A loaded tokenizer + this architecture's chat-template rendering.
pub trait ModelTokenizer: Send + Sync {
    fn encode(&self, text: &str) -> Vec<u32>;
    fn decode(&self, ids: &[u32]) -> String;
    fn decode_bytes(&self, ids: &[u32]) -> Vec<u8>;
    fn eos_token_id(&self) -> Option<u32>;
    fn render_prompt_ids(&self, messages: &[ChatMessage]) -> Vec<u32>;
}
