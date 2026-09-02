//! Renders an Ollama-style `messages` array into the exact token ID
//! sequence the loaded checkpoint's Instruct format expects. Dispatches on
//! `Tokenizer::is_spm()` between two COMPLETELY DIFFERENT template families
//! — this crate covers both a BPE-vocab architecture (Llama 3.x) and an
//! SPM-vocab one riding along (classic Llama 1/2), and they never share a
//! template the way the two BPE pretokenizer variants share one merge
//! engine.
//!
//! **Llama 3.x** (BPE vocab) — VERIFIED against Meta's own reference
//! implementation, fetched 2026-07-12 from
//! `github.com/meta-llama/llama-models/blob/main/models/llama3/chat_format.py`
//! (`ChatFormat.encode_dialog_prompt`/`_encode_header`/`encode_message`):
//! ```text
//!   <|begin_of_text|>                                     (once, whole dialog)
//!   <|start_header_id|>{role}<|end_header_id|>\n\n{content}<|eot_id|>   (per message)
//!   <|start_header_id|>assistant<|end_header_id|>\n\n      (final generation prompt, no trailing eot_id)
//! ```
//! Two deliberate departures from `qwen2::chat_template`, both because this
//! project's target here (Llama 3.x) genuinely differs, not because the
//! rule was copied carelessly:
//!   - No default system message is injected when the first message isn't
//!     role="system". Qwen2's version does this because BOTH Ollama's own
//!     template layer and the GGUF-embedded Jinja template for the Qwen2
//!     checkpoints this project targets do it (checked against ground truth
//!     for THAT architecture). Meta's reference `encode_dialog_prompt` shows
//!     no equivalent injection, and this hasn't been checked against a real
//!     Llama 3 GGUF's own embedded chat_template — so this renders exactly
//!     what's given rather than guessing at behavior with no source backing it.
//!   - Message content is encoded via `Tokenizer::encode_no_bos`, not the
//!     public `encode()`. Llama 3's GGUF has `add_bos_token=true` (Qwen2's
//!     ChatML checkpoints have it false) — `<|begin_of_text|>` must appear
//!     exactly once for the whole dialog (per the reference above), so
//!     per-message content encoding must not trigger its own auto-BOS.
//!
//! **Classic Llama 2** (SPM vocab) — VERIFIED against llama.cpp's own
//! reference rendering, `src/llama-chat.cpp`'s `LLM_CHAT_TEMPLATE_LLAMA_2_SYS`
//! branch, same commit as every other template in this workspace
//! (e3546c7948e3af463d0b401e6421d5a4c2faf565): of llama.cpp's four Llama-2
//! template sub-variants (plain/`_SYS`/`_SYS_BOS`/`_SYS_STRIP`, which one
//! applies is normally detected by scanning the GGUF's own embedded Jinja
//! text — this engine doesn't parse Jinja at all, matching every other
//! chat_template.rs here, so only ONE fixed variant is implemented: `_SYS`,
//! the standard "supports a system message" variant matching Meta's own
//! published Llama-2-Chat prompt format):
//! ```text
//!   [INST] <<SYS>>\n{system}\n<</SYS>>\n\n{user} [/INST]     (first turn, system present)
//!   [INST] {user} [/INST]                                   (first/later turn, no system)
//!   {assistant}</s>[INST] {user} [/INST]                    (later turns)
//! ```
//! `<s>` (BOS) is emitted once for the whole dialog (`_SYS_BOS` would
//! re-emit it before every `[INST]`; not implemented here, consistent with
//! this crate's other templates all following the "BOS once" convention).
//! `[INST]`/`[/INST]`/`<<SYS>>`/`<</SYS>>` are literal TEXT here, encoded
//! through the normal SPM merge (`encode_no_bos`) — unlike Llama 3's
//! `<|start_header_id|>`-style markers, they are NOT dedicated vocab
//! entries in a real Llama-2 tokenizer. Only `</s>` (EOS) is a genuine
//! special token, pushed directly via `tokenizer.eos_token_id`.
//!
//! NOT implemented: classic (pre-Tekken) Mistral's OWN `[INST]` template
//! family (`LLM_CHAT_TEMPLATE_MISTRAL_V1`/`_V3`/`_V7`) — structurally
//! similar but genuinely different (no `<<SYS>>` wrapper; a leading space
//! before `[INST]`; system content merges into the user turn with a plain
//! `\n\n` separator instead). A classic Mistral checkpoint loaded through
//! this crate tokenizes and runs correctly but gets the WRONG chat
//! formatting from `render_prompt_ids` — a real, flagged gap, not silently
//! guessed at.

use model_core::ChatMessage;

use crate::tokenizer::Tokenizer;

