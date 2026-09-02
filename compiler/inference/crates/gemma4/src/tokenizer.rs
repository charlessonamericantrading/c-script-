//! Byte-level... no — CHARACTER-level BPE for Gemma4 ("SPM-style BPE" in
//! llama.cpp's own terms). Genuinely different from `qwen2`/`llama`'s
//! tokenizer, not a variant: no byte-to-unicode remap table at all, base
//! merge symbols are whole UTF-8 characters, spaces are escaped to "▁"
//! before pretokenization, and pretokenization itself is trivial (split on
//! newline vs. non-newline runs — the complex word/contraction/digit regex
//! `qwen2`/`llama` need doesn't apply here at all).
//!
//! Every claim below is VERIFIED against llama.cpp source (not recalled),
//! fetched 2026-07-12 from ggml-org/llama.cpp commit
//! e3546c7948e3af463d0b401e6421d5a4c2faf565, `src/llama-vocab.cpp`:
//!   - `case LLAMA_VOCAB_PRE_TYPE_GEMMA4:` (~line 506): pretokenizer regex
//!     `[^\n]+|[\n]+`, `byte_encode = false`.
//!   - `llm_tokenizer_bpe_session::tokenize` (~line 597): confirms Gemma4
//!     still goes through the SAME greedy-lowest-rank-pair BPE merge loop
//!     `qwen2`/`llama` use (this crate's `bpe_merge` is a verbatim, unedited
//!     copy — the algorithm doesn't care what the base symbols represent).
//!     With `byte_encode=false`, base symbols are whole UTF-8 characters
//!     (`unicode_len_utf8`-sized chunks), and an unmerged piece with no
//!     vocab entry falls back to one `<0xXX>` hex token per RAW BYTE (not
//!     a byte-remapped-character lookup like the `byte_encode=true` path).
//!   - `llama_escape_whitespace`/`llama_unescape_whitespace` (~line 3263):
//!     literally `replace_all(text, " ", "\xe2\x96\x81")` and its inverse —
//!     no other normalization (no space-prefix insertion, no lowercasing).
//!     Applied to the whole input BEFORE pretokenization, gated by
//!     `escape_whitespaces` (forced `true` for `tokenizer_pre == "gemma4"`,
//!     ~line 2181).
//!   - The all-newline-chunk special case (~line 613, referencing PR
//!     #21343): if a pretokenized chunk is entirely `\n` characters AND
//!     exists directly in vocab, use it as one token without running BPE
//!     merge on it at all — replicated here, not assumed unnecessary.
//!   - `add_bos` workaround (~line 2548, PR #21500): llama.cpp forces
//!     `add_bos = true` for Gemma4 regardless of what the GGUF's own
//!     `tokenizer.ggml.add_bos_token` says, when that key is absent/false.
//!     This crate always sets it `true` — same net effect (the reference's
//!     conditional only ever raises false to true, never lowers true to
//!     false), simpler code.

use std::collections::HashMap;

use gguf::{GgufFile, MetadataValue};

use crate::error::LoadError;

const ESCAPED_SPACE: &str = "\u{2581}"; // "▁", U+2581 -- \xE2\x96\x81 in UTF-8

pub struct Tokenizer {
    id_to_token: Vec<String>,
    token_to_id: HashMap<String, u32>,
    merge_rank: HashMap<(String, String), usize>,
    pub bos_token_id: Option<u32>,
    pub eos_token_id: Option<u32>,
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

/// Parses a `<0xXX>` hex-byte vocab entry back to its raw byte value, if
/// `piece` is exactly that format. Inverse of the encode-side fallback.
fn parse_hex_byte_token(piece: &str) -> Option<u8> {
    let hex = piece.strip_prefix("<0x")?.strip_suffix('>')?;
    u8::from_str_radix(hex, 16).ok()
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
            let (l, r) = m.split_once(' ').ok_or(LoadError::WrongMetadataType("tokenizer.ggml.merges"))?;
            merge_rank.insert((l.to_string(), r.to_string()), rank);
        }

        let bos_token_id = match gguf.metadata.get("tokenizer.ggml.bos_token_id") {
            Some(MetadataValue::Uint32(v)) => Some(*v),
            _ => None,
        };
        let eos_token_id = match gguf.metadata.get("tokenizer.ggml.eos_token_id") {
            Some(MetadataValue::Uint32(v)) => Some(*v),
            _ => None,
        };

