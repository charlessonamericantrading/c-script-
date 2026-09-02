//! Two GENUINELY DIFFERENT tokenizer algorithms, selected at load time from
//! `tokenizer.ggml.model` — not just pretokenizer variants of one algorithm:
//!
//! - `"gpt2"` — byte-level BPE (byte-to-unicode remap + an explicit
//!   rank-ordered merges list + greedy-lowest-rank merge, `bpe_merge`). Two
//!   pretokenizer regex variants share this same merge engine, selected
//!   from `tokenizer.ggml.pre`:
//!   - `"llama3"`/`"llama-bpe"` (Llama 3.x) — VERIFIED against llama.cpp's
//!     actual source (`src/llama-vocab.cpp`, `LLAMA_VOCAB_PRE_TYPE_LLAMA3`
//!     case, commit e3546c7948e3af463d0b401e6421d5a4c2faf565, fetched
//!     2026-07-12), not recalled from memory: byte-for-byte identical to
//!     Qwen2's except the digit rule groups 1-3 digits (`\p{N}{1,3}`)
//!     instead of splitting one digit at a time (`\p{N}`):
//!     ```text
//!     (?:'[sS]|'[tT]|'[rR][eE]|'[vV][eE]|'[mM]|'[lL][lL]|'[dD])
//!       | [^\r\n\p{L}\p{N}]?\p{L}+
//!       | \p{N}{1,3}                     <-- only line that differs from qwen2
//!       |  ?[^\s\p{L}\p{N}]+[\r\n]*
//!       | \s*[\r\n]+
//!       | \s+(?!\S)
//!       | \s+
//!     ```
//!   - `"tekken"` (Mistral NeMo/Small/Devstral and other Tekken-tokenizer
//!     releases — VERIFIED both the tokenizer TYPE (`gguf-py/gguf/vocab.py`'s
//!     `MistralVocab.gguf_tokenizer_model` returns `"gpt2"` for
//!     `MistralTokenizerType.tekken`, and its
//!     `extract_vocab_merges_from_model`/`token_bytes_to_string` use the
//!     SAME `bytes_to_unicode()` remap as every other "gpt2"-vocab-type
//!     tokenizer) and the pretokenizer regex (`LLAMA_VOCAB_PRE_TYPE_TEKKEN`
//!     case, same commit — the ACTUAL compiled regex, not the "original
//!     regex from tokenizer.json" the source comments alongside it, since
//!     llama.cpp's own regex engine can't express the original's Unicode
//!     general-category lookaheads):
//!     ```text
//!     [^\r\n\p{L}\p{N}]?((?=[\p{L}])([^a-z]))*((?=[\p{L}])([^A-Z]))+
//!       | [^\r\n\p{L}\p{N}]?((?=[\p{L}])([^a-z]))+((?=[\p{L}])([^A-Z]))*
//!       | \p{N}
//!       |  ?[^\s\p{L}\p{N}]+[\r\n/]*      <-- note the extra '/' vs llama3's [\r\n]*
//!       | \s*[\r\n]+
//!       | \s+(?!\S)
//!       | \s+
//!     ```
//!     The first two alternatives implement case-boundary-aware word
//!     splitting ("camelCase" -> "camel"+"Case", pure-uppercase runs like
//!     "HTTP" alone still match whole) — `upper_ish`/`lower_ish` below
//!     approximate `[^a-z]`/`[^A-Z]` intersected with being a letter at
//!     all; for non-ASCII letters both predicates are simultaneously true,
//!     so non-Latin scripts fall back to simple "consecutive letters = one
//!     word", same as `llama3_pretokenize`. Real Tekken-tokenizer Mistral
//!     checkpoints still very often report `general.architecture = "llama"`
//!     (verified in `conversion/mistral.py`: `MistralModel.__init__` only
//!     switches to `"mistral3"` when the source HF config has
//!     `llama_4_scaling`, a very recent mechanism most Mistral releases
//!     don't have) — meaning `model.rs`/`forward.rs` apply unchanged;
//!     tokenizer support was the actual gap.
//!
//! - `"llama"` — genuine SentencePiece (score-based priority-queue merge,
//!   NO merges list at all — the vocab's per-token score doubles as merge
//!   priority). This is what classic Llama 1/2 AND classic (pre-Tekken)
//!   Mistral 7B v0.1/v0.2 both use — VERIFIED in `conversion/llama.py`:
//!   `LlamaModel.set_vocab` tries `_set_vocab_sentencepiece` first (falling
//!   back to BPE paths only on `FileNotFoundError`), and `MistralModel`
//!   (subclassing `LlamaModel`) sets `is_mistral_format = True`, which makes
//!   `set_vocab` take `_set_vocab_mistral`'s `MistralTokenizerType.spm`
//!   branch for anything that isn't Tekken — same underlying
//!   `gguf_tokenizer_model = "llama"` either way. This crate's SPM engine
//!   is a near-verbatim port of `phi3::tokenizer`'s (same verified
//!   algorithm, ported here since Phi-3's own checkpoint uses a DIFFERENT
//!   `general.architecture` string and lives in a different crate) — see
//!   that crate's tokenizer.rs module doc comment for the full verified
//!   priority-queue merge algorithm and tie-break rule, and
//!   `llama_vocab::byte_to_token`'s `LLAMA_VOCAB_TYPE_SPM` case for the
//!   `<0xXX>`-then-literal-byte-string decode fallback order.
//!
//!   NOT implemented: Mistral's OWN `[INST]`-family chat templates
//!   (`LLM_CHAT_TEMPLATE_MISTRAL_V1`/`_V3`/`_V7`, distinct from both
//!   Llama-2's and Llama-3's) — `chat_template.rs` only renders the
//!   Llama-2-SYS format for this vocab type. A classic Mistral checkpoint
//!   would tokenize and run correctly through this crate but get the WRONG
//!   chat formatting if `render_prompt_ids` is used on it — a real,
//!   flagged gap, not silently guessed at.

