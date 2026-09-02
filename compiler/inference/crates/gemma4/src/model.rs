//! GGUF weight loading for Gemma4 (MoE hybrid — see the crate-level doc
//! comment for the full verified recipe and confidence tiers).
//!
//! Every metadata key and tensor name below is VERIFIED against llama.cpp
//! source (not recalled), fetched 2026-07-12 from ggml-org/llama.cpp commit
//! e3546c7948e3af463d0b401e6421d5a4c2faf565:
//!   - metadata keys: `src/llama-arch.cpp`'s `LLM_KV_*` -> `"%s.<key>"` table
//!     (`%s` = "gemma4", this architecture's `general.architecture` string).
//!   - tensor names: the same file's `LLM_TENSOR_*` -> `"blk.%d.<name>"` /
//!     bare top-level-name table.
//!   - hparams reading logic: `src/models/gemma4.cpp`'s
//!     `load_arch_hparams`/`load_arch_tensors`.
//!
//! Two mechanisms VERIFIED against a real local checkpoint (`gemma4:e4b`,
//! already present via Ollama — read-only inspection, not a new download),
//! not just source — the checkpoint's own metadata forced both gaps this
//! module used to just flag and reject:
//!
//!   - SWA and full-attention layers have GENUINELY DIFFERENT per-head
//!     dimensions, decoupled from `embed_dim/n_head` entirely:
//!     `gemma4:e4b` reports `attention.key_length=512`,
//!     `key_length_swa=256`, while `embed_dim/n_head` = 2560/8 = 320 (none
//!     of the three agree). VERIFIED against `src/models/gemma4.cpp`:
//!     `hparams.n_embd_head_k(il)` is a genuine per-layer function
//!     (`is_swa(il) ? n_embd_head_k_swa : n_embd_head_k_full`,
//!     `src/llama-hparams.cpp`), confirmed against this checkpoint's own
//!     tensor shapes (`blk.N.attn_k.weight` is `[2560,512]` for SWA layers,
//!     `[2560,1024]` for full-attention ones — 512/2=256 and 1024/2=512
//!     per kv-head, matching the two metadata values exactly, n_head_kv=2).
//!     `model_core::CacheShape::per_layer_head_dim` (added for this) now
//!     carries the real per-layer value; `Config::head_dim_for(il)`
//!     resolves it.
//!   - `attention.shared_kv_layers` (18 on this checkpoint, of 42 total
//!     layers) means the LAST 18 layers compute no K/V of their own at all
//!     and instead reuse an earlier layer's already-cached K/V for that
//!     step. VERIFIED against `src/llama-model.cpp`'s `reuse` callback for
//!     `LLM_ARCH_GEMMA3N`/`LLM_ARCH_GEMMA4`: a layer `il >=
//!     n_layer_kv_from_start` (`= n_layer - shared_kv_layers`) reuses layer
//!     `n_layer_kv_from_start - (is_swa(il) ? 2 : 1)` — always one of the
//!     last two layers that DO have their own KV (picking the one matching
//!     `il`'s own SWA-ness, since a virtual layer's Q dimension must match
//!     whatever cache it reads). Safe because layers process in index
//!     order 0..n_layers within one `forward_step`: a virtual layer's donor
//!     always has index < the virtual layer's own index, so the donor's
//!     row for the CURRENT step is already pushed by the time the virtual
//!     layer reads it. `Config::kv_source_layer(il)` resolves this;
//!     `LayerWeights.wk`/`.attn_k_norm` are `None` for `il >=
//!     n_layer_kv_from_start`, matching `hparams.has_kv(il)` exactly (this
//!     checkpoint actually still stores `attn_k.weight` bytes for those
//!     layers on disk — `TENSOR_NOT_REQUIRED` in the reference means
//!     "loading doesn't fail if absent", not "always absent" — so this
//!     loader deliberately does NOT even attempt to read them for
//!     `!has_kv` layers, matching the reference's runtime behavior, which
//!     never touches them either way, rather than the weaker "load
//!     defensively" reading of the flag).

