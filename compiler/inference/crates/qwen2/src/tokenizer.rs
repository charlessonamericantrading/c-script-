//! Byte-level BPE tokenizer, GPT-2 style, with Qwen2's specific
//! pre-tokenizer split rule.
//!
//! Pipeline (matches HF `tokenizers` / llama.cpp's `llama-vocab.cpp`):
//!   1. Split raw text into "word" chunks with `qwen2_pretokenize` — this
//!      runs on the actual Unicode text, not byte-remapped yet.
//!   2. Byte-remap each chunk: every raw UTF-8 byte becomes one character
//!      from a fixed 256-entry table (`byte_to_unicode`), so BPE always
//!      operates on printable characters regardless of what the original
//!      bytes were.
//!   3. Greedily apply merge rules (lowest rank = highest priority) within
//!      each chunk until no adjacent pair has a rule.
//!   4. Look up each resulting piece in the vocab for its token id.
//!
//! No chat template / special-token splitting yet — Fase 1's correctness
//! milestone deliberately tests with a plain-text prompt (see
//! `bin/generate.rs`) to avoid needing that on the first pass.

use std::collections::HashMap;

use gguf::{GgufFile, MetadataValue};

use crate::error::LoadError;

pub struct Tokenizer {
    id_to_token: Vec<String>,
    token_to_id: HashMap<String, u32>,
    merge_rank: HashMap<(String, String), usize>,
    byte_to_unicode: [char; 256],
    unicode_to_byte: HashMap<char, u8>,
    pub bos_token_id: Option<u32>,
    pub eos_token_id: Option<u32>,
    pub add_bos_token: bool,
}

fn byte_to_unicode_table() -> [char; 256] {
    let mut printable = [false; 256];
    for b in 33..=126u16 {
        printable[b as usize] = true;
    }
    for b in 161..=172u16 {
        printable[b as usize] = true;
    }
    for b in 174..=255u16 {
        printable[b as usize] = true;
    }

    let mut table = ['\0'; 256];
    for (b, is_printable) in printable.iter().enumerate() {
        if *is_printable {
            table[b] = char::from_u32(b as u32).unwrap();
        }
    }
    let mut n: u32 = 0;
    for b in 0..256usize {
        if !printable[b] {
            table[b] = char::from_u32(256 + n).unwrap();
            n += 1;
        }
    }
    table
}

fn read_string_array(gguf: &GgufFile, key: &'static str) -> Result<Vec<String>, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Array(items)) => items
            .iter()
            .map(|v| match v {
                MetadataValue::String(s) => Ok(s.clone()),
                _ => Err(LoadError::WrongMetadataType(key)),
            })
            .collect(),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Err(LoadError::MissingMetadata(key)),
    }
}

impl Tokenizer {
    pub fn from_gguf(gguf: &GgufFile) -> Result<Tokenizer, LoadError> {
        let id_to_token = read_string_array(gguf, "tokenizer.ggml.tokens")?;
        let merges_raw = read_string_array(gguf, "tokenizer.ggml.merges")?;

        let mut token_to_id = HashMap::with_capacity(id_to_token.len());
        for (id, tok) in id_to_token.iter().enumerate() {
            token_to_id.insert(tok.clone(), id as u32);
        }

        let mut merge_rank = HashMap::with_capacity(merges_raw.len());
        for (rank, m) in merges_raw.iter().enumerate() {
            // Byte 0x20 (space) is remapped to 'Ġ' (U+0120), never left as a
            // literal space, specifically so this split is unambiguous.
            let (l, r) = m.split_once(' ').ok_or(LoadError::WrongMetadataType("tokenizer.ggml.merges"))?;
            merge_rank.insert((l.to_string(), r.to_string()), rank);
        }

        let byte_to_unicode = byte_to_unicode_table();
        let unicode_to_byte = byte_to_unicode.iter().enumerate().map(|(b, &c)| (c, b as u8)).collect();

        let bos_token_id = match gguf.metadata.get("tokenizer.ggml.bos_token_id") {
            Some(MetadataValue::Uint32(v)) => Some(*v),
            _ => None,
        };
        let eos_token_id = match gguf.metadata.get("tokenizer.ggml.eos_token_id") {
            Some(MetadataValue::Uint32(v)) => Some(*v),
            _ => None,
        };
        let add_bos_token = matches!(gguf.metadata.get("tokenizer.ggml.add_bos_token"), Some(MetadataValue::Bool(true)));

        Ok(Tokenizer { id_to_token, token_to_id, merge_rank, byte_to_unicode, unicode_to_byte, bos_token_id, eos_token_id, add_bos_token })
    }

