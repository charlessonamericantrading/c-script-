//! GGUF weight loading for Qwen3 (dense) and Qwen3MoE — two distinct
//! `general.architecture` strings ("qwen3"/"qwen3moe") that share every
//! hyperparameter/tensor-naming convention except the FFN block, so both
//! are handled by this one crate/struct rather than splitting into two.
//! GGUF's metadata-key template is `"%s.<name>"` with the literal
//! architecture string substituted in, so a key like "block_count" is
//! genuinely spelled "qwen3.block_count" on one checkpoint and
//! "qwen3moe.block_count" on another — every metadata read here builds its
//! key from the detected `arch` string at load time instead of a
//! compile-time literal, unlike every other crate in this workspace (each
//! of which handles exactly one architecture string).
//!
//! VERIFIED against llama.cpp (commit e920c523e3b8a0163fe498af5bf90df35ff51d25:
//! `src/llama-arch.cpp`, `src/llama-model.cpp`, `src/models/qwen3.cpp`,
//! `src/models/qwen3moe.cpp`) and Qwen's own published configs/technical
//! report (arXiv:2505.09388), via web research this session — not recalled
//! from memory. Two real architectural differences from the
//! already-implemented `qwen2`:
//!   - QK-Norm: `blk.N.attn_q_norm.weight`/`attn_k_norm.weight`, each a
//!     `head_dim`-long RMSNorm weight applied per-head (one shared vector
//!     across all heads), AFTER the Q/K projection is reshaped into heads,
//!     BEFORE RoPE.
//!   - No QKV bias (Qwen2 has `attn_{q,k,v}.bias`; Qwen3 dropped them —
//!     confirmed in the technical report, not just absence-implies-removal:
//!     "Qwen3 removes QKV-bias used in Qwen2 and introduces QK-Norm").
//!
//! Also unlike Qwen2: `head_dim` is EXPLICIT metadata
//! (`attention.key_length`), not derived as `embed_dim/n_heads` — the same
//! gotcha Gemma4 and Phi-3 already taught this codebase not to assume away.
//!
//! Qwen3MoE adds, per layer: a router (`ffn_gate_inp.weight`) and three
//! fused 3-D expert tensors (`ffn_{gate,up,down}_exps.weight` — `n_expert`
//! is the SLOWEST-varying axis in GGUF's fastest-first convention, so
//! on-disk bytes are `n_expert` consecutive equal-sized 2-D blocks, sliced
//! the same way Gemma4's own `get_expert_qmatrices` already does).
//! Routing is plain softmax-over-all-experts + top-k + renormalize,
//! hardcoded in llama.cpp rather than read from a metadata key — exactly
//! `tensor_core::ops::moe_route`'s existing contract, no new primitive
//! needed. Unlike Gemma4's MoE layers (which keep a dense "shared expert"
//! FFN running in parallel with the routed experts on every layer),
//! Qwen3MoE has NO shared expert at all (confirmed in the technical
//! report: "the Qwen3-MoE design excludes shared experts") — a layer's FFN
//! block is either the dense trio (`ffn_gate`/`ffn_up`/`ffn_down`, on
//! "qwen3"/non-MoE layers) or the routed-expert path (`moe`, when
//! `ffn_gate_inp.weight` is present for that layer), never both.
//! Per-layer (not per-checkpoint) presence is checked directly from the
//! tensor list rather than assumed from the top-level architecture string,
//! mirroring Gemma4's own defensive pattern, even though every Qwen3MoE
//! checkpoint released so far is uniformly MoE on every layer.
//!
//! STANDING CONSTRAINT: no real Qwen3/Qwen3MoE GGUF file exists locally
//! (the "never download a model" rule applies here same as everywhere else
//! in this project) and none is available via the local Ollama install
//! either, so — like `llama`'s Mistral/classic-Llama support — this crate
//! is built from documented spec and verified only by synthetic unit
//! tests (shapes, no NaN/Inf, the two Qwen3-specific mechanisms actually
//! changing output when exercised). Treat it as unverified-in-anger.