use gguf::{GgufFile, MetadataValue};
use model_core::CacheShape;
use tensor_core::QuantizedMatrix;

use crate::error::LoadError;

#[derive(Debug, Clone)]
pub struct Config {
    pub n_layers: usize,
    pub embed_dim: usize,
    pub ffn_dim: usize,
    /// MoE-expert FFN size ("expert_feed_forward_length") — separate from
    /// `ffn_dim`, which sizes the dense "shared expert" FFN every layer has.
    pub n_ff_exp: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    /// Full-attention layers' per-head K/V dimension. NOT `embed_dim /
    /// n_heads` — see module doc comment.
    pub n_embd_head_k_full: usize,
    /// SWA layers' per-head K/V dimension — genuinely different from
    /// `n_embd_head_k_full` on real checkpoints, not just structurally
    /// possible.
    pub n_embd_head_k_swa: usize,
    /// First index (inclusive) from which layers stop computing their own
    /// K/V and instead reuse an earlier layer's cache — `n_layers` if the
    /// checkpoint has no shared-KV layers at all (`shared_kv_layers`
    /// absent/0). See module doc comment for the exact reuse formula.
    pub n_layer_kv_from_start: usize,
    pub vocab_size: usize,
    /// Full-attention layers' RoPE base. SWA layers use `rope_freq_base_swa`
    /// if the GGUF declares it, else this same value (optional key).
    pub rope_freq_base: f32,
    pub rope_freq_base_swa: f32,
    pub rms_eps: f32,
    pub context_length: usize,
    pub n_expert: usize,
    pub n_expert_used: usize,
    /// 0 when the checkpoint has no Per-Layer Embeddings mechanism.
    pub n_embd_per_layer: usize,
    /// 0.0 means disabled (no final-logit softcap applied).
    pub final_logit_softcapping: f32,
    /// Sliding-window size in tokens, for SWA layers.
    pub n_swa: usize,
    /// One entry per layer: true = sliding-window (local) attention,
    /// false = full (global) attention. VERIFIED semantics from
    /// `llama_hparams::is_swa`/`is_swa_impl` (`src/llama-hparams.{h,cpp}`).
    pub is_swa: Vec<bool>,
}

impl Config {
    pub fn cache_shape(&self) -> CacheShape {
        let per_layer_head_dim = (0..self.n_layers).map(|il| self.head_dim_for(il)).collect();
        CacheShape {
            n_layers: self.n_layers,
            n_kv_heads: self.n_kv_heads,
            head_dim: self.n_embd_head_k_full, // fallback value; per_layer_head_dim below is authoritative
            context_length: self.context_length,
            per_layer_head_dim: Some(per_layer_head_dim),
        }
    }

    /// This layer's real per-head K/V dimension — see module doc comment.
    pub fn head_dim_for(&self, il: usize) -> usize {
        if self.is_swa[il] {
            self.n_embd_head_k_swa
        } else {
            self.n_embd_head_k_full
        }
    }

    /// Which `KvCache` layer slot to attend against for layer `il` — itself
    /// for layers with their own K/V, an earlier donor layer otherwise. See
    /// module doc comment for the verified reuse formula.
    pub fn kv_source_layer(&self, il: usize) -> usize {
        if il < self.n_layer_kv_from_start {
            il
        } else {
            self.n_layer_kv_from_start - if self.is_swa[il] { 2 } else { 1 }
        }
    }

    pub fn has_own_kv(&self, il: usize) -> bool {
        il < self.n_layer_kv_from_start
    }
}

