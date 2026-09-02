//! GGUF weight loading for the Llama architecture (targeting Llama 3.x —
//! see the crate-level doc comment for why 3.x specifically, not 1/2).
//!
//! Structurally near-identical to `qwen2::model` (RMSNorm, SwiGLU MLP, same
//! GGUF tensor-naming convention — llama.cpp reuses these short tensor names
//! across most of its supported architectures, this isn't Qwen2-specific).
//! The two real differences, both HIGH confidence (well-established, stable
//! llama.cpp/GGUF convention, not something that changes per-checkpoint):
//!   - No attention Q/K/V bias tensors at all (Qwen2 always has them).
//!   - Metadata keys are prefixed `llama.*` instead of `qwen2.*`.
//! Tied-vs-untied embeddings gets the same defensive check-if-present
//! treatment `qwen2::model::Model` already learned the hard way (see that
//! crate's doc comment on `output_weight`) — not assumed either way here either.

use gguf::{GgufFile, MetadataValue};
use model_core::CacheShape;
use tensor_core::QuantizedMatrix;

use crate::error::LoadError;

#[derive(Debug, Clone)]
pub struct Config {
    pub n_layers: usize,
    pub embed_dim: usize,
    pub ffn_dim: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    pub head_dim: usize,
    pub vocab_size: usize,
    pub rope_freq_base: f32,
    pub rms_eps: f32,
    pub context_length: usize,
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

/// No `bq`/`bk`/`bv` fields — Llama has no attention bias tensors at all
/// (unlike Qwen2, which always does). Not an optional/sometimes-present
/// field: genuinely absent from every Llama checkpoint's GGUF, so there is
/// nothing to load defensively here.
pub struct LayerWeights {
    pub attn_norm: Vec<f32>,
    pub wq: QuantizedMatrix,
    pub wk: QuantizedMatrix,
    pub wv: QuantizedMatrix,
    pub wo: QuantizedMatrix,
    pub ffn_norm: Vec<f32>,
    pub w_gate: QuantizedMatrix,
    pub w_up: QuantizedMatrix,
    pub w_down: QuantizedMatrix,
}

pub struct Model {
    pub config: Config,
    pub token_embd: QuantizedMatrix,
    /// `None` means tied embeddings (reuse `token_embd` for the final
    /// projection) — see `qwen2::model::Model::output_weight`'s doc comment
    /// for why this is checked per-file rather than assumed.
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

fn meta_u32(gguf: &GgufFile, key: &'static str) -> Result<u32, LoadError> {
    match gguf.metadata.get(key) {
        Some(MetadataValue::Uint32(v)) => Ok(*v),
        Some(MetadataValue::Int32(v)) if *v >= 0 => Ok(*v as u32),
        Some(_) => Err(LoadError::WrongMetadataType(key)),
        None => Err(LoadError::MissingMetadata(key)),
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

impl Model {
    pub fn load(bytes: &[u8]) -> Result<Model, LoadError> {
        let gguf = GgufFile::parse(bytes)?;

        let arch = gguf.architecture().unwrap_or("").to_string();
        if arch != "llama" {
            return Err(LoadError::UnexpectedArchitecture(arch));
        }

        let n_layers = meta_u32(&gguf, "llama.block_count")? as usize;
        let embed_dim = meta_u32(&gguf, "llama.embedding_length")? as usize;
        let ffn_dim = meta_u32(&gguf, "llama.feed_forward_length")? as usize;
        let n_heads = meta_u32(&gguf, "llama.attention.head_count")? as usize;
        let n_kv_heads = meta_u32(&gguf, "llama.attention.head_count_kv")? as usize;
        let rope_freq_base = meta_f32(&gguf, "llama.rope.freq_base")?;
        let rms_eps = meta_f32(&gguf, "llama.attention.layer_norm_rms_epsilon")?;
        let context_length = meta_u32(&gguf, "llama.context_length")? as usize;
        let head_dim = embed_dim / n_heads;

        let token_embd = get_qmatrix2d(&gguf, bytes, "token_embd.weight")?;
        let vocab_size = token_embd.rows;
        let output_norm = get_vector(&gguf, bytes, "output_norm.weight")?;
        let output_weight = match gguf.tensors.iter().find(|t| t.name == "output.weight") {
            Some(_) => Some(get_qmatrix2d(&gguf, bytes, "output.weight")?),
            None => None,
        };

        let mut layers = Vec::with_capacity(n_layers);
        for i in 0..n_layers {
            let p = |suffix: &str| format!("blk.{i}.{suffix}");
            layers.push(LayerWeights {
                attn_norm: get_vector(&gguf, bytes, &p("attn_norm.weight"))?,
                wq: get_qmatrix2d(&gguf, bytes, &p("attn_q.weight"))?,
                wk: get_qmatrix2d(&gguf, bytes, &p("attn_k.weight"))?,
                wv: get_qmatrix2d(&gguf, bytes, &p("attn_v.weight"))?,
                wo: get_qmatrix2d(&gguf, bytes, &p("attn_output.weight"))?,
                ffn_norm: get_vector(&gguf, bytes, &p("ffn_norm.weight"))?,
                w_gate: get_qmatrix2d(&gguf, bytes, &p("ffn_gate.weight"))?,
                w_up: get_qmatrix2d(&gguf, bytes, &p("ffn_up.weight"))?,
                w_down: get_qmatrix2d(&gguf, bytes, &p("ffn_down.weight"))?,
            });
        }

        Ok(Model {
            config: Config {
                n_layers,
                embed_dim,
                ffn_dim,
                n_heads,
                n_kv_heads,
                head_dim,
                vocab_size,
                rope_freq_base,
                rms_eps,
                context_length,
            },
            token_embd,
            output_weight,
            layers,
            output_norm,
        })
    }
}