use gguf::{GgufFile, MetadataValue};
use model_core::CacheShape;
use tensor_core::QuantizedMatrix;

use crate::error::LoadError;

#[derive(Debug, Clone)]
pub struct Config {
    pub n_layers: usize,
    pub embed_dim: usize,
    pub head_dim: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    pub vocab_size: usize,
    pub rope_freq_base: f32,
    pub rms_eps: f32,
    pub context_length: usize,
    /// 0 on a checkpoint with no MoE layers at all ("qwen3", or a
    /// hypothetical dense-only slice of a mixed checkpoint) — every
    /// layer's `moe` is then `None` and this is never read.
    pub n_expert_used: usize,
}

impl Config {
    pub fn cache_shape(&self) -> CacheShape {
        CacheShape {
            n_layers: self.n_layers,
            n_kv_heads: self.n_kv_heads,
            head_dim: self.head_dim,
            context_length: self.context_length,
            per_layer_head_dim: None,
        }
    }
}

/// Per-expert weight matrices for one MoE layer. Simpler than Gemma4's own
/// `MoeWeights` — no elementwise router-input scale, no extra pre/post
/// norms wrapping the branch, no shared-expert output to add — Qwen3MoE's
/// router reads straight off the block's normalized hidden state and the
/// weighted expert sum IS the FFN block's entire output, matching a plain
/// dense FFN's shape exactly (see module doc comment).
pub struct MoeWeights {
    pub gate_inp: QuantizedMatrix,       // router: [n_expert, embed_dim]
    pub gate_exps: Vec<QuantizedMatrix>, // n_expert x [n_ff_exp, embed_dim]
    pub up_exps: Vec<QuantizedMatrix>,   // n_expert x [n_ff_exp, embed_dim]
    pub down_exps: Vec<QuantizedMatrix>, // n_expert x [embed_dim, n_ff_exp]
}

pub struct LayerWeights {
    pub attn_norm: Vec<f32>,
    pub wq: QuantizedMatrix,
    pub wk: QuantizedMatrix,
    pub wv: QuantizedMatrix,
    pub wo: QuantizedMatrix,
    /// Per-head learned norm on Q, applied after projection+reshape,
    /// before RoPE. Length `head_dim`, shared across all `n_heads`.
    pub attn_q_norm: Vec<f32>,
    /// Same for K, shared across all `n_kv_heads`.
    pub attn_k_norm: Vec<f32>,
    pub ffn_norm: Vec<f32>,
    /// Dense FFN weights — `Some` on a non-MoE layer, `None` when `moe` is
    /// `Some` instead (see module doc comment: never both).
    pub ffn_gate: Option<QuantizedMatrix>,
    pub ffn_up: Option<QuantizedMatrix>,
    pub ffn_down: Option<QuantizedMatrix>,
    pub moe: Option<MoeWeights>,
}

pub struct Model {
    pub config: Config,
    pub token_embd: QuantizedMatrix,
    /// `None` means tied input/output embeddings — see `qwen2::Model`'s
    /// identical field for why this is a real per-checkpoint difference,
    /// not a detail to assume either way (Qwen3-0.6B ties, Qwen3-8B/
    /// Qwen3-30B-A3B don't — confirmed against all three real configs).
    pub output_weight: Option<QuantizedMatrix>,
    pub layers: Vec<LayerWeights>,
    pub output_norm: Vec<f32>,
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

/// Slices a 3-D `[embed_dim, n_ff_exp, n_expert]` tensor (`n_expert` is the
/// SLOWEST-varying axis in GGUF's fastest-first convention, so the raw
/// bytes are `n_expert` consecutive equal-sized 2-D blocks) into `n_expert`
/// independent `QuantizedMatrix`es. Identical technique to Gemma4's own
/// `get_expert_qmatrices` — duplicated rather than shared because it lives
/// in each crate's own `LoadError`-returning loader code, matching this
/// workspace's established one-crate-per-architecture-is-self-contained
/// convention (see `qwen2`/`llama`/`phi3`/`gemma4`'s own near-identical
/// `get_qmatrix2d`/`get_vector`/`meta_u32` helpers, also each duplicated
/// rather than centralized).
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

fn get_raw_bytes<'a>(gguf: &GgufFile, bytes: &'a [u8], t: &gguf::TensorInfo) -> Result<&'a [u8], LoadError> {
    let ty = t.ggml_type.ok_or_else(|| LoadError::UnexpectedTensorShape { name: t.name.clone(), dims: t.dimensions.clone() })?;
    let size = t.size_bytes().ok_or(tensor_core::DequantError::Unsupported(ty))?;
    let abs = gguf.tensor_absolute_offset(t)?;
    Ok(&bytes[abs as usize..abs as usize + size as usize])
}