/// Per-expert weight matrices for one MoE-capable layer. `None` for layers
/// with no MoE branch (`ffn_gate_inp` absent in the GGUF for that layer —
/// `is_moe_layer` in the reference).
pub struct MoeWeights {
    pub gate_inp: QuantizedMatrix, // router: [n_expert, n_embd] -- projects router_in -> per-expert logits
    pub gate_inp_scale: Vec<f32>,  // elementwise scale applied to the router's input, len n_embd
    pub gate_exps: Vec<QuantizedMatrix>, // one [n_ff_exp, n_embd] matrix per expert
    pub up_exps: Vec<QuantizedMatrix>,
    pub down_exps: Vec<QuantizedMatrix>, // one [n_embd, n_ff_exp] matrix per expert
    pub pre_norm_2: Vec<f32>,  // ffn_pre_norm_2: normalizes the MoE branch's input
    pub post_norm_1: Vec<f32>, // ffn_post_norm_1: normalizes the shared-expert branch's output
    pub post_norm_2: Vec<f32>, // ffn_post_norm_2: normalizes the MoE branch's output
}

pub struct LayerWeights {
    pub attn_norm: Vec<f32>,
    pub wq: QuantizedMatrix,
    /// `None` for layers that reuse an earlier layer's K/V entirely
    /// (`!Config::has_own_kv(il)`) — see module doc comment. Deliberately
    /// NOT loaded even when the on-disk tensor happens to be present for
    /// such a layer (real checkpoints can still have the bytes on disk;
    /// the reference simply never reads them for these layers either).
    pub wk: Option<QuantizedMatrix>,
    /// `None` means EITHER "reuse K (before K's own norm) as V" (real,
    /// independent of the layer's own-KV status — VERIFIED: the
    /// reference's `Vcur = model.layers[il].wv ? ... : Kcur`) OR "this
    /// layer has no KV of its own at all" (`wk` is also `None` in that
    /// case). `forward.rs` only ever consults `wv` when `wk.is_some()`.
    pub wv: Option<QuantizedMatrix>,
    pub wo: QuantizedMatrix,
    /// Per-head learned norm on Q, applied after projection+reshape, before
    /// RoPE. Shape `[head_dim_for(il)]`.
    pub attn_q_norm: Vec<f32>,
    /// Same for K. `None` exactly when `wk` is `None`.
    pub attn_k_norm: Option<Vec<f32>>,
    pub attn_post_norm: Vec<f32>,
    /// Which `KvCache` layer slot this layer's attention reads from — see
    /// `Config::kv_source_layer`. Equals this layer's own index when
    /// `wk.is_some()`.
    pub kv_source_layer: usize,
    /// Present only for full-attention (non-SWA) layers — "proportional
    /// rope" scaling. `None` for SWA layers.
    pub rope_freqs: Option<Vec<f32>>,
    /// The dense "shared expert" FFN every layer has, MoE or not.
    pub ffn_norm: Vec<f32>,
    pub ffn_gate: QuantizedMatrix,
    pub ffn_up: QuantizedMatrix,
    pub ffn_down: QuantizedMatrix,
    pub ffn_post_norm: Vec<f32>,
    pub moe: Option<MoeWeights>,
    /// Learned per-layer output scale, applied last. Rare; `None` on most
    /// layers/checkpoints.
    pub out_scale: Option<f32>,
    /// Per-layer-embedding injection weights — `None` if the model has no
    /// PLE mechanism (`n_embd_per_layer == 0`).
    pub per_layer_inp_gate: Option<QuantizedMatrix>,
    pub per_layer_proj: Option<QuantizedMatrix>,
    pub per_layer_post_norm: Option<Vec<f32>>,
}

pub struct Model {
    pub config: Config,
    pub token_embd: QuantizedMatrix,
    pub output_weight: Option<QuantizedMatrix>,
    pub output_norm: Vec<f32>,
    /// Per-layer-embedding lookup table, `[n_embd_per_layer * n_layer, vocab_size]`
    /// laid out as `n_layer` consecutive per-vocab-entry blocks. `None` if
    /// `n_embd_per_layer == 0`.
    pub per_layer_tok_embd: Option<QuantizedMatrix>,
    pub per_layer_model_proj: Option<QuantizedMatrix>,
    pub per_layer_proj_norm: Option<Vec<f32>>,
    pub layers: Vec<LayerWeights>,
}