fn encode_header(tokenizer: &Tokenizer, ids: &mut Vec<u32>, start: u32, end: u32, role: &str) {
    ids.push(start);
    ids.extend(tokenizer.encode_no_bos(role));
    ids.push(end);
    ids.extend(tokenizer.encode_no_bos("\n\n"));
}

/// Renders `messages` into token IDs ready for `Model::forward_step`.
/// Dispatches on `Tokenizer::is_spm()` — see module doc comment for both
/// template families and why classic Mistral (also SPM) isn't covered by
/// either branch.
pub fn render_prompt_ids(tokenizer: &Tokenizer, messages: &[ChatMessage]) -> Vec<u32> {
    if tokenizer.is_spm() {
        render_llama2_prompt_ids(tokenizer, messages)
    } else {
        render_llama3_prompt_ids(tokenizer, messages)
    }
}

/// Ends with the `<|start_header_id|>assistant<|end_header_id|>\n\n`
/// generation prompt (Ollama's `add_generation_prompt`, always true for a
/// `/api/chat` completion call — same as `qwen2::chat_template`).
///
/// # Panics
/// If the model's vocab has no `<|begin_of_text|>`/`<|start_header_id|>`/
/// `<|end_header_id|>`/`<|eot_id|>` entries — this engine only supports
/// Llama-3.x-Instruct checkpoints, and a missing special token means the
/// loaded GGUF isn't one, which is a caller error worth failing loudly on
/// rather than silently emitting a malformed prompt (same policy as
/// `qwen2::chat_template::render_prompt_ids`).
fn render_llama3_prompt_ids(tokenizer: &Tokenizer, messages: &[ChatMessage]) -> Vec<u32> {
    let begin_of_text = tokenizer
        .special_token_id("<|begin_of_text|>")
        .expect("model vocab has no <|begin_of_text|> token — not a Llama-3-Instruct checkpoint");
    let start_header = tokenizer
        .special_token_id("<|start_header_id|>")
        .expect("model vocab has no <|start_header_id|> token — not a Llama-3-Instruct checkpoint");
    let end_header = tokenizer
        .special_token_id("<|end_header_id|>")
        .expect("model vocab has no <|end_header_id|> token — not a Llama-3-Instruct checkpoint");
    let eot = tokenizer
        .special_token_id("<|eot_id|>")
        .expect("model vocab has no <|eot_id|> token — not a Llama-3-Instruct checkpoint");

    let mut ids = vec![begin_of_text];
    for msg in messages {
        encode_header(tokenizer, &mut ids, start_header, end_header, &msg.role);
        ids.extend(tokenizer.encode_no_bos(&msg.content));
        ids.push(eot);
    }
    encode_header(tokenizer, &mut ids, start_header, end_header, "assistant");
    ids
}

