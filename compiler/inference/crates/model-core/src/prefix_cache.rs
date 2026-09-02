//! Prefix KV Cache Manager.
//!
//! Stores previously evaluated token ID sequences and their corresponding
//! `KvCache` prefix states. When a new generation request arrives with prompt
//! tokens `T`, `PrefixCacheManager` finds the longest cached prefix `P` of `T`
//! and returns the pre-calculated `KvCache` state for `P` along with the
//! remaining uncached tokens `T[P.len()..]`.
//!
//! Eliminates prefill latency for recurring system prompts and agent tool schemas.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::cache::KvCache;

struct CacheEntry {
    token_ids: Vec<u32>,
    kv_cache: KvCache,
    last_used_step: u64,
}

pub struct PrefixCacheManager {
    entries: Mutex<HashMap<String, Vec<CacheEntry>>>,
    max_entries_per_model: usize,
    step_counter: Mutex<u64>,
}

impl PrefixCacheManager {
    pub fn new(max_entries_per_model: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            max_entries_per_model,
            step_counter: Mutex::new(0),
        }
    }

    /// Finds the longest matching prefix for `token_ids` for `model_name`.
    /// Returns `(matched_prefix_len, cloned_kv_cache)`.
    pub fn find_longest_prefix(&self, model_name: &str, token_ids: &[u32]) -> (usize, Option<KvCache>) {
        if token_ids.is_empty() {
            return (0, None);
        }

        let mut step = self.step_counter.lock().unwrap();
        *step += 1;
        let current_step = *step;

        let mut entries_guard = self.entries.lock().unwrap();
        let model_entries = match entries_guard.get_mut(model_name) {
            Some(list) => list,
            None => return (0, None),
        };

        let mut best_match_len = 0usize;
        let mut best_kv: Option<KvCache> = None;

        for entry in model_entries.iter_mut() {
            let common_len = common_prefix_len(&entry.token_ids, token_ids);
            if common_len > best_match_len && common_len > 8 {
                best_match_len = common_len;
                entry.last_used_step = current_step;
                best_kv = Some(entry.kv_cache.prefix_clone(common_len));
            }
        }

        (best_match_len, best_kv)
    }

    /// Inserts or updates a prefix KV cache entry for `model_name`.
    pub fn insert_prefix(&self, model_name: &str, token_ids: Vec<u32>, kv_cache: KvCache) {
        if token_ids.len() < 8 {
            return;
        }

        let mut step = self.step_counter.lock().unwrap();
        *step += 1;
        let current_step = *step;

        let mut entries_guard = self.entries.lock().unwrap();
        let model_entries = entries_guard.entry(model_name.to_string()).or_default();

        // Check if exact or prefix already exists
        for entry in model_entries.iter_mut() {
            if entry.token_ids == token_ids {
                entry.kv_cache = kv_cache;
                entry.last_used_step = current_step;
                return;
            }
        }

        // Evict LRU if at capacity
        if model_entries.len() >= self.max_entries_per_model {
            if let Some((min_idx, _)) = model_entries.iter().enumerate().min_by_key(|(_, e)| e.last_used_step) {
                model_entries.remove(min_idx);
            }
        }

        model_entries.push(CacheEntry {
            token_ids,
            kv_cache,
            last_used_step: current_step,
        });
    }
}

fn common_prefix_len(a: &[u32], b: &[u32]) -> usize {
    let limit = a.len().min(b.len());
    let mut i = 0;
    while i < limit && a[i] == b[i] {
        i += 1;
    }
    i
}

/// Clamps a matched prefix length to leave at least one token for the
/// caller to feed through `forward_step` fresh.
///
/// `forward_step` always appends one new KV position per token it's given
/// -- there's no way to ask it for "logits after a cache" without handing
/// it at least one token. If the full prompt matched a cached prefix
/// (`matched_len == prompt_len`), reusing the clone verbatim and then
/// re-feeding the last token anyway would append a *second*, duplicate KV
/// entry for that same token at a new position, diverging from what a
/// fresh prefill of the same prompt would have produced. Reserving the
/// last token unconditionally keeps the two paths equivalent.
pub fn safe_reuse_len(matched_len: usize, prompt_len: usize) -> usize {
    matched_len.min(prompt_len.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_prefix_len() {
        assert_eq!(common_prefix_len(&[1, 2, 3, 4], &[1, 2, 3, 5]), 3);
        assert_eq!(common_prefix_len(&[1, 2], &[3, 4]), 0);
    }

    #[test]
    fn test_safe_reuse_len_never_consumes_the_whole_prompt() {
        // Exact repeat: must reserve the last token, never reuse all of it.
        assert_eq!(safe_reuse_len(10, 10), 9);
        // Partial match: already leaves new tokens, unaffected.
        assert_eq!(safe_reuse_len(6, 10), 6);
        // No match at all.
        assert_eq!(safe_reuse_len(0, 10), 0);
        // Single-token prompt: nothing safe to reuse.
        assert_eq!(safe_reuse_len(1, 1), 0);
        assert_eq!(safe_reuse_len(0, 1), 0);
    }
}