    pub fn vocab_size(&self) -> usize {
        self.id_to_token.len()
    }

    /// Looks up a special/control token (e.g. `"<|im_start|>"`) directly by
    /// its exact vocab string, bypassing BPE entirely. Chat-template
    /// rendering needs this: these tokens must never be produced by
    /// merging byte-mapped characters (`encode()`'s normal path) — they are
    /// atomic vocab entries the model was trained to see as single units,
    /// not spelled out one BPE piece at a time.
    pub fn special_token_id(&self, token_str: &str) -> Option<u32> {
        self.token_to_id.get(token_str).copied()
    }

    fn bpe_merge(&self, mut symbols: Vec<String>) -> Vec<String> {
        loop {
            let mut best: Option<(usize, usize)> = None; // (position, rank)
            for i in 0..symbols.len().saturating_sub(1) {
                if let Some(&rank) = self.merge_rank.get(&(symbols[i].clone(), symbols[i + 1].clone())) {
                    if best.is_none_or(|(_, best_rank)| rank < best_rank) {
                        best = Some((i, rank));
                    }
                }
            }
            match best {
                None => return symbols,
                Some((i, _)) => {
                    let merged = format!("{}{}", symbols[i], symbols[i + 1]);
                    symbols.splice(i..i + 2, [merged]);
                }
            }
        }
    }

    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        if self.add_bos_token {
            if let Some(bos) = self.bos_token_id {
                ids.push(bos);
            }
        }
        for chunk in qwen2_pretokenize(text) {
            let mapped: Vec<String> = chunk.bytes().map(|b| self.byte_to_unicode[b as usize].to_string()).collect();
            for piece in self.bpe_merge(mapped) {
                match self.token_to_id.get(&piece) {
                    Some(&id) => ids.push(id),
                    None => panic!("encode: no vocab entry for BPE piece {piece:?} (chunk {chunk:?}) — every byte-mapped character should be a base vocab entry for a complete GPT-2-style vocab"),
                }
            }
        }
        ids
    }

    pub fn decode(&self, ids: &[u32]) -> String {
        String::from_utf8_lossy(&self.decode_bytes(ids)).into_owned()
    }

    /// Raw bytes for `ids`, before any UTF-8 interpretation. Streaming needs
    /// this: a single token's bytes may end mid-character (Qwen's byte-level
    /// BPE can split a 2-byte char like `ñ`/`á` across two tokens), so a
    /// per-token `decode` would emit U+FFFD garbage. The streaming path
    /// buffers these bytes and only emits complete UTF-8.
    pub fn decode_bytes(&self, ids: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for &id in ids {
            let Some(piece) = self.id_to_token.get(id as usize) else { continue };
            for c in piece.chars() {
                if let Some(&b) = self.unicode_to_byte.get(&c) {
                    bytes.push(b);
                }
            }
        }
        bytes
    }
}

#[cfg(test)]
impl Tokenizer {
    /// Test-only: byte-level base vocab (every byte-mapped character as its
    /// own token, matching a real GGUF's vocab structure) plus `extra_tokens`
    /// appended after — no merge rules, so `encode` never merges adjacent
    /// characters. Real BPE merging isn't what `chat_template`'s tests
    /// check; they check that special tokens land in the right positions
    /// relative to encoded content, which this is sufficient for.
    pub(crate) fn test_instance(extra_tokens: &[&str]) -> Tokenizer {
        let byte_to_unicode = byte_to_unicode_table();
        let mut id_to_token: Vec<String> = byte_to_unicode.iter().map(|c| c.to_string()).collect();
        id_to_token.extend(extra_tokens.iter().map(|s| s.to_string()));

        let mut token_to_id = HashMap::with_capacity(id_to_token.len());
        for (id, tok) in id_to_token.iter().enumerate() {
            token_to_id.insert(tok.clone(), id as u32);
        }
        let unicode_to_byte = byte_to_unicode.iter().enumerate().map(|(b, &c)| (c, b as u8)).collect();

        Tokenizer {
            id_to_token,
            token_to_id,
            merge_rank: HashMap::new(),
            byte_to_unicode,
            unicode_to_byte,
            bos_token_id: None,
            eos_token_id: None,
            add_bos_token: false,
        }
    }
}