use std::cmp::Ordering;
use std::collections::HashMap;

use gguf::{GgufFile, MetadataValue};

use crate::error::LoadError;

/// Which pretokenizer regex applies within the BPE engine — selected once
/// at load time from `tokenizer.ggml.pre`. See module doc comment for the
/// exact regex each one implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pretokenizer {
    Llama3,
    Tekken,
}

/// `byte_to_unicode: [char; 256]` alone is 1KB — `Box`ed (as the whole
/// `BpeVocab`, not just that one field, so every field here uses the same
/// plain-dot access) so `VocabKind::Spm` (a lone `Vec<f32>`, 24 bytes)
/// doesn't pay for it in every `Tokenizer` instance's size.
struct BpeVocab {
    byte_to_unicode: [char; 256],
    unicode_to_byte: HashMap<char, u8>,
    merge_rank: HashMap<(String, String), usize>,
    pretokenizer: Pretokenizer,
}

/// The two genuinely different tokenizer algorithms this crate implements
/// — see module doc comment. Selected once at load time from
/// `tokenizer.ggml.model`, never mixed.
enum VocabKind {
    Bpe(Box<BpeVocab>),
    Spm { scores: Vec<f32> },
}

pub struct Tokenizer {
    id_to_token: Vec<String>,
    token_to_id: HashMap<String, u32>,
    vocab: VocabKind,
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

const ESCAPED_SPACE: &str = "\u{2581}"; // "▁", U+2581 -- \xE2\x96\x81 in UTF-8 -- SPM vocab only.

fn parse_hex_byte_token(piece: &str) -> Option<u8> {
    let hex = piece.strip_prefix("<0x")?.strip_suffix('>')?;
    u8::from_str_radix(hex, 16).ok()
}

/// One character-span in the SPM working symbol chain — see
/// `phi3::tokenizer`'s module doc comment for the full verified algorithm
/// this implements (near-verbatim port).
#[derive(Clone, Copy)]
struct Symbol {
    start: usize,
    len: usize,
    prev: Option<usize>,
    next: Option<usize>,
}

struct Bigram {
    left: usize,
    right: usize,
    score: f32,
    size: usize,
}

impl Bigram {
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
        let id_to_token = read_string_array(gguf, "tokenizer.ggml.tokens")?;
        let mut token_to_id = HashMap::with_capacity(id_to_token.len());
        for (id, tok) in id_to_token.iter().enumerate() {
            token_to_id.insert(tok.clone(), id as u32);
        }

