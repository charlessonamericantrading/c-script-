//! Renders an Ollama-style `messages` array into the exact token ID
//! sequence Phi-3-Instruct models expect.
//!
//! VERIFIED against llama.cpp's own reference rendering, fetched
//! 2026-07-12 from ggml-org/llama.cpp commit
//! e3546c7948e3af463d0b401e6421d5a4c2faf565, `src/llama-chat.cpp`
//! (`LLM_CHAT_TEMPLATE_PHI_3` branch, detected by the GGUF-embedded Jinja
//! template containing both `<|assistant|>` and `<|end|>`):
//!
//! ```text
//! for each message:
//!   emit "<|{role}|>\n{content}<|end|>\n"   (role used verbatim, NOT
//!                                             renamed like Gemma's
//!                                             assistant->model; content
//!                                             NOT trimmed, unlike Gemma's
//!                                             template)
//! if add_generation_prompt: emit "<|assistant|>\n"
//! ```
//!
//! Simpler than `gemma4::chat_template`: no system-role merging, no
//! renaming, no trimming — every message renders independently. The C++
//! reference builds this as one big STRING (literal `<|user|>` etc as
//! text) and tokenizes the whole thing in one pass with special-token
//! recognition enabled; this crate uses the same simplified approach
//! `llama`/`gemma4` already do instead (push each special token's ID
//! directly via `special_token_id`, `encode_no_bos` for plain content) —
//! no general "recognize special-token substrings inside arbitrary text"
//! fragment partitioner here either. Since Phi-3's marker format is
//! LITERALLY `<|{role}|>` with no renaming, the marker string can be built
//! directly from `msg.role` rather than needing a per-role mapping
//! function (contrast `gemma4::chat_template::gemma_role`).

use model_core::ChatMessage;

use crate::tokenizer::Tokenizer;

/// Renders `messages` into token IDs ready for `Model::forward_step`,
/// ending with the `<|assistant|>\n` generation prompt (Ollama's
/// `add_generation_prompt`, always true for a `/api/chat` completion call —
/// same convention as every other crate's `render_prompt_ids`).
///
/// # Panics
/// If the model's vocab is missing any of `<|{role}|>`/`<|end|>` for a role
/// actually used in `messages` (plus `<|assistant|>` for the trailing
/// generation prompt) — this engine only supports Phi-3-Instruct
/// checkpoints, and a missing special token means the loaded GGUF isn't
/// one, a caller error worth failing loudly on (same policy as every other
/// crate's chat template).
pub fn render_prompt_ids(tokenizer: &Tokenizer, messages: &[ChatMessage]) -> Vec<u32> {
    let end = tokenizer.special_token_id("<|end|>").expect("model vocab has no <|end|> token — not a Phi-3-Instruct checkpoint");

    let mut ids = Vec::new();
    for msg in messages {
        let role_marker = format!("<|{}|>", msg.role);
        let role_id = tokenizer
            .special_token_id(&role_marker)
            .unwrap_or_else(|| panic!("model vocab has no {role_marker} token — not a Phi-3-Instruct checkpoint, or an unexpected role \"{}\"", msg.role));

        ids.push(role_id);
        ids.extend(tokenizer.encode_no_bos("\n"));
        ids.extend(tokenizer.encode_no_bos(&msg.content));
        ids.push(end);
        ids.extend(tokenizer.encode_no_bos("\n"));
    }

    let assistant = tokenizer.special_token_id("<|assistant|>").expect("model vocab has no <|assistant|> token — not a Phi-3-Instruct checkpoint");
    ids.push(assistant);
    ids.extend(tokenizer.encode_no_bos("\n"));
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok() -> Tokenizer {
        Tokenizer::test_instance(&[("<|system|>", 1.0), ("<|user|>", 1.0), ("<|assistant|>", 1.0), ("<|end|>", 1.0)])
    }

    #[test]
    fn renders_exact_phi3_skeleton_for_a_single_user_turn() {
        let t = tok();
        let messages = [ChatMessage { role: "user".to_string(), content: "hi".to_string() }];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(t.decode(&ids), "<|user|>\nhi<|end|>\n<|assistant|>\n");
    }

    #[test]
    fn renders_system_and_multi_turn_history_with_roles_used_verbatim() {
        let t = tok();
        let messages = [
            ChatMessage { role: "system".to_string(), content: "sys prompt".to_string() },
            ChatMessage { role: "user".to_string(), content: "q1".to_string() },
            ChatMessage { role: "assistant".to_string(), content: "a1".to_string() },
            ChatMessage { role: "user".to_string(), content: "q2".to_string() },
        ];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(
            t.decode(&ids),
            "<|system|>\nsys prompt<|end|>\n<|user|>\nq1<|end|>\n<|assistant|>\na1<|end|>\n<|user|>\nq2<|end|>\n<|assistant|>\n"
        );
    }

    #[test]
    fn does_not_trim_message_content_unlike_gemma() {
        let t = tok();
        let messages = [ChatMessage { role: "user".to_string(), content: "  padded  ".to_string() }];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(t.decode(&ids), "<|user|>\n  padded  <|end|>\n<|assistant|>\n");
    }

    #[test]
    #[should_panic(expected = "no <|end|> token")]
    fn panics_on_missing_end_token() {
        let t = Tokenizer::test_instance(&[("<|user|>", 1.0), ("<|assistant|>", 1.0)]);
        let messages = [ChatMessage { role: "user".to_string(), content: "hi".to_string() }];
        render_prompt_ids(&t, &messages);
    }
}
