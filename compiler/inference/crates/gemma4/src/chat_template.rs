//! Renders an Ollama-style `messages` array into the exact token ID
//! sequence Gemma-Instruct models expect.
//!
//! **Corrected 2026-08-19** (Fase: JARVIS native-engine validation, see
//! `docs/ROADMAP-PERF-WAVE3.md`) — the previous version of this file
//! claimed Gemma4 reuses classic Gemma's `<start_of_turn>`/`<end_of_turn>`
//! turn-wrapper tokens, cited as "VERIFIED against llama.cpp". That claim
//! was WRONG for the real gemma4:e4b GGUF this engine actually loads
//! (`~/.ollama/models/blobs/sha256-4c27e0f5...`) — calling `/api/chat`
//! against it panicked 100% of the time, unconditionally, the first time
//! this path was ever exercised end-to-end (the model's own generation
//! correctness was verified earlier via `gemma4-generate.exe`'s raw
//! `forward_step` path, which never goes through chat templating at all —
//! this bug was latent and untested until now).
//!
//! Root cause, verified two independent ways: (1) dumping this GGUF's own
//! `tokenizer.ggml.tokens` array shows id 105 = `"<|turn>"`, id 106 =
//! `"<turn|>"` — no `<start_of_turn>`/`<end_of_turn>` entries anywhere in
//! the 262144-token vocab; (2) the real `chat_template.jinja` published at
//! `huggingface.co/google/gemma-4-E4B-it/blob/main/chat_template.jinja`
//! confirms `<|turn>`/`<turn|>` as the actual Gemma4 turn delimiters — a
//! genuine tokenizer change from prior Gemma generations, not a
//! conversion-tool quirk. This specific GGUF also has no
//! `tokenizer.chat_template` metadata key at all (a documented common
//! issue with Gemma4 GGUF conversions), so there was no embedded reference
//! to catch this against at load time either.
//!
//! The turn-wrapping STRUCTURE below (role rename, system-content-into-
//! first-turn merge, trailing generation prompt) matches the real
//! template's basic non-tool-calling path and is unchanged from the
//! pre-fix version — only the two token strings were wrong. Gemma4's real
//! template additionally defines `<|tool_call>`/`<tool_call|>`,
//! `<|tool_response>`/`<tool_response|>`, and `<|channel>thought`/
//! `<channel|>` for tool-calling and reasoning-trace turns — NOT
//! implemented here, since no caller in this codebase sends `tools` to
//! gemma4 through this path today (`local-ai-tools.ts`'s `callLocalEngine`
//! sends plain messages only). Implement those if/when a caller needs them.
//!
//! ```text
//! for each message:
//!   if role == "system": accumulate content (trimmed), don't emit yet
//!   else:
//!     role = (role == "assistant") ? "model" : role
//!     emit "<|turn>{role}\n"
//!     if pending system content AND role != "model":
//!       emit that content + "\n\n", then clear it (merged into the FIRST
//!       non-model turn only, not repeated on every turn)
//!     emit trim(content) + "<turn|>\n"
//! if add_generation_prompt: emit "<|turn>model\n"  (no trailing turn-close)
//! ```
//!
//! Two behaviors easy to miss without reading the source: "assistant" is
//! literally renamed to "model" in the emitted text (Gemma has no
//! "assistant" role token), and there is no dedicated system-turn wrapper
//! at all — system content rides along inside the first real turn.

use model_core::ChatMessage;

use crate::tokenizer::Tokenizer;

fn gemma_role(role: &str) -> &str {
    if role == "assistant" {
        "model"
    } else {
        role
    }
}

/// Renders `messages` into token IDs ready for `Model::forward_step`,
/// ending with the `<|turn>model\n` generation prompt (Ollama's
/// `add_generation_prompt`, always true for a `/api/chat` completion call —
/// same convention as `qwen2`/`llama`'s `render_prompt_ids`).
///
/// # Panics
/// If the model's vocab has no `<|turn>`/`<turn|>` entries — this engine
/// only supports Gemma4-Instruct checkpoints, and missing special tokens
/// means the loaded GGUF isn't one, a caller error worth failing loudly on
/// (same policy as `qwen2`/`llama`'s chat templates).
pub fn render_prompt_ids(tokenizer: &Tokenizer, messages: &[ChatMessage]) -> Vec<u32> {
    let start_of_turn = tokenizer
        .special_token_id("<|turn>")
        .expect("model vocab has no <|turn> token — not a Gemma4-Instruct checkpoint");
    let end_of_turn = tokenizer
        .special_token_id("<turn|>")
        .expect("model vocab has no <turn|> token — not a Gemma4-Instruct checkpoint");

    let mut ids = Vec::new();
    let mut pending_system = String::new();

    for msg in messages {
        if msg.role == "system" {
            if !pending_system.is_empty() {
                pending_system.push(' ');
            }
            pending_system.push_str(msg.content.trim());
            continue;
        }

        let role = gemma_role(&msg.role);
        ids.push(start_of_turn);
        ids.extend(tokenizer.encode_no_bos(role));
        ids.extend(tokenizer.encode_no_bos("\n"));

        if !pending_system.is_empty() && role != "model" {
            ids.extend(tokenizer.encode_no_bos(&pending_system));
            ids.extend(tokenizer.encode_no_bos("\n\n"));
            pending_system.clear();
        }

        ids.extend(tokenizer.encode_no_bos(msg.content.trim()));
        ids.push(end_of_turn);
        ids.extend(tokenizer.encode_no_bos("\n"));
    }

    ids.push(start_of_turn);
    ids.extend(tokenizer.encode_no_bos("model"));
    ids.extend(tokenizer.encode_no_bos("\n"));
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok() -> Tokenizer {
        Tokenizer::test_instance(&["<|turn>", "<turn|>"])
    }

    #[test]
    fn renders_exact_gemma_skeleton_with_system_merged_into_first_turn() {
        let t = tok();
        let messages = [
            ChatMessage { role: "system".to_string(), content: "sys prompt".to_string() },
            ChatMessage { role: "user".to_string(), content: "hello".to_string() },
        ];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(
            t.decode(&ids),
            "<|turn>user\nsys prompt\n\nhello<turn|>\n<|turn>model\n"
        );
    }

    #[test]
    fn renames_assistant_role_to_model() {
        let t = tok();
        let messages = [
            ChatMessage { role: "user".to_string(), content: "q1".to_string() },
            ChatMessage { role: "assistant".to_string(), content: "a1".to_string() },
            ChatMessage { role: "user".to_string(), content: "q2".to_string() },
        ];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(
            t.decode(&ids),
            "<|turn>user\nq1<turn|>\n<|turn>model\na1<turn|>\n<|turn>user\nq2<turn|>\n<|turn>model\n"
        );
    }

    #[test]
    fn no_system_message_renders_without_the_merge_branch() {
        let t = tok();
        let messages = [ChatMessage { role: "user".to_string(), content: "hi".to_string() }];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(t.decode(&ids), "<|turn>user\nhi<turn|>\n<|turn>model\n");
    }

    #[test]
    fn trims_whitespace_from_message_content() {
        let t = tok();
        let messages = [ChatMessage { role: "user".to_string(), content: "  padded  ".to_string() }];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(t.decode(&ids), "<|turn>user\npadded<turn|>\n<|turn>model\n");
    }
}
