//! Regression test for the 2026-08-19 chat-template fix (see
//! `src/chat_template.rs`'s doc comment): `render_prompt_ids` panicked
//! 100% of the time against the real local gemma4:e4b checkpoint because
//! this engine looked up `<start_of_turn>`/`<end_of_turn>`, but Gemma4's
//! real vocab uses `<|turn>`/`<turn|>`. Runs against the actual GGUF (not
//! a synthetic tokenizer) so a future regression to the old token names,
//! or to a differently-tokenized checkpoint, fails loudly here instead of
//! only at first real `/api/chat` request in production.
//!
//! `#[ignore]`d by default — needs the real ~9.6GB local checkpoint, same
//! convention as every other real-checkpoint test in this workspace.
//! Run: `cargo test -p gemma4 --release --test chat_template_real_checkpoint -- --ignored --nocapture`.

use gemma4::chat_template::render_prompt_ids;
use gemma4::tokenizer::Tokenizer;
use gguf::GgufFile;
use model_core::ChatMessage;

const MODEL_PATH: &str = "C:/Users/repre/.ollama/models/blobs/sha256-4c27e0f5b5adf02ac956c7322bd2ee7636fe3f45a8512c9aba5385242cb6e09a";

#[test]
#[ignore]
fn render_prompt_ids_does_not_panic_on_the_real_checkpoint() {
    let bytes = std::fs::read(MODEL_PATH).expect("read gemma4:e4b gguf -- is it still pulled locally?");
    let gguf = GgufFile::parse(&bytes).expect("parse gguf");
    let tokenizer = Tokenizer::from_gguf(&gguf).expect("build tokenizer from real gemma4:e4b metadata");

    let messages = [
        ChatMessage { role: "system".to_string(), content: "Eres un asistente breve.".to_string() },
        ChatMessage { role: "user".to_string(), content: "Hola".to_string() },
    ];

    // The real assertion: this must not panic (it did, unconditionally,
    // before the fix). `catch_unwind` makes the failure mode explicit in
    // the test output rather than a bare test-runner abort.
    let ids = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render_prompt_ids(&tokenizer, &messages)))
        .expect("render_prompt_ids panicked against the real gemma4:e4b checkpoint");

    assert!(!ids.is_empty(), "rendered prompt should not be empty");

    // Decode and spot-check the real turn delimiters appear, not the old
    // (wrong) classic-Gemma ones -- catches a silent revert to the old
    // token strings even if some other tokenizer happened to define them.
    let decoded = tokenizer.decode(&ids);
    assert!(decoded.contains("<|turn>"), "expected the real Gemma4 turn-open token in: {decoded}");
    assert!(decoded.contains("<turn|>"), "expected the real Gemma4 turn-close token in: {decoded}");
    assert!(decoded.ends_with("<|turn>model\n"), "expected the generation prompt to end the render: {decoded}");
    assert!(!decoded.contains("<start_of_turn>"), "should not emit the classic-Gemma (wrong for Gemma4) token: {decoded}");
}
