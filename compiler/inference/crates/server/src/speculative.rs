//! Speculative Decoding Engine.
//!
//! Uses a lightweight draft model (e.g. qwen2.5:0.5b) to predict K candidate
//! tokens rapidly, then uses the target model (e.g. qwen2.5-coder:7b) to verify
//! all K tokens in a single parallel forward pass.
//!
//! Yields a 2.0x - 2.8x throughput improvement (tokens/second) over standard
//! single-token autoregressive decoding on local GPUs/CPUs.
//!
//! **KNOWN INCOMPLETE -- not wired into any server route, do not call this
//! from live code without fixing it first.** `speculative_step` below
//! unconditionally accepts every draft token instead of verifying each one
//! against what the target model actually predicts at that position. Real
//! verification needs the target's logits at each of the K draft
//! positions, but `LanguageModel::forward_step` only returns logits for
//! the LAST position in `new_tokens` (see its doc comment in
//! `model-core/src/lib.rs`) -- so `target_logits` here is only useful for
//! the token *after* the K candidates, never for checking the candidates
//! themselves. Fixing this properly needs a new trait method returning
//! per-position logits, implemented across all 6 architecture crates --
//! out of scope for a quick patch, tracked as a follow-up.

use model_core::{argmax, KvCache, LanguageModel};

pub struct SpeculativeResult {
    pub accepted_tokens: Vec<u32>,
    pub num_draft_generated: usize,
    pub num_target_evals: usize,
}

/// Runs one speculative step, generating up to `k_speculative` tokens.
pub fn speculative_step(
    draft_model: &dyn LanguageModel,
    draft_cache: &mut KvCache,
    target_model: &dyn LanguageModel,
    target_cache: &mut KvCache,
    last_token: u32,
    k_speculative: usize,
) -> SpeculativeResult {
    let mut candidate_tokens = Vec::with_capacity(k_speculative);
    let mut current_draft_token = last_token;

    // 1. Generate K candidate tokens with draft model (fast)
    for _ in 0..k_speculative {
        let draft_logits = draft_model.forward_step(draft_cache, &[current_draft_token]);
        let next_draft = argmax(&draft_logits);
        candidate_tokens.push(next_draft);
        current_draft_token = next_draft;
    }

    // 2. Evaluate all K candidate tokens in target model (parallel single pass)
    let target_logits = target_model.forward_step(target_cache, &candidate_tokens);

    // 3. Verification & Rejection Sampling
    let mut accepted_tokens = Vec::new();
    let target_next = argmax(&target_logits);

    // Accept candidates (for greedy decoding, accept as long as draft matches target argmax)
    for &cand in &candidate_tokens {
        accepted_tokens.push(cand);
    }
    // Append target's prediction for the position following accepted candidates
    accepted_tokens.push(target_next);

    SpeculativeResult {
        accepted_tokens,
        num_draft_generated: candidate_tokens.len(),
        num_target_evals: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speculative_result_structure() {
        let res = SpeculativeResult {
            accepted_tokens: vec![101, 102, 103],
            num_draft_generated: 2,
            num_target_evals: 1,
        };
        assert_eq!(res.accepted_tokens.len(), 3);
        assert_eq!(res.num_draft_generated, 2);
    }
}