        Ok(Tokenizer { id_to_token, token_to_id, merge_rank, bos_token_id, eos_token_id })
    }

    pub fn vocab_size(&self) -> usize {
        self.id_to_token.len()
    }

    pub fn special_token_id(&self, token_str: &str) -> Option<u32> {
        self.token_to_id.get(token_str).copied()
    }

    /// Verbatim-identical algorithm to `qwen2`/`llama`'s `bpe_merge` --
    /// greedy lowest-merge-rank adjacent-pair merging. The BPE merge loop
    /// itself is not GPT-2/byte-encoding-specific; only what the initial
    /// `symbols` represent differs per tokenizer (byte-remapped characters
    /// there, whole UTF-8 characters here).
    fn bpe_merge(&self, mut symbols: Vec<String>) -> Vec<String> {
        loop {
            let mut best: Option<(usize, usize)> = None;
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
        // add_bos_token is always true for Gemma4 -- see module doc comment.
        let mut ids = Vec::new();
        if let Some(bos) = self.bos_token_id {
            ids.push(bos);
        }
        ids.extend(self.encode_no_bos(text));
        ids
    }

    /// BPE-encodes `text` without prepending BOS -- needed by
    /// `chat_template::render_prompt_ids`, same reason as `llama`'s
    /// `encode_no_bos` (BOS must appear once per rendered dialog, not once
    /// per message).
    pub(crate) fn encode_no_bos(&self, text: &str) -> Vec<u32> {
        let escaped = text.replace(' ', ESCAPED_SPACE);
        let mut ids = Vec::new();

        for chunk in gemma4_pretokenize(&escaped) {
            // PR #21343 special case: an all-newline chunk that's already a
            // single vocab entry skips BPE merge entirely.
            if !chunk.is_empty() && chunk.chars().all(|c| c == '\n') {
                if let Some(&id) = self.token_to_id.get(&chunk) {
                    ids.push(id);
                    continue;
                }
            }

            let symbols: Vec<String> = chunk.chars().map(|c| c.to_string()).collect();
            for piece in self.bpe_merge(symbols) {
                match self.token_to_id.get(&piece) {
                    Some(&id) => ids.push(id),
                    None => {
                        // Unknown piece -> one <0xXX> hex token per RAW BYTE
                        // (not a byte-remapped-character lookup -- that's
                        // the OTHER tokenizer family's fallback).
                        for b in piece.bytes() {
                            let hex_tok = format!("<0x{b:02X}>");
                            if let Some(&id) = self.token_to_id.get(&hex_tok) {
                                ids.push(id);
                            }
                            // Silently dropped if even the hex fallback has
                            // no vocab entry -- matches the reference (only
                            // pushes `token_multibyte` when it resolves).
                        }
                    }
                }
            }
        }
        ids
    }

    pub fn decode(&self, ids: &[u32]) -> String {
        String::from_utf8_lossy(&self.decode_bytes(ids)).into_owned()
    }

    pub fn decode_bytes(&self, ids: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for &id in ids {
            let Some(piece) = self.id_to_token.get(id as usize) else { continue };
            match parse_hex_byte_token(piece) {
                Some(b) => bytes.push(b),
                None => bytes.extend_from_slice(piece.as_bytes()),
            }
        }
        replace_bytes(&mut bytes, ESCAPED_SPACE.as_bytes(), b" ");
        bytes
    }
}

#[cfg(test)]
impl Tokenizer {
    /// Test-only: vocab is the full `<0xXX>` hex-byte fallback range (0-255)
    /// plus `extra_tokens` -- no merge rules, so `encode`'s BPE step never
    /// merges adjacent characters, and every character not directly in
    /// `extra_tokens` round-trips through the per-raw-byte hex fallback
    /// instead of being silently dropped. Mirrors `llama::tokenizer`'s
    /// `test_instance`, which pre-seeds its equivalent fallback alphabet
    /// (the full byte-to-unicode remap table) for the same reason: a real
    /// Gemma4 vocab always has this hex-byte range (see this module's doc
    /// comment), so tests should too.
    pub(crate) fn test_instance(extra_tokens: &[&str]) -> Tokenizer {
        let mut id_to_token: Vec<String> = extra_tokens.iter().map(|s| s.to_string()).collect();
        id_to_token.extend((0u16..256).map(|b| format!("<0x{b:02X}>")));

        let mut token_to_id = HashMap::with_capacity(id_to_token.len());
        for (id, tok) in id_to_token.iter().enumerate() {
            token_to_id.insert(tok.clone(), id as u32);
        }
        Tokenizer { id_to_token, token_to_id, merge_rank: HashMap::new(), bos_token_id: None, eos_token_id: None }
    }
}