/// Unlike every other crate here, the metadata key prefix isn't a
/// compile-time literal — `arch` is "qwen3" or "qwen3moe", detected at
/// load time (module doc comment). Split out from `meta_u32`/`meta_f32`
/// so this one genuinely novel bit (everywhere else in this workspace,
/// the prefix is a `&'static str` literal) has its own direct test rather
/// than only being exercised indirectly through a full `Model::load` call.
fn full_key(arch: &str, key: &str) -> String {
    format!("{arch}.{key}")
}

fn meta_u32(gguf: &GgufFile, arch: &str, key: &str) -> Result<u32, LoadError> {
    let full_key = full_key(arch, key);
    match gguf.metadata.get(&full_key) {
        Some(MetadataValue::Uint32(v)) => Ok(*v),
        Some(MetadataValue::Int32(v)) if *v >= 0 => Ok(*v as u32),
        Some(_) => Err(LoadError::WrongMetadataType(full_key)),
        None => Err(LoadError::MissingMetadata(full_key)),
    }
}

fn meta_u32_opt(gguf: &GgufFile, arch: &str, key: &str) -> Result<Option<u32>, LoadError> {
    let full_key = full_key(arch, key);
    match gguf.metadata.get(&full_key) {
        Some(MetadataValue::Uint32(v)) => Ok(Some(*v)),
        Some(MetadataValue::Int32(v)) if *v >= 0 => Ok(Some(*v as u32)),
        Some(_) => Err(LoadError::WrongMetadataType(full_key)),
        None => Ok(None),
    }
}

fn meta_f32(gguf: &GgufFile, arch: &str, key: &str) -> Result<f32, LoadError> {
    let full_key = full_key(arch, key);
    match gguf.metadata.get(&full_key) {
        Some(MetadataValue::Float32(v)) => Ok(*v),
        Some(MetadataValue::Uint32(v)) => Ok(*v as f32),
        Some(_) => Err(LoadError::WrongMetadataType(full_key)),
        None => Err(LoadError::MissingMetadata(full_key)),
    }
}