fn get_qmatrix2d(gguf: &GgufFile, bytes: &[u8], name: &str) -> Result<QuantizedMatrix, LoadError> {
    let t = gguf.tensors.iter().find(|t| t.name == name).ok_or_else(|| LoadError::MissingTensor(name.to_string()))?;
    if t.dimensions.len() != 2 {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let (in_features, out_features) = (t.dimensions[0] as usize, t.dimensions[1] as usize);
    let ty = t.ggml_type.ok_or_else(|| LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() })?;
    let raw = get_raw_bytes(gguf, bytes, t)?;
    Ok(QuantizedMatrix::from_raw(out_features, in_features, ty, raw.to_vec())?)
}

fn get_qmatrix2d_opt(gguf: &GgufFile, bytes: &[u8], name: &str) -> Result<Option<QuantizedMatrix>, LoadError> {
    match gguf.tensors.iter().find(|t| t.name == name) {
        Some(_) => Ok(Some(get_qmatrix2d(gguf, bytes, name)?)),
        None => Ok(None),
    }
}

fn get_vector(gguf: &GgufFile, bytes: &[u8], name: &str) -> Result<Vec<f32>, LoadError> {
    let t = gguf.tensors.iter().find(|t| t.name == name).ok_or_else(|| LoadError::MissingTensor(name.to_string()))?;
    if t.dimensions.len() != 1 {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let ty = t.ggml_type.ok_or_else(|| LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() })?;
    let n_elements = t.n_elements().ok_or_else(|| LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() })? as usize;
    let raw = get_raw_bytes(gguf, bytes, t)?;
    Ok(tensor_core::dequantize(ty, raw, n_elements)?)
}

fn get_vector_opt(gguf: &GgufFile, bytes: &[u8], name: &str) -> Result<Option<Vec<f32>>, LoadError> {
    match gguf.tensors.iter().find(|t| t.name == name) {
        Some(_) => Ok(Some(get_vector(gguf, bytes, name)?)),
        None => Ok(None),
    }
}

fn get_raw_bytes<'a>(gguf: &GgufFile, bytes: &'a [u8], t: &gguf::TensorInfo) -> Result<&'a [u8], LoadError> {
    let ty = t.ggml_type.ok_or_else(|| LoadError::UnexpectedTensorShape { name: t.name.clone(), dims: t.dimensions.clone() })?;
    let size = t.size_bytes().ok_or(tensor_core::DequantError::Unsupported(ty))?;
    let abs = gguf.tensor_absolute_offset(t)?;
    Ok(&bytes[abs as usize..abs as usize + size as usize])
}

/// Slices a 3-D `[n_embd, n_ff_exp, n_expert]` tensor (GGUF's fastest-varying
/// -first convention: `n_expert` is the SLOWEST-varying axis, so the raw
/// bytes are `n_expert` consecutive, equally-sized 2-D blocks) into
/// `n_expert` independent `QuantizedMatrix`es, one per expert. Reuses the
/// same fastest-first -> (out_features, in_features) convention
/// `get_qmatrix2d` uses for plain 2-D tensors — each expert's slice is
/// itself exactly that shape.
fn get_expert_qmatrices(gguf: &GgufFile, bytes: &[u8], name: &str, n_expert: usize) -> Result<Vec<QuantizedMatrix>, LoadError> {
    let t = gguf.tensors.iter().find(|t| t.name == name).ok_or_else(|| LoadError::MissingTensor(name.to_string()))?;
    if t.dimensions.len() != 3 {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let (in_features, out_features, n_expert_in_file) = (t.dimensions[0] as usize, t.dimensions[1] as usize, t.dimensions[2] as usize);
    if n_expert_in_file != n_expert {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let ty = t.ggml_type.ok_or_else(|| LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() })?;
    let total_bytes = t.size_bytes().ok_or(tensor_core::DequantError::Unsupported(ty))? as usize;
    if !total_bytes.is_multiple_of(n_expert) {
        return Err(LoadError::UnexpectedTensorShape { name: name.to_string(), dims: t.dimensions.clone() });
    }
    let per_expert_bytes = total_bytes / n_expert;
    let raw = get_raw_bytes(gguf, bytes, t)?;

    let mut out = Vec::with_capacity(n_expert);
    for e in 0..n_expert {
        let slice = &raw[e * per_expert_bytes..(e + 1) * per_expert_bytes];
        out.push(QuantizedMatrix::from_raw(out_features, in_features, ty, slice.to_vec())?);
    }
    Ok(out)
}

fn meta_u32(gguf: &GgufFile, key: &'static str) -> Result<u32, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Uint32(v)) => Ok(*v),
        Some(MetadataValue::Int32(v)) if *v >= 0 => Ok(*v as u32),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Err(LoadError::MissingMetadata(key)),
    }
}