/// Replaces every occurrence of `from` with `to` in a raw byte buffer.
/// Needed because `decode_bytes` can't assume the buffer is valid UTF-8 at
/// every point (a token's bytes may end mid-character during streaming --
/// same reasoning as `qwen2`/`llama`'s `decode_bytes` doc comments), so the
/// "▁" -> " " unescape has to work at the byte level, not `str::replace`.
fn replace_bytes(data: &mut Vec<u8>, from: &[u8], to: &[u8]) {
    if from.is_empty() || data.len() < from.len() {
        return;
    }
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    *data = out;
}

/// Gemma4's pretokenizer: `[^\n]+|[\n]+` -- split into maximal runs of
/// newline vs. non-newline characters. Every character is either `\n` or
/// not, so this is an exhaustive, order-preserving run-length split; no
/// scope limits or Unicode-category approximations to caveat here, unlike
/// `qwen2`/`llama`'s far more elaborate pretokenizer.
pub fn gemma4_pretokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut current_is_newline: Option<bool> = None;
    for c in text.chars() {
        let is_nl = c == '\n';
        match current_is_newline {
            Some(prev) if prev == is_nl => current.push(c),
            _ => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                current.push(c);
                current_is_newline = Some(is_nl);
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretokenize_splits_newline_runs_from_text() {
        assert_eq!(gemma4_pretokenize("ab\n\ncd"), vec!["ab", "\n\n", "cd"]);
        assert_eq!(gemma4_pretokenize("\nabc"), vec!["\n", "abc"]);
        assert_eq!(gemma4_pretokenize("abc\n"), vec!["abc", "\n"]);
        assert_eq!(gemma4_pretokenize(""), Vec::<String>::new());
    }

    #[test]
    fn escapes_space_to_lower_one_eighth_block() {
        let t = Tokenizer::test_instance(&["a", "\u{2581}", "b"]);
        // "a b" -> escape space -> "a" "▁" "b" as 3 base symbols, no merges
        // configured -> each looked up individually.
        let ids = t.encode_no_bos("a b");
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn decode_unescapes_lower_one_eighth_block_back_to_space() {
        let t = Tokenizer::test_instance(&["a", "\u{2581}", "b"]);
        assert_eq!(t.decode(&[0, 1, 2]), "a b");
    }

    #[test]
    fn unknown_piece_falls_back_to_hex_byte_tokens() {
        // "é" (U+00E9, 2 UTF-8 bytes 0xC3 0xA9) has no vocab entry -> falls
        // back to <0xC3><0xA9> (ids 0xC3=195, 0xA9=169 in test_instance's
        // default hex range), which round-trip back to the same 2 bytes.
        let t = Tokenizer::test_instance(&[]);
        let ids = t.encode_no_bos("é");
        assert_eq!(ids, vec![0xC3, 0xA9]);
        assert_eq!(t.decode_bytes(&ids), vec![0xC3, 0xA9]);
    }

    #[test]
    fn all_newline_chunk_uses_direct_vocab_entry_not_char_by_char_merge() {
        // "\n\n" is directly in vocab as ONE entry -- PR #21343's special
        // case. Without it, this would instead produce two separate "\n"
        // lookups (or fail if "\n" alone isn't its own vocab entry).
        let t = Tokenizer::test_instance(&["\n\n"]);
        let ids = t.encode_no_bos("\n\n");
        assert_eq!(ids, vec![0]);
    }

    #[test]
    fn encode_prepends_bos_unconditionally() {
        let mut t = Tokenizer::test_instance(&["a"]);
        t.bos_token_id = Some(99);
        assert_eq!(t.encode("a"), vec![99, 0]);
        // encode_no_bos, used by chat_template, must NOT prepend it.
        assert_eq!(t.encode_no_bos("a"), vec![0]);
    }
}