fn is_letter(c: char) -> bool {
    c.is_alphabetic()
}
fn is_digit(c: char) -> bool {
    c.is_numeric()
}
fn is_ws(c: char) -> bool {
    c.is_whitespace()
}
fn is_symbol(c: char) -> bool {
    !is_ws(c) && !is_letter(c) && !is_digit(c)
}
fn is_r2_prefix(c: char) -> bool {
    c != '\r' && c != '\n' && !is_letter(c) && !is_digit(c)
}

/// Qwen2's pre-tokenizer split, hand-implemented against the exact regex
/// llama.cpp uses for `LLAMA_VOCAB_PRE_TYPE_QWEN2` (verified in
/// `src/llama-vocab.cpp`, not assumed to be the plain GPT-2 pattern — the
/// two differ: digits split one-at-a-time here (`\p{N}` not `\p{N}+`), and
/// the "optional prefix" before a word allows any non-letter/digit/newline
/// character, not just a space):
///
/// ```text
/// (?:'[sS]|'[tT]|'[rR][eE]|'[vV][eE]|'[mM]|'[lL][lL]|'[dD])
///   | [^\r\n\p{L}\p{N}]?\p{L}+
///   | \p{N}
///   |  ?[^\s\p{L}\p{N}]+[\r\n]*
///   | \s*[\r\n]+
///   | \s+(?!\S)
///   | \s+
/// ```
///
/// Rust's `regex` crate can't express the `(?!\S)` lookahead, so this is a
/// manual priority-ordered scanner rather than a compiled pattern — not a
/// general regex engine, just this one rule.
///
/// Known scope limit: `\p{L}`/`\p{N}` are approximated with
/// `char::is_alphabetic`/`is_numeric`, which are close but not byte-for-byte
/// identical to Unicode's General_Category=Letter/Number for every script.
/// Exact for the ASCII/Latin-1 text this project's test prompts use.
pub fn qwen2_pretokenize(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < n {
        // R1: contraction suffixes.
        if chars[i] == '\'' && i + 1 < n {
            let c1 = chars[i + 1].to_ascii_lowercase();
            if matches!(c1, 's' | 't' | 'm' | 'd') {
                out.push(chars[i..i + 2].iter().collect());
                i += 2;
                continue;
            }
            if matches!(c1, 'r' | 'v') && i + 2 < n && chars[i + 2].eq_ignore_ascii_case(&'e') {
                out.push(chars[i..i + 3].iter().collect());
                i += 3;
                continue;
            }
            if c1 == 'l' && i + 2 < n && chars[i + 2].eq_ignore_ascii_case(&'l') {
                out.push(chars[i..i + 3].iter().collect());
                i += 3;
                continue;
            }
        }

        // R2: [^\r\n\p{L}\p{N}]?\p{L}+  (greedy optional prefix, else bare word)
        if is_r2_prefix(chars[i]) && i + 1 < n && is_letter(chars[i + 1]) {
            let mut j = i + 2;
            while j < n && is_letter(chars[j]) {
                j += 1;
            }
            out.push(chars[i..j].iter().collect());
            i = j;
            continue;
        }
        if is_letter(chars[i]) {
            let mut j = i + 1;
            while j < n && is_letter(chars[j]) {
                j += 1;
            }
            out.push(chars[i..j].iter().collect());
            i = j;
            continue;
        }

        // R3: \p{N}  (one digit at a time, deliberately not a run)
        if is_digit(chars[i]) {
            out.push(chars[i].to_string());
            i += 1;
            continue;
        }

        // R4:  ?[^\s\p{L}\p{N}]+[\r\n]*
        let symbol_run_end = |start: usize| -> Option<usize> {
            if start < n && is_symbol(chars[start]) {
                let mut j = start + 1;
                while j < n && is_symbol(chars[j]) {
                    j += 1;
                }
                while j < n && (chars[j] == '\r' || chars[j] == '\n') {
                    j += 1;
                }
                Some(j)
            } else {
                None
            }
        };
        if chars[i] == ' ' {
            if let Some(end) = symbol_run_end(i + 1) {
                out.push(chars[i..end].iter().collect());
                i = end;
                continue;
            }
        }
        if let Some(end) = symbol_run_end(i) {
            out.push(chars[i..end].iter().collect());
            i = end;
            continue;
        }

        // Remaining case: whitespace. Compute the maximal contiguous run
        // once, then decide which of R5/R6/R7 it falls under.
        if is_ws(chars[i]) {
            let mut j = i;
            let mut last_newline = None;
            while j < n && is_ws(chars[j]) {
                if chars[j] == '\r' || chars[j] == '\n' {
                    last_newline = Some(j);
                }
                j += 1;
            }
            let run_end = j;

            // R5: \s*[\r\n]+ — consume through the last \r/\n in the run.
            if let Some(last_nl) = last_newline {
                out.push(chars[i..=last_nl].iter().collect());
                i = last_nl + 1;
                continue;
            }

            // R6: \s+(?!\S) — take the whole run if it's at end-of-input;
            // otherwise leave exactly one trailing whitespace char behind
            // for the next word/symbol match to claim as its leading " ?".
            if run_end == n {
                out.push(chars[i..run_end].iter().collect());
                i = run_end;
                continue;
            }
            if run_end - i >= 2 {
                out.push(chars[i..run_end - 1].iter().collect());
                i = run_end - 1;
                continue;
            }

            // R7: \s+ fallback — a lone whitespace char immediately
            // followed by something that isn't a word or symbol (a digit).
            out.push(chars[i..run_end].iter().collect());
            i = run_end;
            continue;
        }

        // Unreachable for well-formed Unicode text (letter/digit/whitespace/
        // symbol is exhaustive), but never silently drop a character.
        out.push(chars[i].to_string());
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_to_unicode_is_involution_on_reverse_map() {
        let table = byte_to_unicode_table();
        let reverse: HashMap<char, u8> = table.iter().enumerate().map(|(b, &c)| (c, b as u8)).collect();
        for b in 0..256usize {
            assert_eq!(reverse[&table[b]], b as u8);
        }
        // Printable ASCII must map to itself (identity), per the spec.
        assert_eq!(table[b'A' as usize], 'A');
        assert_eq!(table[b'!' as usize], '!');
        // The space byte must NOT map to a literal space (that's the whole
        // point of the remap — keeps merge-rule text files unambiguous).
        assert_ne!(table[b' ' as usize], ' ');
    }

    #[test]
    fn pretokenize_splits_contraction() {
        let chunks = qwen2_pretokenize("don't");
        assert_eq!(chunks, vec!["don", "'t"]);
    }

    #[test]
    fn pretokenize_splits_digits_one_at_a_time() {
        let chunks = qwen2_pretokenize("abc123");
        assert_eq!(chunks, vec!["abc", "1", "2", "3"]);
    }

    #[test]
    fn pretokenize_leading_space_attaches_to_word() {
        let chunks = qwen2_pretokenize("hello world");
        assert_eq!(chunks, vec!["hello", " world"]);
    }

    #[test]
    fn pretokenize_multiple_spaces_keep_one_for_next_word() {
        let chunks = qwen2_pretokenize("a   b");
        // "a", then two bare spaces (R6, run-1), then " b" (R2 with leading space).
        assert_eq!(chunks, vec!["a", "  ", " b"]);
    }

    #[test]
    fn pretokenize_newline_run() {
        let chunks = qwen2_pretokenize("a\n\nb");
        assert_eq!(chunks, vec!["a", "\n\n", "b"]);
    }

    #[test]
    fn pretokenize_punctuation_run() {
        let chunks = qwen2_pretokenize("hi!!");
        assert_eq!(chunks, vec!["hi", "!!"]);
    }
}
