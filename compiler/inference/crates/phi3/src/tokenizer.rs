//! SentencePiece (`LLAMA_VOCAB_TYPE_SPM`) tokenizer — genuinely different
//! algorithm from every other crate in this workspace, not a pretokenizer
//! variant: qwen2/llama/gemma4 are all "BPE with an explicit merges list"
//! (greedy lowest-merge-*rank* adjacent-pair scan). SPM has no merges list
//! at all — the vocab's per-token SCORE doubles as the merge priority, and
//! a priority queue picks the highest-scoring valid merge across the WHOLE
//! remaining symbol chain each step, not a left-to-right scan.
//!
//! This is Phi-3's tokenizer because its GGUF conversion emits
//! `tokenizer.ggml.model = "llama"` with per-token `tokenizer.ggml.scores`
//! and NO `tokenizer.ggml.merges` key (`conversion/phi.py`'s
//! `Phi3MiniModel.set_vocab`, loading a real `SentencePieceProcessor` and
//! writing `add_tokenizer_model("llama")` — "llama" is llama.cpp's vocab-
//! type label for genuine SentencePiece, not this engine's `llama` crate).
//! It is also, unverified here since it wasn't this session's target, the
//! tokenizer classic Llama 1/2 checkpoints use — this crate's `llama.rs`
//! doc comment already flagged that as an out-of-scope gap; this same
//! `Tokenizer` would very likely cover it too, sharing the same vocab type.
//!
//! NOT every Phi-3-arch GGUF uses this: `phi.py`'s comment "Phi-4 model
//! uses GPT2Tokenizer" means some checkpoints (detected by the source HF
//! model's `tokenizer_class`) get byte-level BPE instead
//! (`tokenizer.ggml.model = "gpt2"`) — a DIFFERENT algorithm this module
//! does not implement (that's qwen2/llama's family, not this one).
//! `Tokenizer::from_gguf` checks `tokenizer.ggml.model` and returns
//! `LoadError::UnsupportedTokenizerModel` rather than silently
//! mis-tokenizing if it isn't `"llama"`.
//!
//! Every claim below is VERIFIED against llama.cpp source (not recalled),
//! fetched 2026-07-12 from ggml-org/llama.cpp commit
//! e3546c7948e3af463d0b401e6421d5a4c2faf565, `src/llama-vocab.cpp`:
//!   - `llm_tokenizer_spm_session::tokenize` (~line 117) and
//!     `llm_bigram_spm::comparator` (~line 97): the priority-queue merge
//!     algorithm and its exact tie-break (higher score wins; on a score
//!     tie, the bigram starting EARLIEST in the text wins — `l.left >
//!     r.left` in a "less-than" comparator means smaller `left` sorts
//!     greater/higher-priority).
//!   - `llama_vocab::impl::tokenize`'s `LLAMA_VOCAB_TYPE_SPM` case
//!     (~line 3305): `add_bos` is true, `add_space_prefix` is true (both
//!     defaults for this vocab type) — a literal space is prepended before
//!     escaping when the text immediately follows BOS, then
//!     `llama_escape_whitespace` (" " -> "\xe2\x96\x81", i.e. "▁") runs
//!     over the whole thing before the priority-queue algorithm sees it.
//!     Only the top-level `encode()` gets this space-prefix treatment (it
//!     always emits BOS first, so text always "follows BOS" here); this
//!     crate's chat template instead manually pushes special-token IDs and
//!     calls `encode_no_bos` for plain-text pieces in between, the same
//!     simplified design already used by `llama`/`gemma4` (no general
//!     "recognize special-token substrings inside arbitrary text" fragment
//!     partitioner here either), so `encode_no_bos` does NOT add the prefix.
//!   - `llama_vocab::byte_to_token` (~line 3830), `LLAMA_VOCAB_TYPE_SPM`
//!     case: byte fallback tries `<0xXX>` first (same format gemma4's
//!     tokenizer already uses), then a literal single-byte string.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use gguf::{GgufFile, MetadataValue};