/// Classic Llama-2 `[INST]`/`<<SYS>>` format (the `_SYS` variant — see
/// module doc comment). `[INST]`/`[/INST]`/`<<SYS>>`/`<</SYS>>` are plain
/// text here (encoded via `encode_no_bos`, the normal SPM merge path), NOT
/// dedicated vocab entries like Llama 3's markers — only `</s>` (EOS) is a
/// genuine special token.
///
/// # Panics
/// If the model has no `eos_token_id` — a real Llama-2 GGUF always has one;
/// its absence means this isn't actually an Instruct checkpoint this
/// template applies to (same fail-loud policy as the Llama-3 branch).
fn render_llama2_prompt_ids(tokenizer: &Tokenizer, messages: &[ChatMessage]) -> Vec<u32> {
    let eos = tokenizer.eos_token_id.expect("model has no eos_token_id — not a Llama-2-Instruct checkpoint");

    let mut ids = Vec::new();
    if let Some(bos) = tokenizer.bos_token_id {
        ids.push(bos);
    }

    let mut is_inside_turn = false;
    for msg in messages {
        if !is_inside_turn {
            ids.extend(tokenizer.encode_no_bos("[INST] "));
            is_inside_turn = true;
        }
        match msg.role.as_str() {
            "system" => {
                ids.extend(tokenizer.encode_no_bos("<<SYS>>\n"));
                ids.extend(tokenizer.encode_no_bos(&msg.content));
                ids.extend(tokenizer.encode_no_bos("\n<</SYS>>\n\n"));
            }
            "user" => {
                ids.extend(tokenizer.encode_no_bos(&msg.content));
                ids.extend(tokenizer.encode_no_bos(" [/INST]"));
            }
            _ => {
                // assistant
                ids.extend(tokenizer.encode_no_bos(&msg.content));
                ids.push(eos);
                is_inside_turn = false;
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok() -> Tokenizer {
        Tokenizer::test_instance(&["<|begin_of_text|>", "<|start_header_id|>", "<|end_header_id|>", "<|eot_id|>"])
    }

    #[test]
    fn renders_exact_llama3_skeleton_two_turns() {
        let t = tok();
        let messages = [
            ChatMessage { role: "system".to_string(), content: "sys prompt".to_string() },
            ChatMessage { role: "user".to_string(), content: "hello".to_string() },
        ];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(
            t.decode(&ids),
            "<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\nsys prompt<|eot_id|><|start_header_id|>user<|end_header_id|>\n\nhello<|eot_id|><|start_header_id|>assistant<|end_header_id|>\n\n"
        );
    }

    #[test]
    fn does_not_inject_a_default_system_message() {
        // Deliberate departure from qwen2::chat_template — see module doc.
        let t = tok();
        let messages = [ChatMessage { role: "user".to_string(), content: "hi".to_string() }];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(
            t.decode(&ids),
            "<|begin_of_text|><|start_header_id|>user<|end_header_id|>\n\nhi<|eot_id|><|start_header_id|>assistant<|end_header_id|>\n\n"
        );
    }

    #[test]
    fn begin_of_text_appears_exactly_once_across_multiple_turns() {
        // The bug this test guards against: if content encoding went through
        // the public encode() (which auto-prepends BOS when add_bos_token is
        // set) instead of encode_no_bos(), this would fail by finding one
        // begin_of_text per message instead of one for the whole dialog.
        let t = tok();
        let messages = [
            ChatMessage { role: "system".to_string(), content: "s".to_string() },
            ChatMessage { role: "user".to_string(), content: "q1".to_string() },
            ChatMessage { role: "assistant".to_string(), content: "a1".to_string() },
            ChatMessage { role: "user".to_string(), content: "q2".to_string() },
        ];
        let ids = render_prompt_ids(&t, &messages);
        let begin_of_text_id = t.special_token_id("<|begin_of_text|>").unwrap();
        assert_eq!(ids.iter().filter(|&&id| id == begin_of_text_id).count(), 1);
        assert_eq!(ids[0], begin_of_text_id);
    }

    fn spm_tok() -> Tokenizer {
        // "<s>"/"</s>" as real vocab entries (not just bare numeric ids with
        // no matching content) so decode() reconstructs literal "</s>" text
        // in these tests, same as a real Llama-2 vocab would.
        let mut t = Tokenizer::test_instance_spm(&[("<s>", 1.0), ("</s>", 1.0)]);
        t.bos_token_id = t.special_token_id("<s>");
        t.eos_token_id = t.special_token_id("</s>");
        t
    }

    #[test]
    fn render_prompt_ids_dispatches_to_llama2_for_spm_vocab() {
        // The dispatch itself: is_spm() must actually route here, not just
        // the llama2 renderer working in isolation.
        let t = spm_tok();
        let messages = [ChatMessage { role: "user".to_string(), content: "hi".to_string() }];
        let ids = render_prompt_ids(&t, &messages);
        assert_eq!(t.decode(&ids), "<s>[INST] hi [/INST]");
    }

    #[test]
    fn llama2_renders_system_message_with_sys_wrapper() {
        let t = spm_tok();
        let messages = [
            ChatMessage { role: "system".to_string(), content: "sys prompt".to_string() },
            ChatMessage { role: "user".to_string(), content: "hello".to_string() },
        ];
        let ids = render_llama2_prompt_ids(&t, &messages);
        assert_eq!(t.decode(&ids), "<s>[INST] <<SYS>>\nsys prompt\n<</SYS>>\n\nhello [/INST]");
    }

    #[test]
    fn llama2_multi_turn_reopens_inst_after_each_assistant_reply() {
        let t = spm_tok();
        let messages = [
            ChatMessage { role: "user".to_string(), content: "q1".to_string() },
            ChatMessage { role: "assistant".to_string(), content: "a1".to_string() },
            ChatMessage { role: "user".to_string(), content: "q2".to_string() },
        ];
        let ids = render_llama2_prompt_ids(&t, &messages);
        assert_eq!(t.decode(&ids), "<s>[INST] q1 [/INST]a1</s>[INST] q2 [/INST]");
    }

    #[test]
    fn llama2_bos_appears_exactly_once_across_multiple_turns() {
        let t = spm_tok();
        let messages = [
            ChatMessage { role: "user".to_string(), content: "q1".to_string() },
            ChatMessage { role: "assistant".to_string(), content: "a1".to_string() },
            ChatMessage { role: "user".to_string(), content: "q2".to_string() },
        ];
        let ids = render_llama2_prompt_ids(&t, &messages);
        let bos = t.bos_token_id.unwrap();
        assert_eq!(ids.iter().filter(|&&id| id == bos).count(), 1);
        assert_eq!(ids[0], bos);
    }
}
