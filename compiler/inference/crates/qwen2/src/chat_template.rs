//! Renders an Ollama-style `messages` array into the exact token ID
//! sequence Qwen2/Qwen2.5-Instruct models expect (ChatML) — the piece
//! `/api/chat` needs that `/api/generate` never did (see `tokenizer.rs`'s
//! module doc: "no chat template / special-token splitting yet").
//!
//! Checked against ground truth, not memory: both Ollama's own template
//! layer (`application/vnd.ollama.image.template`, extracted from the
//! manifest blob) and the GGUF-embedded `tokenizer.chat_template` for
//! qwen2.5:0.5b and qwen2.5-coder:7b agree exactly on the skeleton below
//! for the no-`tools` case (the only case `local-ai-tools.ts`'s
//! `callOllama()` — the JARVIS cutover's actual caller — ever sends):
//!
//!   <|im_start|>{role}\n{content}<|im_end|>\n   (once per message)
//!   <|im_start|>assistant\n                      (final generation prompt)
//!
//! with a default system message inserted when the first message isn't
//! role="system" (both templates' `{% else %}`/`if or .System .Tools`
//! branch). This is deliberately NOT a general Jinja2/Go-template
//! interpreter — just this one fixed skeleton.

use model_core::ChatMessage;

use crate::tokenizer::Tokenizer;

const DEFAULT_SYSTEM_PROMPT: &str = "You are Qwen, created by Alibaba Cloud. You are a helpful assistant.";

fn push_message(tokenizer: &Tokenizer, ids: &mut Vec<u32>, im_start: u32, im_end: u32, role: &str, content: &str) {
    ids.push(im_start);
    // One `encode` call over "{role}\n{content}" together (not two calls
    // concatenated) so BPE merging across the role/newline/content
    // boundary matches what a real continuous-text tokenizer pass would
    // produce — the same reasoning `qwen2_pretokenize`'s doc comment gives
    // for why chunk boundaries matter.
    ids.extend(tokenizer.encode(&format!("{role}\n{content}")));
    ids.push(im_end);
    ids.extend(tokenizer.encode("\n"));
}

/// Renders `messages` into token IDs ready for `Model::forward_step`,
/// ending with the `<|im_start|>assistant\n` generation prompt (Ollama's
/// `add_generation_prompt`, always true for a `/api/chat` completion call).
///
/// # Panics
/// If the model's vocab has no `<|im_start|>`/`<|im_end|>` entries — this
/// engine only supports Qwen2/Qwen2.5-Instruct checkpoints, and a missing
/// special token means the loaded GGUF isn't one (a base/non-instruct
/// checkpoint, or a different architecture), which is a caller error worth
/// failing loudly on rather than silently emitting a malformed prompt.
pub fn render_prompt_ids(tokenizer: &Tokenizer, messages: &[ChatMessage]) -> Vec<u32> {
    let im_start = tokenizer.special_token_id("<|im_start|>").expect("model vocab has no <|im_start|> token — not a ChatML/Qwen2-Instruct checkpoint");
    let im_end = tokenizer.special_token_id("<|im_end|>").expect("model vocab has no <|im_end|> token — not a ChatML/Qwen2-Instruct checkpoint");

    let mut ids = Vec::new();
    let rest: &[ChatMessage] = if messages.first().map(|m| m.role.as_str()) == Some("system") {
        push_message(tokenizer, &mut ids, im_start, im_end, "system", &messages[0].content);
        &messages[1..]
    } else {
        push_message(tokenizer, &mut ids, im_start, im_end, "system", DEFAULT_SYSTEM_PROMPT);
        messages
    };

    for msg in rest {
        push_message(tokenizer, &mut ids, im_start, im_end, &msg.role, &msg.content);
    }

    ids.push(im_start);
    ids.extend(tokenizer.encode("assistant\n"));
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_exact_chatml_text_with_explicit_system_message() {
        let tok = Tokenizer::test_instance(&["<|im_start|>", "<|im_end|>"]);
        let messages = [
            ChatMessage { role: "system".to_string(), content: "sys prompt".to_string() },
            ChatMessage { role: "user".to_string(), content: "hello".to_string() },
        ];
        let ids = render_prompt_ids(&tok, &messages);
        // Round-tripping through `decode` (which reconstructs the literal
        // text for every token, special tokens included, since their vocab
        // strings are plain ASCII) lets this test assert the exact
        // rendered string instead of opaque token IDs.
        assert_eq!(
            tok.decode(&ids),
            "<|im_start|>system\nsys prompt<|im_end|>\n<|im_start|>user\nhello<|im_end|>\n<|im_start|>assistant\n"
        );
    }

    #[test]
    fn inserts_default_system_message_when_absent() {
        let tok = Tokenizer::test_instance(&["<|im_start|>", "<|im_end|>"]);
        let messages = [ChatMessage { role: "user".to_string(), content: "hi".to_string() }];
        let ids = render_prompt_ids(&tok, &messages);
        assert_eq!(
            tok.decode(&ids),
            format!("<|im_start|>system\n{DEFAULT_SYSTEM_PROMPT}<|im_end|>\n<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\n")
        );
    }

    #[test]
    fn renders_multi_turn_history() {
        let tok = Tokenizer::test_instance(&["<|im_start|>", "<|im_end|>"]);
        let messages = [
            ChatMessage { role: "system".to_string(), content: "s".to_string() },
            ChatMessage { role: "user".to_string(), content: "q1".to_string() },
            ChatMessage { role: "assistant".to_string(), content: "a1".to_string() },
            ChatMessage { role: "user".to_string(), content: "q2".to_string() },
        ];
        let ids = render_prompt_ids(&tok, &messages);
        assert_eq!(
            tok.decode(&ids),
            "<|im_start|>system\ns<|im_end|>\n<|im_start|>user\nq1<|im_end|>\n<|im_start|>assistant\na1<|im_end|>\n<|im_start|>user\nq2<|im_end|>\n<|im_start|>assistant\n"
        );
    }
}