use crate::error::LoadError;

const ESCAPED_SPACE: &str = "\u{2581}"; // "▁", U+2581 -- \xE2\x96\x81 in UTF-8

pub struct Tokenizer {
    id_to_token: Vec<String>,
    token_to_id: HashMap<String, u32>,
    scores: Vec<f32>,
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

fn read_f32_array(gguf: &GgufFile, key: &'static str) -> Result<Vec<f32>, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Array(items)) => items
            .iter()
            .map(|v| match v {
                MetadataValue::Float32(f) => Ok(*f),
                _ => Err(LoadError::WrongMetadataType(key)),
            })
            .collect(),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Err(LoadError::MissingMetadata(key)),
    }
}

fn parse_hex_byte_token(piece: &str) -> Option<u8> {
    let hex = piece.strip_prefix("<0x")?.strip_suffix('>')?;
    u8::from_str_radix(hex, 16).ok()
}

/// One character-span in the working symbol chain — a doubly-linked list
/// over byte ranges of the (already ▁-escaped) input text, threaded via
/// `prev`/`next` indices into the `symbols` vec rather than raw pointers
/// (the borrow checker doesn't like self-referential slices into a Vec
/// that's simultaneously being mutated; indices sidestep that entirely).
/// `len == 0` marks a symbol that has been merged into its left neighbor
/// and is now dead — same sentinel the C++ reference uses (`sym.n == 0`).
#[derive(Clone, Copy)]
struct Symbol {
    start: usize,
    len: usize,
    prev: Option<usize>,
    next: Option<usize>,
}

/// A candidate merge in the priority queue. `size` is the byte length of
/// `left`+`right` combined AT THE TIME this bigram was queued — used to
/// detect staleness (one side already got absorbed by a competing merge)
/// without needing back-references, same technique as the reference.
struct Bigram {
    left: usize,
    right: usize,
    score: f32,
    size: usize,
}

impl Bigram {
    /// Higher score wins; on a tie, the EARLIER (smaller `left`) bigram
    /// wins. VERIFIED against `llm_bigram_spm::comparator` (see module doc
    /// comment) — not the "obvious" tie-break, worth pinning precisely.
    fn priority_cmp(&self, other: &Self) -> Ordering {
        self.score.total_cmp(&other.score).then_with(|| other.left.cmp(&self.left))
    }
}
impl PartialEq for Bigram {
    fn eq(&self, other: &Self) -> bool {
        self.priority_cmp(other) == Ordering::Equal
    }
}
impl Eq for Bigram {}
impl PartialOrd for Bigram {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Bigram {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority_cmp(other)
    }
}

impl Tokenizer {
    pub fn from_gguf(gguf: &GgufFile) -> Result<Tokenizer, LoadError> {
        match gguf.metadata.get("tokenizer.ggml.model") {
            Some(MetadataValue::String(m)) if m == "llama" => {}
            Some(MetadataValue::String(m)) => return Err(LoadError::UnsupportedTokenizerModel(m.clone())),
            Some(_) => return Err(LoadError::WrongMetadataType("tokenizer.ggml.model")),
            None => return Err(LoadError::MissingMetadata("tokenizer.ggml.model")),
        }

        let id_to_token = read_string_array(gguf, "tokenizer.ggml.tokens")?;
        let scores = read_f32_array(gguf, "tokenizer.ggml.scores")?;
        if scores.len() != id_to_token.len() {
            return Err(LoadError::WrongMetadataType("tokenizer.ggml.scores"));
        }

        let mut token_to_id = HashMap::with_capacity(id_to_token.len());
        for (id, tok) in id_to_token.iter().enumerate() {
            token_to_id.insert(tok.clone(), id as u32);
        }

        let bos_token_id = match gguf.metadata.get("tokenizer.ggml.bos_token_id") {
            Some(MetadataValue::Uint32(v)) => Some(*v),
            _ => None,
        };
        let eos_token_id = match gguf.metadata.get("tokenizer.ggml.eos_token_id") {
            Some(MetadataValue::Uint32(v)) => Some(*v),
            _ => None,
        };

        Ok(Tokenizer { id_to_token, token_to_id, scores, bos_token_id, eos_token_id })
    }