fn meta_u32_opt(gguf: &GgufFile, key: &'static str) -> Result<Option<u32>, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Uint32(v)) => Ok(Some(*v)),
        Some(MetadataValue::Int32(v)) if *v >= 0 => Ok(Some(*v as u32)),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Ok(None),
    }
}

fn meta_f32(gguf: &GgufFile, key: &'static str) -> Result<f32, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Float32(v)) => Ok(*v),
        Some(MetadataValue::Uint32(v)) => Ok(*v as f32),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Err(LoadError::MissingMetadata(key)),
    }
}

fn meta_f32_opt(gguf: &GgufFile, key: &'static str) -> Result<Option<f32>, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Float32(v)) => Ok(Some(*v)),
        Some(MetadataValue::Uint32(v)) => Ok(Some(*v as f32)),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Ok(None),
    }
}

/// Reads `gemma4.attention.sliding_window_pattern` — VERIFIED (not assumed)
/// to allow two on-disk encodings, per `llama_model_loader::get_key_or_arr`
/// (`src/llama-model-loader.cpp`): a real per-layer GGUF array (length
/// `n_layers`), or a single scalar broadcast to every layer. Handles both;
/// treats nonzero as SWA, matching `is_swa_impl[il] == 1 -> SWA`.
fn read_swa_pattern(gguf: &GgufFile, key: &'static str, n_layers: usize) -> Result<Vec<bool>, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Array(items)) => {
            if items.len() != n_layers {
                return Err(LoadError::WrongMetadataType(key));
            }
            items
                .iter()
                .map(|v| match v {
                    MetadataValue::Uint32(n) => Ok(*n != 0),
                    MetadataValue::Int32(n) => Ok(*n != 0),
                    MetadataValue::Bool(b) => Ok(*b),
                    _ => Err(LoadError::WrongMetadataType(key)),
                })
                .collect()
        }
        Some(MetadataValue::Uint32(n)) => Ok(vec![*n != 0; n_layers]),
        Some(MetadataValue::Int32(n)) => Ok(vec![*n != 0; n_layers]),
        Some(MetadataValue::Bool(b)) => Ok(vec![*b; n_layers]),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Err(LoadError::MissingMetadata(key)),
    }
}