        let vocab = match gguf.metadata.get("tokenizer.ggml.model") {
            Some(MetadataValue::String(m)) if m == "gpt2" => {
                let merges_raw = read_string_array(gguf, "tokenizer.ggml.merges")?;
                let mut merge_rank = HashMap::with_capacity(merges_raw.len());
                for (rank, m) in merges_raw.iter().enumerate() {
                    let (l, r) = m.split_once(' ').ok_or(LoadError::WrongMetadataType("tokenizer.ggml.merges"))?;
                    merge_rank.insert((l.to_string(), r.to_string()), rank);
                }
                let byte_to_unicode = byte_to_unicode_table();
                let unicode_to_byte = byte_to_unicode.iter().enumerate().map(|(b, &c)| (c, b as u8)).collect();
                let pretokenizer = match gguf.metadata.get("tokenizer.ggml.pre") {
                    Some(MetadataValue::String(s)) if s == "llama3" || s == "llama-bpe" => Pretokenizer::Llama3,
                    Some(MetadataValue::String(s)) if s == "tekken" => Pretokenizer::Tekken,
                    Some(MetadataValue::String(s)) => return Err(LoadError::UnsupportedTokenizerPre(s.clone())),
                    Some(_) => return Err(LoadError::WrongMetadataType("tokenizer.ggml.pre")),
                    None => return Err(LoadError::MissingMetadata("tokenizer.ggml.pre")),
                };
                VocabKind::Bpe(Box::new(BpeVocab { byte_to_unicode, unicode_to_byte, merge_rank, pretokenizer }))
            }
            Some(MetadataValue::String(m)) if m == "llama" => {
                let scores = read_f32_array(gguf, "tokenizer.ggml.scores")?;
                if scores.len() != id_to_token.len() {
                    return Err(LoadError::WrongMetadataType("tokenizer.ggml.scores"));
                }
                VocabKind::Spm { scores }
            }
            Some(MetadataValue::String(m)) => return Err(LoadError::UnsupportedTokenizerModel(m.clone())),
            Some(_) => return Err(LoadError::WrongMetadataType("tokenizer.ggml.model")),
            None => return Err(LoadError::MissingMetadata("tokenizer.ggml.model")),
        };

        let bos_token_id = match gguf.metadata.get("tokenizer.ggml.bos_token_id") {
            Some(MetadataValue::Uint32(v)) => Some(*v),
            _ => None,
        };
        let eos_token_id = match gguf.metadata.get("tokenizer.ggml.eos_token_id") {
            Some(MetadataValue::Uint32(v)) => Some(*v),
            _ => None,
        };
        // SPM forces add_bos=true regardless of this key (VERIFIED,
        // `llama_vocab::impl::load` LLAMA_VOCAB_TYPE_SPM case: `add_bos =
        // true` unconditionally) -- BPE respects the GGUF's own flag.
        let add_bos_token = match &vocab {
            VocabKind::Spm { .. } => true,
            VocabKind::Bpe(_) => matches!(gguf.metadata.get("tokenizer.ggml.add_bos_token"), Some(MetadataValue::Bool(true))),
        };

        Ok(Tokenizer { id_to_token, token_to_id, vocab, bos_token_id, eos_token_id, add_bos_token })
    }

    pub fn vocab_size(&self) -> usize {
        self.id_to_token.len()
    }

    pub fn special_token_id(&self, token_str: &str) -> Option<u32> {
        self.token_to_id.get(token_str).copied()
    }

    /// Which chat template applies — SPM-vocab checkpoints (classic
    /// Llama 1/2, classic Mistral) use `[INST]`/`<<SYS>>`, not Llama 3's
    /// `<|start_header_id|>` markers. See `chat_template.rs`'s module doc
    /// comment for the dispatch and its Mistral-template caveat.
    pub(crate) fn is_spm(&self) -> bool {
        matches!(self.vocab, VocabKind::Spm { .. })
    }