    pub fn vocab_size(&self) -> usize {
        self.id_to_token.len()
    }

    pub fn special_token_id(&self, token_str: &str) -> Option<u32> {
        self.token_to_id.get(token_str).copied()
    }

    /// Top-level encode: BOS, then (per `add_space_prefix`, unconditional
    /// here — see module doc comment) a literal leading space, then the
    /// standard ▁-escape + priority-queue merge.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        if let Some(bos) = self.bos_token_id {
            ids.push(bos);
        }
        let prefixed = format!(" {text}");
        ids.extend(self.encode_no_bos(&prefixed));
        ids
    }

    /// The SPM priority-queue merge, without BOS or the leading-space
    /// prefix -- used standalone by `chat_template` for per-turn content,
    /// same reasoning as `llama`/`gemma4`'s `encode_no_bos`.
    pub(crate) fn encode_no_bos(&self, text: &str) -> Vec<u32> {
        let escaped = text.replace(' ', ESCAPED_SPACE);
        if escaped.is_empty() {
            return Vec::new();
        }

        let mut symbols: Vec<Symbol> = Vec::new();
        for (start, ch) in escaped.char_indices() {
            let idx = symbols.len();
            symbols.push(Symbol { start, len: ch.len_utf8(), prev: idx.checked_sub(1), next: None });
            if idx > 0 {
                symbols[idx - 1].next = Some(idx);
            }
        }

        let mut heap: BinaryHeap<Bigram> = BinaryHeap::new();
        let mut rev_merge: HashMap<String, (usize, usize)> = HashMap::new();

        let try_add_bigram = |symbols: &[Symbol], heap: &mut BinaryHeap<Bigram>, rev_merge: &mut HashMap<String, (usize, usize)>, left: Option<usize>, right: Option<usize>| {
            let (Some(left), Some(right)) = (left, right) else { return };
            let text = format!(
                "{}{}",
                &escaped[symbols[left].start..symbols[left].start + symbols[left].len],
                &escaped[symbols[right].start..symbols[right].start + symbols[right].len],
            );
            let Some(&token) = self.token_to_id.get(&text) else { return };
            heap.push(Bigram { left, right, score: self.scores[token as usize], size: text.len() });
            rev_merge.insert(text, (left, right));
        };

        for i in 1..symbols.len() {
            try_add_bigram(&symbols, &mut heap, &mut rev_merge, Some(i - 1), Some(i));
        }

        while let Some(bigram) = heap.pop() {
            let (left_len, right_len) = (symbols[bigram.left].len, symbols[bigram.right].len);
            if left_len == 0 || right_len == 0 || left_len + right_len != bigram.size {
                continue; // stale: one side was already consumed by a competing merge
            }

            symbols[bigram.left].len += right_len;
            symbols[bigram.right].len = 0;
            symbols[bigram.left].next = symbols[bigram.right].next;
            if let Some(next) = symbols[bigram.right].next {
                symbols[next].prev = Some(bigram.left);
            }

            let left_prev = symbols[bigram.left].prev;
            let left_next = symbols[bigram.left].next;
            try_add_bigram(&symbols, &mut heap, &mut rev_merge, left_prev, Some(bigram.left));
            try_add_bigram(&symbols, &mut heap, &mut rev_merge, Some(bigram.left), left_next);
        }

        let mut output = Vec::new();
        let mut cursor = Some(0usize);
        while let Some(i) = cursor {
            self.resegment(&escaped, &symbols[i], &rev_merge, &symbols, &mut output);
            cursor = symbols[i].next;
        }
        output
    }

    /// Mirrors `llm_tokenizer_spm_session::resegment`: if this exact span's
    /// text is itself a vocab entry, emit it directly; otherwise, if it was
    /// formed by a recorded merge, recursively resegment the two pieces
    /// that made it; otherwise fall back to one byte-fallback token per raw
    /// byte. The middle (recursive) branch mirrors the reference faithfully
    /// but this session could not construct an input that actually reaches
    /// it — every bigram this algorithm ever queues already has its
    /// concatenated text validated as a direct vocab hit (see
    /// `try_add_bigram`), so by construction the direct-hit branch above
    /// seems to always fire first for any span this function receives. The
    /// C++ source's own "// Do we need to support is_unused?" comments (at
    /// both call sites) suggest even upstream isn't fully certain when this
    /// path triggers; implemented faithfully rather than dropped.
    fn resegment(&self, text: &str, symbol: &Symbol, rev_merge: &HashMap<String, (usize, usize)>, symbols: &[Symbol], output: &mut Vec<u32>) {
        let span = &text[symbol.start..symbol.start + symbol.len];
        if let Some(&id) = self.token_to_id.get(span) {
            output.push(id);
            return;
        }
        match rev_merge.get(span) {
            Some(&(left, right)) => {
                self.resegment(text, &symbols[left], rev_merge, symbols, output);
                self.resegment(text, &symbols[right], rev_merge, symbols, output);
            }
            None => {
                for b in span.bytes() {
                    output.push(self.byte_to_token(b));
                }
            }
        }
    }

    /// `<0xXX>` first, then a literal single-byte string -- VERIFIED order
    /// against `llama_vocab::byte_to_token`'s `LLAMA_VOCAB_TYPE_SPM` case
    /// (module doc comment). Unlike the C++ reference (which throws if
    /// neither resolves, assuming a well-formed vocab always has one), this
    /// returns 0 as a last resort rather than panicking on malformed/test
    /// vocabs -- a deliberate, noted divergence, not a silent one.
    fn byte_to_token(&self, byte: u8) -> u32 {
        let hex = format!("<0x{byte:02X}>");
        if let Some(&id) = self.token_to_id.get(&hex) {
            return id;
        }
        let single = (byte as char).to_string();
        self.token_to_id.get(&single).copied().unwrap_or(0)
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
    /// Test-only vocab: the full `<0xXX>` hex-byte fallback range (scores
    /// all 0.0, so no accidental byte-level merges) plus `extra_tokens`
    /// (inserted FIRST, so they land at predictable low ids) with scores
    /// `10.0, 20.0, 30.0, ...` in listed order -- high enough that any
    /// deliberately-listed multi-character token always outranks the
    /// zero-scored byte range on ties. Same reasoning as
    /// `gemma4::tokenizer::test_instance`: a real SentencePiece vocab
    /// always has the full byte-fallback range, so tests should too, or
    /// ordinary text silently degrades to nothing (the bug this fixes
    /// pre-emptively -- see gemma4's tokenizer.rs commit history for the
    /// hard way that was found).
    pub(crate) fn test_instance(extra_tokens: &[(&str, f32)]) -> Tokenizer {
        let mut id_to_token: Vec<String> = extra_tokens.iter().map(|(s, _)| s.to_string()).collect();
        let mut scores: Vec<f32> = extra_tokens.iter().map(|(_, s)| *s).collect();
        for b in 0u16..256 {
            id_to_token.push(format!("<0x{b:02X}>"));
            scores.push(0.0);
        }

        let mut token_to_id = HashMap::with_capacity(id_to_token.len());
        for (id, tok) in id_to_token.iter().enumerate() {
            token_to_id.insert(tok.clone(), id as u32);
        }
        Tokenizer { id_to_token, token_to_id, scores, bos_token_id: None, eos_token_id: None }
    }
}