impl Model {
    pub fn load(bytes: &[u8]) -> Result<Model, LoadError> {
        let gguf = GgufFile::parse(bytes)?;

        let arch = gguf.architecture().unwrap_or("").to_string();
        if arch != "gemma4" {
            return Err(LoadError::UnexpectedArchitecture(arch));
        }

        let n_layers = meta_u32(&gguf, "gemma4.block_count")? as usize;
        let embed_dim = meta_u32(&gguf, "gemma4.embedding_length")? as usize;
        let ffn_dim = meta_u32(&gguf, "gemma4.feed_forward_length")? as usize;
        let n_ff_exp = meta_u32_opt(&gguf, "gemma4.expert_feed_forward_length")?.unwrap_or(0) as usize;
        let n_heads = meta_u32(&gguf, "gemma4.attention.head_count")? as usize;
        let n_kv_heads = meta_u32(&gguf, "gemma4.attention.head_count_kv")? as usize;
        let rms_eps = meta_f32(&gguf, "gemma4.attention.layer_norm_rms_epsilon")?;
        let context_length = meta_u32(&gguf, "gemma4.context_length")? as usize;
        let rope_freq_base = meta_f32(&gguf, "gemma4.rope.freq_base")?;
        let rope_freq_base_swa = meta_f32_opt(&gguf, "gemma4.rope.freq_base_swa")?.unwrap_or(rope_freq_base);
        let n_swa = meta_u32(&gguf, "gemma4.attention.sliding_window")? as usize;
        let n_expert = meta_u32_opt(&gguf, "gemma4.expert_count")?.unwrap_or(0) as usize;
        let n_expert_used = meta_u32_opt(&gguf, "gemma4.expert_used_count")?.unwrap_or(0) as usize;
        let n_embd_per_layer = meta_u32_opt(&gguf, "gemma4.embedding_length_per_layer_input")?.unwrap_or(0) as usize;
        let final_logit_softcapping = meta_f32_opt(&gguf, "gemma4.final_logit_softcapping")?.unwrap_or(0.0);

        // Real per-head dimensions -- NOT embed_dim/n_heads. See module doc
        // comment: VERIFIED against a real checkpoint that these differ
        // from each other AND from embed_dim/n_heads.
        let n_embd_head_k_full = meta_u32(&gguf, "gemma4.attention.key_length")? as usize;
        let n_embd_head_k_swa = meta_u32(&gguf, "gemma4.attention.key_length_swa")? as usize;

        let n_kv_shared_layers = meta_u32_opt(&gguf, "gemma4.attention.shared_kv_layers")?.unwrap_or(0) as usize;
        let n_layer_kv_from_start = n_layers.saturating_sub(n_kv_shared_layers);

        let is_swa = read_swa_pattern(&gguf, "gemma4.attention.sliding_window_pattern", n_layers)?;

        let token_embd = get_qmatrix2d(&gguf, bytes, "token_embd.weight")?;
        let vocab_size = token_embd.rows;
        let output_norm = get_vector(&gguf, bytes, "output_norm.weight")?;
        let output_weight = get_qmatrix2d_opt(&gguf, bytes, "output.weight")?;

        let (per_layer_tok_embd, per_layer_model_proj, per_layer_proj_norm) = if n_embd_per_layer > 0 {
            (
                get_qmatrix2d_opt(&gguf, bytes, "per_layer_token_embd.weight")?,
                get_qmatrix2d_opt(&gguf, bytes, "per_layer_model_proj.weight")?,
                get_vector_opt(&gguf, bytes, "per_layer_proj_norm.weight")?,
            )
        } else {
            (None, None, None)
        };

        let kv_source_layer = |i: usize| -> usize {
            if i < n_layer_kv_from_start {
                i
            } else {
                n_layer_kv_from_start - if is_swa[i] { 2 } else { 1 }
            }
        };

        let mut layers = Vec::with_capacity(n_layers);
        for i in 0..n_layers {
            let p = |suffix: &str| format!("blk.{i}.{suffix}");
            let has_own_kv = i < n_layer_kv_from_start;

            let rope_freqs = if !is_swa[i] { get_vector_opt(&gguf, bytes, &p("rope_freqs.weight"))? } else { None };

            let gate_inp = get_qmatrix2d_opt(&gguf, bytes, &p("ffn_gate_inp.weight"))?;
            let moe = match gate_inp {
                Some(gate_inp) => Some(MoeWeights {
                    gate_inp,
                    gate_inp_scale: get_vector(&gguf, bytes, &p("ffn_gate_inp.scale"))?,
                    gate_exps: get_expert_qmatrices(&gguf, bytes, &p("ffn_gate_exps.weight"), n_expert)?,
                    up_exps: get_expert_qmatrices(&gguf, bytes, &p("ffn_up_exps.weight"), n_expert)?,
                    down_exps: get_expert_qmatrices(&gguf, bytes, &p("ffn_down_exps.weight"), n_expert)?,
                    pre_norm_2: get_vector(&gguf, bytes, &p("pre_ffw_norm_2.weight"))?,
                    post_norm_1: get_vector(&gguf, bytes, &p("post_ffw_norm_1.weight"))?,
                    post_norm_2: get_vector(&gguf, bytes, &p("post_ffw_norm_2.weight"))?,
                }),
                None => None,
            };

            let (per_layer_inp_gate, per_layer_proj, per_layer_post_norm) = if n_embd_per_layer > 0 {
                (
                    get_qmatrix2d_opt(&gguf, bytes, &p("inp_gate.weight"))?,
                    get_qmatrix2d_opt(&gguf, bytes, &p("proj.weight"))?,
                    get_vector_opt(&gguf, bytes, &p("post_norm.weight"))?,
                )
            } else {
                (None, None, None)
            };

            // Deliberately NOT loaded at all for !has_own_kv layers, even
            // though the on-disk tensor can still be present -- see
            // LayerWeights::wk's doc comment.
            let (wk, wv, attn_k_norm) = if has_own_kv {
                (
                    Some(get_qmatrix2d(&gguf, bytes, &p("attn_k.weight"))?),
                    get_qmatrix2d_opt(&gguf, bytes, &p("attn_v.weight"))?,
                    Some(get_vector(&gguf, bytes, &p("attn_k_norm.weight"))?),
                )
            } else {
                (None, None, None)
            };

            layers.push(LayerWeights {
                attn_norm: get_vector(&gguf, bytes, &p("attn_norm.weight"))?,
                wq: get_qmatrix2d(&gguf, bytes, &p("attn_q.weight"))?,
                wk,
                wv,
                wo: get_qmatrix2d(&gguf, bytes, &p("attn_output.weight"))?,
                attn_q_norm: get_vector(&gguf, bytes, &p("attn_q_norm.weight"))?,
                attn_k_norm,
                attn_post_norm: get_vector(&gguf, bytes, &p("post_attention_norm.weight"))?,
                kv_source_layer: kv_source_layer(i),
                rope_freqs,
                ffn_norm: get_vector(&gguf, bytes, &p("ffn_norm.weight"))?,
                ffn_gate: get_qmatrix2d(&gguf, bytes, &p("ffn_gate.weight"))?,
                ffn_up: get_qmatrix2d(&gguf, bytes, &p("ffn_up.weight"))?,
                ffn_down: get_qmatrix2d(&gguf, bytes, &p("ffn_down.weight"))?,
                ffn_post_norm: get_vector(&gguf, bytes, &p("post_ffw_norm.weight"))?,
                moe,
                out_scale: get_vector_opt(&gguf, bytes, &p("layer_output_scale.weight"))?.map(|v| v[0]),
                per_layer_inp_gate,
                per_layer_proj,
                per_layer_post_norm,
            });
        }

        Ok(Model {
            config: Config {
                n_layers,
                embed_dim,
                ffn_dim,
                n_ff_exp,
                n_heads,
                n_kv_heads,
                n_embd_head_k_full,
                n_embd_head_k_swa,
                n_layer_kv_from_start,
                vocab_size,
                rope_freq_base,
                rope_freq_base_swa,
                rms_eps,
                context_length,
                n_expert,
                n_expert_used,
                n_embd_per_layer,
                final_logit_softcapping,
                n_swa,
                is_swa,
            },
            token_embd,
            output_weight,
            output_norm,
            per_layer_tok_embd,
            per_layer_model_proj,
            per_layer_proj_norm,
            layers,
        })
    }
}