    fn bpe_merge(&self, merge_rank: &HashMap<(String, String), usize>, mut symbols: Vec<String>) -> Vec<String> {
        loop {
            let mut best: Option<(usize, usize)> = None;
            for i in 0..symbols.len().saturating_sub(1) {
                if let Some(&rank) = merge_rank.get(&(symbols[i].clone(), symbols[i + 1].clone())) {
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
        // SPM also prepends a literal leading space before the BOS-following
        // text (`add_space_prefix`, VERIFIED default `true` for this vocab
        // type) -- see `phi3::tokenizer::encode`'s identical reasoning.
        if let VocabKind::Spm { .. } = &self.vocab {
            let prefixed = format!(" {text}");
            ids.extend(self.encode_no_bos(&prefixed));
        } else {
            ids.extend(self.encode_no_bos(text));
        }
        ids
    }

    /// Encodes WITHOUT the leading `add_bos_token`/space-prefix handling —
    /// needed by `chat_template::render_prompt_ids`, which pushes BOS
    /// itself exactly once at the very start of the whole rendered dialog,
    /// not once per message (same reasoning documented on every other
    /// crate's `encode_no_bos`).
    pub(crate) fn encode_no_bos(&self, text: &str) -> Vec<u32> {
        match &self.vocab {
            VocabKind::Bpe(bpe) => {
                let mut ids = Vec::new();
                let chunks = match bpe.pretokenizer {
                    Pretokenizer::Llama3 => llama3_pretokenize(text),
                    Pretokenizer::Tekken => tekken_pretokenize(text),
                };
                for chunk in chunks {
                    let mapped: Vec<String> = chunk.bytes().map(|b| bpe.byte_to_unicode[b as usize].to_string()).collect();
                    for piece in self.bpe_merge(&bpe.merge_rank, mapped) {
                        match self.token_to_id.get(&piece) {
                            Some(&id) => ids.push(id),
                            None => panic!("encode: no vocab entry for BPE piece {piece:?} (chunk {chunk:?}) — every byte-mapped character should be a base vocab entry for a complete GPT-2-style vocab"),
                        }
                    }
                }
                ids
            }
            VocabKind::Spm { scores } => self.spm_encode(scores, text),
        }
    }

    /// The SPM priority-queue merge — near-verbatim port of
    /// `phi3::tokenizer::encode_no_bos`, see that crate's module doc
    /// comment for the full verified algorithm.
    fn spm_encode(&self, scores: &[f32], text: &str) -> Vec<u32> {
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

        let mut heap: std::collections::BinaryHeap<Bigram> = std::collections::BinaryHeap::new();
        let mut rev_merge: HashMap<String, (usize, usize)> = HashMap::new();

        let try_add_bigram = |symbols: &[Symbol], heap: &mut std::collections::BinaryHeap<Bigram>, rev_merge: &mut HashMap<String, (usize, usize)>, left: Option<usize>, right: Option<usize>| {
            let (Some(left), Some(right)) = (left, right) else { return };
            let text = format!(
                "{}{}",
                &escaped[symbols[left].start..symbols[left].start + symbols[left].len],
                &escaped[symbols[right].start..symbols[right].start + symbols[right].len],
            );
            let Some(&token) = self.token_to_id.get(&text) else { return };
            heap.push(Bigram { left, right, score: scores[token as usize], size: text.len() });
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
            self.spm_resegment(&escaped, &symbols[i], &rev_merge, &symbols, &mut output);
            cursor = symbols[i].next;
        }
        output
    }

    fn spm_resegment(&self, text: &str, symbol: &Symbol, rev_merge: &HashMap<String, (usize, usize)>, symbols: &[Symbol], output: &mut Vec<u32>) {
        let span = &text[symbol.start..symbol.start + symbol.len];
        if let Some(&id) = self.token_to_id.get(span) {
            output.push(id);
            return;
        }
        match rev_merge.get(span) {
            Some(&(left, right)) => {
                self.spm_resegment(text, &symbols[left], rev_merge, symbols, output);
                self.spm_resegment(text, &symbols[right], rev_merge, symbols, output);
            }
            None => {
                for b in span.bytes() {
                    output.push(self.spm_byte_to_token(b));
                }
            }
        }
    }

    /// `<0xXX>` first, then a literal single-byte string -- VERIFIED order
    /// against `llama_vocab::byte_to_token`'s `LLAMA_VOCAB_TYPE_SPM` case.
    /// Unlike the C++ reference (which throws if neither resolves), returns
    /// 0 as a last resort rather than panicking on malformed/test vocabs.
    fn spm_byte_to_token(&self, byte: u8) -> u32 {
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
        match &self.vocab {
            VocabKind::Bpe(bpe) => {
                let mut bytes = Vec::new();
                for &id in ids {
                    let Some(piece) = self.id_to_token.get(id as usize) else { continue };
                    for c in piece.chars() {
                        if let Some(&b) = bpe.unicode_to_byte.get(&c) {
                            bytes.push(b);
                        }
                    }
                }
                bytes
            }
            VocabKind::Spm { .. } => {
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
    }
}

/// Replaces every occurrence of `from` with `to` in a raw byte buffer --
/// byte-level (not `str::replace`) because a token's bytes may end
/// mid-character mid-stream. SPM decode only.
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
impl Tokenizer {
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
            vocab: VocabKind::Bpe(Box::new(BpeVocab { byte_to_unicode, unicode_to_byte, merge_rank: HashMap::new(), pretokenizer: Pretokenizer::Llama3 })),
            bos_token_id: None,
            eos_token_id: None,
            add_bos_token: false,
        }
    }

    /// SPM counterpart of `test_instance` — full `<0xXX>` hex-byte fallback
    /// range (score 0.0) plus `extra_tokens` (inserted first, predictable
    /// low ids, given scores). Same reasoning as `gemma4`/`phi3`'s test
    /// fixtures: a real SPM vocab always has the full byte-fallback range.
    pub(crate) fn test_instance_spm(extra_tokens: &[(&str, f32)]) -> Tokenizer {
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

        Tokenizer {
            id_to_token,
            token_to_id,
            vocab: VocabKind::Spm { scores },
            bos_token_id: None,
            eos_token_id: None,
            add_bos_token: true,
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
/// Tekken word-splitting predicates — see the module doc comment for the
/// exact regex and why `is_letter(c)` guarantees at least one of these two
/// holds for any given letter (and both hold simultaneously for non-ASCII
/// letters).
fn upper_ish(c: char) -> bool {
    is_letter(c) && !c.is_ascii_lowercase()
}
fn lower_ish(c: char) -> bool {
    is_letter(c) && !c.is_ascii_uppercase()
}

/// Llama 3's pre-tokenizer split — see this file's module doc comment for
/// the verified-against-source regex and its one-rule difference from
/// `qwen2::tokenizer::qwen2_pretokenize` (R3 below: digits group in runs of
/// up to 3, not one at a time). Same scope limits as qwen2's version:
/// `\p{L}`/`\p{N}` approximated with `char::is_alphabetic`/`is_numeric`
/// (close but not byte-identical to Unicode General_Category for every
/// script); exact for ASCII/Latin-1.
pub fn llama3_pretokenize(text: &str) -> Vec<String> {
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

        // R3: \p{N}{1,3}  (up to 3 digits per piece — THE line that differs
        // from qwen2_pretokenize's \p{N}, single digit).
        if is_digit(chars[i]) {
            let mut j = i + 1;
            while j < n && is_digit(chars[j]) && j - i < 3 {
                j += 1;
            }
            out.push(chars[i..j].iter().collect());
            i = j;
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
        // once, then decide which of R5/R6/R7 it falls under. (Faithful
        // port of qwen2_pretokenize's identical logic — R5/R6/R7 are
        // unchanged from Qwen2's regex, only R3 above differs.)
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

            // R5: \s*[\r\n]+ — consume through the last \r/\n in the run,
            // even if non-newline whitespace is interleaved before/after it
            // within the run (e.g. " \n \n" is ONE R5 token, not several).
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

/// Tekken's pre-tokenizer split — see this file's module doc comment for
/// the verified-against-source regex. Same scope limits as
/// `llama3_pretokenize`: `\p{L}`/`\p{N}` approximated with
/// `char::is_alphabetic`/`is_numeric`, exact for ASCII/Latin-1.
pub fn tekken_pretokenize(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < n {
        // Word rules (the regex's first two alternatives): optional leading
        // non-letter/digit/newline prefix (same class as llama3's R2, reuses
        // is_r2_prefix), then a case-boundary-aware letter run. VERIFIED
        // equivalence to the source's two-alternative backtracking regex:
        // upper_ish/lower_ish are mutually exclusive for ASCII, so "greedily
        // eat upper_ish, then greedily eat lower_ish right after; if that
        // got 0 lower_ish chars, fall back to just the upper_ish run" always
        // reaches the same fixed point backtracking would (see module doc
        // comment for the full derivation) — no backtracking simulation
        // needed.
        let has_prefix = is_r2_prefix(chars[i]) && i + 1 < n && is_letter(chars[i + 1]);
        if has_prefix || is_letter(chars[i]) {
            let word_start = if has_prefix { i + 1 } else { i };
            let mut j = word_start;
            while j < n && upper_ish(chars[j]) {
                j += 1;
            }
            let upper_end = j;
            while j < n && lower_ish(chars[j]) {
                j += 1;
            }
            let end = if j > upper_end {
                j // group 1: 0+ upper_ish then 1+ lower_ish
            } else {
                upper_end // group 2: 1+ upper_ish, no lower_ish tail (an all-caps run)
            };
            out.push(chars[i..end].iter().collect());
            i = end;
            continue;
        }

        // \p{N} — single digit, NOT grouped (unlike llama3's \p{N}{1,3}).
        if is_digit(chars[i]) {
            out.push(chars[i].to_string());
            i += 1;
            continue;
        }

        //  ?[^\s\p{L}\p{N}]+[\r\n/]* — same as llama3's R4 except the
        // trailing run also swallows literal '/' (not just \r\n).
        let symbol_run_end = |start: usize| -> Option<usize> {
            if start < n && is_symbol(chars[start]) {
                let mut j = start + 1;
                while j < n && is_symbol(chars[j]) {
                    j += 1;
                }
                while j < n && (chars[j] == '\r' || chars[j] == '\n' || chars[j] == '/') {
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

        // Whitespace rules (\s*[\r\n]+ / \s+(?!\S) / \s+) — byte-for-byte
        // identical to llama3_pretokenize's R5/R6/R7, see that function for
        // the full reasoning.
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

            if let Some(last_nl) = last_newline {
                out.push(chars[i..=last_nl].iter().collect());
                i = last_nl + 1;
                continue;
            }
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
            out.push(chars[i..run_end].iter().collect());
            i = run_end;
            continue;
        }

        // Unreachable for well-formed Unicode text, but never silently drop
        // a character.
        out.push(chars[i].to_string());
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretokenize_groups_digits_up_to_three() {
        assert_eq!(llama3_pretokenize("12345"), vec!["123", "45"]);
        assert_eq!(llama3_pretokenize("7"), vec!["7"]);
        assert_eq!(llama3_pretokenize("999"), vec!["999"]);
    }

    #[test]
    fn pretokenize_splits_contraction() {
        assert_eq!(llama3_pretokenize("don't"), vec!["don", "'t"]);
    }

    #[test]
    fn pretokenize_punctuation_run() {
        assert_eq!(llama3_pretokenize("hello!!!"), vec!["hello", "!!!"]);
    }

    #[test]
    fn pretokenize_newline_run() {
        assert_eq!(llama3_pretokenize("a\n\nb"), vec!["a", "\n\n", "b"]);
    }

    #[test]
    fn pretokenize_interleaved_space_and_newline_is_one_r5_token() {
        // \s*[\r\n]+ consumes through the LAST \r/\n even with plain
        // whitespace interleaved before/after it in the same run — this is
        // the exact case a naive "scan spaces, then scan newlines" rewrite
        // gets wrong (splits into two tokens instead of one).
        assert_eq!(llama3_pretokenize("a \n \nb"), vec!["a", " \n \n", "b"]);
    }

    #[test]
    fn pretokenize_trailing_space_leaves_one_char_for_next_word() {
        // R6 (\s+(?!\S)): a multi-space run before a word keeps all but the
        // last space as its own token, leaving exactly one space for the
        // next word's optional R2 prefix.
        assert_eq!(llama3_pretokenize("a   b"), vec!["a", "  ", " b"]);
    }

    #[test]
    fn tekken_pretokenize_splits_camelcase_at_the_case_boundary() {
        // "camel" (0 upper_ish + 5 lower_ish, group 1) stops the instant it
        // hits 'C' (upper_ish, not lower_ish); "Case" then starts a NEW
        // word (1 upper_ish + 3 lower_ish, group 1 again).
        assert_eq!(tekken_pretokenize("camelCase"), vec!["camel", "Case"]);
    }

    #[test]
    fn tekken_pretokenize_keeps_a_standalone_allcaps_acronym_whole() {
        // Group 1 (0+ upper then REQUIRED 1+ lower) fails here -- there's no
        // lowercase tail -- so this falls to group 2 (1+ upper, 0 lower).
        assert_eq!(tekken_pretokenize("HTTP request"), vec!["HTTP", " request"]);
    }

    #[test]
    fn tekken_pretokenize_allcaps_prefix_then_lowercase_is_one_token() {
        // Group 1 still matches the WHOLE thing: 0+ upper_ish is happy to
        // consume "HELLO" (5 chars) before requiring the 1+ lower_ish tail
        // "world" -- unlike camelCase, there's no EARLIER point where an
        // upper_ish char immediately follows a lower_ish one, so nothing
        // splits it.
        assert_eq!(tekken_pretokenize("HELLOworld"), vec!["HELLOworld"]);
    }

    #[test]
    fn tekken_pretokenize_single_digit_not_grouped() {
        // The one other rule that differs from llama3 (\p{N} vs \p{N}{1,3}).
        assert_eq!(tekken_pretokenize("123"), vec!["1", "2", "3"]);
    }

    #[test]
    fn tekken_pretokenize_punctuation_run_includes_slash() {
        assert_eq!(tekken_pretokenize("a://b"), vec!["a", "://", "b"]);
    }

    #[test]
    fn byte_to_unicode_is_involution_on_reverse_map() {
        let table = byte_to_unicode_table();
        let reverse: HashMap<char, u8> = table.iter().enumerate().map(|(b, &c)| (c, b as u8)).collect();
        for (b, &c) in table.iter().enumerate() {
            assert_eq!(reverse.get(&c), Some(&(b as u8)));
        }
    }

    #[test]
    fn spm_single_unknown_character_falls_back_to_hex_byte_tokens() {
        let t = Tokenizer::test_instance_spm(&[]);
        let ids = t.encode_no_bos("é");
        assert_eq!(ids, vec![0xC3, 0xA9]);
        assert_eq!(t.decode_bytes(&ids), vec![0xC3, 0xA9]);
    }

    #[test]
    fn spm_escapes_space_to_lower_one_eighth_block() {
        let t = Tokenizer::test_instance_spm(&[("a", 10.0), ("\u{2581}", 20.0), ("b", 30.0)]);
        let ids = t.encode_no_bos("a b");
        assert_eq!(ids, vec![0, 1, 2]);
        assert_eq!(t.decode(&ids), "a b");
    }

    #[test]
    fn spm_encode_prepends_bos_and_a_leading_space() {
        let mut t = Tokenizer::test_instance_spm(&[("a", 10.0), ("\u{2581}", 20.0)]);
        t.bos_token_id = Some(99);
        assert_eq!(t.encode("a"), vec![99, 1, 0]);
        assert_eq!(t.encode_no_bos("a"), vec![0]);
    }

    #[test]
    fn spm_priority_queue_merges_highest_score_first() {
        let t = Tokenizer::test_instance_spm(&[("a", 0.1), ("b", 0.1), ("c", 0.1), ("ab", 1.0), ("abc", 2.0)]);
        let ids = t.encode_no_bos("abc");
        assert_eq!(ids, vec![t.special_token_id("abc").unwrap()]);
    }

    #[test]
    fn spm_score_tie_breaks_toward_the_leftmost_bigram() {
        let t = Tokenizer::test_instance_spm(&[("a", 0.1), ("aa", 5.0)]);
        let ids = t.encode_no_bos("aaa");
        assert_eq!(ids, vec![t.special_token_id("aa").unwrap(), t.special_token_id("a").unwrap()]);
    }
}