/// Replaces every occurrence of `from` with `to` in a raw byte buffer --
/// same byte-level (not `str::replace`) approach `gemma4::tokenizer` uses,
/// for the same reason (a token's bytes may end mid-character mid-stream).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_unknown_character_falls_back_to_hex_byte_tokens() {
        // "é" (U+00E9, 2 UTF-8 bytes 0xC3 0xA9) has no vocab entry of its
        // own and no multi-char merge target exists -> falls back to
        // <0xC3><0xA9> (ids 195, 169 in test_instance's hex range).
        let t = Tokenizer::test_instance(&[]);
        let ids = t.encode_no_bos("é");
        assert_eq!(ids, vec![0xC3, 0xA9]);
        assert_eq!(t.decode_bytes(&ids), vec![0xC3, 0xA9]);
    }

    #[test]
    fn escapes_space_to_lower_one_eighth_block() {
        let t = Tokenizer::test_instance(&[("a", 10.0), ("\u{2581}", 20.0), ("b", 30.0)]);
        let ids = t.encode_no_bos("a b");
        assert_eq!(ids, vec![0, 1, 2]);
        assert_eq!(t.decode(&ids), "a b");
    }

    #[test]
    fn encode_prepends_bos_and_a_leading_space() {
        let mut t = Tokenizer::test_instance(&[("a", 10.0), ("\u{2581}", 20.0)]);
        t.bos_token_id = Some(99);
        // encode("a") -> BOS, then " a" escaped to "▁a" -> [▁, a] = [1, 0]
        assert_eq!(t.encode("a"), vec![99, 1, 0]);
        // encode_no_bos gets neither BOS nor the leading space.
        assert_eq!(t.encode_no_bos("a"), vec![0]);
    }

    #[test]
    fn priority_queue_merges_highest_score_first_and_builds_up_multi_char_tokens() {
        // Vocab: base chars a/b/c (low score), "ab" (score 1.0), "abc"
        // (score 2.0, higher). "bc" is NOT a vocab entry, so the only path
        // to a full merge is: (a,b)->"ab" first (the only bigram initially
        // valid), which then unlocks ("ab", c)->"abc" as a NEW candidate --
        // exercising the "look for new bigrams after a merge" step, not
        // just a single top-of-queue pop.
        let t = Tokenizer::test_instance(&[("a", 0.1), ("b", 0.1), ("c", 0.1), ("ab", 1.0), ("abc", 2.0)]);
        let ids = t.encode_no_bos("abc");
        assert_eq!(ids, vec![t.special_token_id("abc").unwrap()]);
    }

    #[test]
    fn priority_queue_stops_at_the_highest_reachable_merge_when_no_full_word_exists() {
        // Same vocab as above but WITHOUT "abc" -- the algorithm should
        // merge "a"+"b" into "ab" (the only valid bigram) and then find
        // that ("ab", "c") has no vocab entry, leaving two final symbols:
        // "ab" and "c", each resolved directly (no fallback needed).
        let t = Tokenizer::test_instance(&[("a", 0.1), ("b", 0.1), ("c", 0.1), ("ab", 1.0)]);
        let ids = t.encode_no_bos("abc");
        assert_eq!(ids, vec![t.special_token_id("ab").unwrap(), t.special_token_id("c").unwrap()]);
    }

    #[test]
    fn score_tie_breaks_toward_the_leftmost_bigram() {
        // "aa" with vocab {"a", "aa" (score 5.0)}, input "aaa": two
        // candidate bigrams (0,1) and (1,2) both target "aa" with the SAME
        // score (it's the same vocab entry). The leftmost (0,1) must win
        // the tie-break and be merged first, leaving "aa"+"a" as the final
        // split (not "a"+"aa" -- the merge could only ever produce one or
        // the other depending on which bigram is processed first, since
        // consuming (0,1) invalidates (1,2) as a candidate before it's
        // ever tried, and vice versa).
        let t = Tokenizer::test_instance(&[("a", 0.1), ("aa", 5.0)]);
        let ids = t.encode_no_bos("aaa");
        assert_eq!(ids, vec![t.special_token_id("aa").unwrap(), t.special_token_id("a").unwrap()]);
    }
}