impl Model {
    pub fn load(bytes: &[u8]) -> Result<Model, LoadError> {
        let gguf = GgufFile::parse(bytes)?;

        let arch = gguf.architecture().unwrap_or("").to_string();
        if arch != "qwen3" && arch != "qwen3moe" {
            return Err(LoadError::UnexpectedArchitecture(arch));
        }

        let n_layers = meta_u32(&gguf, &arch, "block_count")? as usize;
        let embed_dim = meta_u32(&gguf, &arch, "embedding_length")? as usize;
        let n_heads = meta_u32(&gguf, &arch, "attention.head_count")? as usize;
        let n_kv_heads = meta_u32(&gguf, &arch, "attention.head_count_kv")? as usize;
        // Explicit, NOT derived as embed_dim/n_heads -- see module doc
        // comment (the same gotcha Gemma4/Phi-3 already required).
        let head_dim = meta_u32(&gguf, &arch, "attention.key_length")? as usize;
        let rope_freq_base = meta_f32(&gguf, &arch, "rope.freq_base")?;
        let rms_eps = meta_f32(&gguf, &arch, "attention.layer_norm_rms_epsilon")?;
        let context_length = meta_u32(&gguf, &arch, "context_length")? as usize;
        // Both optional -- required-and-nonzero on a real qwen3moe
        // checkpoint (llama.cpp itself throws if either is absent/zero
        // THERE), but reading them as optional here means a "qwen3"
        // (dense) checkpoint that simply doesn't declare them at all loads
        // cleanly with n_expert=0, giving every layer's `ffn_gate_inp.weight`
        // presence check (below) nothing to find -- so every layer
        // correctly takes the dense path without a separate "is this a
        // MoE checkpoint" flag needed anywhere.
        let n_expert = meta_u32_opt(&gguf, &arch, "expert_count")?.unwrap_or(0) as usize;
        let n_expert_used = meta_u32_opt(&gguf, &arch, "expert_used_count")?.unwrap_or(0) as usize;

        let token_embd = get_qmatrix2d(&gguf, bytes, "token_embd.weight")?;
        let vocab_size = token_embd.rows;
        let output_norm = get_vector(&gguf, bytes, "output_norm.weight")?;
        let output_weight = get_qmatrix2d_opt(&gguf, bytes, "output.weight")?;

        let mut layers = Vec::with_capacity(n_layers);
        for i in 0..n_layers {
            let p = |suffix: &str| format!("blk.{i}.{suffix}");

            let gate_inp = get_qmatrix2d_opt(&gguf, bytes, &p("ffn_gate_inp.weight"))?;
            let (ffn_gate, ffn_up, ffn_down, moe) = match gate_inp {
                Some(gate_inp) => (
                    None,
                    None,
                    None,
                    Some(MoeWeights {
                        gate_inp,
                        gate_exps: get_expert_qmatrices(&gguf, bytes, &p("ffn_gate_exps.weight"), n_expert)?,
                        up_exps: get_expert_qmatrices(&gguf, bytes, &p("ffn_up_exps.weight"), n_expert)?,
                        down_exps: get_expert_qmatrices(&gguf, bytes, &p("ffn_down_exps.weight"), n_expert)?,
                    }),
                ),
                None => (
                    Some(get_qmatrix2d(&gguf, bytes, &p("ffn_gate.weight"))?),
                    Some(get_qmatrix2d(&gguf, bytes, &p("ffn_up.weight"))?),
                    Some(get_qmatrix2d(&gguf, bytes, &p("ffn_down.weight"))?),
                    None,
                ),
            };

            layers.push(LayerWeights {
                attn_norm: get_vector(&gguf, bytes, &p("attn_norm.weight"))?,
                wq: get_qmatrix2d(&gguf, bytes, &p("attn_q.weight"))?,
                wk: get_qmatrix2d(&gguf, bytes, &p("attn_k.weight"))?,
                wv: get_qmatrix2d(&gguf, bytes, &p("attn_v.weight"))?,
                wo: get_qmatrix2d(&gguf, bytes, &p("attn_output.weight"))?,
                attn_q_norm: get_vector(&gguf, bytes, &p("attn_q_norm.weight"))?,
                attn_k_norm: get_vector(&gguf, bytes, &p("attn_k_norm.weight"))?,
                ffn_norm: get_vector(&gguf, bytes, &p("ffn_norm.weight"))?,
                ffn_gate,
                ffn_up,
                ffn_down,
                moe,
            });
        }

        Ok(Model {
            config: Config { n_layers, embed_dim, head_dim, n_heads, n_kv_heads, vocab_size, rope_freq_base, rms_eps, context_length, n_expert_used },
            token_embd,
            output_weight,
            layers,
            output_norm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_key_uses_the_detected_architecture_string_as_prefix() {
        // The one genuinely novel mechanism this crate has that no other
        // architecture crate needs: the SAME logical hparam ("block_count")
        // is spelled under two different top-level prefixes depending on
        // which of the two architecture strings this checkpoint declared.
        assert_eq!(full_key("qwen3", "block_count"), "qwen3.block_count");
        assert_eq!(full_key("qwen3moe", "block_count"), "qwen3moe.block_count");
        assert_eq!(full_key("qwen3", "attention.key_length"), "qwen3.attention.key_length");
    }
}
